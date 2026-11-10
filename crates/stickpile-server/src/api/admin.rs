//! Admin endpoints (role `admin`; authenticated non-admins get 403).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use stickpile_api::routes as r;
use stickpile_api::{
    AdminCreateUser, AdminUpdateUser, AdminUser, ApiError, AuditEntry, CreateInvitation, Invitation, InvitationCreated,
    Job, JobStatus, StorageUsage,
};
use utoipa::IntoParams;

use super::empty;
use crate::auth::AdminUser as Admin;
use crate::error::AppResult;
use crate::jobs;
use crate::service::admin as svc;
use crate::state::AppState;

/// All users.
#[utoipa::path(get, path = r::ADMIN_USERS, tag = "admin",
    responses((status = 200, body = Vec<AdminUser>), (status = 403, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn list_users(State(state): State<AppState>, _admin: Admin) -> AppResult<Json<Vec<AdminUser>>> {
    Ok(Json(svc::list_users(&state).await?))
}

/// Create a user.
#[utoipa::path(post, path = r::ADMIN_USERS, tag = "admin", request_body = AdminCreateUser,
    responses((status = 201, body = AdminUser), (status = 400, body = ApiError), (status = 403, body = ApiError),
              (status = 409, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn create_user(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Json(req): Json<AdminCreateUser>,
) -> AppResult<Response> {
    let u = svc::create_user(&state, &admin, req).await?;
    Ok((StatusCode::CREATED, Json(u)).into_response())
}

/// Update role, disabled flag, password, tailscale login, or reset TOTP.
#[utoipa::path(patch, path = r::ADMIN_USER, tag = "admin", params(("id" = String, Path, description = "user id")),
    request_body = AdminUpdateUser, responses((status = 200, body = AdminUser), (status = 403, body = ApiError),
    (status = 404, body = ApiError), (status = 409, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn update_user(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    Json(req): Json<AdminUpdateUser>,
) -> AppResult<Json<AdminUser>> {
    Ok(Json(svc::update_user(&state, &admin, &id, req).await?))
}

/// Delete a user and everything they own.
#[utoipa::path(delete, path = r::ADMIN_USER, tag = "admin", params(("id" = String, Path, description = "user id")),
    responses((status = 204), (status = 403, body = ApiError), (status = 404, body = ApiError), (status = 409, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn delete_user(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> AppResult<Response> {
    svc::delete_user(&state, &admin, &id).await?;
    Ok(empty(StatusCode::NO_CONTENT))
}

/// Invitations (codes are never shown again).
#[utoipa::path(get, path = r::ADMIN_INVITATIONS, tag = "admin", responses((status = 200, body = Vec<Invitation>)),
    security(("bearer" = []), ("session" = [])))]
pub async fn list_invitations(State(state): State<AppState>, _admin: Admin) -> AppResult<Json<Vec<Invitation>>> {
    Ok(Json(svc::list_invitations(&state).await?))
}

/// Create an invitation; the returned URL contains the one-time code.
#[utoipa::path(post, path = r::ADMIN_INVITATIONS, tag = "admin", request_body = CreateInvitation,
    responses((status = 201, body = InvitationCreated), (status = 400, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn create_invitation(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Json(req): Json<CreateInvitation>,
) -> AppResult<Response> {
    let inv = svc::create_invitation(&state, &admin, req).await?;
    Ok((StatusCode::CREATED, Json(inv)).into_response())
}

/// Delete an invitation.
#[utoipa::path(delete, path = r::ADMIN_INVITATION, tag = "admin", params(("id" = String, Path, description = "invitation id")),
    responses((status = 204), (status = 404, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn delete_invitation(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> AppResult<Response> {
    svc::delete_invitation(&state, &admin, &id).await?;
    Ok(empty(StatusCode::NO_CONTENT))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditQuery {
    /// Return entries with an id below this (paging).
    pub before: Option<i64>,
    /// Default 100, max 1000.
    pub limit: Option<i64>,
}

/// Audit log, newest first.
#[utoipa::path(get, path = r::ADMIN_AUDIT, tag = "admin", params(AuditQuery),
    responses((status = 200, body = Vec<AuditEntry>)), security(("bearer" = []), ("session" = [])))]
pub async fn audit(
    State(state): State<AppState>,
    _admin: Admin,
    Query(q): Query<AuditQuery>,
) -> AppResult<Json<Vec<AuditEntry>>> {
    Ok(Json(svc::audit_page(&state, q.before, q.limit).await?))
}

/// Disk usage of media and database.
#[utoipa::path(get, path = r::ADMIN_STORAGE, tag = "admin", responses((status = 200, body = StorageUsage)),
    security(("bearer" = []), ("session" = [])))]
pub async fn storage(State(state): State<AppState>, _admin: Admin) -> AppResult<Json<StorageUsage>> {
    Ok(Json(svc::storage(&state).await?))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct JobsQuery {
    /// `queued`, `running`, `done` or `failed`.
    pub status: Option<JobStatus>,
    /// Default 200, max 1000.
    pub limit: Option<i64>,
}

/// Job queue, newest first.
#[utoipa::path(get, path = r::ADMIN_JOBS, tag = "admin", params(JobsQuery), responses((status = 200, body = Vec<Job>)),
    security(("bearer" = []), ("session" = [])))]
pub async fn jobs(
    State(state): State<AppState>,
    _admin: Admin,
    Query(q): Query<JobsQuery>,
) -> AppResult<Json<Vec<Job>>> {
    Ok(Json(
        jobs::list(&state.db, q.status, q.limit.unwrap_or(200).clamp(1, 1000)).await?,
    ))
}
