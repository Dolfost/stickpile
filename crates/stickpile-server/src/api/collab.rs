//! Collaboration endpoints: a set's contributors and proposals to other
//! users' sets (docs/collaboration.md).

use axum::body::Body;
use axum::extract::{Multipart, Path, Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use stickpile_api::routes as r;
use stickpile_api::{ApiError, DecideProposal, Member, NewProposal, Proposal, ProposalBox, ProposalStatus, PutMember};
use tower::ServiceExt;
use utoipa::IntoParams;

use super::empty;
use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::files;
use crate::service::{members, proposals};
use crate::state::AppState;

/// The contributors of a set (owner and contributors).
#[utoipa::path(get, path = r::SET_MEMBERS, tag = "collaboration", params(("id" = String, Path, description = "set id")),
    responses((status = 200, body = Vec<Member>), (status = 403, body = ApiError), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn list_members(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<Member>>> {
    Ok(Json(members::list(&state, &user, &id).await?))
}

/// Add a contributor or change their permissions (owner).
#[utoipa::path(put, path = r::SET_MEMBER, tag = "collaboration",
    params(("id" = String, Path, description = "set id"), ("username" = String, Path, description = "the contributor")),
    request_body = PutMember,
    responses((status = 200, body = Member), (status = 400, body = ApiError), (status = 403, body = ApiError),
              (status = 404, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn put_member(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, username)): Path<(String, String)>,
    Json(req): Json<PutMember>,
) -> AppResult<Json<Member>> {
    Ok(Json(members::put(&state, &user, &id, &username, req).await?))
}

/// Remove a contributor (owner), or leave (the contributor).
#[utoipa::path(delete, path = r::SET_MEMBER, tag = "collaboration",
    params(("id" = String, Path, description = "set id"), ("username" = String, Path, description = "the contributor")),
    responses((status = 204), (status = 403, body = ApiError), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn delete_member(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, username)): Path<(String, String)>,
) -> AppResult<Response> {
    members::remove(&state, &user, &id, &username).await?;
    Ok(empty(StatusCode::NO_CONTENT))
}

/// Propose changes to someone else's set: name, description, cover item.
#[utoipa::path(post, path = r::SET_PROPOSALS, tag = "collaboration", params(("id" = String, Path, description = "set id")),
    request_body = NewProposal,
    responses((status = 201, body = Proposal), (status = 400, body = ApiError), (status = 404, body = ApiError),
              (status = 409, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn propose_set(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(req): Json<NewProposal>,
) -> AppResult<Response> {
    let p = proposals::propose_set_changes(&state, &user, &id, req).await?;
    Ok((StatusCode::CREATED, Json(p)).into_response())
}

/// Propose new stickers for someone else's set (multipart: files, optional
/// `meta` JSON and `message`).
#[utoipa::path(post, path = r::SET_PROPOSAL_ITEMS, tag = "collaboration", params(("id" = String, Path, description = "set id")),
    request_body(content_type = "multipart/form-data", description = "file parts, optional `meta` JSON and `message` parts"),
    responses((status = 201, body = Proposal), (status = 400, body = ApiError), (status = 404, body = ApiError),
              (status = 413, body = ApiError), (status = 415, body = ApiError), (status = 429, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn propose_items(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    mp: Multipart,
) -> AppResult<Response> {
    proposals::check_target(&state, &user, &id).await?;
    let (parts, message) = proposals::read_multipart(&state, mp).await?;
    let p = proposals::propose_items(&state, &user, &id, parts, &message).await?;
    Ok((StatusCode::CREATED, Json(p)).into_response())
}

/// Propose a cover image for someone else's set (multipart: one `file`, optional `message`).
#[utoipa::path(post, path = r::SET_PROPOSAL_COVER, tag = "collaboration", params(("id" = String, Path, description = "set id")),
    request_body(content_type = "multipart/form-data", description = "one image part, optional `message` part"),
    responses((status = 201, body = Proposal), (status = 400, body = ApiError), (status = 404, body = ApiError),
              (status = 413, body = ApiError), (status = 415, body = ApiError), (status = 429, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn propose_cover(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    mp: Multipart,
) -> AppResult<Response> {
    proposals::check_target(&state, &user, &id).await?;
    let (parts, message) = proposals::read_multipart(&state, mp).await?;
    let [file] = <[_; 1]>::try_from(parts.files).map_err(|_| AppError::bad_request("send exactly one image"))?;
    let p = proposals::propose_cover_image(&state, &user, &id, &file, &message).await?;
    Ok((StatusCode::CREATED, Json(p)).into_response())
}

/// Propose changes to someone else's sticker: name, keywords, emojis, tags, vibe.
#[utoipa::path(post, path = r::ITEM_PROPOSALS, tag = "collaboration", params(("id" = String, Path, description = "item id")),
    request_body = NewProposal,
    responses((status = 201, body = Proposal), (status = 400, body = ApiError), (status = 404, body = ApiError),
              (status = 409, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn propose_item(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(req): Json<NewProposal>,
) -> AppResult<Response> {
    let p = proposals::propose_item_changes(&state, &user, &id, req).await?;
    Ok((StatusCode::CREATED, Json(p)).into_response())
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// `incoming` (default: for the sets you own or review) or `outgoing` (yours).
    #[serde(rename = "box")]
    pub which: Option<ProposalBox>,
    /// Only proposals in this state.
    pub status: Option<ProposalStatus>,
    /// At most this many (default 100, at most 500).
    pub limit: Option<usize>,
}

/// Incoming or outgoing proposals, newest first.
#[utoipa::path(get, path = r::PROPOSALS, tag = "collaboration", params(ListQuery),
    responses((status = 200, body = Vec<Proposal>), (status = 401, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Vec<Proposal>>> {
    let which = q.which.unwrap_or_default();
    Ok(Json(
        proposals::list(&state, &user, which, q.status, q.limit.unwrap_or(100)).await?,
    ))
}

/// One proposal (its proposer, the set's owner and reviewers).
#[utoipa::path(get, path = r::PROPOSAL, tag = "collaboration", params(("id" = String, Path, description = "proposal id")),
    responses((status = 200, body = Proposal), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn get(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Proposal>> {
    Ok(Json(proposals::get(&state, &user, &id).await?))
}

/// Accept: the change is made (owner or reviewer).
#[utoipa::path(post, path = r::PROPOSAL_ACCEPT, tag = "collaboration", params(("id" = String, Path, description = "proposal id")),
    request_body = DecideProposal,
    responses((status = 200, body = Proposal), (status = 403, body = ApiError), (status = 404, body = ApiError),
              (status = 409, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn accept(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    body: Option<Json<DecideProposal>>,
) -> AppResult<Json<Proposal>> {
    let note = body.map(|b| b.0.note).unwrap_or_default();
    Ok(Json(proposals::accept(&state, &user, &id, &note).await?))
}

/// Decline (owner or reviewer).
#[utoipa::path(post, path = r::PROPOSAL_DECLINE, tag = "collaboration", params(("id" = String, Path, description = "proposal id")),
    request_body = DecideProposal,
    responses((status = 200, body = Proposal), (status = 403, body = ApiError), (status = 404, body = ApiError),
              (status = 409, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn decline(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    body: Option<Json<DecideProposal>>,
) -> AppResult<Json<Proposal>> {
    let note = body.map(|b| b.0.note).unwrap_or_default();
    Ok(Json(proposals::decline(&state, &user, &id, &note).await?))
}

/// Withdraw your own open proposal.
#[utoipa::path(post, path = r::PROPOSAL_WITHDRAW, tag = "collaboration", params(("id" = String, Path, description = "proposal id")),
    responses((status = 200, body = Proposal), (status = 403, body = ApiError), (status = 404, body = ApiError),
              (status = 409, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn withdraw(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Proposal>> {
    Ok(Json(proposals::withdraw(&state, &user, &id).await?))
}

/// Thumbnail of a proposed sticker (`n`: its position) or cover image (`cover`), while open.
#[utoipa::path(get, path = r::PROPOSAL_THUMB, tag = "collaboration",
    params(("id" = String, Path, description = "proposal id"), ("n" = String, Path, description = "position, or `cover`")),
    responses((status = 200, description = "the thumbnail"), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn thumb(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, n)): Path<(String, String)>,
    req: Request,
) -> AppResult<Response> {
    let row = proposals::thumb(&state, &user, &id, &n).await?;
    let abs = files::resolve(&state.media_dir(), &row.path).ok_or_else(AppError::not_found)?;
    if !abs.is_file() {
        return Err(AppError::not_found());
    }
    let (parts, _) = req.into_parts();
    let inner = Request::from_parts(parts, Body::empty());
    let mut resp = tower_http::services::ServeFile::new(&abs)
        .oneshot(inner)
        .await
        .map_err(|e| AppError::internal(format!("serving file: {e}")))?
        .map(Body::new);
    if resp.status().is_success() {
        let h = resp.headers_mut();
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&row.mime).map_err(AppError::internal)?,
        );
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
        h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    }
    Ok(resp.into_response())
}
