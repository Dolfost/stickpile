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
//!   exports and file downloads use [`ApiClient::transfer_timeout`].

use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use reqwest::multipart::{Form, Part};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use stickpile_api::profiles::TargetProfile;
use stickpile_api::routes::{self, path};
use stickpile_api::*;

use crate::{ClientConfig, CoreError, Result};

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

/// Async REST client. Cheap to clone (shares the connection pool and token).
#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base: String,
    token: Arc<RwLock<Option<String>>>,
    timeout: Duration,
    transfer_timeout: Duration,
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
        let http = reqwest::Client::builder()
            .connect_timeout(timeout)
            .user_agent(concat!("stickpile-core/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| CoreError::Other(format!("HTTP client: {e}")))?;
        Ok(ApiClient {
            http,
            base: base_url.trim_end_matches('/').to_string(),
            token: Arc::new(RwLock::new(None)),
            timeout,
            transfer_timeout: DEFAULT_TRANSFER_TIMEOUT.max(timeout),
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
            http: self.http.clone(),
            base: self.base.clone(),
            token: Arc::new(RwLock::new(None)),
            timeout: self.timeout,
            transfer_timeout: self.transfer_timeout,
        }
    }

    pub fn with_transfer_timeout(mut self, timeout: Duration) -> ApiClient {
        self.transfer_timeout = timeout;
        self
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

    fn request(&self, method: Method, route_path: &str, auth: bool) -> Result<RequestBuilder> {
        let mut rb = self.http.request(method, self.url(route_path)).timeout(self.timeout);
        if auth {
            if let Some(t) = self.token() {
                rb = rb.bearer_auth(t);
            }
        }
        Ok(rb)
    }

    async fn send(&self, rb: RequestBuilder) -> Result<Response> {
        let resp = rb.send().await.map_err(map_reqwest)?;
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
        if let Some(meta) = meta {
            let json = serde_json::to_vec(meta).map_err(|e| CoreError::Other(e.to_string()))?;
            let part = Part::bytes(json).mime_str("application/json").map_err(map_reqwest)?;
            form = form.part("meta", part);
        }
        let rb = self
            .request(Method::POST, &path(routes::SET_ITEMS, &[set_id]), true)?
            .timeout(self.transfer_timeout);
        self.json(rb.multipart(form)).await
    }

    /// Download the set directory as a tar (`set.toml` + numbered files).
    pub async fn export_tar(&self, set_id: &str) -> Result<Vec<u8>> {
        let rb = self
            .request(Method::GET, &path(routes::SET_EXPORT, &[set_id]), true)?
            .timeout(self.transfer_timeout);
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
        let rb = self
            .request(Method::POST, routes::SETS_IMPORT, true)?
            .timeout(self.transfer_timeout)
            .query(&q);
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
        let rb = self
            .request(Method::GET, &path(routes::ITEM_FILE, &[id, variant.as_str()]), true)?
            .timeout(self.transfer_timeout);
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
        let rb = self
            .request(Method::GET, url_path, true)?
            .timeout(self.transfer_timeout);
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

/// Routes that never need credentials (a 401 there is not "not logged in").
fn is_public_path(path: &str) -> bool {
    [
        routes::HEALTH,
        routes::OPENAPI,
        routes::AUTH_TOKEN,
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
