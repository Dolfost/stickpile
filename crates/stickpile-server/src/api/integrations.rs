//! Git-pack and Telegram endpoints (docs/git-packs.md, docs/telegram.md).
//! The work runs as background jobs; these handlers validate, enqueue and
//! return the job. The git webhook is the only unauthenticated route here.

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use stickpile_api::routes as r;
use stickpile_api::{ApiError, CreateGitPack, GitPackCreated, Job, TelegramExport, TelegramImport};

use crate::auth::{ClientIp, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::service::{gitpack, telegram};
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
