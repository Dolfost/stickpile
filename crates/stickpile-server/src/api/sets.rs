//! Set endpoints: CRUD, reorder, upload, import, export.

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use stickpile_api::routes as r;
use stickpile_api::{
    ApiError, CreateSet, ImportReport, ReorderItems, Set, SetDetail, SetSummary, UpdateSet, UploadResult,
};
use utoipa::IntoParams;

use super::{disposition, empty};
use crate::auth::CurrentUser;
use crate::error::AppResult;
use crate::service::sets::{self as svc, SetScope};
use crate::service::{transfer, upload as upload_svc};
use crate::state::AppState;

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// `mine` (default), `public`, or `all` (own + others' public).
    pub scope: Option<SetScope>,
}

/// Browse sets.
#[utoipa::path(get, path = r::SETS, tag = "sets", params(ListQuery),
    responses((status = 200, body = Vec<SetSummary>), (status = 401, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Vec<SetSummary>>> {
    Ok(Json(svc::list(&state, &user, q.scope.unwrap_or_default()).await?))
}

/// Create a set.
#[utoipa::path(post, path = r::SETS, tag = "sets", request_body = CreateSet,
    responses((status = 201, body = Set), (status = 400, body = ApiError), (status = 409, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<CreateSet>,
) -> AppResult<Response> {
    let set = svc::create(&state, &user, req).await?;
    Ok((StatusCode::CREATED, Json(set)).into_response())
}

/// A set with its items in order.
#[utoipa::path(get, path = r::SET, tag = "sets", params(("id" = String, Path, description = "set id")),
    responses((status = 200, body = SetDetail), (status = 404, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn get(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<SetDetail>> {
    Ok(Json(svc::detail(&state, &user, &id).await?))
}

/// Update set metadata (owner; not read-only git packs).
#[utoipa::path(patch, path = r::SET, tag = "sets", params(("id" = String, Path, description = "set id")), request_body = UpdateSet,
    responses((status = 200, body = Set), (status = 403, body = ApiError), (status = 404, body = ApiError),
              (status = 409, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateSet>,
) -> AppResult<Json<Set>> {
    Ok(Json(svc::update(&state, &user, &id, req).await?))
}

/// Delete a set and its items (owner).
#[utoipa::path(delete, path = r::SET, tag = "sets", params(("id" = String, Path, description = "set id")),
    responses((status = 204), (status = 403, body = ApiError), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn delete(State(state): State<AppState>, user: CurrentUser, Path(id): Path<String>) -> AppResult<Response> {
    svc::delete(&state, &user, &id).await?;
    Ok(empty(StatusCode::NO_CONTENT))
}

/// Replace the item order (must list every item exactly once).
#[utoipa::path(put, path = r::SET_ORDER, tag = "sets", params(("id" = String, Path, description = "set id")),
    request_body = ReorderItems, responses((status = 200, body = SetDetail), (status = 400, body = ApiError),
    (status = 403, body = ApiError), (status = 404, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn reorder(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(req): Json<ReorderItems>,
) -> AppResult<Json<SetDetail>> {
    Ok(Json(svc::reorder(&state, &user, &id, req).await?))
}

/// Upload files into a set: multipart with one or more file parts and an
/// optional `meta` JSON part (`Vec<UploadMeta>`, matched by index). A
/// single-file upload that fails returns the error status (413 `too_large`,
/// 415 `unsupported_media`); multi-file uploads report per-file errors.
#[utoipa::path(post, path = r::SET_ITEMS, tag = "sets", params(("id" = String, Path, description = "set id")),
    request_body(content_type = "multipart/form-data", description = "file parts plus optional `meta` JSON part"),
    responses((status = 200, body = Vec<UploadResult>), (status = 403, body = ApiError), (status = 404, body = ApiError),
              (status = 413, body = ApiError), (status = 415, body = ApiError), (status = 429, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn upload(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    mp: Multipart,
) -> AppResult<Json<Vec<UploadResult>>> {
    Ok(Json(upload_svc::upload_multipart(&state, &user, &id, mp).await?))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ImportQuery {
    /// Re-import into this existing own set (making it equal to the directory).
    pub into: Option<String>,
}

/// Import a set directory: a `set.toml` part plus file parts named by their
/// path relative to the set directory, or one `archive` part with a tar.
#[utoipa::path(post, path = r::SETS_IMPORT, tag = "sets", params(ImportQuery),
    request_body(content_type = "multipart/form-data", description = "`set.toml` + files, or `archive` (tar)"),
    responses((status = 200, body = ImportReport), (status = 400, body = ApiError), (status = 409, body = ApiError),
              (status = 413, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn import(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ImportQuery>,
    mp: Multipart,
) -> AppResult<Json<ImportReport>> {
    Ok(Json(
        transfer::import_multipart(&state, &user, q.into.as_deref(), mp).await?,
    ))
}

/// Export a set directory as a tar (`<slug>/set.toml` + numbered originals).
#[utoipa::path(get, path = r::SET_EXPORT, tag = "sets", params(("id" = String, Path, description = "set id")),
    responses((status = 200, content_type = "application/x-tar", description = "tar archive"), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn export(State(state): State<AppState>, user: CurrentUser, Path(id): Path<String>) -> AppResult<Response> {
    let (slug, tar) = transfer::export_tar(&state, &user, &id).await?;
    let mut resp = tar.into_response();
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/x-tar"));
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        disposition("attachment", &format!("{slug}.tar")),
    );
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok(resp)
}
