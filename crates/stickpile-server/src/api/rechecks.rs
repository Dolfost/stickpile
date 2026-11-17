//! Rechecks: batches that recompute metadata of the caller's items, or of
//! every item for admins ([`crate::service::recheck`]).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use stickpile_api::routes as r;
use stickpile_api::{ApiError, Recheck, RecheckFailure, StartRecheck};

use crate::auth::{AdminUser as Admin, CurrentUser};
use crate::error::AppResult;
use crate::service::recheck as svc;
use crate::state::AppState;

fn started((recheck, created): (Recheck, bool)) -> Response {
    let status = if created { StatusCode::CREATED } else { StatusCode::OK };
    (status, Json(recheck)).into_response()
}

/// Recheck the caller's own items (of one of their sets with `set_id`).
/// 200 with the running batch of the same scope instead of a new one.
#[utoipa::path(post, path = r::RECHECKS, tag = "rechecks", request_body = StartRecheck,
    responses((status = 201, body = Recheck), (status = 200, body = Recheck, description = "the same recheck runs"),
              (status = 400, body = ApiError), (status = 403, body = ApiError), (status = 404, body = ApiError),
              (status = 429, body = ApiError), (status = 503, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn start(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<StartRecheck>,
) -> AppResult<Response> {
    Ok(started(svc::start_own(&state, &user, req).await?))
}

/// Recheck every item on the server (of one set with `set_id`); audited.
/// 200 with the running batch of the same scope instead of a new one.
#[utoipa::path(post, path = r::ADMIN_RECHECKS, tag = "rechecks", request_body = StartRecheck,
    responses((status = 201, body = Recheck), (status = 200, body = Recheck, description = "the same recheck runs"),
              (status = 400, body = ApiError), (status = 403, body = ApiError), (status = 404, body = ApiError),
              (status = 503, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn start_server(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Json(req): Json<StartRecheck>,
) -> AppResult<Response> {
    Ok(started(svc::start_server(&state, &admin, req).await?))
}

/// The caller's rechecks (an admin's include the server-wide ones),
/// newest first.
#[utoipa::path(get, path = r::RECHECKS, tag = "rechecks", responses((status = 200, body = Vec<Recheck>)),
    security(("bearer" = []), ("session" = [])))]
pub async fn list(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<Vec<Recheck>>> {
    Ok(Json(svc::list(&state, &user).await?))
}

/// One recheck with its progress (its owner or an admin).
#[utoipa::path(get, path = r::RECHECK, tag = "rechecks", params(("id" = String, Path, description = "recheck id")),
    responses((status = 200, body = Recheck), (status = 404, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn get(State(state): State<AppState>, user: CurrentUser, Path(id): Path<String>) -> AppResult<Json<Recheck>> {
    Ok(Json(svc::get(&state, &user, &id).await?))
}

/// The pictures whose recheck failed, with the error.
#[utoipa::path(get, path = r::RECHECK_FAILURES, tag = "rechecks", params(("id" = String, Path, description = "recheck id")),
    responses((status = 200, body = Vec<RecheckFailure>), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn failures(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<RecheckFailure>>> {
    Ok(Json(svc::failures(&state, &user, &id).await?))
}

/// Stop a recheck (its owner or an admin): queued pictures are dropped, one
/// that runs finishes. A recheck that is over is returned as it is.
#[utoipa::path(post, path = r::RECHECK_CANCEL, tag = "rechecks", params(("id" = String, Path, description = "recheck id")),
    responses((status = 200, body = Recheck), (status = 404, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn cancel(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Recheck>> {
    Ok(Json(svc::cancel(&state, &user, &id).await?))
}
