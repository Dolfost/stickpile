//! Item endpoints: detail, edit, delete, file download, likes, better
//! versions, re-enrichment, client cache sync.

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use stickpile_api::routes as r;
use stickpile_api::{ApiError, Item, ItemDetail, ItemPage, ItemSummary, Job, LikeState, UpdateItem, Variant};
use tower::ServiceExt;
use utoipa::IntoParams;

use super::{disposition, empty};
use crate::auth::CurrentUser;
use crate::authz;
use crate::error::{AppError, AppResult};
use crate::files;
use crate::service::{hydrate, items as svc};
use crate::state::AppState;

/// One item with set summary, quality report, near-duplicates and incompatibilities.
#[utoipa::path(get, path = r::ITEM, tag = "items", params(("id" = String, Path, description = "item id")),
    responses((status = 200, body = ItemDetail), (status = 404, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn get(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<ItemDetail>> {
    Ok(Json(svc::detail(&state, &user, &id).await?))
}

/// Edit, confirm AI values, or move to another own set.
#[utoipa::path(patch, path = r::ITEM, tag = "items", params(("id" = String, Path, description = "item id")),
    request_body = UpdateItem, responses((status = 200, body = Item), (status = 400, body = ApiError),
    (status = 403, body = ApiError), (status = 404, body = ApiError), (status = 409, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateItem>,
) -> AppResult<Json<Item>> {
    Ok(Json(svc::update(&state, &user, &id, req).await?))
}

/// Remove an item from its set.
#[utoipa::path(delete, path = r::ITEM, tag = "items", params(("id" = String, Path, description = "item id")),
    responses((status = 204), (status = 403, body = ApiError), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn delete(State(state): State<AppState>, user: CurrentUser, Path(id): Path<String>) -> AppResult<Response> {
    svc::delete(&state, &user, &id).await?;
    Ok(empty(StatusCode::NO_CONTENT))
}

/// Raw bytes of a variant. Strong `ETag`, immutable caching, `Range` and
/// `HEAD` support; 404 for variants not (yet) generated.
#[utoipa::path(get, path = r::ITEM_FILE, tag = "items",
    params(("id" = String, Path, description = "item id"), ("variant" = Variant, Path, description = "variant name")),
    responses((status = 200, description = "file bytes", content_type = "application/octet-stream"),
              (status = 304, description = "not modified"), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn file(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, variant)): Path<(String, String)>,
    req: Request,
) -> AppResult<Response> {
    let item = authz::readable_item(&state, user.id(), &id).await?;
    let variant = Variant::parse(&variant).ok_or_else(AppError::not_found)?;
    let rows = hydrate::variant_rows(&state.db, std::slice::from_ref(&item.media_id)).await?;
    let row = rows
        .get(&item.media_id)
        .and_then(|v| v.iter().find(|r| r.variant() == Some(variant)))
        .cloned()
        .ok_or_else(AppError::not_found)?;
    let stem: String = sqlx::query_scalar("SELECT filename FROM items WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await?;
    let ext = row.path.rsplit('.').next().unwrap_or("bin").to_string();
    let filename = if variant == Variant::Original {
        format!("{stem}.{ext}")
    } else {
        format!("{stem}-{}.{ext}", variant.as_str())
    };
    let etag = format!("\"{}\"", &row.sha256);
    let etag_hv = HeaderValue::from_str(&etag).map_err(AppError::internal)?;

    let headers = |resp: &mut Response| {
        let h = resp.headers_mut();
        h.insert(header::ETAG, etag_hv.clone());
        h.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, max-age=31536000, immutable"),
        );
        h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        h.insert(header::CONTENT_DISPOSITION, disposition("inline", &filename));
    };
    let inm = req.headers().get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok());
    if inm.is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*")) {
        let mut resp = empty(StatusCode::NOT_MODIFIED);
        headers(&mut resp);
        return Ok(resp);
    }
    let abs = files::resolve(&state.media_dir(), &row.path).ok_or_else(AppError::not_found)?;
    if !abs.is_file() {
        tracing::error!("variant file missing on disk: {}", abs.display());
        return Err(AppError::not_found());
    }
    // ServeFile handles Range / If-Range / HEAD; the body of `req` is unused.
    let (parts, _) = req.into_parts();
    let mut inner = Request::from_parts(parts, Body::empty());
    inner.headers_mut().remove(header::IF_NONE_MATCH);
    let mut resp = tower_http::services::ServeFile::new(&abs)
        .oneshot(inner)
        .await
        .map_err(|e| AppError::internal(format!("serving file: {e}")))?
        .map(Body::new);
    if resp.status().is_success() || resp.status() == StatusCode::NOT_MODIFIED {
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&row.mime).map_err(AppError::internal)?,
        );
        headers(&mut resp);
    }
    Ok(resp.into_response())
}

/// Like (idempotent).
#[utoipa::path(put, path = r::ITEM_LIKE, tag = "items", params(("id" = String, Path, description = "item id")),
    responses((status = 200, body = LikeState), (status = 404, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn like(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<LikeState>> {
    Ok(Json(svc::like(&state, &user, &id).await?))
}

/// Unlike (idempotent).
#[utoipa::path(delete, path = r::ITEM_LIKE, tag = "items", params(("id" = String, Path, description = "item id")),
    responses((status = 200, body = LikeState), (status = 404, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn unlike(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<LikeState>> {
    Ok(Json(svc::unlike(&state, &user, &id).await?))
}

/// Same picture (pHash within `media.near_duplicate_distance`) at higher
/// quality, among items the caller can search.
#[utoipa::path(get, path = r::ITEM_BETTER, tag = "items", params(("id" = String, Path, description = "item id")),
    responses((status = 200, body = Vec<ItemSummary>), (status = 404, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn better(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<ItemSummary>>> {
    Ok(Json(svc::better(&state, &user, &id).await?))
}

/// Re-run AI enrichment (owner). 503 `unavailable` when AI is off, `ai_loading`
/// (with `Retry-After`) while the model loads.
#[utoipa::path(post, path = r::ITEM_ENRICH, tag = "items", params(("id" = String, Path, description = "item id")),
    responses((status = 200, body = Job), (status = 404, body = ApiError), (status = 503, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn enrich(State(state): State<AppState>, user: CurrentUser, Path(id): Path<String>) -> AppResult<Json<Job>> {
    Ok(Json(svc::enrich(&state, &user, &id).await?))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SyncQuery {
    /// `server_time` of the previous sync; enables `removed`.
    pub updated_since: Option<i64>,
    /// `next_cursor` of the previous page.
    pub cursor: Option<String>,
    /// Page size, default 500, max 2000.
    pub limit: Option<i64>,
}

/// Client cache feed: own and public items (never unlisted), oldest change first.
#[utoipa::path(get, path = r::ITEMS, tag = "items", params(SyncQuery),
    responses((status = 200, body = ItemPage), (status = 400, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn sync(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SyncQuery>,
) -> AppResult<Json<ItemPage>> {
    Ok(Json(
        svc::sync_page(&state, &user, q.updated_since, q.cursor.as_deref(), q.limit).await?,
    ))
}
