//! Git-pack, Telegram, Discord, Signal, publishing and WhatsApp endpoints
//! (docs/git-packs.md, docs/telegram.md, docs/discord.md, docs/signal.md,
//! docs/whatsapp.md). The work runs as background jobs; these handlers
//! validate, enqueue and return the job (the Signal-ready zip is made on
//! request). The git webhook is the only unauthenticated route here.

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use stickpile_api::routes as r;
use stickpile_api::{
    ApiError, CreateGitPack, DiscordImport, GitPackCreated, Job, Publication, PublishJobs, PublishRequest,
    SignalImport, TelegramExport, TelegramImport, TelegramPlan, WhatsappStatus,
};
use tower::ServiceExt;
use utoipa::IntoParams;

use super::disposition;
use crate::auth::{ClientIp, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::service::{discord, gitpack, publish, signal, telegram, whatsapp};
use crate::state::AppState;

/// Largest webhook payload read (the route's default body limit is 2 MiB).
const MAX_WEBHOOK_BODY: usize = 2 * 1024 * 1024;

/// Bind a new set to a git repository. The clone runs as a `git_pull` job;
/// the webhook secret is returned only here.
#[utoipa::path(post, path = r::SETS_GITPACK, tag = "integrations", request_body = CreateGitPack,
    responses((status = 201, body = GitPackCreated),
              (status = 400, description = "URL not allowed by `git.allowed_schemes`, bad branch/subdir/slug", body = ApiError),
              (status = 404, description = "git packs are disabled", body = ApiError),
              (status = 409, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn create_gitpack(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<CreateGitPack>,
) -> AppResult<Response> {
    let created = gitpack::create(&state, &user, req).await?;
    Ok((StatusCode::CREATED, Json(created)).into_response())
}

/// Re-pull a git pack now (owner only; a queued pull is reused).
#[utoipa::path(post, path = r::SET_PULL, tag = "integrations", params(("id" = String, Path, description = "set id")),
    responses((status = 200, body = Job), (status = 400, description = "not a git pack", body = ApiError),
              (status = 403, body = ApiError), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn pull(State(state): State<AppState>, user: CurrentUser, Path(id): Path<String>) -> AppResult<Json<Job>> {
    Ok(Json(gitpack::pull_now(&state, &user, &id).await?))
}

/// Forge push webhook (unauthenticated): `X-Stickpile-Webhook-Secret` or
/// `X-Gitlab-Token` with the secret, or `X-Hub-Signature-256` with an
/// HMAC-SHA256 of the body. Rate limited per client IP.
#[utoipa::path(post, path = r::GIT_WEBHOOK, tag = "integrations", params(("id" = String, Path, description = "set id")),
    responses((status = 202, description = "pull enqueued", body = Job),
              (status = 401, description = "no secret or signature", body = ApiError),
              (status = 403, description = "secret or signature does not match", body = ApiError),
              (status = 404, description = "unknown set, not a git pack, or git packs disabled", body = ApiError),
              (status = 429, body = ApiError)))]
pub async fn git_webhook(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(id): Path<String>,
    req: Request,
) -> AppResult<Response> {
    // The raw body is needed for the HMAC; forges send small JSON payloads.
    let (parts, body) = req.into_parts();
    let body = axum::body::to_bytes(body, MAX_WEBHOOK_BODY)
        .await
        .map_err(|_| AppError::too_large("webhook payload too large"))?;
    let job = gitpack::webhook(&state, &id, ip, &parts.headers, &body).await?;
    Ok((StatusCode::ACCEPTED, Json(job)).into_response())
}

/// Publish a set as a Telegram sticker pack (create or update), as a job.
#[utoipa::path(post, path = r::SET_TELEGRAM_EXPORT, tag = "integrations", params(("id" = String, Path, description = "set id")),
    request_body = TelegramExport,
    responses((status = 200, body = Job),
              (status = 400, description = "bad pack name/title, or no Telegram user id to own the pack", body = ApiError),
              (status = 403, body = ApiError),
              (status = 404, description = "unknown set, or Telegram disabled", body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn telegram_export(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(req): Json<TelegramExport>,
) -> AppResult<Json<Job>> {
    Ok(Json(telegram::export(&state, &user, &id, req).await?))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TelegramPlanQuery {
    /// Plan a repack: every sticker assigned to the parts again in set
    /// order (default false).
    #[serde(default)]
    pub repack: bool,
    /// Plan the export under this pack name (as `TelegramExport.pack_name`;
    /// default: the set's own packs). A repack under a name that is not one
    /// of the set's own packs is a 400.
    #[serde(default)]
    pub pack_name: Option<String>,
}

/// What a Telegram export of the set would do to its packs (its parts,
/// 120 stickers each), without changing anything: `moved` is how many
/// published stickers a repack would move to another pack.
#[utoipa::path(get, path = r::SET_TELEGRAM_PLAN, tag = "integrations",
    params(("id" = String, Path, description = "set id"), TelegramPlanQuery),
    responses((status = 200, body = TelegramPlan),
              (status = 400, description = "no item of the set can become a Telegram sticker, a bad pack name, or a repack under a name that is not one of the set's packs", body = ApiError),
              (status = 403, body = ApiError),
              (status = 404, description = "unknown set, or Telegram disabled", body = ApiError),
              (status = 503, description = "Telegram did not answer", body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn telegram_plan(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Query(q): Query<TelegramPlanQuery>,
) -> AppResult<Json<TelegramPlan>> {
    Ok(Json(
        telegram::plan(&state, &user, &id, q.repack, q.pack_name.as_deref()).await?,
    ))
}

/// Import a Telegram sticker pack into a new set, as a job.
#[utoipa::path(post, path = r::TELEGRAM_IMPORT, tag = "integrations", request_body = TelegramImport,
    responses((status = 200, body = Job), (status = 400, body = ApiError),
              (status = 404, description = "Telegram disabled", body = ApiError),
              (status = 409, description = "slug taken", body = ApiError), (status = 429, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn telegram_import(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<TelegramImport>,
) -> AppResult<Json<Job>> {
    Ok(Json(telegram::import(&state, &user, req).await?))
}

/// Import a Discord server's stickers and/or emoji into a new set, as a job.
#[utoipa::path(post, path = r::DISCORD_IMPORT, tag = "integrations", request_body = DiscordImport,
    responses((status = 200, body = Job),
              (status = 400, description = "bad server id or slug, or neither stickers nor emojis", body = ApiError),
              (status = 404, description = "Discord disabled", body = ApiError),
              (status = 409, description = "slug taken", body = ApiError), (status = 429, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn discord_import(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<DiscordImport>,
) -> AppResult<Json<Job>> {
    Ok(Json(discord::import(&state, &user, req).await?))
}

/// Import a Signal sticker pack from its link into a new set, as a job.
/// The link (it holds the pack's key) is used for the import only.
#[utoipa::path(post, path = r::SIGNAL_IMPORT, tag = "integrations", request_body = SignalImport,
    responses((status = 200, body = Job),
              (status = 400, description = "not a Signal sticker pack link, or a bad slug", body = ApiError),
              (status = 404, description = "Signal disabled", body = ApiError),
              (status = 409, description = "slug taken", body = ApiError), (status = 429, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn signal_import(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<SignalImport>,
) -> AppResult<Json<Job>> {
    Ok(Json(signal::import(&state, &user, req).await?))
}

/// The set as a Signal-ready zip: 512x512 stickers numbered in order,
/// `emoji.txt`, `manifest.json` (signal-cli) and `README.txt`, for
/// uploading by hand. Anyone who can read the set; works without Signal
/// configured. Made on the first request after a change (counts as an
/// upload for rate limiting), then reused.
#[utoipa::path(get, path = r::SET_SIGNAL_ZIP, tag = "integrations", params(("id" = String, Path, description = "set id")),
    responses((status = 200, content_type = "application/zip", description = "zip archive"),
              (status = 400, description = "no item can become a Signal sticker", body = ApiError),
              (status = 404, body = ApiError), (status = 429, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn signal_zip(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let (name, bytes) = signal::ready_zip(&state, &user, &id).await?;
    Ok(super::sets::attachment(bytes, "application/zip", &name))
}

/// Where the set is published, one entry per platform the server knows
/// (configured or not). Owner only.
#[utoipa::path(get, path = r::SET_PUBLISH, tag = "integrations", params(("id" = String, Path, description = "set id")),
    responses((status = 200, body = Vec<Publication>), (status = 403, body = ApiError), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn publications(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<Publication>>> {
    Ok(Json(publish::list(&state, &user, &id).await?))
}

/// Publish a set to several platforms at once: one job per platform, in
/// the order given. Every platform is checked before any job is queued.
/// Owner only.
#[utoipa::path(post, path = r::SET_PUBLISH, tag = "integrations", params(("id" = String, Path, description = "set id")),
    request_body = PublishRequest,
    responses((status = 200, body = PublishJobs),
              (status = 400, description = "no platform, an unknown or unconfigured one, or bad options for one", body = ApiError),
              (status = 403, body = ApiError), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn publish_set(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(req): Json<PublishRequest>,
) -> AppResult<Json<PublishJobs>> {
    Ok(Json(publish::publish(&state, &user, &id, req).await?))
}

/// The set's last WhatsApp build, whether it matches the set as it is now,
/// and the caller's build in progress. Anyone who can read the set.
#[utoipa::path(get, path = r::SET_WHATSAPP, tag = "integrations", params(("id" = String, Path, description = "set id")),
    responses((status = 200, body = WhatsappStatus), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn whatsapp_status(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<WhatsappStatus>> {
    Ok(Json(whatsapp::status(&state, &user, &id).await?))
}

/// Build the set's WhatsApp sticker packs, as a job (the caller's queued
/// build is returned instead of a second one; an unchanged set reuses the
/// last build). Anyone who can read the set; counts as an upload for rate
/// limiting.
#[utoipa::path(post, path = r::SET_WHATSAPP, tag = "integrations", params(("id" = String, Path, description = "set id")),
    responses((status = 200, body = Job), (status = 404, body = ApiError), (status = 429, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn whatsapp_build(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Job>> {
    Ok(Json(whatsapp::build(&state, &user, &id).await?))
}

/// The last WhatsApp build as one zip: every `.wastickers` plus
/// `contents.json`. 404 before the first build.
#[utoipa::path(get, path = r::SET_WHATSAPP_ZIP, tag = "integrations", params(("id" = String, Path, description = "set id")),
    responses((status = 200, content_type = "application/zip", description = "zip archive"), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn whatsapp_zip(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    req: Request,
) -> AppResult<Response> {
    let (name, path) = whatsapp::zip_file(&state, &user, &id).await?;
    // ServeFile handles Range / If-Range / HEAD; the body of `req` is unused.
    let (parts, _) = req.into_parts();
    let inner = Request::from_parts(parts, Body::empty());
    let mut resp = tower_http::services::ServeFile::new(&path)
        .oneshot(inner)
        .await
        .map_err(|e| AppError::internal(format!("serving file: {e}")))?
        .map(Body::new);
    if resp.status().is_success() {
        let h = resp.headers_mut();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
        h.insert(header::CONTENT_DISPOSITION, disposition("attachment", &name));
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    }
    Ok(resp.into_response())
}

/// One pack of the last WhatsApp build as a `.wastickers` file (a zip of
/// `title.txt`, `author.txt`, `cover.png` and the stickers).
#[utoipa::path(get, path = r::SET_WHATSAPP_PACK, tag = "integrations",
    params(("id" = String, Path, description = "set id"), ("n" = u32, Path, description = "pack number, from 1")),
    responses((status = 200, content_type = "application/octet-stream", description = ".wastickers file"), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn whatsapp_pack(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, n)): Path<(String, String)>,
) -> AppResult<Response> {
    let (name, bytes) = whatsapp::pack_file(&state, &user, &id, &n).await?;
    Ok(super::sets::attachment(bytes, "application/octet-stream", &name))
}
