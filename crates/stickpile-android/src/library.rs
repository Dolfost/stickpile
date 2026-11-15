//! The [`Library`] object: one [`Session`] on a Tokio runtime of its own,
//! plus what the app needs around it (the sync schedule, the config file,
//! the items it has shown).

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use stickpile_api::{pairing, ItemSummary, PairRequest, TokenRequest, UploadMeta, Variant};
use stickpile_core::client::UploadFile as CoreUploadFile;
use stickpile_core::{settings, token, ApiClient, ClientConfig, CoreError, Session, SyncPolicy};
use stickpile_quick::suggest::{self, Vocabulary};

use crate::types::*;
use crate::{device, LibraryError, LibraryErrorKind};

/// Worker threads of the library's runtime (network and files only; the
/// work is small).
const WORKER_THREADS: usize = 2;

/// Items kept by id after a list showed them, so that `thumbnail`,
/// `prepare` and `record_use` need no request for them. Beyond this the
/// memory is cleared and refills from the next lists.
const SEEN_MAX: usize = 4096;

/// Cache `meta` key prefix of the favorites last read from the server, per
/// user id: their ids, one per line. The cache keeps no likes, so this is
/// what [`Library::favorites`] answers with offline.
const FAVORITES_KEY: &str = "android_favorites:";

/// The stickpile library: an account on one server, its offline cache, and
/// the sync. Create one per process and share it; every method may be
/// called from any thread. `suspend` methods run on the library's own Tokio
/// runtime; the others only read local state (the config, the offline
/// cache) and return quickly.
#[derive(uniffi::Object)]
pub struct Library {
    /// `Some` until the library is dropped.
    runtime: Option<tokio::runtime::Runtime>,
    handle: tokio::runtime::Handle,
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library").field("dirs", &self.inner.dirs).finish()
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // Never blocks, so it is fine wherever the last reference goes.
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

#[derive(Debug)]
struct Dirs {
    config_file: PathBuf,
    cache_dir: PathBuf,
    token_file: PathBuf,
}

impl Dirs {
    /// The config file (layered as on the desktop, see
    /// `stickpile_core::settings`) with the library's own cache and token.
    fn load_config(&self) -> Result<ClientConfig, LibraryError> {
        let layered = settings::load_layered(&self.config_file)?;
        for w in &layered.warnings {
            tracing::warn!("{w}");
        }
        let mut config = layered.config;
        config.cache_dir = Some(self.cache_dir.clone());
        config.token_file = Some(self.token_file.clone());
        Ok(config)
    }

    /// Save `url` as the config file's `server_url`, keeping the rest of it.
    fn save_server_url(&self, url: &str) -> Result<(), LibraryError> {
        let layered = settings::load_layered(&self.config_file)?;
        settings::set_value(&layered.write_path, "server_url", toml::Value::String(url.to_string()))?;
        Ok(())
    }
}

struct Inner {
    dirs: Dirs,
    /// Replaced when the server changes.
    session: RwLock<Arc<Session>>,
    sync: Mutex<SyncStatus>,
    /// Held while a sync runs and while the account or server changes, so
    /// the two never overlap.
    busy: tokio::sync::Mutex<()>,
    seen: Mutex<HashMap<String, ItemSummary>>,
    /// The suggestions' vocabulary and the cache generation it was made at.
    vocabulary: Mutex<Option<(u64, Arc<Vocabulary>)>>,
}

/// The sync schedule of the current session and account.
struct SyncStatus {
    policy: SyncPolicy,
    /// Unix time of the last sync that went through in this process.
    last_ok: Option<i64>,
    last_error: Option<LibraryErrorKind>,
}

impl SyncStatus {
    /// A fresh schedule, as if the last sync had been at the cache's sync
    /// cursor (the server's time of it), so a restarted app does not sync
    /// again at once. No cursor (a new account or server): due now.
    fn new(session: &Session) -> SyncStatus {
        let mut policy = SyncPolicy::default();
        if let Some(cursor) = session.cache().sync_cursor().ok().flatten() {
            let age = Duration::from_secs(u64::try_from(now_unix() - cursor).unwrap_or(0));
            if let Some(at) = Instant::now().checked_sub(age) {
                policy.started(at);
                policy.finished(at, true);
            }
        }
        SyncStatus {
            policy,
            last_ok: None,
            last_error: None,
        }
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Errors after which the offline cache answers instead (as in `Session`):
/// the server is unreachable or failing.
fn falls_back(e: &CoreError) -> bool {
    e.is_offline() || e.status().is_some_and(|s| s >= 500)
}

/// A server URL as typed: trimmed, `http` or `https` with a host, without
/// the trailing `/`.
fn server_url_from(input: &str) -> Result<String, LibraryError> {
    let url = input.trim().trim_end_matches('/');
    let parsed = url::Url::parse(url).map_err(|e| LibraryError::invalid("server_url", format!("{url:?}: {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(LibraryError::invalid(
            "server_url",
            format!("{url:?}: not an http or https URL"),
        ));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(LibraryError::invalid("server_url", format!("{url:?}: no host")));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(LibraryError::invalid(
            "server_url",
            format!("{url:?}: a query or fragment"),
        ));
    }
    Ok(url.to_string())
}

/// Aborts a task when the caller stops waiting for it (the Kotlin coroutine
/// was cancelled). Aborting a finished task does nothing.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Inner {
    fn session(&self) -> Arc<Session> {
        self.session.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Use `session` from now on (the server changed).
    fn replace_session(&self, session: Session) {
        let session = Arc::new(session);
        *lock(&self.sync) = SyncStatus::new(&session);
        lock(&self.seen).clear();
        *lock(&self.vocabulary) = None;
        *self.session.write().unwrap_or_else(|e| e.into_inner()) = session;
    }

    /// The account changed on the same server (a login or logout).
    fn account_changed(&self) {
        *lock(&self.sync) = SyncStatus::new(&self.session());
        lock(&self.seen).clear();
    }

    fn remember(&self, items: &[ItemSummary]) {
        let mut seen = lock(&self.seen);
        if seen.len() + items.len() > SEEN_MAX {
            seen.clear();
        }
        for item in items {
            seen.insert(item.id.clone(), item.clone());
        }
    }

    fn list(&self, items: Vec<ItemSummary>, offline: bool) -> ItemList {
        self.remember(&items);
        ItemList {
            items: items.iter().map(Item::from).collect(),
            offline,
        }
    }

    /// An item shown before, else the cached one; no request.
    fn local_item(&self, session: &Session, id: &str) -> Result<Option<ItemSummary>, LibraryError> {
        if let Some(item) = lock(&self.seen).get(id) {
            return Ok(Some(item.clone()));
        }
        Ok(session.cache().item(id)?)
    }

    /// An item shown before, else the cached one, else from the server.
    async fn item_local_first(&self, session: &Session, id: &str) -> Result<ItemSummary, LibraryError> {
        if let Some(item) = self.local_item(session, id)? {
            return Ok(item);
        }
        let item = session.item_summary(id).await?;
        self.remember(std::slice::from_ref(&item));
        Ok(item)
    }

    /// An item shown before, else from the server (with its current
    /// renditions), else the cached one when the server is unreachable.
    async fn item_fresh_first(&self, session: &Session, id: &str) -> Result<ItemSummary, LibraryError> {
        if let Some(item) = lock(&self.seen).get(id) {
            return Ok(item.clone());
        }
        let item = session.item_summary(id).await?;
        self.remember(std::slice::from_ref(&item));
        Ok(item)
    }

    fn vocabulary(&self, session: &Session) -> Arc<Vocabulary> {
        let generation = session.cache().generation().unwrap_or(0);
        let mut slot = lock(&self.vocabulary);
        if let Some((g, v)) = slot.as_ref() {
            if *g == generation {
                return v.clone();
            }
        }
        let vocabulary = match session.offline_index() {
            Ok(index) => Arc::new(Vocabulary::from_items(index.items())),
            Err(e) => {
                tracing::debug!("no suggestions from the cache: {e}");
                Arc::new(Vocabulary::default())
            }
        };
        *slot = Some((generation, vocabulary.clone()));
        vocabulary
    }

    fn favorites_key(session: &Session) -> Option<String> {
        let me = session.cache().me().ok().flatten()?;
        Some(format!("{FAVORITES_KEY}{}", me.id))
    }

    fn save_favorites(&self, session: &Session, ids: &[String]) {
        let Some(key) = Self::favorites_key(session) else {
            return;
        };
        if let Err(e) = session.cache().meta_set(&key, &ids.join("\n")) {
            tracing::debug!("remembering the favorites: {e}");
        }
    }

    /// The favorites last read from the server, as far as they are cached;
    /// `None` when there are none.
    fn saved_favorites(&self, session: &Session) -> Result<Option<Vec<ItemSummary>>, LibraryError> {
        let Some(key) = Self::favorites_key(session) else {
            return Ok(None);
        };
        let Some(ids) = session.cache().meta_get(&key)? else {
            return Ok(None);
        };
        let mut items = Vec::new();
        for id in ids.lines().filter(|l| !l.is_empty()) {
            if let Some(item) = session.cache().item(id)? {
                items.push(item);
            }
        }
        Ok(Some(items))
    }

    /// Keep the saved favorites in step with a like that went through.
    fn favorite_changed(&self, session: &Session, item_id: &str, liked: bool) {
        let Some(key) = Self::favorites_key(session) else {
            return;
        };
        let Ok(Some(saved)) = session.cache().meta_get(&key) else {
            return;
        };
        let mut ids: Vec<String> = saved
            .lines()
            .filter(|l| !l.is_empty() && *l != item_id)
            .map(str::to_string)
            .collect();
        if liked {
            ids.insert(0, item_id.to_string());
        }
        self.save_favorites(session, &ids);
    }

    fn sync_state(&self) -> SyncState {
        let session = self.session();
        let status = lock(&self.sync);
        SyncState {
            last_sync: status.last_ok.or_else(|| session.cache().sync_cursor().ok().flatten()),
            last_error: status.last_error,
            pending_usage: to_u32(session.pending_usage()),
            due: status.policy.due(Instant::now()),
        }
    }

    /// Sync now; the caller holds `busy`.
    async fn run_sync(&self) -> Result<SyncState, LibraryError> {
        let session = self.session();
        lock(&self.sync).policy.started(Instant::now());
        let result = session.sync().await.map_err(LibraryError::from);
        {
            let mut status = lock(&self.sync);
            status.policy.finished(Instant::now(), result.is_ok());
            match &result {
                Ok(()) => {
                    status.last_ok = Some(now_unix());
                    status.last_error = None;
                }
                Err(e) => status.last_error = Some(e.kind()),
            }
        }
        result.map(|()| self.sync_state())
    }
}

async fn open_session(config: ClientConfig) -> Result<Session, LibraryError> {
    Ok(Session::open(config).await?.with_client_name(crate::client_name()))
}

impl Library {
    /// Run `f` on the runtime. When the caller stops waiting (the Kotlin
    /// coroutine was cancelled), the work is aborted: for calls that only
    /// read.
    async fn read<T, F, Fut>(&self, f: F) -> Result<T, LibraryError>
    where
        F: FnOnce(Arc<Inner>) -> Fut,
        Fut: Future<Output = Result<T, LibraryError>> + Send + 'static,
        T: Send + 'static,
    {
        let task = self.handle.spawn(f(self.inner.clone()));
        let _abort = AbortOnDrop(task.abort_handle());
        task.await?
    }

    /// Run `f` on the runtime to its end, even when the caller stops waiting:
    /// for calls that change something or write files.
    async fn write<T, F, Fut>(&self, f: F) -> Result<T, LibraryError>
    where
        F: FnOnce(Arc<Inner>) -> Fut,
        Fut: Future<Output = Result<T, LibraryError>> + Send + 'static,
        T: Send + 'static,
    {
        self.handle.spawn(f(self.inner.clone())).await?
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Library {
    /// Open the library: read `dirs.config_file` if it exists (else the
    /// defaults), keep the cache and the token in `dirs.cache_dir` and
    /// `dirs.token_file`, and open the offline cache. Nothing goes to the
    /// network, so this works offline and signed out.
    ///
    /// Throws `InvalidInput` (field `config_file`, `cache_dir` or
    /// `token_file`) for a path that is not absolute or a config file that
    /// cannot be read as a stickpile config (deleting it gives the
    /// defaults), `Io` when the cache cannot be opened, `Other` for a token
    /// file readable by others.
    #[uniffi::constructor]
    pub fn new(dirs: LibraryDirs) -> Result<Arc<Library>, LibraryError> {
        let path = |field: &str, p: &str| {
            let p = PathBuf::from(p);
            if p.is_absolute() {
                Ok(p)
            } else {
                Err(LibraryError::invalid(field, format!("{p:?} is not an absolute path")))
            }
        };
        let dirs = Dirs {
            config_file: path("config_file", &dirs.config_file)?,
            cache_dir: path("cache_dir", &dirs.cache_dir)?,
            token_file: path("token_file", &dirs.token_file)?,
        };
        let config = dirs.load_config()?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKER_THREADS)
            .thread_name("stickpile-rt")
            .enable_all()
            .build()
            .map_err(|e| LibraryError::Io {
                detail: format!("starting the runtime: {e}"),
            })?;
        let handle = runtime.handle().clone();
        // On a thread of its own: `block_on` must not run inside another
        // runtime, and the caller may be in one.
        let opened = std::thread::scope(|s| {
            s.spawn(|| handle.block_on(open_session(config)))
                .join()
                .unwrap_or_else(|_| Err(LibraryError::other("opening the session panicked")))
        });
        let session = match opened {
            Ok(session) => session,
            Err(e) => {
                // Dropping a runtime blocks, which panics in async code.
                runtime.shutdown_background();
                return Err(e);
            }
        };
        let inner = Inner {
            dirs,
            sync: Mutex::new(SyncStatus::new(&session)),
            session: RwLock::new(Arc::new(session)),
            busy: tokio::sync::Mutex::new(()),
            seen: Mutex::new(HashMap::new()),
            vocabulary: Mutex::new(None),
        };
        tracing::info!(
            "stickpile {} (Android library), server {}",
            crate::version(),
            inner.session().config().server_url
        );
        Ok(Arc::new(Library {
            runtime: Some(runtime),
            handle,
            inner: Arc::new(inner),
        }))
    }

    // -- account ----------------------------------------------------------------

    /// The server's URL, as configured (`http://localhost:8080` until one
    /// is set).
    pub fn server_url(&self) -> String {
        self.inner.session().config().server_url.clone()
    }

    /// Use another server: `url` is checked (`http` or `https` with a host;
    /// spaces and a trailing `/` are dropped), saved as `server_url` in the
    /// config file and the library reopened for it. The cache belongs to
    /// one server: going to another drops the cached items, history and
    /// unsent uses. The stored token stays (it may be the same server under
    /// another URL). Throws `InvalidInput` (field `server_url`) for a bad
    /// URL, nothing changed; `Io` when the config cannot be written.
    pub async fn set_server_url(&self, url: String) -> Result<(), LibraryError> {
        self.write(move |inner| async move {
            let url = server_url_from(&url)?;
            let _busy = inner.busy.lock().await;
            if inner.session().config().server_url.trim_end_matches('/') == url {
                return Ok(());
            }
            inner.dirs.save_server_url(&url)?;
            let session = open_session(inner.dirs.load_config()?).await?;
            inner.replace_session(session);
            Ok(())
        })
        .await
    }

    /// Who is signed in, asking the server; `None` when nobody is (no token
    /// and no tailnet identity, or the server refused the token). When the
    /// server cannot be reached, the account the cache last saw; throws
    /// `Unreachable` if there is none.
    pub async fn account(&self) -> Result<Option<Account>, LibraryError> {
        self.read(|inner| async move {
            let session = inner.session();
            let server_url = session.config().server_url.clone();
            let auth = if session.is_logged_in() {
                AuthKind::Token
            } else {
                AuthKind::Tailnet
            };
            match session.client().me().await {
                Ok(user) => Ok(Some(Account::new(&user, &server_url, auth))),
                Err(e) if e.is_unauthorized() => Ok(None),
                Err(e) if falls_back(&e) => match session.cache().me()? {
                    Some(user) => Ok(Some(Account::new(&user, &server_url, auth))),
                    None => Err(e.into()),
                },
                Err(e) => Err(e.into()),
            }
        })
        .await
    }

    /// Sign in without a password where the server knows this device by its
    /// tailnet identity (behind `tailscale serve`): a token is made and
    /// stored. `None`: the server does not know this device that way.
    pub async fn login_tailnet(&self) -> Result<Option<Account>, LibraryError> {
        self.write(|inner| async move {
            let _busy = inner.busy.lock().await;
            let session = inner.session();
            let Some(user) = session.login_tailnet(&device::token_name()).await? else {
                return Ok(None);
            };
            inner.account_changed();
            Ok(Some(Account::new(&user, &session.config().server_url, AuthKind::Token)))
        })
        .await
    }

    /// Sign in with a username and password (and the two-factor code, if the
    /// account has two-factor authentication on); the token is stored.
    /// Throws `Unauthorized` for a wrong password or code, and
    /// `InvalidInput` with field `totp` when the account needs a code and
    /// none was given (ask for it and call again).
    pub async fn login_password(
        &self,
        username: String,
        password: String,
        totp: Option<String>,
    ) -> Result<Account, LibraryError> {
        self.write(move |inner| async move {
            let _busy = inner.busy.lock().await;
            let session = inner.session();
            let req = TokenRequest {
                username: username.trim().to_string(),
                password,
                totp: totp.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
                token_name: device::token_name(),
            };
            let user = match session.login_password(&req).await {
                Ok(user) => user,
                Err(e) if e.is_totp_required() => return Err(LibraryError::invalid("totp", e.to_string())),
                Err(e) => return Err(LibraryError::input(e, "username")),
            };
            inner.account_changed();
            Ok(Account::new(&user, &session.config().server_url, AuthKind::Token))
        })
        .await
    }

    /// Sign in with a one-time pairing code made on a signed-in device, for
    /// the server at `server_url` (both from a scanned pairing link, or
    /// typed). The code is exchanged first; only when that works is
    /// `server_url` saved (as by `set_server_url`) and the token stored, so a
    /// wrong code changes nothing. The token is named after the device,
    /// `Android (<model>)`. Throws `InvalidInput` (field `code` or
    /// `server_url`) for input that cannot be a code or URL, `Unauthorized`
    /// for a wrong, expired or used code, `InvalidInput` (field
    /// `token_name`) if the server refuses the token name.
    pub async fn login_pairing(&self, server_url: String, code: String) -> Result<Account, LibraryError> {
        self.write(move |inner| async move {
            let code = pairing::normalize(&code).ok_or_else(|| LibraryError::invalid("code", "not a pairing code"))?;
            let url = server_url_from(&server_url)?;
            let _busy = inner.busy.lock().await;
            let mut config = inner.dirs.load_config()?;
            config.server_url = url.clone();
            let client = ApiClient::from_config(&config)?;
            let req = PairRequest {
                code,
                token_name: device::token_name(),
            };
            let created = client
                .pair(&req)
                .await
                .map_err(|e| LibraryError::input(e, "token_name"))?;
            // Stored like `stickpile login` (`session::save_login`): the
            // token, the cache bound to the server (by `Session::open`) and
            // to the account.
            inner.dirs.save_server_url(&url)?;
            token::write_token(&config.token_path(), &created.token)?;
            let session = open_session(config).await?;
            session.cache().set_me(&created.user)?;
            inner.replace_session(session);
            Ok(Account::new(&created.user, &url, AuthKind::Token))
        })
        .await
    }

    /// Sign out: send the unsent uses, revoke the token on the server (best
    /// effort: also when that fails, the token is deleted here), and drop
    /// the account's data from the cache. Signed out already: nothing
    /// happens.
    pub async fn logout(&self) -> Result<(), LibraryError> {
        self.write(|inner| async move {
            let _busy = inner.busy.lock().await;
            let out = inner.session().logout().await?;
            if let Err(e) = &out.revoked {
                tracing::warn!("the token was deleted but not revoked on the server: {e}");
            }
            inner.account_changed();
            Ok(())
        })
        .await
    }

    // -- reading ----------------------------------------------------------------

    /// Items matching `query` (the search syntax of the other clients:
    /// words, `vibe:`, `tag:`, `set:`, `is:liked`, ...), best first, with
    /// hints for `target`. A `key:value` filter whose value is half typed or
    /// misspelled searches for its best suggestion (`suggestions`).
    /// When the server is unreachable the offline cache answers
    /// (`offline = true`, names and keywords only).
    pub async fn search(&self, query: String, target: Target, limit: u32) -> Result<ItemList, LibraryError> {
        self.read(move |inner| async move {
            let session = inner.session();
            let query = if query.contains(':') {
                let (inner2, session2) = (inner.clone(), session.clone());
                let vocabulary = tokio::task::spawn_blocking(move || inner2.vocabulary(&session2)).await?;
                suggest::resolve(query.trim(), &vocabulary).0
            } else {
                query.trim().to_string()
            };
            let out = session.search(&query, Some(target.into()), limit as usize).await?;
            Ok(inner.list(out.hits.into_iter().map(|h| h.item).collect(), out.offline))
        })
        .await
    }

    /// The items used last on this device, newest first (no network).
    pub fn recent(&self, limit: u32) -> Vec<Item> {
        let items = self.inner.session().recent(limit as usize);
        self.inner.remember(&items);
        items.iter().map(Item::from).collect()
    }

    /// The items the account liked, most used first. Offline: the
    /// favorites last read from the server, as far as the cache has them
    /// (`offline = true`); throws `Unreachable` if they were never read.
    pub async fn favorites(&self, limit: u32) -> Result<ItemList, LibraryError> {
        self.read(move |inner| async move {
            let session = inner.session();
            match session.favorites(limit as usize).await {
                Ok(items) => {
                    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
                    inner.save_favorites(&session, &ids);
                    Ok(inner.list(items, false))
                }
                Err(e) if falls_back(&e) => match inner.saved_favorites(&session)? {
                    Some(mut items) => {
                        items.truncate(limit as usize);
                        Ok(inner.list(items, true))
                    }
                    None => Err(e.into()),
                },
                Err(e) => Err(e.into()),
            }
        })
        .await
    }

    /// The account's most used items: the server's count over all its
    /// devices, then this device's history. Offline: this device's history
    /// (`offline = true`).
    pub async fn most_used(&self, limit: u32) -> Result<ItemList, LibraryError> {
        self.read(move |inner| async move {
            let listing = inner.session().most_used(limit as usize).await?;
            Ok(inner.list(listing.items, listing.offline))
        })
        .await
    }

    /// The packs to pick from: the account's own, most recently changed
    /// first, then other users' public ones; empty packs are left out.
    /// Offline: the packs of the cached items (`offline = true`); throws
    /// `Unreachable` if there are none.
    pub async fn packs(&self) -> Result<PackList, LibraryError> {
        self.read(|inner| async move {
            let session = inner.session();
            let listing = session.packs().await?;
            let me = match session.cache().me()? {
                Some(user) => Some(user.username),
                None if !listing.offline => session.client().me().await.ok().map(|u| u.username),
                None => None,
            };
            Ok(PackList {
                packs: listing.items.iter().map(|s| Pack::new(s, me.as_deref())).collect(),
                offline: listing.offline,
            })
        })
        .await
    }

    /// One pack's items in pack order. Offline: its cached items by name
    /// (`offline = true`); throws `Unreachable` if none are cached.
    pub async fn pack_items(&self, pack_id: String) -> Result<ItemList, LibraryError> {
        self.read(move |inner| async move {
            let listing = inner.session().pack_items(&pack_id).await?;
            Ok(inner.list(listing.items, listing.offline))
        })
        .await
    }

    /// The values to offer while a `key:value` filter is typed at the end of
    /// `query` (`vibe:sar`, `-tag:`, `set:ca`): the vibes, tags, packs and
    /// owners of the cached items, and the fixed words of `type:`, `is:` and
    /// `sort:`. `None` when the end of `query` is no such filter (no
    /// network).
    pub fn suggestions(&self, query: String) -> Option<Suggestions> {
        let session = self.inner.session();
        suggest::suggest(&query, &self.inner.vocabulary(&session)).map(Suggestions::from)
    }

    // -- files ------------------------------------------------------------------

    /// The path of the item's thumbnail (WebP, at most 256 px), downloaded
    /// into the cache unless it is there. Offline: the cached file; throws
    /// `Unreachable` if it is not cached.
    pub async fn thumbnail(&self, item_id: String) -> Result<String, LibraryError> {
        self.write(move |inner| async move {
            let session = inner.session();
            let item = inner.item_local_first(&session, &item_id).await?;
            Ok(path_string(&session.thumbnail(&item).await?))
        })
        .await
    }

    /// The file to insert into a text field of `editor_package` that accepts
    /// `accepted_mime_types` (as the field lists them, wildcards included): the best
    /// rendition for the chat app behind the package (the sticker rendition
    /// first when `sticker` is set and the app has stickers), never of a type
    /// the field did not list. The file is fetched into the cache and named
    /// `<item name>.<extension>`; offline the cached file is used. Throws
    /// `NothingFits` when no rendition fits the field, `Unreachable` when the
    /// file is not cached and the server cannot be reached.
    pub async fn prepare(
        &self,
        item_id: String,
        editor_package: String,
        accepted_mime_types: Vec<String>,
        sticker: bool,
    ) -> Result<Prepared, LibraryError> {
        self.write(move |inner| async move {
            let session = inner.session();
            let item = inner.item_fresh_first(&session, &item_id).await?;
            let target = stickpile_core::target_for_package(&editor_package);
            let choice =
                stickpile_core::variant_for(&item, target, &accepted_mime_types, sticker).ok_or_else(|| {
                    LibraryError::NothingFits {
                        detail: format!(
                            "{}: no rendition of [{}] for {}",
                            item.name,
                            accepted_mime_types.join(", "),
                            target.as_str()
                        ),
                    }
                })?;
            let path = session.fetch_variant(&item, choice.variant.variant).await?;
            Ok(Prepared {
                display_name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: path_string(&path),
                mime_type: choice.mime_type.to_string(),
                target: target.into(),
                variant: choice.variant.variant.as_str().to_string(),
                sticker: choice.sticker,
            })
        })
        .await
    }

    // -- writing ----------------------------------------------------------------

    /// Record that the item went into a field of `editor_package` as
    /// `variant` (`Prepared.variant`): it heads the recent items, and the
    /// use is sent to the server in the background, or with the next sync
    /// when it cannot be reached (no network here). Throws `NotFound` for
    /// an item neither shown nor cached, `InvalidInput` (field `variant`)
    /// for an unknown variant.
    pub fn record_use(&self, item_id: String, variant: String, editor_package: String) -> Result<(), LibraryError> {
        let variant = Variant::parse(&variant)
            .ok_or_else(|| LibraryError::invalid("variant", format!("unknown variant {variant:?}")))?;
        let session = self.inner.session();
        let item = self
            .inner
            .local_item(&session, &item_id)?
            .ok_or_else(|| LibraryError::not_found(format!("item {item_id} is not cached")))?;
        let target = stickpile_core::target_for_package(&editor_package);
        session.record_use(&item, target, variant)?;
        Ok(())
    }

    /// Like the item, or take the like back; returns the new state (true:
    /// liked). Needs the server.
    pub async fn toggle_like(&self, item_id: String) -> Result<bool, LibraryError> {
        self.write(move |inner| async move {
            let session = inner.session();
            let liked = session.toggle_like(&item_id).await?;
            inner.favorite_changed(&session, &item_id, liked);
            Ok(liked)
        })
        .await
    }

    /// Upload one file into the pack `set_id` (one of the account's own) and
    /// return the new item, or the one already in the pack with the same
    /// content. To upload several, call this once per file. Throws
    /// `InvalidInput` (field `file`) when the server refuses the file (not
    /// a picture or video it takes, too large), `NotFound` for a pack that
    /// is not the account's, `Io` when the file cannot be read.
    pub async fn upload(&self, set_id: String, file: UploadFile) -> Result<Item, LibraryError> {
        self.write(move |inner| async move {
            let session = inner.session();
            let path = PathBuf::from(&file.path);
            let read_path = path.clone();
            let bytes = tokio::task::spawn_blocking(move || std::fs::read(&read_path))
                .await?
                .map_err(|e| LibraryError::Io {
                    detail: format!("{}: {e}", path.display()),
                })?;
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "upload".into());
            let listed = |v: Vec<String>| {
                let v: Vec<String> = v
                    .into_iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                (!v.is_empty()).then_some(v)
            };
            let meta = UploadMeta {
                name: file.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
                keywords: listed(file.keywords),
                emojis: listed(file.emojis),
                ..UploadMeta::default()
            };
            let upload = CoreUploadFile {
                filename,
                bytes,
                mime: None,
            };
            let results = session
                .client()
                .upload(&set_id, vec![upload], Some(&[meta]))
                .await
                .map_err(|e| LibraryError::input(e, "file"))?;
            let result = results
                .into_iter()
                .next()
                .ok_or_else(|| LibraryError::other("the server answered for no file"))?;
            if let Some(error) = result.error {
                return Err(LibraryError::invalid("file", error));
            }
            let item = result
                .item
                .ok_or_else(|| LibraryError::other("the server answered with no item"))?;
            let summary = session.item_summary(&item.id).await?;
            inner.remember(std::slice::from_ref(&summary));
            Ok(Item::from(&summary))
        })
        .await
    }

    // -- pairing input ----------------------------------------------------------

    /// Read a scanned pairing link (`stickpile://pair?url=...&code=...`);
    /// `None` when `text` is not one (no network).
    pub fn parse_pairing_link(&self, text: String) -> Option<PairingLink> {
        pairing::parse_link(&text).map(|l| PairingLink {
            server_url: l.server_url,
            code: pairing::display(&l.code),
        })
    }

    /// The display form of a typed pairing code (`7KQ2-M9XA-4FTR`); `None`
    /// while `input` is not a whole code yet. Case, spaces and dashes do not
    /// matter; `O`, `I` and `L` read as `0`, `1`, `1` (no network).
    pub fn normalize_pairing_code(&self, input: String) -> Option<String> {
        pairing::normalize(&input).map(|c| pairing::display(&c))
    }

    // -- sync -------------------------------------------------------------------

    /// Sync if one is due: the last one is older than 10 minutes (or there
    /// was none since the account or server changed) and the last failed
    /// attempt is older than 60 s. A sync pulls the item changes into the
    /// offline cache and sends the unsent uses. Returns the state after it;
    /// throws what made the sync fail (`Unreachable`, `NotSignedIn`, ...).
    /// When none is due, or one is running, returns at once.
    pub async fn sync_if_due(&self) -> Result<SyncState, LibraryError> {
        self.write(|inner| async move {
            let Ok(_busy) = inner.busy.try_lock() else {
                return Ok(inner.sync_state());
            };
            if !lock(&inner.sync).policy.due(Instant::now()) {
                return Ok(inner.sync_state());
            }
            inner.run_sync().await
        })
        .await
    }

    /// Sync now, whether or not one is due (after one that is running).
    /// Returns the state after it; throws what made it fail.
    pub async fn sync_now(&self) -> Result<SyncState, LibraryError> {
        self.write(|inner| async move {
            let _busy = inner.busy.lock().await;
            inner.run_sync().await
        })
        .await
    }

    /// Where the sync stands (no network).
    pub fn sync_state(&self) -> SyncState {
        self.inner.sync_state()
    }

    /// Uses recorded on this device and not yet sent (no network).
    pub fn pending_usage(&self) -> u32 {
        to_u32(self.inner.session().pending_usage())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls_are_checked() {
        assert_eq!(server_url_from(" https://s.example/ ").unwrap(), "https://s.example");
        assert_eq!(server_url_from("http://10.0.0.2:8080").unwrap(), "http://10.0.0.2:8080");
        assert_eq!(
            server_url_from("https://example.org/stickers/").unwrap(),
            "https://example.org/stickers"
        );
        for bad in [
            "",
            "example.org",
            "ftp://example.org",
            "http://",
            "https://x?a=1",
            "https://x#f",
        ] {
            let e = server_url_from(bad).unwrap_err();
            assert!(
                matches!(&e, LibraryError::InvalidInput { field, .. } if field == "server_url"),
                "{bad:?}: {e:?}"
            );
        }
    }

    #[test]
    fn library_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Library>();
    }
}
