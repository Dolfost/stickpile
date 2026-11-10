//! Git-pack and Telegram endpoints: answered with 501 `not_implemented`
//! until the integrations land. Authentication still applies, so the
//! status codes clients see for anonymous calls are already final. The
//! request bodies are not parsed yet.

use axum::extract::Path;
use stickpile_api::routes as r;
use stickpile_api::{ApiError, CreateGitPack, GitPackCreated, Job, TelegramExport, TelegramImport};

use crate::auth::CurrentUser;
use crate::error::AppError;

fn todo(what: &str) -> AppError {
    AppError::not_implemented(format!("{what} is not implemented yet"))
}

/// Bind a new set to a git repository (clone runs as a job).
#[utoipa::path(post, path = r::SETS_GITPACK, tag = "integrations", request_body = CreateGitPack,
    responses((status = 200, body = GitPackCreated), (status = 501, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn create_gitpack(_user: CurrentUser) -> AppError {
    todo("git packs")
}

/// Re-pull a git pack now.
#[utoipa::path(post, path = r::SET_PULL, tag = "integrations", params(("id" = String, Path, description = "set id")),
    responses((status = 200, body = Job), (status = 501, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn pull(_user: CurrentUser, Path(_id): Path<String>) -> AppError {
    todo("git pull")
}

/// Git push webhook (unauthenticated; secret or HMAC signature).
#[utoipa::path(post, path = r::GIT_WEBHOOK, tag = "integrations", params(("id" = String, Path, description = "set id")),
    responses((status = 202, description = "pull enqueued"), (status = 501, body = ApiError)))]
pub async fn git_webhook(Path(_id): Path<String>) -> AppError {
    todo("the git webhook")
}

/// Publish a set as a Telegram sticker pack.
#[utoipa::path(post, path = r::SET_TELEGRAM_EXPORT, tag = "integrations", params(("id" = String, Path, description = "set id")),
    request_body = TelegramExport, responses((status = 200, body = Job), (status = 501, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn telegram_export(_user: CurrentUser, Path(_id): Path<String>) -> AppError {
    todo("Telegram export")
}

/// Import a Telegram sticker pack into a new set.
#[utoipa::path(post, path = r::TELEGRAM_IMPORT, tag = "integrations", request_body = TelegramImport,
    responses((status = 200, body = Job), (status = 501, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn telegram_import(_user: CurrentUser) -> AppError {
    todo("Telegram import")
}
