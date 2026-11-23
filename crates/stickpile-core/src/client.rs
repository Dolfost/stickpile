//! Typed async REST client for every route in [`stickpile_api::routes`].
//!
//! * Bearer auth from the token file ([`crate::token`]). Without a token,
//!   requests are still sent: behind the trusted `tailscale serve` proxy the
//!   server authenticates tailnet users by identity headers. A 401 on an
//!   authenticated route without a token becomes [`CoreError::NotLoggedIn`].
//! * Non-2xx responses decode the [`ApiError`] body into [`CoreError::Api`].
//! * Connection failures and timeouts (and 502/503/504 from a reverse proxy
//!   without an `ApiError` body) become [`CoreError::Offline`] so callers can
//!   fall back to the offline cache.
//! * Ordinary requests use the configured `timeout_secs`; uploads, imports,
//!   exports and file downloads use [`ApiClient::transfer_timeout`]. Reads
//!   with the ordinary timeout connect within
//!   [`INTERACTIVE_CONNECT_TIMEOUT`] (2 s), the others within
//!   `timeout_secs`.
//! * Opt-in ([`ApiClient::with_fail_fast`]): once a request finds the server
//!   unreachable, reads fail as offline at once instead of each waiting for
//!   the timeout, `GET /healthz` checks the server in the background until
//!   it answers, and the state is published as a watch value
//!   ([`ApiClient::reach`], [`Reach`]).

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, RwLock, Weak};
use std::time::{Duration, SystemTime};

use futures::future::BoxFuture;
use reqwest::multipart::{Form, Part};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use stickpile_api::profiles::TargetProfile;
use stickpile_api::routes::{self, path};
use stickpile_api::*;
use tokio::sync::{watch, Notify};

use crate::{ClientConfig, CoreError, Result};

/// Archive formats of a set export ([`ApiClient::download_export`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetArchive {
    /// `export.zip`
    Zip,
    /// `export.tar`
    Tar,
    /// `export.tar.gz`
    TarGz,
}

impl SetArchive {
    pub fn route(self) -> &'static str {
        match self {
            SetArchive::Zip => routes::SET_EXPORT_ZIP,
            SetArchive::Tar => routes::SET_EXPORT,
            SetArchive::TarGz => routes::SET_EXPORT_TGZ,
        }
    }

    /// File extension without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            SetArchive::Zip => "zip",
            SetArchive::Tar => "tar",
            SetArchive::TarGz => "tar.gz",
        }
    }
}

/// Default timeout for uploads, imports, exports and file downloads.
pub const DEFAULT_TRANSFER_TIMEOUT: Duration = Duration::from_secs(600);

/// Upper bound for a downloaded export tar when unpacking it.
pub const MAX_EXPORT_BYTES: u64 = 16 * 1024 * 1024 * 1024;

/// One file of a multipart upload to [`routes::SET_ITEMS`].
#[derive(Debug, Clone)]
pub struct UploadFile {
    pub filename: String,
    pub bytes: Vec<u8>,
    /// Defaults to a guess from the extension.
    pub mime: Option<String>,
}

impl UploadFile {
    pub fn from_path(p: &Path) -> Result<UploadFile> {
        let bytes = std::fs::read(p).map_err(|e| CoreError::Other(format!("{}: {e}", p.display())))?;
        let filename = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "upload".into());
        Ok(UploadFile {
            filename,
            bytes,
            mime: None,
        })
    }
}

/// Mime type guessed from a filename extension (for multipart parts only; the
/// server sniffs content and never trusts it).
pub fn mime_from_name(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "toml" => "application/toml",
        _ => "application/octet-stream",
    }
}

/// `GET /search` parameters.
#[derive(Debug, Clone, Default)]
pub struct SearchParams {
    pub q: String,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub target: Option<TargetApp>,
}

/// `GET /popular` parameters (`scope=server|mine|set`, `window=day|week|month|all`).
#[derive(Debug, Clone, Default)]
pub struct PopularParams {
    pub scope: Option<String>,
    pub set: Option<String>,
    pub window: Option<String>,
    pub limit: Option<u32>,
}

/// Response of [`routes::SETS_GITPACK`]: the model documents
/// [`GitPackCreated`]; a bare [`Set`] is accepted too.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GitPackResponse {
    Created(Box<GitPackCreated>),
    Set(Box<Set>),
}

impl GitPackResponse {
    pub fn set(&self) -> &Set {
        match self {
            GitPackResponse::Created(c) => &c.set,
            GitPackResponse::Set(s) => s,
        }
    }
}

/// The error of a recheck call to a server that has no rechecks (0.7.0 and
/// older answer their routes with 404).
pub const SERVER_TOO_OLD_FOR_RECHECKS: &str =
    "the server is too old for rechecks (it has no /api/v1/rechecks); update stickpile on the server";

/// The error when a server ignored the item text it was sent: servers
/// older than item texts (0.7.0) drop the unknown `text` field.
pub const SERVER_TOO_OLD_FOR_TEXTS: &str =
    "the server is too old for item texts (it ignored the text); update stickpile on the server";

/// Whether `item`, as a server answered a `PATCH` or an upload that gave
/// it the text `sent` (as the user's), lacks that text: the server is too
/// old for item texts ([`SERVER_TOO_OLD_FOR_TEXTS`]). Such a server answers
/// without `text`, which reads as an empty `ai` text; a newer one stores
/// the text cleaned ([`stickpile_api::text::clean_text`]) and as the user's.
pub fn text_ignored(sent: &str, item: &Item) -> bool {
    item.text != stickpile_api::text::clean_text(sent) || item.text_source != FieldSource::User
}

/// What [`ApiClient::start_recheck`] got back.
#[derive(Debug, Clone, PartialEq)]
pub struct RecheckStarted {
    pub recheck: Recheck,
    /// False: the same recheck was running, and this is it.
    pub started: bool,
}

/// The message of a read refused because the server is believed
/// unreachable ([`ApiClient::with_fail_fast`]).
pub const RECENTLY_UNREACHABLE: &str = "the server could not be reached; it is checked again in the background";

/// How long a read (a `GET` with the ordinary timeout: searches, lists,
/// the sync, `/healthz`) may take to connect, so that a server that is
/// gone (a tailnet peer that left, a host that is off) is recognised
/// quickly. `timeout_secs` still bounds the whole request; writes,
/// uploads, imports, exports and file downloads connect within
/// `timeout_secs` as before. Never more than the client's timeout.
pub const INTERACTIVE_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// While a server is believed unreachable ([`ApiClient::with_fail_fast`]),
/// `GET /healthz` checks it this often for the first [`PROBE_FAST_FOR`] of
/// the outage, then every [`PROBE_SLOW`].
pub const PROBE_FAST: Duration = Duration::from_secs(15);
/// How long [`PROBE_FAST`] applies, from the start of an outage.
pub const PROBE_FAST_FOR: Duration = Duration::from_secs(5 * 60);
/// The check's period after [`PROBE_FAST_FOR`].
pub const PROBE_SLOW: Duration = Duration::from_secs(60);

/// What a client with [`ApiClient::with_fail_fast`] knows of its server,
/// published as a watch value ([`ApiClient::reach`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Nothing has been asked yet, or the outage was forgotten
    /// ([`ApiClient::forget_unreachable`]).
    Unknown,
    /// The server answered the last request or check (any answer, an
    /// error status too).
    Reachable,
    /// A request found the server unreachable (no connection, a timeout,
    /// a proxy's 502 to 504 without an API error) and nothing answered
    /// since: reads fail at once, and `GET /healthz` checks the server in
    /// the background ([`PROBE_FAST`], [`PROBE_SLOW`]).
    Unreachable {
        /// When the outage was noticed (its first failure).
        since: SystemTime,
    },
}

impl Reach {
    pub fn is_unreachable(&self) -> bool {
        matches!(self, Reach::Unreachable { .. })
    }
}

/// A check of the server: `Ok`, or an error that is not offline, when it
/// answers.
pub(crate) type ProbeFn = Arc<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// What a client with [`ApiClient::with_fail_fast`] remembers of the
/// server, and its background check. Shared by the client's clones.
struct Reachability {
    /// The published state; set only with `state` locked.
    value: watch::Sender<Reach>,
    state: Mutex<ReachState>,
    /// `GET /healthz`, past the gate of the reads.
    probe: ProbeFn,
}

#[derive(Default)]
struct ReachState {
    /// When the outage began, on Tokio's clock (the checks' schedule);
    /// `Some` exactly while the value is `Unreachable`.
    down_since: Option<tokio::time::Instant>,
    /// The wake-up of the check task while one runs.
    prober: Option<Arc<Notify>>,
    /// A check was asked for ([`ApiClient::check_now`]) and has not
    /// started yet.
    asked: bool,
    /// A check is waiting for its answer.
    probing: bool,
    /// The runtime of the last request, where a check asked for outside a
    /// runtime (from a UI thread) runs.
    runtime: Option<tokio::runtime::Handle>,
}

impl Reachability {
    fn new(probe: ProbeFn) -> Reachability {
        Reachability {
            value: watch::channel(Reach::Unknown).0,
            state: Mutex::new(ReachState {
                runtime: tokio::runtime::Handle::try_current().ok(),
                ..ReachState::default()
            }),
            probe,
        }
    }

    fn state(&self) -> MutexGuard<'_, ReachState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now(&self) -> Reach {
        *self.value.borrow()
    }

    /// Remember the current runtime for checks asked for from outside one.
    fn note_runtime(&self) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            self.state().runtime = Some(runtime);
        }
    }

    /// Whether a read is to fail at once: the server is believed
    /// unreachable. The check task is started again if it is gone (its
    /// runtime ended); reads never check the server themselves.
    fn refuse_read(self: &Arc<Self>) -> bool {
        if self.state().down_since.is_none() {
            return false;
        }
        self.start_checks();
        true
    }

    /// Note what a request's or a check's `result` says of the server: an
    /// offline error begins an outage (and the checks), anything else ends
    /// it.
    fn record<T>(self: &Arc<Self>, result: &Result<T>) {
        if matches!(result, Err(e) if e.is_offline()) {
            self.went_down();
        } else {
            self.set_up(Reach::Reachable);
        }
    }

    fn went_down(self: &Arc<Self>) {
        {
            let mut s = self.state();
            if s.down_since.is_none() {
                s.down_since = Some(tokio::time::Instant::now());
                self.value.send_replace(Reach::Unreachable {
                    since: SystemTime::now(),
                });
            }
        }
        self.start_checks();
    }

    /// The outage is over (`Reachable`) or forgotten (`Unknown`). A check
    /// task waiting for its next turn ends.
    fn set_up(&self, to: Reach) {
        let s = &mut *self.state();
        s.down_since = None;
        self.value.send_if_modified(|v| {
            let changed = *v != to;
            *v = to;
            changed
        });
        if let Some(wake) = &s.prober {
            wake.notify_waiters();
        }
    }

    /// [`ApiClient::forget_unreachable`].
    fn forget(&self) {
        if self.state().down_since.is_some() {
            self.set_up(Reach::Unknown);
        }
    }

    /// [`ApiClient::check_now`].
    fn check_now(self: &Arc<Self>) {
        {
            let mut s = self.state();
            if s.probing {
                return;
            }
            s.asked = true;
            if let Some(wake) = &s.prober {
                wake.notify_waiters();
                return;
            }
        }
        self.start_checks();
    }

    /// Start the check task unless it runs: on the current runtime, else on
    /// the one of the last request.
    fn start_checks(self: &Arc<Self>) {
        let (wake, runtime) = {
            let mut s = self.state();
            if s.prober.is_some() {
                return;
            }
            let Some(runtime) = tokio::runtime::Handle::try_current().ok().or_else(|| s.runtime.clone()) else {
                return;
            };
            let wake = Arc::new(Notify::new());
            s.prober = Some(wake.clone());
            (wake, runtime)
        };
        // Spawned with the lock released: a runtime that is shutting down
        // drops the task at once, and the drop takes the lock.
        let task = CheckTask {
            reach: Arc::downgrade(self),
            wake,
        };
        runtime.spawn(task.run());
    }
}

impl Drop for Reachability {
    /// The client and its clones are gone: the check task ends now rather
    /// than at its next turn.
    fn drop(&mut self) {
        let s = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        if let Some(wake) = &s.prober {
            wake.notify_waiters();
        }
    }
}

/// The background check of a server believed unreachable: `GET /healthz`
/// on the schedule of [`PROBE_FAST`] and [`PROBE_SLOW`], and at once when
/// asked ([`ApiClient::check_now`]), until the server answers. One at a
/// time; when it is dropped (its runtime ended) the next read or check
/// starts another. It does not keep the client alive: once the client and
/// its clones are dropped (a shell opened another session), nothing can
/// read what a check finds, and it ends.
struct CheckTask {
    reach: Weak<Reachability>,
    wake: Arc<Notify>,
}

impl CheckTask {
    async fn run(self) {
        let mut last = tokio::time::Instant::now();
        loop {
            // Listening before the state is read: a wake-up sent after it
            // is not lost.
            let woken = self.wake.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            let next = {
                let Some(reach) = self.reach.upgrade() else { return };
                let mut s = reach.state();
                match s.down_since {
                    _ if s.asked => None,
                    Some(since) => {
                        let fast = last.saturating_duration_since(since) < PROBE_FAST_FOR;
                        Some(last + if fast { PROBE_FAST } else { PROBE_SLOW })
                    }
                    None => return self.end(&mut s),
                }
            };
            if let Some(at) = next {
                let _ = tokio::time::timeout_at(at, woken).await;
            }
            let probe = {
                let Some(reach) = self.reach.upgrade() else { return };
                {
                    let mut s = reach.state();
                    if !s.asked && s.down_since.is_none() {
                        return self.end(&mut s);
                    }
                    s.asked = false;
                    s.probing = true;
                }
                (reach.probe)()
            };
            last = tokio::time::Instant::now();
            let result = probe.await;
            let Some(reach) = self.reach.upgrade() else { return };
            reach.record(&result);
            reach.state().probing = false;
        }
    }

    /// Give up the task's place, with the state locked: the next failure
    /// or check starts a new one.
    fn end(&self, s: &mut ReachState) {
        if s.prober.as_ref().is_some_and(|w| Arc::ptr_eq(w, &self.wake)) {
            s.prober = None;
            s.probing = false;
        }
    }
}

impl Drop for CheckTask {
    fn drop(&mut self) {
        if let Some(reach) = self.reach.upgrade() {
            self.end(&mut reach.state());
        }
    }
}

/// Async REST client. Cheap to clone (shares the connection pools and token).
#[derive(Clone)]
pub struct ApiClient {
    /// Reads with the ordinary timeout: they connect within
    /// [`INTERACTIVE_CONNECT_TIMEOUT`].
    http_read: reqwest::Client,
    /// Writes and transfers: they connect within `timeout`.
    http: reqwest::Client,
    base: String,
    token: Arc<RwLock<Option<String>>>,
    timeout: Duration,
    transfer_timeout: Duration,
    /// `Some` with [`ApiClient::with_fail_fast`].
    reach: Option<Arc<Reachability>>,
}

impl std::fmt::Debug for ApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiClient")
            .field("base", &self.base)
            .field("has_token", &self.has_token())
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl ApiClient {
    /// `base_url` is the server root (`https://stickers.example.ts.net`, a path prefix is kept).
    pub fn new(base_url: &str, timeout: Duration) -> Result<ApiClient> {
        let parsed =
            url::Url::parse(base_url).map_err(|e| CoreError::Other(format!("invalid server URL {base_url:?}: {e}")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(CoreError::Other(format!("server URL must be http(s): {base_url:?}")));
        }
        let http = |connect: Duration| {
            reqwest::Client::builder()
                .connect_timeout(connect)
                .user_agent(concat!("stickpile-core/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|e| CoreError::Other(format!("HTTP client: {e}")))
        };
        Ok(ApiClient {
            http_read: http(INTERACTIVE_CONNECT_TIMEOUT.min(timeout))?,
            http: http(timeout)?,
            base: base_url.trim_end_matches('/').to_string(),
            token: Arc::new(RwLock::new(None)),
            timeout,
            transfer_timeout: DEFAULT_TRANSFER_TIMEOUT.max(timeout),
            reach: None,
        })
    }

    /// Client for `config.server_url` with `config.timeout_secs` (no token).
    pub fn from_config(config: &ClientConfig) -> Result<ApiClient> {
        ApiClient::new(&config.server_url, Duration::from_secs(config.timeout_secs.max(1)))
    }

    pub fn with_token(self, token: Option<String>) -> ApiClient {
        self.set_token(token);
        self
    }

    /// A client on the same connection pool without a token: what the
    /// server makes of us by tailnet identity alone.
    pub fn anonymous(&self) -> ApiClient {
        ApiClient {
            http_read: self.http_read.clone(),
            http: self.http.clone(),
            base: self.base.clone(),
            token: Arc::new(RwLock::new(None)),
            timeout: self.timeout,
            transfer_timeout: self.transfer_timeout,
            reach: self.reach.clone(),
        }
    }

    pub fn with_transfer_timeout(mut self, timeout: Duration) -> ApiClient {
        self.transfer_timeout = timeout;
        self
    }

    /// Fail fast while the server is away, for the clients that show
    /// results (docs/offline.md). Once a request finds it unreachable
    /// ([`CoreError::Offline`]), the state is [`Reach::Unreachable`] and
    /// reads (`GET`) fail the same way at once ([`RECENTLY_UNREACHABLE`])
    /// instead of each waiting for the timeout, so a caller with an offline
    /// cache answers from it straight away. Meanwhile a background task
    /// checks the server with `GET /healthz` every [`PROBE_FAST`] for the
    /// first [`PROBE_FAST_FOR`] of the outage, then every [`PROBE_SLOW`],
    /// whether or not anything reads; reads never check it themselves.
    /// Any answer from the server (a check, a write, a request that was
    /// out already) ends the outage: [`Reach::Reachable`]. Writes are
    /// always sent. The state is a watch value ([`ApiClient::reach`]); a
    /// shell asks for a check at once with [`ApiClient::check_now`].
    /// Shared with the clones made from now on. The checks run on a Tokio
    /// runtime: the one of the request that failed, or of the last request.
    pub fn with_fail_fast(mut self) -> ApiClient {
        let http = self.http_read.clone();
        let url = self.url(routes::HEALTH);
        let timeout = self.timeout;
        let probe: ProbeFn = Arc::new(move || {
            let rb = http.get(&url).timeout(timeout);
            Box::pin(async move {
                let resp = check_status(rb.send().await.map_err(map_reqwest)?).await?;
                // Drain the body so the connection can be reused.
                let _ = resp.bytes().await;
                Ok(())
            })
        });
        self.reach = Some(Arc::new(Reachability::new(probe)));
        self
    }

    /// [`ApiClient::with_fail_fast`] with another check than `GET /healthz`
    /// (tests count and steer the checks).
    #[cfg(test)]
    pub(crate) fn with_fail_fast_probe(mut self, probe: ProbeFn) -> ApiClient {
        self.reach = Some(Arc::new(Reachability::new(probe)));
        self
    }

    /// Forget that the server was unreachable: the state is
    /// [`Reach::Unknown`] and the next read goes out (the user asked to
    /// refresh). Nothing without [`ApiClient::with_fail_fast`].
    pub fn forget_unreachable(&self) {
        if let Some(reach) = &self.reach {
            reach.forget();
        }
    }

    /// The server is believed unreachable and reads fail at once
    /// ([`Reach::Unreachable`]).
    pub fn believed_unreachable(&self) -> bool {
        self.reach_now().is_unreachable()
    }

    /// What the client knows of the server, as a watch value to subscribe
    /// to (docs/offline.md, "Reachability"). Without
    /// [`ApiClient::with_fail_fast`] it is [`Reach::Unknown`] and never
    /// changes ([`watch::Receiver::changed`] fails at once).
    pub fn reach(&self) -> watch::Receiver<Reach> {
        match &self.reach {
            Some(reach) => reach.value.subscribe(),
            None => watch::channel(Reach::Unknown).1,
        }
    }

    /// The current value of [`ApiClient::reach`].
    pub fn reach_now(&self) -> Reach {
        self.reach.as_ref().map_or(Reach::Unknown, |r| r.now())
    }

    /// A read would fail at once now ([`ApiClient::believed_unreachable`]);
    /// like such a read, this starts the background check again if it is
    /// gone (its runtime ended).
    pub(crate) fn refuses_reads(&self) -> bool {
        self.reach.as_ref().is_some_and(|r| r.refuse_read())
    }

    /// Wait until the state is another than `seen`, and return it: a long
    /// poll for a bridge that cannot hold a watch receiver (UniFFI).
    /// Without [`ApiClient::with_fail_fast`] it returns only if `seen` is
    /// not [`Reach::Unknown`].
    pub async fn reach_changed(&self, seen: Reach) -> Reach {
        let mut rx = self.reach();
        if let Ok(now) = rx.wait_for(|v| *v != seen).await {
            return *now;
        }
        let now = *rx.borrow();
        if now != seen {
            return now;
        }
        std::future::pending().await
    }

    /// Check the server now with `GET /healthz`, in the background: when
    /// the popup or the app opens, or a network appeared. The answer
    /// updates [`ApiClient::reach`] (a failure starts the outage and its
    /// checks, as a failed request does). Nothing if a check is waiting for
    /// its answer already, and nothing without
    /// [`ApiClient::with_fail_fast`]. Callable from any thread: outside a
    /// Tokio runtime the check runs on the runtime of the client's last
    /// request (or the one it was made fail-fast on).
    pub fn check_now(&self) {
        if let Some(reach) = &self.reach {
            reach.check_now();
        }
    }

    pub fn set_token(&self, token: Option<String>) {
        *self.token.write().unwrap_or_else(|e| e.into_inner()) = token;
    }

    pub fn token(&self) -> Option<String> {
        self.token.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn has_token(&self) -> bool {
        self.token.read().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn transfer_timeout(&self) -> Duration {
        self.transfer_timeout
    }

    /// Absolute URL of a route path.
    pub fn url(&self, route_path: &str) -> String {
        format!("{}{}", self.base, route_path)
    }

    // -- plumbing -----------------------------------------------------------

    /// A request with the ordinary timeout; a read connects within
    /// [`INTERACTIVE_CONNECT_TIMEOUT`].
    fn request(&self, method: Method, route_path: &str, auth: bool) -> Result<RequestBuilder> {
        let http = if method == Method::GET {
            &self.http_read
        } else {
            &self.http
        };
        let rb = http.request(method, self.url(route_path)).timeout(self.timeout);
        Ok(self.authorize(rb, auth))
    }

    /// An upload, import, export or file download: it connects within the
    /// ordinary timeout and runs within [`ApiClient::transfer_timeout`].
    fn transfer(&self, method: Method, url_path: &str) -> RequestBuilder {
        let rb = self
            .http
            .request(method, self.url(url_path))
            .timeout(self.transfer_timeout);
        self.authorize(rb, true)
    }

    fn authorize(&self, rb: RequestBuilder, auth: bool) -> RequestBuilder {
        match self.token() {
            Some(t) if auth => rb.bearer_auth(t),
            _ => rb,
        }
    }

    async fn send(&self, rb: RequestBuilder) -> Result<Response> {
        self.send_as(rb, false).await
    }

    /// [`ApiClient::send`]; with `pass_429` a `429 Too Many Requests` is
    /// returned as a response, so that its `Retry-After` can be read.
    async fn send_as(&self, rb: RequestBuilder, pass_429: bool) -> Result<Response> {
        let Some(reach) = self.reach.clone() else {
            return self.answer(rb.send().await, pass_429).await;
        };
        reach.note_runtime();
        let (http, request) = rb.build_split();
        let request = request.map_err(map_reqwest)?;
        if *request.method() == Method::GET && reach.refuse_read() {
            return Err(CoreError::Offline(RECENTLY_UNREACHABLE.into()));
        }
        let result = self.answer(http.execute(request).await, pass_429).await;
        reach.record(&result);
        result
    }

    /// What a request got back: its response, or the error it means (with
    /// `pass_429`, a `429` is a response).
    async fn answer(&self, sent: reqwest::Result<Response>, pass_429: bool) -> Result<Response> {
        let resp = sent.map_err(map_reqwest)?;
        if pass_429 && resp.status() == StatusCode::TOO_MANY_REQUESTS {
            return Ok(resp);
        }
        let public = is_public_path(resp.url().path());
        match check_status(resp).await {
            Err(e) if e.status() == Some(401) && !public && !self.has_token() => Err(CoreError::NotLoggedIn),
            other => other,
        }
    }

    async fn json<T: DeserializeOwned>(&self, rb: RequestBuilder) -> Result<T> {
        let resp = self.send(rb).await?;
        let url = resp.url().path().to_string();
        let bytes = resp.bytes().await.map_err(map_reqwest)?;
        serde_json::from_slice(&bytes).map_err(|e| CoreError::Other(format!("unexpected response from {url}: {e}")))
    }

    async fn empty(&self, rb: RequestBuilder) -> Result<()> {
        let resp = self.send(rb).await?;
        // Drain the body so the connection can be reused.
        let _ = resp.bytes().await;
        Ok(())
    }

    async fn get<T: DeserializeOwned>(&self, route_path: &str, query: &[(&str, String)]) -> Result<T> {
        self.json(self.request(Method::GET, route_path, true)?.query(query))
            .await
    }

    async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(&self, route_path: &str, body: &B) -> Result<T> {
        self.json(self.request(Method::POST, route_path, true)?.json(body))
            .await
    }

    async fn patch<B: Serialize + ?Sized, T: DeserializeOwned>(&self, route_path: &str, body: &B) -> Result<T> {
        self.json(self.request(Method::PATCH, route_path, true)?.json(body))
            .await
    }

    async fn delete(&self, route_path: &str) -> Result<()> {
        self.empty(self.request(Method::DELETE, route_path, true)?).await
    }

    // -- health & auth --------------------------------------------------------

    /// `GET /healthz`; `Ok` if the server answers 2xx.
    pub async fn health(&self) -> Result<()> {
        self.empty(self.request(Method::GET, routes::HEALTH, false)?).await
    }

    /// The OpenAPI document.
    pub async fn openapi(&self) -> Result<serde_json::Value> {
        self.json(self.request(Method::GET, routes::OPENAPI, false)?).await
    }

    /// Password (+TOTP) login. Does not store the returned token anywhere.
    pub async fn login(&self, req: &TokenRequest) -> Result<TokenCreated> {
        self.json(self.request(Method::POST, routes::AUTH_TOKEN, false)?.json(req))
            .await
    }

    /// Exchange a one-time pairing code (made by [`ApiClient::create_pairing`]
    /// on a signed-in device) for a token. Needs no credentials and does not
    /// store the returned token anywhere. A wrong, expired or used code is a
    /// `401 unauthorized`.
    pub async fn pair(&self, req: &PairRequest) -> Result<TokenCreated> {
        self.json(self.request(Method::POST, routes::AUTH_PAIR, false)?.json(req))
            .await
    }

    /// A one-time pairing code for this account, valid for five minutes,
    /// with the `stickpile://pair` link for its QR code.
    pub async fn create_pairing(&self) -> Result<PairingCreated> {
        self.post(routes::PAIRINGS, &serde_json::json!({})).await
    }

    /// Self-service registration (subject to the server's registration policy).
    pub async fn register(&self, req: &RegisterRequest) -> Result<User> {
        self.json(self.request(Method::POST, routes::AUTH_REGISTER, false)?.json(req))
            .await
    }

    pub async fn me(&self) -> Result<User> {
        self.get(routes::ME, &[]).await
    }

    /// PATCH the caller's account (`UpdateMe`: absent fields stay).
    pub async fn update_me(&self, req: &stickpile_api::UpdateMe) -> Result<User> {
        self.patch(routes::ME, req).await
    }

    pub async fn change_password(&self, req: &ChangePassword) -> Result<()> {
        self.empty(self.request(Method::PUT, routes::ME_PASSWORD, true)?.json(req))
            .await
    }

    /// Start TOTP enrolment; confirm with [`ApiClient::totp_confirm`].
    pub async fn totp_setup(&self) -> Result<TotpSetup> {
        self.post(routes::ME_TOTP, &serde_json::json!({})).await
    }

    pub async fn totp_confirm(&self, code: &str) -> Result<User> {
        self.post(routes::ME_TOTP_CONFIRM, &TotpCode { code: code.to_string() })
            .await
    }

    pub async fn totp_disable(&self, code: &str) -> Result<User> {
        self.post(routes::ME_TOTP_DISABLE, &TotpCode { code: code.to_string() })
            .await
    }

    pub async fn tokens(&self) -> Result<Vec<ApiToken>> {
        self.get(routes::TOKENS, &[]).await
    }

    pub async fn create_token(&self, req: &CreateToken) -> Result<TokenCreated> {
        self.post(routes::TOKENS, req).await
    }

    pub async fn revoke_token(&self, id: &str) -> Result<()> {
        self.delete(&path(routes::TOKEN, &[id])).await
    }

    /// Id of the token this client authenticates with, found by its prefix in
    /// the token list.
    pub async fn current_token_id(&self) -> Result<Option<String>> {
        let Some(secret) = self.token() else {
            return Err(CoreError::NotLoggedIn);
        };
        let tokens = self.tokens().await?;
        Ok(tokens
            .into_iter()
            .filter(|t| !t.revoked)
            .filter(|t| {
                let prefix = t.prefix.trim_end_matches('…').trim_end_matches("...");
                !prefix.is_empty() && secret.starts_with(prefix)
            })
            .max_by_key(|t| t.prefix.len())
            .map(|t| t.id))
    }

    // -- sets -----------------------------------------------------------------

    /// `scope`: `mine`, `public` or `all` (server default when `None`).
    pub async fn sets(&self, scope: Option<&str>) -> Result<Vec<SetSummary>> {
        let q: Vec<(&str, String)> = scope.map(|s| ("scope", s.to_string())).into_iter().collect();
        self.get(routes::SETS, &q).await
    }

    pub async fn create_set(&self, req: &CreateSet) -> Result<Set> {
        self.post(routes::SETS, req).await
    }

    pub async fn set(&self, id: &str) -> Result<SetDetail> {
        self.get(&path(routes::SET, &[id]), &[]).await
    }

    pub async fn update_set(&self, id: &str, req: &UpdateSet) -> Result<Set> {
        self.patch(&path(routes::SET, &[id]), req).await
    }

    pub async fn delete_set(&self, id: &str) -> Result<()> {
        self.delete(&path(routes::SET, &[id])).await
    }

    pub async fn reorder_set(&self, id: &str, req: &ReorderItems) -> Result<()> {
        self.empty(
            self.request(Method::PUT, &path(routes::SET_ORDER, &[id]), true)?
                .json(req),
        )
        .await
    }

    /// Multipart upload of one or more files, with optional per-file metadata.
    pub async fn upload(
        &self,
        set_id: &str,
        files: Vec<UploadFile>,
        meta: Option<&[UploadMeta]>,
    ) -> Result<Vec<UploadResult>> {
        let mut form = Form::new();
        for f in files {
            let mime = f
                .mime
                .clone()
                .unwrap_or_else(|| mime_from_name(&f.filename).to_string());
            let part = Part::bytes(f.bytes)
                .file_name(f.filename)
                .mime_str(&mime)
                .map_err(map_reqwest)?;
            form = form.part("file", part);
        }
        self.send_upload(set_id, form, meta).await
    }

    /// [`ApiClient::upload`] of one file on disk, streamed from it: the file
    /// is never read into memory whole (a shared video may be gigabytes), and
    /// the part carries its length. `filename` names the part, as
    /// [`UploadFile::filename`] does (the MIME type is guessed from it);
    /// `meta` is the file's metadata. An `Io` error when the file cannot be
    /// opened.
    pub async fn upload_path(
        &self,
        set_id: &str,
        file: &Path,
        filename: &str,
        meta: Option<&UploadMeta>,
    ) -> Result<Vec<UploadResult>> {
        let in_context =
            |e: std::io::Error| CoreError::Io(std::io::Error::new(e.kind(), format!("{}: {e}", file.display())));
        let f = tokio::fs::File::open(file).await.map_err(in_context)?;
        let len = f.metadata().await.map_err(in_context)?.len();
        let part = Part::stream_with_length(f, len)
            .file_name(filename.to_string())
            .mime_str(mime_from_name(filename))
            .map_err(map_reqwest)?;
        let form = Form::new().part("file", part);
        self.send_upload(set_id, form, meta.map(std::slice::from_ref)).await
    }

    /// Send the `file` parts of `form` with `meta` to [`routes::SET_ITEMS`].
    async fn send_upload(
        &self,
        set_id: &str,
        mut form: Form,
        meta: Option<&[UploadMeta]>,
    ) -> Result<Vec<UploadResult>> {
        if let Some(meta) = meta {
            let json = serde_json::to_vec(meta).map_err(|e| CoreError::Other(e.to_string()))?;
            let part = Part::bytes(json).mime_str("application/json").map_err(map_reqwest)?;
            form = form.part("meta", part);
        }
        let rb = self.transfer(Method::POST, &path(routes::SET_ITEMS, &[set_id]));
        self.json(rb.multipart(form)).await
    }

    /// Download the set directory as a tar (`set.toml` + numbered files).
    pub async fn export_tar(&self, set_id: &str) -> Result<Vec<u8>> {
        let rb = self.transfer(Method::GET, &path(routes::SET_EXPORT, &[set_id]));
        let resp = self.send(rb).await?;
        Ok(resp.bytes().await.map_err(map_reqwest)?.to_vec())
    }

    /// Export a set into `dir` (the set directory itself; created, and media
    /// files not in the export are removed). Returns the bundle written.
    pub async fn export_to_dir(&self, set_id: &str, dir: &Path) -> Result<stickpile_setfmt::SetBundle> {
        let tar = self.export_tar(set_id).await?;
        let dir = dir.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<stickpile_setfmt::SetBundle> {
            let bundle = stickpile_setfmt::read_tar(&tar, MAX_EXPORT_BYTES)?;
            stickpile_setfmt::write_dir(&dir, &bundle)?;
            Ok(bundle)
        })
        .await
        .map_err(|e| CoreError::Other(format!("export task: {e}")))?
    }

    /// Import a set from a tar (`archive` multipart part). `into` re-imports
    /// into an existing own set.
    pub async fn import_tar(&self, tar: Vec<u8>, into: Option<&str>) -> Result<ImportReport> {
        let part = Part::bytes(tar)
            .file_name("set.tar")
            .mime_str("application/x-tar")
            .map_err(map_reqwest)?;
        let form = Form::new().part("archive", part);
        let q: Vec<(&str, String)> = into.map(|s| ("into", s.to_string())).into_iter().collect();
        let rb = self.transfer(Method::POST, routes::SETS_IMPORT).query(&q);
        self.json(rb.multipart(form)).await
    }

    /// Read a set directory with `stickpile-setfmt`, pack it as a tar and
    /// import it. Returns the report and the warnings from reading the directory.
    pub async fn import_dir(&self, dir: &Path, into: Option<&str>) -> Result<(ImportReport, Vec<String>)> {
        let dir = dir.to_path_buf();
        let (tar, warnings) = tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, Vec<String>)> {
            let bundle = stickpile_setfmt::read_dir(&dir)?;
            let top = bundle
                .manifest
                .slug
                .clone()
                .or_else(|| dir.file_name().map(|n| stickpile_setfmt::slugify(&n.to_string_lossy())))
                .unwrap_or_else(|| "set".into());
            let tar = stickpile_setfmt::write_tar(&bundle, &stickpile_setfmt::slugify(&top))?;
            Ok((tar, bundle.warnings))
        })
        .await
        .map_err(|e| CoreError::Other(format!("import task: {e}")))??;
        let report = self.import_tar(tar, into).await?;
        Ok((report, warnings))
    }

    pub async fn create_gitpack(&self, req: &CreateGitPack) -> Result<GitPackResponse> {
        self.post(routes::SETS_GITPACK, req).await
    }

    /// Re-pull a git pack now.
    pub async fn pull_set(&self, set_id: &str) -> Result<Job> {
        self.post(&path(routes::SET_PULL, &[set_id]), &serde_json::json!({}))
            .await
    }

    /// Trigger the (unauthenticated) git webhook with the shared secret.
    pub async fn git_webhook(&self, set_id: &str, secret: &str) -> Result<()> {
        let rb = self.request(Method::POST, &path(routes::GIT_WEBHOOK, &[set_id]), false)?;
        self.empty(rb.header("X-Stickpile-Webhook-Secret", secret)).await
    }

    pub async fn telegram_export(&self, set_id: &str, req: &TelegramExport) -> Result<Job> {
        self.post(&path(routes::SET_TELEGRAM_EXPORT, &[set_id]), req).await
    }

    pub async fn telegram_import(&self, req: &TelegramImport) -> Result<Job> {
        self.post(routes::TELEGRAM_IMPORT, req).await
    }

    // -- export page: archives, publishing, WhatsApp ---------------------------

    /// Download a set export as `format` into `dest` (via a temporary file,
    /// see [`ApiClient::download`]). Returns the number of bytes.
    pub async fn download_export(&self, set_id: &str, format: SetArchive, dest: &Path) -> Result<u64> {
        self.download(&path(format.route(), &[set_id]), dest).await
    }

    /// Where the set is published, per platform the server knows (owner).
    pub async fn publications(&self, set_id: &str) -> Result<Vec<Publication>> {
        self.get(&path(routes::SET_PUBLISH, &[set_id]), &[]).await
    }

    /// Publish a set to several platforms at once: one job per platform
    /// (owner). The whole request fails if one platform is unknown, not
    /// configured, or refuses its options.
    pub async fn publish(&self, set_id: &str, req: &PublishRequest) -> Result<PublishJobs> {
        self.post(&path(routes::SET_PUBLISH, &[set_id]), req).await
    }

    /// The set's last WhatsApp build and whether it is up to date.
    pub async fn whatsapp_status(&self, set_id: &str) -> Result<WhatsappStatus> {
        self.get(&path(routes::SET_WHATSAPP, &[set_id]), &[]).await
    }

    /// Build the set's WhatsApp sticker packs (a job; an unchanged set
    /// reuses the last build).
    pub async fn whatsapp_build(&self, set_id: &str) -> Result<Job> {
        self.post(&path(routes::SET_WHATSAPP, &[set_id]), &serde_json::json!({}))
            .await
    }

    /// Download the last WhatsApp build (every `.wastickers` plus
    /// `contents.json`, one zip) into `dest`.
    pub async fn download_whatsapp(&self, set_id: &str, dest: &Path) -> Result<u64> {
        self.download(&path(routes::SET_WHATSAPP_ZIP, &[set_id]), dest).await
    }

    /// Download pack `number` of the last WhatsApp build (a `.wastickers`)
    /// into `dest`.
    pub async fn download_whatsapp_pack(&self, set_id: &str, number: u32, dest: &Path) -> Result<u64> {
        self.download(&path(routes::SET_WHATSAPP_PACK, &[set_id, &number.to_string()]), dest)
            .await
    }

    // -- items ----------------------------------------------------------------

    /// One page of the client-cache feed.
    pub async fn items(
        &self,
        updated_since: Option<i64>,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<ItemPage> {
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(s) = updated_since {
            q.push(("updated_since", s.to_string()));
        }
        if let Some(c) = cursor {
            q.push(("cursor", c.to_string()));
        }
        if let Some(l) = limit {
            q.push(("limit", l.to_string()));
        }
        self.get(routes::ITEMS, &q).await
    }

    pub async fn item(&self, id: &str) -> Result<ItemDetail> {
        self.get(&path(routes::ITEM, &[id]), &[]).await
    }

    pub async fn update_item(&self, id: &str, req: &UpdateItem) -> Result<Item> {
        self.patch(&path(routes::ITEM, &[id]), req).await
    }

    pub async fn delete_item(&self, id: &str) -> Result<()> {
        self.delete(&path(routes::ITEM, &[id])).await
    }

    /// Raw bytes of a variant and its `Content-Type`.
    pub async fn item_file(&self, id: &str, variant: Variant) -> Result<(Vec<u8>, Option<String>)> {
        let rb = self.transfer(Method::GET, &path(routes::ITEM_FILE, &[id, variant.as_str()]));
        let resp = self.send(rb).await?;
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        Ok((resp.bytes().await.map_err(map_reqwest)?.to_vec(), ct))
    }

    /// Stream a variant into `dest` (via a temporary file + rename, so `dest`
    /// is never partial). Returns the number of bytes written.
    pub async fn download_item_file(&self, id: &str, variant: Variant, dest: &Path) -> Result<u64> {
        self.download(&path(routes::ITEM_FILE, &[id, variant.as_str()]), dest)
            .await
    }

    /// [`ApiClient::download_item_file`] for a URL path the server handed
    /// out, such as a set's `cover_url` (query string included).
    pub async fn download(&self, url_path: &str, dest: &Path) -> Result<u64> {
        use tokio::io::AsyncWriteExt;
        let rb = self.transfer(Method::GET, url_path);
        let mut resp = self.send(rb).await?;
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = dest.with_file_name(format!(
            ".{}.{}.part",
            dest.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            crate::new_id()
        ));
        let result = async {
            let mut f = tokio::fs::File::create(&tmp).await?;
            let mut n = 0u64;
            while let Some(chunk) = resp.chunk().await.map_err(map_reqwest)? {
                f.write_all(&chunk).await?;
                n += chunk.len() as u64;
            }
            f.flush().await?;
            drop(f);
            tokio::fs::rename(&tmp, dest).await?;
            Ok::<u64, CoreError>(n)
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&tmp).await;
        }
        result
    }

    /// A file for the offline downloader ([`crate::offline`]): the response
    /// of `GET url_path` to stream when the server sends the file, or how
    /// long a `429` asks to wait. Other errors as for every request.
    pub(crate) async fn fetch_file(&self, url_path: &str) -> Result<FileFetch> {
        let rb = self.transfer(Method::GET, url_path);
        let resp = self.send_as(rb, true).await?;
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            let wait = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            // Drain the body so the connection can be reused.
            let _ = resp.bytes().await;
            return Ok(FileFetch::RateLimited(wait));
        }
        let sha256 = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .and_then(etag_sha256);
        Ok(FileFetch::Body { resp, sha256 })
    }

    pub async fn like(&self, id: &str) -> Result<LikeState> {
        self.json(self.request(Method::PUT, &path(routes::ITEM_LIKE, &[id]), true)?)
            .await
    }

    pub async fn unlike(&self, id: &str) -> Result<LikeState> {
        self.json(self.request(Method::DELETE, &path(routes::ITEM_LIKE, &[id]), true)?)
            .await
    }

    pub async fn better(&self, id: &str) -> Result<Vec<ItemSummary>> {
        self.get(&path(routes::ITEM_BETTER, &[id]), &[]).await
    }

    pub async fn enrich(&self, id: &str) -> Result<Job> {
        self.post(&path(routes::ITEM_ENRICH, &[id]), &serde_json::json!({}))
            .await
    }

    // -- search, usage, stats ---------------------------------------------------

    pub async fn search(&self, p: &SearchParams) -> Result<SearchResults> {
        let mut q: Vec<(&str, String)> = vec![("q", p.q.clone())];
        if let Some(l) = p.limit {
            q.push(("limit", l.to_string()));
        }
        if let Some(o) = p.offset {
            q.push(("offset", o.to_string()));
        }
        if let Some(t) = p.target {
            q.push(("target", t.as_str().to_string()));
        }
        self.get(routes::SEARCH, &q).await
    }

    pub async fn usage(&self, batch: &UsageBatch) -> Result<UsageAck> {
        self.post(routes::USAGE, batch).await
    }

    pub async fn popular(&self, p: &PopularParams) -> Result<Popular> {
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(s) = &p.scope {
            q.push(("scope", s.clone()));
        }
        if let Some(s) = &p.set {
            q.push(("set", s.clone()));
        }
        if let Some(w) = &p.window {
            q.push(("window", w.clone()));
        }
        if let Some(l) = p.limit {
            q.push(("limit", l.to_string()));
        }
        self.get(routes::POPULAR, &q).await
    }

    pub async fn stats_me(&self) -> Result<UserStats> {
        self.get(routes::STATS_ME, &[]).await
    }

    pub async fn profiles(&self) -> Result<Vec<TargetProfile>> {
        self.get(routes::PROFILES, &[]).await
    }

    pub async fn job(&self, id: &str) -> Result<Job> {
        self.get(&path(routes::JOB, &[id]), &[]).await
    }

    /// Poll a job every `interval` until it is done or failed.
    pub async fn wait_job(&self, id: &str, interval: Duration, max_wait: Duration) -> Result<Job> {
        let deadline = tokio::time::Instant::now() + max_wait;
        loop {
            let job = self.job(id).await?;
            if matches!(job.status, JobStatus::Done | JobStatus::Failed) || tokio::time::Instant::now() >= deadline {
                return Ok(job);
            }
            tokio::time::sleep(interval).await;
        }
    }

    // -- rechecks ---------------------------------------------------------------
    //
    // Servers older than rechecks (0.7.0) answer these routes with 404: the
    // calls then fail with [`SERVER_TOO_OLD_FOR_RECHECKS`].

    /// `POST /api/v1/rechecks`: recheck the caller's own items. `started`
    /// is false when the same recheck was running and is returned instead.
    pub async fn start_recheck(&self, req: &StartRecheck) -> Result<RecheckStarted> {
        self.start_recheck_at(routes::RECHECKS, req).await
    }

    /// `POST /api/v1/admin/rechecks`: recheck every item on the server (admins).
    pub async fn admin_start_recheck(&self, req: &StartRecheck) -> Result<RecheckStarted> {
        self.start_recheck_at(routes::ADMIN_RECHECKS, req).await
    }

    async fn start_recheck_at(&self, route: &str, req: &StartRecheck) -> Result<RecheckStarted> {
        let rb = self.request(Method::POST, route, true)?.json(req);
        let resp = match self.send(rb).await {
            Ok(r) => r,
            Err(e) => return Err(self.recheck_error(e).await),
        };
        let started = resp.status() == StatusCode::CREATED;
        let bytes = resp.bytes().await.map_err(map_reqwest)?;
        let recheck = serde_json::from_slice(&bytes)
            .map_err(|e| CoreError::Other(format!("unexpected response from {route}: {e}")))?;
        Ok(RecheckStarted { recheck, started })
    }

    /// The caller's rechecks (an admin's include the server-wide ones), newest first.
    pub async fn rechecks(&self) -> Result<Vec<Recheck>> {
        match self.get(routes::RECHECKS, &[]).await {
            Err(e) if e.status() == Some(404) => Err(CoreError::Other(SERVER_TOO_OLD_FOR_RECHECKS.into())),
            other => other,
        }
    }

    pub async fn recheck(&self, id: &str) -> Result<Recheck> {
        match self.get(&path(routes::RECHECK, &[id]), &[]).await {
            Err(e) => Err(self.recheck_error(e).await),
            ok => ok,
        }
    }

    pub async fn recheck_failures(&self, id: &str) -> Result<Vec<RecheckFailure>> {
        match self.get(&path(routes::RECHECK_FAILURES, &[id]), &[]).await {
            Err(e) => Err(self.recheck_error(e).await),
            ok => ok,
        }
    }

    pub async fn cancel_recheck(&self, id: &str) -> Result<Recheck> {
        match self
            .post(&path(routes::RECHECK_CANCEL, &[id]), &serde_json::json!({}))
            .await
        {
            Err(e) => Err(self.recheck_error(e).await),
            ok => ok,
        }
    }

    /// Poll a recheck every `interval` until it is over (or `max_wait`
    /// passed), calling `progress` with each answer.
    pub async fn wait_recheck(
        &self,
        id: &str,
        interval: Duration,
        max_wait: Duration,
        mut progress: impl FnMut(&Recheck),
    ) -> Result<Recheck> {
        let deadline = tokio::time::Instant::now() + max_wait;
        loop {
            let r = self.recheck(id).await?;
            progress(&r);
            if r.state != RecheckState::Running || tokio::time::Instant::now() >= deadline {
                return Ok(r);
            }
            tokio::time::sleep(interval).await;
        }
    }

    /// A 404 from a recheck route: the server is too old for rechecks when
    /// it does not know the list either; else the error as it was (an
    /// unknown recheck or set).
    async fn recheck_error(&self, e: CoreError) -> CoreError {
        if e.status() != Some(404) {
            return e;
        }
        match self.get::<serde_json::Value>(routes::RECHECKS, &[]).await {
            Err(probe) if probe.status() == Some(404) => CoreError::Other(SERVER_TOO_OLD_FOR_RECHECKS.into()),
            _ => e,
        }
    }

    // -- admin ------------------------------------------------------------------

    pub async fn admin_users(&self) -> Result<Vec<AdminUser>> {
        self.get(routes::ADMIN_USERS, &[]).await
    }

    pub async fn admin_create_user(&self, req: &AdminCreateUser) -> Result<AdminUser> {
        self.post(routes::ADMIN_USERS, req).await
    }

    pub async fn admin_update_user(&self, id: &str, req: &AdminUpdateUser) -> Result<AdminUser> {
        self.patch(&path(routes::ADMIN_USER, &[id]), req).await
    }

    pub async fn admin_delete_user(&self, id: &str) -> Result<()> {
        self.delete(&path(routes::ADMIN_USER, &[id])).await
    }

    pub async fn admin_invitations(&self) -> Result<Vec<Invitation>> {
        self.get(routes::ADMIN_INVITATIONS, &[]).await
    }

    pub async fn admin_create_invitation(&self, req: &CreateInvitation) -> Result<InvitationCreated> {
        self.post(routes::ADMIN_INVITATIONS, req).await
    }

    pub async fn admin_delete_invitation(&self, id: &str) -> Result<()> {
        self.delete(&path(routes::ADMIN_INVITATION, &[id])).await
    }

    pub async fn admin_audit(&self, before: Option<i64>, limit: Option<u32>) -> Result<Vec<AuditEntry>> {
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(b) = before {
            q.push(("before", b.to_string()));
        }
        if let Some(l) = limit {
            q.push(("limit", l.to_string()));
        }
        self.get(routes::ADMIN_AUDIT, &q).await
    }

    pub async fn admin_storage(&self) -> Result<StorageUsage> {
        self.get(routes::ADMIN_STORAGE, &[]).await
    }

    pub async fn admin_jobs(&self, status: Option<JobStatus>) -> Result<Vec<Job>> {
        let q: Vec<(&str, String)> = status
            .map(|s| {
                (
                    "status",
                    serde_json::to_value(s)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default(),
                )
            })
            .into_iter()
            .collect();
        self.get(routes::ADMIN_JOBS, &q).await
    }
}

/// What [`ApiClient::fetch_file`] got.
pub(crate) enum FileFetch {
    /// The file: the response to stream, and the sha256 its `ETag` names.
    Body { resp: Response, sha256: Option<String> },
    /// `429 Too Many Requests`: wait this long (`Retry-After` in seconds;
    /// `None` without one).
    RateLimited(Option<Duration>),
}

/// The sha256 a strong `ETag` of 64 hex digits names (the server's file
/// route sends the file's sha256 so); `None` for anything else.
fn etag_sha256(etag: &str) -> Option<String> {
    let tag = etag.trim();
    if tag.starts_with("W/") {
        return None;
    }
    let tag = tag.trim_matches('"');
    (tag.len() == 64 && tag.chars().all(|c| c.is_ascii_hexdigit())).then(|| tag.to_ascii_lowercase())
}

/// Routes that never need credentials (a 401 there is not "not logged in").
fn is_public_path(path: &str) -> bool {
    [
        routes::HEALTH,
        routes::OPENAPI,
        routes::AUTH_TOKEN,
        routes::AUTH_PAIR,
        routes::AUTH_REGISTER,
    ]
    .iter()
    .any(|r| path.ends_with(r))
        || path.contains("/api/v1/hooks/git/")
}

fn error_chain(e: &dyn std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        let s_msg = s.to_string();
        if !msg.contains(&s_msg) {
            msg.push_str(": ");
            msg.push_str(&s_msg);
        }
        src = s.source();
    }
    msg
}

/// Transport errors: connect/timeout/request failures are "offline".
pub(crate) fn map_reqwest(e: reqwest::Error) -> CoreError {
    if e.is_connect() || e.is_timeout() || e.is_request() || e.is_body() {
        CoreError::Offline(error_chain(&e))
    } else {
        CoreError::Other(error_chain(&e))
    }
}

fn code_for_status(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => "bad_request",
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        409 => "conflict",
        413 => "too_large",
        415 => "unsupported_media",
        429 => "rate_limited",
        _ => "internal",
    }
}

async fn check_status(resp: Response) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let bytes = resp.bytes().await.unwrap_or_default();
    match serde_json::from_slice::<ApiError>(&bytes) {
        Ok(error) => Err(CoreError::Api {
            status: status.as_u16(),
            error,
        }),
        Err(_) if matches!(status.as_u16(), 502..=504) => {
            Err(CoreError::Offline(format!("{status} from the server or its proxy")))
        }
        Err(_) => {
            let text = String::from_utf8_lossy(&bytes);
            let mut message: String = text.trim().chars().take(300).collect();
            if message.is_empty() {
                message = status.canonical_reason().unwrap_or("error").to_string();
            }
            Err(CoreError::Api {
                status: status.as_u16(),
                error: ApiError {
                    code: code_for_status(status).into(),
                    message,
                },
            })
        }
    }
}

/// Build an [`ItemSummary`] from an [`ItemDetail`] (e.g. for copying by id).
pub fn summary_from_detail(d: &ItemDetail) -> ItemSummary {
    summary_of(&d.item, &d.set.slug, &d.set.name, &d.set.owner_username)
}

/// Build an [`ItemSummary`] from an item of a [`SetDetail`].
pub fn summary_from_set_item(item: &Item, set: &Set) -> ItemSummary {
    summary_of(item, &set.slug, &set.name, &set.owner_username)
}

fn summary_of(i: &Item, set_slug: &str, set_name: &str, owner_username: &str) -> ItemSummary {
    ItemSummary {
        id: i.id.clone(),
        set_id: i.set_id.clone(),
        set_slug: set_slug.to_string(),
        set_name: set_name.to_string(),
        owner_username: owner_username.to_string(),
        name: i.name.clone(),
        keywords: i.keywords.clone(),
        emojis: i.emojis.clone(),
        vibe: i.vibe.clone(),
        tags: i.tags.clone(),
        text: i.text.clone(),
        kind: i.media.kind,
        format: i.media.format,
        width: i.media.width,
        height: i.media.height,
        quality: i.media.quality,
        likes: i.likes,
        copies: i.copies,
        media_id: i.media.id.clone(),
        sha256: i.media.sha256.clone(),
        phash: i.media.phash.clone(),
        variants: i.media.variants.clone(),
        updated_at: i.updated_at,
    }
}
