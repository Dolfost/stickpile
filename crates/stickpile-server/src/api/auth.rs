//! Auth, account and token endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use stickpile_api::routes as r;
use stickpile_api::{
    ApiError, ApiToken, ChangePassword, CreateToken, RegisterRequest, TokenCreated, TokenRequest, TotpCode, TotpSetup,
    UpdateMe, User,
};

use super::empty;
use crate::auth::login::{self, LoginPurpose};
use crate::auth::{ClientIp, CurrentUser};
use crate::error::AppResult;
use crate::service::account;
use crate::state::AppState;

/// Password (+TOTP) login for API clients; returns a new API token.
#[utoipa::path(post, path = r::AUTH_TOKEN, tag = "auth", request_body = TokenRequest,
    responses((status = 200, body = TokenCreated),
              (status = 401, description = "`unauthorized`, or `totp_required` when a TOTP code is needed", body = ApiError),
              (status = 429, body = ApiError)))]
pub async fn token(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(req): Json<TokenRequest>,
) -> AppResult<Json<TokenCreated>> {
    let user = login::password_login(
        &state,
        &req.username,
        &req.password,
        req.totp.as_deref(),
        ip,
        LoginPurpose::ApiToken,
    )
    .await?;
    let created = crate::auth::tokens::create(&state.db, &user, &req.token_name, None).await?;
    crate::db::audit(
        &state.db,
        Some(&user.id),
        "token.create",
        Some(&created.info.id),
        ip.map(|i| i.to_string()).as_deref(),
        Some(serde_json::json!({"name": created.info.name, "via": "password_login"})),
    )
    .await;
    Ok(Json(created))
}

/// Self-service registration per the server's policy (closed / invite / open).
#[utoipa::path(post, path = r::AUTH_REGISTER, tag = "auth", request_body = RegisterRequest,
    responses((status = 201, body = User), (status = 403, body = ApiError), (status = 409, body = ApiError),
              (status = 429, body = ApiError)))]
pub async fn register(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(req): Json<RegisterRequest>,
) -> AppResult<Response> {
    let user = login::register(&state, req, ip).await?;
    Ok((StatusCode::CREATED, Json(user.to_api())).into_response())
}

/// The caller.
#[utoipa::path(get, path = r::ME, tag = "auth", responses((status = 200, body = User), (status = 401, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn me(user: CurrentUser) -> Json<User> {
    Json(user.user.to_api())
}

/// Update the caller's profile (display name, Telegram user id for pack export).
#[utoipa::path(patch, path = r::ME, tag = "auth", request_body = UpdateMe,
    responses((status = 200, body = User), (status = 400, body = ApiError), (status = 401, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn update_me(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<UpdateMe>,
) -> AppResult<Json<User>> {
    Ok(Json(account::update_me(&state, &user, req).await?))
}

/// Change the caller's password; other web sessions are revoked (API tokens are kept).
#[utoipa::path(put, path = r::ME_PASSWORD, tag = "auth", request_body = ChangePassword,
    responses((status = 204), (status = 400, body = ApiError), (status = 403, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn change_password(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<ChangePassword>,
) -> AppResult<Response> {
    account::change_password(&state, &user, req).await?;
    Ok(empty(StatusCode::NO_CONTENT))
}

/// Start TOTP enrolment.
#[utoipa::path(post, path = r::ME_TOTP, tag = "auth", responses((status = 200, body = TotpSetup), (status = 409, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn totp_begin(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<TotpSetup>> {
    Ok(Json(account::totp_begin(&state, &user).await?))
}

/// Confirm TOTP enrolment with a current code.
#[utoipa::path(post, path = r::ME_TOTP_CONFIRM, tag = "auth", request_body = TotpCode,
    responses((status = 200, body = User), (status = 400, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn totp_confirm(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<TotpCode>,
) -> AppResult<Json<User>> {
    Ok(Json(account::totp_confirm(&state, &user, &req.code).await?))
}

/// Disable TOTP (needs a current code).
#[utoipa::path(post, path = r::ME_TOTP_DISABLE, tag = "auth", request_body = TotpCode,
    responses((status = 200, body = User), (status = 400, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn totp_disable(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<TotpCode>,
) -> AppResult<Json<User>> {
    Ok(Json(account::totp_disable(&state, &user, &req.code).await?))
}

/// The caller's API tokens (secrets are never shown again).
#[utoipa::path(get, path = r::TOKENS, tag = "auth", responses((status = 200, body = Vec<ApiToken>)),
    security(("bearer" = []), ("session" = [])))]
pub async fn list_tokens(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<Vec<ApiToken>>> {
    Ok(Json(account::list_tokens(&state, &user).await?))
}

/// Create an API token for the caller.
#[utoipa::path(post, path = r::TOKENS, tag = "auth", request_body = CreateToken,
    responses((status = 200, body = TokenCreated), (status = 400, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn create_token(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<CreateToken>,
) -> AppResult<Json<TokenCreated>> {
    Ok(Json(
        account::create_token(&state, &user, &req.name, req.expires_at).await?,
    ))
}

/// Revoke one of the caller's tokens.
#[utoipa::path(delete, path = r::TOKEN, tag = "auth", params(("id" = String, Path, description = "token id")),
    responses((status = 204), (status = 404, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn revoke_token(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Response> {
    account::revoke_token(&state, &user, &id).await?;
    Ok(empty(StatusCode::NO_CONTENT))
}
