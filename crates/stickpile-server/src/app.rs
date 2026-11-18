//! Startup, the router, listeners and background tasks.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use axum::middleware::{from_fn, from_fn_with_state};
use axum::Router;
use stickpile_ai::{Enricher, TextReader, Vocabulary, ZeroShot};

use tokio::net::{TcpListener, UnixListener};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower_http::trace::TraceLayer;

use crate::config::{read_secret, Config};
use crate::error::AppResult;
use crate::models::{self, ModelSource, ModelSources};
use crate::security::{self, RateLimiters};
use crate::state::{Ai, AiStatus, AppState, Inner, LruCache};
use crate::{api, assets, auth, db, files, index, jobs, service, web};

/// Where the AI backend comes from.
#[derive(Clone, Default)]
pub enum EnricherSource {
    /// `ai.enabled` decides; CLIP is loaded from `ai.model_dir`.
    #[default]
    FromConfig,
    /// Use this enricher regardless of `ai.enabled` (tests: `MockEnricher`).
    Provided(Arc<dyn Enricher>),
    /// No AI.
    Disabled,
}

/// Where the OCR backend (the item text) comes from.
#[derive(Clone, Default)]
pub enum TextReaderSource {
    /// `ai.enabled` and `ai.ocr` decide; see [`text_reader_from_config`].
    #[default]
    FromConfig,
    /// Use this reader regardless of `ai.enabled`, still only with
    /// `ai.ocr = true` (tests: `MockTextReader`).
    Provided(Arc<dyn TextReader>),
    /// No OCR.
    Disabled,
}

#[derive(Clone, Default)]
pub struct StartOptions {
    pub enricher: EnricherSource,
    pub text_reader: TextReaderSource,
    /// Where `FromConfig` backends download missing model files from
    /// ([`models::ModelSources::default`]: the pinned hosts; tests serve
    /// their own).
    pub models: models::ModelSources,
}

/// Prepare everything except listeners and background tasks: data
/// directories, database + migrations, bootstrap admin, AI, search index.
pub async fn build_state(config: Config, opts: StartOptions) -> anyhow::Result<AppState> {
    for dir in [
        config.server.data_dir.clone(),
        config.media_dir(),
        config.tmp_dir(),
        config.gitpacks_dir(),
    ] {
        std::fs::create_dir_all(&dir).map_err(|e| anyhow::anyhow!("creating {}: {e}", dir.display()))?;
    }
    let pool = db::connect(&config.db_path()).await?;
    let requeued = jobs::requeue_interrupted(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if requeued > 0 {
        tracing::info!("requeued {requeued} interrupted jobs");
    }
    let outdated = jobs::media::requeue_outdated_variants(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if outdated > 0 {
        tracing::info!("queued {outdated} Variants jobs to remake renditions made by an older version");
    }
    service::users::bootstrap_admin(&pool, &config).await?;

    let vocab = match &config.ai.vocabulary_file {
        Some(p) => Vocabulary::load(p).map_err(|e| anyhow::anyhow!("ai.vocabulary_file {}: {e}", p.display()))?,
        None => Vocabulary::builtin(),
    };
    let proxy_secret = match &config.trusted_proxy.secret_file {
        Some(p) => Some(
            read_secret(p).map_err(|e| anyhow::anyhow!("reading trusted_proxy.secret_file {}: {e}", p.display()))?,
        )
        .filter(|s| !s.is_empty()),
        None => None,
    };
    let tools = stickpile_media::Tools {
        ffmpeg: config.media.ffmpeg.clone(),
        ffprobe: config.media.ffprobe.clone(),
        timeout: Duration::from_secs(config.media.ffmpeg_timeout_secs),
        tmp_dir: config.tmp_dir(),
    };
    let ffmpeg_available = probe_tool(&tools.ffmpeg).await && probe_tool(&tools.ffprobe).await;
    if !ffmpeg_available {
        tracing::warn!(
            "ffmpeg/ffprobe not found ({} / {}): video uploads and animated variants will fail",
            tools.ffmpeg.display(),
            tools.ffprobe.display()
        );
    }
    let state = AppState(Arc::new(Inner {
        limiters: RateLimiters::new(&config.limits),
        db: pool,
        index: RwLock::new(stickpile_search::Index::new()),
        ai: OnceLock::new(),
        ai_status: AtomicU8::new(AiStatus::Off as u8),
        model_download: models::Progress::default(),
        text_reader: OnceLock::new(),
        vocab,
        jobs_notify: Notify::new(),
        tools,
        ffmpeg_available,
        proxy_secret,
        media_lock: tokio::sync::RwLock::new(()),
        text_embeddings: Mutex::new(LruCache::new(256)),
        config,
    }));
    let lifted = service::transfer::lift_text_from_extra(&state.db).await?;
    if lifted > 0 {
        tracing::info!("took the text of {lifted} items out of their unknown set.toml keys");
    }
    let n = index::load_all(&state).await?;
    tracing::info!("search index loaded: {n} items");
    // The backends that load from `ai.model_dir`, and the files they need
    // (downloaded first when missing, `ai.download_models`).
    let ai = &state.config.ai;
    let clip_from_files = matches!(opts.enricher, EnricherSource::FromConfig) && ai.enabled;
    let ocr_from_files = matches!(opts.text_reader, TextReaderSource::FromConfig) && ai.enabled && ai.ocr;
    let plan = models::plan(ai, &opts.models, clip_from_files, ocr_from_files);
    let mut ocr_fetch = Vec::new();
    let reader = match opts.text_reader {
        TextReaderSource::Disabled => None,
        _ if !state.config.ai.ocr => None,
        TextReaderSource::Provided(r) => Some(r),
        TextReaderSource::FromConfig if !state.config.ai.enabled => None,
        // A few MB: checked here, so that a reader whose files are in place
        // is there before the first `Enrich` job, as without downloads.
        TextReaderSource::FromConfig => {
            if plan.blocked.is_none() {
                ocr_fetch = models::stale(&state.config.ai.model_dir, &plan.ocr).await;
            }
            if ocr_fetch.is_empty() {
                load_text_reader(&state.config, &opts.models).await
            } else {
                None
            }
        }
    };
    if let Some(r) = reader {
        publish_text_reader(&state, r).await;
    }
    match opts.enricher {
        EnricherSource::Disabled => {}
        // Provided backends (tests) are cheap: ready before the first request.
        EnricherSource::Provided(e) => {
            state.set_ai_status(AiStatus::Loading);
            finish_ai(&state, Ok(e)).await;
        }
        // CLIP + vocabulary embedding take seconds, a download minutes:
        // serve meanwhile (search reports `semantic: false` until ready).
        EnricherSource::FromConfig if clip_from_files => state.set_ai_status(AiStatus::Loading),
        EnricherSource::FromConfig => {}
    }
    if clip_from_files || !ocr_fetch.is_empty() {
        let st = state.clone();
        let sources = opts.models.clone();
        tokio::spawn(async move { load_models(st, sources, plan, ocr_fetch, clip_from_files).await });
    }
    Ok(state)
}

/// The background part of loading from `ai.model_dir`: check the CLIP
/// files (`clip`), download what is missing or wrong (the OCR files first:
/// they are small), then load each backend once its files are in place.
/// CLIP loads at once when its files are fine.
async fn load_models(st: AppState, sources: ModelSources, plan: models::Plan, ocr_fetch: Vec<ModelSource>, clip: bool) {
    let dir = st.config.ai.model_dir.clone();
    let clip_fetch = if clip && plan.blocked.is_none() {
        models::stale(&dir, &plan.clip).await
    } else {
        Vec::new()
    };
    if clip && clip_fetch.is_empty() {
        let st = st.clone();
        let stand_in = sources.enricher.clone();
        tokio::spawn(async move {
            let loaded = load_enricher(&st.config, stand_in).await;
            finish_ai(&st, loaded).await;
        });
    }
    let all: Vec<ModelSource> = ocr_fetch.iter().chain(&clip_fetch).cloned().collect();
    if all.is_empty() {
        return;
    }
    let mut downloads = match models::Downloader::start(&sources, &dir, &st.model_download, &all) {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::error!("cannot download the model files: {e:#}");
            None
        }
    };
    if !ocr_fetch.is_empty() {
        if let Some(d) = downloads.as_mut() {
            d.fetch(&ocr_fetch).await;
        }
        // Logs why not when files are still missing.
        if let Some(r) = load_text_reader(&st.config, &sources).await {
            publish_text_reader(&st, r).await;
        }
    }
    if let Some(d) = downloads.as_mut() {
        d.fetch(&clip_fetch).await;
    }
    if let Some(d) = downloads {
        d.finish();
    }
    if !clip_fetch.is_empty() {
        let loaded = load_enricher(&st.config, sources.enricher.clone()).await;
        finish_ai(&st, loaded).await;
    }
}

/// Publish the OCR backend and start the server's recheck of the pictures
/// this reader never read.
async fn publish_text_reader(state: &AppState, r: Arc<dyn TextReader>) {
    tracing::info!("OCR ready: model {}", r.model_id());
    let _ = state.text_reader.set(r);
    match service::recheck::start_system_text(state).await {
        Ok(Some(r)) => tracing::info!("started recheck {} of the text of {} pictures", r.id, r.total),
        Ok(None) => {}
        Err(e) => tracing::error!("starting the recheck of the text: {e}"),
    }
}

/// CLIP from `ai.model_dir`, or the tests' stand-in once the files are there.
async fn load_enricher(config: &Config, stand_in: Option<Arc<dyn Enricher>>) -> anyhow::Result<Arc<dyn Enricher>> {
    let kind = models::text_encoder_kind(config.ai.text_encoder);
    match stand_in {
        Some(e) if stickpile_ai::missing_model_files(&config.ai.model_dir, kind).is_empty() => Ok(e),
        _ => load_clip(config).await,
    }
}

/// [`text_reader_from_config`], or the tests' stand-in once the files are there.
async fn load_text_reader(config: &Config, sources: &ModelSources) -> Option<Arc<dyn TextReader>> {
    match &sources.text_reader {
        Some(r) if stickpile_ai::missing_ocr_files(&config.ai.model_dir).is_empty() => Some(r.clone()),
        _ => text_reader_from_config(config).await,
    }
}

/// Embed the vocabulary, publish the AI backend, load stored embeddings
/// into the index and start the server's recheck of media not yet embedded
/// with this model ([`service::recheck::start_system_clip`]). On failure AI
/// stays off for the life of the process.
async fn finish_ai(state: &AppState, enricher: anyhow::Result<Arc<dyn Enricher>>) {
    let enricher = match enricher {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("AI disabled: could not load the CLIP model: {e:#}; continuing without semantic search");
            state.set_ai_status(AiStatus::Failed);
            return;
        }
    };
    let vocab = state.vocab.clone();
    let e2 = enricher.clone();
    let zeroshot = match tokio::task::spawn_blocking(move || ZeroShot::new(&*e2, vocab)).await {
        Ok(Ok(z)) => z,
        Ok(Err(e)) => {
            tracing::error!("AI disabled: zero-shot vocabulary embedding failed: {e}");
            state.set_ai_status(AiStatus::Failed);
            return;
        }
        Err(e) => {
            tracing::error!("AI disabled: {e}");
            state.set_ai_status(AiStatus::Failed);
            return;
        }
    };
    tracing::info!(
        "AI ready: model {} (images {})",
        enricher.model_id(),
        enricher.image_model_id()
    );
    let _ = state.ai.set(Ai { enricher, zeroshot });
    state.set_ai_status(AiStatus::Ready);
    if let Err(e) = index::load_embeddings(state).await {
        tracing::error!("loading embeddings into the search index: {e}");
    }
    match service::recheck::start_system_clip(state).await {
        Ok(Some(r)) => tracing::info!("started recheck {} of the AI metadata of {} pictures", r.id, r.total),
        Ok(None) => {}
        Err(e) => tracing::error!("starting the recheck of the AI metadata: {e}"),
    }
}

/// The OCR backend `ai.ocr` asks for, when `ai.enabled`: PaddleOCR from
/// `<ai.model_dir>/ocr/` (docs/ai.md). Loading takes well under a second, so
/// it is awaited here: the reader is there before the first `Enrich` job.
/// Missing model files or a failed load leave OCR off with one log line;
/// the server never fails for it.
#[cfg(feature = "ocr")]
async fn text_reader_from_config(config: &Config) -> Option<Arc<dyn TextReader>> {
    let ai = config.ai.clone();
    let missing = stickpile_ai::missing_ocr_files(&ai.model_dir);
    if !missing.is_empty() {
        let list: Vec<String> = missing.iter().map(|p| p.display().to_string()).collect();
        tracing::info!(
            "ai.ocr is on, but the OCR models are missing ({}): the words in pictures are not read \
             (fetch them with scripts/fetch-models.sh, see docs/ai.md)",
            list.join(", ")
        );
        return None;
    }
    let loaded = tokio::task::spawn_blocking(move || {
        // CLIP loads later, in the background: pick the configured runtime first.
        stickpile_ai::init_runtime(ai.onnxruntime_library.as_deref())?;
        stickpile_ai::paddle::PaddleOcr::load(&ai.model_dir, ai.threads.max(1))
    })
    .await;
    match loaded {
        Ok(Ok(r)) => Some(Arc::new(r)),
        Ok(Err(e)) => {
            tracing::warn!(
                "ai.ocr is on, but the OCR models could not be loaded: {e}; the words in pictures are not read"
            );
            None
        }
        Err(e) => {
            tracing::warn!("ai.ocr is on, but loading the OCR models failed: {e}; the words in pictures are not read");
            None
        }
    }
}

#[cfg(not(feature = "ocr"))]
async fn text_reader_from_config(config: &Config) -> Option<Arc<dyn TextReader>> {
    let _ = config;
    tracing::info!(
        "ai.ocr is on, but this build has no OCR (feature `ocr` disabled): the words in pictures are not read"
    );
    None
}

/// `ffmpeg -version` with fixed arguments; false if it cannot run.
async fn probe_tool(bin: &Path) -> bool {
    let bin = bin.to_path_buf();
    tokio::task::spawn_blocking(move || {
        std::process::Command::new(bin)
            .arg("-version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
    .await
    .unwrap_or(false)
}

#[cfg(feature = "clip")]
async fn load_clip(config: &Config) -> anyhow::Result<Arc<dyn Enricher>> {
    let opts = stickpile_ai::ClipOptions {
        model_dir: config.ai.model_dir.clone(),
        text_encoder: models::text_encoder_kind(config.ai.text_encoder),
        onnxruntime_library: config.ai.onnxruntime_library.clone(),
        threads: config.ai.threads.max(1) as usize,
    };
    let e = tokio::task::spawn_blocking(move || stickpile_ai::clip::ClipEnricher::load(&opts)).await??;
    Ok(Arc::new(e))
}

#[cfg(not(feature = "clip"))]
async fn load_clip(config: &Config) -> anyhow::Result<Arc<dyn Enricher>> {
    let _ = config;
    anyhow::bail!("this build has no CLIP support (feature `clip` disabled)")
}

/// The whole HTTP application: API, static assets, web UI, and the
/// middleware stack (trace -> security headers -> auth -> language -> API
/// error JSON).
pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(api::routes(&state))
        .merge(assets::routes())
        .merge(web::routes())
        .layer(from_fn(api::normalize_errors))
        .layer(from_fn(crate::i18n::middleware))
        .layer(from_fn_with_state(state.clone(), auth::auth_middleware))
        .layer(from_fn_with_state(state.clone(), security::security_headers))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Serve the public TCP listener (tailnet headers ignored unless from a
/// loopback proxy with the shared secret).
pub async fn serve_tcp(state: AppState, listener: TcpListener, shutdown: CancellationToken) -> std::io::Result<()> {
    let app = router(state).layer(from_fn(security::tag_tcp));
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}

/// Serve the trusted unix socket of `tailscale serve`.
pub async fn serve_unix(state: AppState, listener: UnixListener, shutdown: CancellationToken) -> std::io::Result<()> {
    let app = router(state).layer(from_fn(security::tag_trusted_socket));
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}

/// Bind the trusted unix socket (removing a stale one) with `mode` (octal).
///
/// The socket is never reachable with wider permissions than `mode`, not even
/// briefly: it is bound inside a fresh 0700 directory next to `path`, gets
/// its mode there, and is then renamed into place (no process-wide umask
/// change, which would race with other threads creating files).
pub fn bind_unix(path: &Path, mode: &str) -> anyhow::Result<UnixListener> {
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
    let bits = u32::from_str_radix(mode.trim_start_matches("0o"), 8)?;
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_socket() {
            std::fs::remove_file(path)?;
        } else {
            anyhow::bail!("{} exists and is not a socket", path.display());
        }
    }
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    std::fs::create_dir_all(&parent)?;
    if path.file_name().is_none() {
        anyhow::bail!("{} has no file name", path.display());
    }
    // Short names: the staging path must fit sun_path (108 bytes) as well.
    let staging = parent.join(format!(".{}", crate::ids::random_base32(4)));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&staging)
        .map_err(|e| anyhow::anyhow!("creating {}: {e}", staging.display()))?;
    let tmp = staging.join("s");
    let bound = (|| -> anyhow::Result<UnixListener> {
        let listener = UnixListener::bind(&tmp).map_err(|e| anyhow::anyhow!("binding {}: {e}", path.display()))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(bits))?;
        std::fs::rename(&tmp, path).map_err(|e| anyhow::anyhow!("moving the socket to {}: {e}", path.display()))?;
        Ok(listener)
    })();
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_dir(&staging);
    bound
}

/// Job workers plus the periodic cleanup task.
pub fn spawn_background(state: &AppState, cancel: &CancellationToken) -> Vec<JoinHandle<()>> {
    let mut handles = jobs::spawn_workers(state, cancel);
    let st = state.clone();
    let c = cancel.clone();
    handles.push(tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(600));
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if let Err(e) = cleanup_once(&st).await {
                        tracing::error!("periodic cleanup: {e}");
                    }
                }
                _ = c.cancelled() => break,
            }
        }
    }));
    // Git packs due for a scheduled pull.
    let st = state.clone();
    let c = cancel.clone();
    handles.push(tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(service::gitpack::SCHEDULER_TICK_SECS));
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    match service::gitpack::schedule_due(&st).await {
                        Ok(0) => {}
                        Ok(n) => tracing::info!("queued {n} scheduled git pulls"),
                        Err(e) => tracing::error!("scheduling git pulls: {e}"),
                    }
                }
                _ = c.cancelled() => break,
            }
        }
    }));
    handles
}

/// Expired sessions and pairing codes, stale temp files, idle rate-limit
/// keys, old finished jobs and tombstones, orphaned media, git checkouts
/// and exports of deleted sets.
pub async fn cleanup_once(state: &AppState) -> AppResult<()> {
    let sessions = auth::session::purge_expired(&state.db).await?;
    let pairing_codes = auth::pairing::purge(&state.db).await?;
    let tmp = state.config.tmp_dir();
    let stale =
        tokio::task::spawn_blocking(move || files::purge_old_files_blocking(&tmp, Duration::from_secs(86_400))).await?;
    state.limiters.prune();
    let jobs = jobs::purge_finished(&state.db, 30 * 86_400).await?;
    let tombstones = service::items::purge_tombstones(&state.db, 180 * 86_400).await?;
    let media = gc_orphan_media(state).await?;
    let checkouts = service::gitpack::gc_checkouts(state).await? as u64;
    let exports = service::whatsapp::gc_exports(state).await? as u64;
    if sessions + pairing_codes + stale as u64 + jobs + tombstones + media + checkouts + exports > 0 {
        tracing::info!(
            sessions,
            pairing_codes,
            stale_tmp = stale,
            jobs,
            tombstones,
            media,
            checkouts,
            exports,
            "cleanup"
        );
    }
    Ok(())
}

/// Delete media no item references, no set has as its cover image and no
/// proposal needs any more (rows and files).
pub async fn gc_orphan_media(state: &AppState) -> AppResult<u64> {
    let _guard = state.media_lock.write().await;
    let keep_since = crate::ids::now() - crate::service::proposals::KEEP_DECIDED_SECS;
    let unused = format!(
        "NOT EXISTS (SELECT 1 FROM items i WHERE i.media_id = m.id) \
         AND NOT EXISTS (SELECT 1 FROM sets s WHERE s.cover_media_id = m.id) \
         AND NOT {}",
        crate::service::proposals::PROPOSAL_MEDIA_SQL
    );
    let orphans: Vec<(String, String)> = sqlx::query_as(&format!("SELECT id, sha256 FROM media m WHERE {unused}"))
        .bind(keep_since)
        .fetch_all(&state.db)
        .await?;
    let mut n = 0;
    for (id, sha) in orphans {
        let deleted = sqlx::query(&format!("DELETE FROM media AS m WHERE m.id = ?2 AND {unused}"))
            .bind(keep_since)
            .bind(&id)
            .execute(&state.db)
            .await?
            .rows_affected();
        if deleted == 0 {
            continue;
        }
        if let Some(dir) = files::resolve(&state.media_dir(), &files::media_rel_dir(&sha)) {
            if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("removing {}: {e}", dir.display());
                }
            }
        }
        n += 1;
    }
    Ok(n)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}

/// Run the server until SIGINT/SIGTERM.
pub async fn run(config: Config) -> anyhow::Result<()> {
    let state = build_state(config.clone(), StartOptions::default()).await?;
    let cancel = CancellationToken::new();
    let background = spawn_background(&state, &cancel);

    let tcp = TcpListener::bind(config.server.listen)
        .await
        .map_err(|e| anyhow::anyhow!("binding {}: {e}", config.server.listen))?;
    tracing::info!(
        "listening on http://{} (public URL {})",
        config.server.listen,
        config.server.public_url
    );
    let mut servers = vec![tokio::spawn(serve_tcp(state.clone(), tcp, cancel.clone()))];
    if let Some(path) = &config.trusted_proxy.socket {
        let unix = bind_unix(path, &config.trusted_proxy.socket_mode)?;
        tracing::info!("trusted proxy socket at {}", path.display());
        servers.push(tokio::spawn(serve_unix(state.clone(), unix, cancel.clone())));
    }
    {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            tracing::info!("shutting down");
            cancel.cancel();
        });
    }
    let mut result = Ok(());
    for s in servers {
        match s.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => result = Err(anyhow::anyhow!("server error: {e}")),
            Err(e) => result = Err(anyhow::anyhow!("server task: {e}")),
        }
        cancel.cancel();
    }
    let _ = tokio::time::timeout(Duration::from_secs(10), futures::future::join_all(background)).await;
    if let Some(path) = &config.trusted_proxy.socket {
        let _ = std::fs::remove_file(path);
    }
    state.db.close().await;
    result
}

/// A running in-process server for integration tests of other crates (the
/// CLI end-to-end test): TCP listener on `config.server.listen` (use port 0),
/// job workers and background tasks, no AI. Stops when dropped.
#[doc(hidden)]
pub struct TestServerHandle {
    pub state: AppState,
    /// `http://127.0.0.1:<port>`; also written to `server.public_url`.
    pub url: String,
    cancel: CancellationToken,
}

impl Drop for TestServerHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[doc(hidden)]
pub async fn spawn_for_tests(mut config: Config) -> anyhow::Result<TestServerHandle> {
    let tcp = TcpListener::bind(config.server.listen).await?;
    let addr = tcp.local_addr()?;
    config.server.listen = addr;
    config.server.public_url = format!("http://{addr}");
    config.validate()?;
    let state = build_state(
        config,
        StartOptions {
            enricher: EnricherSource::Disabled,
            text_reader: TextReaderSource::Disabled,
            models: ModelSources::none(),
        },
    )
    .await?;
    let cancel = CancellationToken::new();
    spawn_background(&state, &cancel);
    tokio::spawn(serve_tcp(state.clone(), tcp, cancel.clone()));
    Ok(TestServerHandle {
        state,
        url: format!("http://{addr}"),
        cancel,
    })
}
