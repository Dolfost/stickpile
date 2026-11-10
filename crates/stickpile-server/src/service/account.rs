//! The caller's own account: profile, password change, TOTP enrolment, API
//! tokens.
//!
//! Changing your own password revokes your other web sessions but keeps your
//! API tokens (the CLI and desktop clients stay logged in; revoke tokens
//! explicitly if they may be compromised). An admin password reset revokes
//! sessions *and* tokens ([`crate::service::admin::update_user`]).

use stickpile_api::{ApiToken, ChangePassword, TokenCreated, TotpSetup, UpdateMe, User};

use crate::auth::{password, session, tokens, totp, CurrentUser};
use crate::db;
use crate::error::{AppError, AppResult};
use crate::ids::now;
use crate::state::AppState;

async fn reload(state: &AppState, viewer: &CurrentUser) -> AppResult<db::UserRow> {
    db::user_by_id(&state.db, viewer.id())
        .await?
        .ok_or_else(|| AppError::unauthorized("account no longer exists"))
}

/// Update the caller's profile: display name (empty resets it to the
/// username) and the Telegram user id that owns exported packs.
pub async fn update_me(state: &AppState, viewer: &CurrentUser, req: UpdateMe) -> AppResult<User> {
    let user = reload(state, viewer).await?;
    let mut changes = serde_json::Map::new();
    if let Some(d) = &req.display_name {
        let d = crate::service::users::validate_display_name(d)?;
        let d = if d.is_empty() { user.username.clone() } else { d };
        sqlx::query("UPDATE users SET display_name = ? WHERE id = ?")
            .bind(&d)
            .bind(viewer.id())
            .execute(&state.db)
            .await?;
        changes.insert("display_name".into(), d.into());
    }
    if let Some(tg) = req.telegram_user_id {
        if tg.is_some_and(|id| id <= 0) {
            return Err(AppError::bad_request("telegram_user_id must be a positive number"));
        }
        sqlx::query("UPDATE users SET telegram_user_id = ? WHERE id = ?")
            .bind(tg)
            .bind(viewer.id())
            .execute(&state.db)
            .await?;
        changes.insert(
            "telegram_user_id".into(),
            tg.map(Into::into).unwrap_or(serde_json::Value::Null),
        );
    }
    if !changes.is_empty() {
        db::audit(
            &state.db,
            Some(viewer.id()),
            "user.update_self",
            Some(viewer.id()),
            viewer.ip_string().as_deref(),
            Some(serde_json::Value::Object(changes)),
        )
        .await;
    }
    Ok(reload(state, viewer).await?.to_api())
}

/// Change the caller's password (the current one is required if set). Other
/// web sessions of the user are revoked; API tokens are kept.
pub async fn change_password(state: &AppState, viewer: &CurrentUser, req: ChangePassword) -> AppResult<()> {
    let user = reload(state, viewer).await?;
    if let Some(hash) = user.password_hash.clone() {
        let current = req.current_password.clone().unwrap_or_default();
        state.limiters.check_login(viewer.ip, &user.username)?;
        if !password::verify(current, Some(hash)).await {
            return Err(AppError::forbidden("current password is wrong"));
        }
    }
    password::validate(&req.new_password)?;
    let hash = password::hash(req.new_password).await?;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(viewer.id())
        .execute(&state.db)
        .await?;
    session::destroy_all(&state.db, viewer.id(), viewer.session_id()).await?;
    db::audit(
        &state.db,
        Some(viewer.id()),
        "password.change",
        Some(viewer.id()),
        viewer.ip_string().as_deref(),
        None,
    )
    .await;
    Ok(())
}

/// Start TOTP enrolment: a new pending secret (replaces any pending one).
pub async fn totp_begin(state: &AppState, viewer: &CurrentUser) -> AppResult<TotpSetup> {
    let user = reload(state, viewer).await?;
    if user.totp_secret.is_some() {
        return Err(AppError::conflict("TOTP is already enabled; disable it first"));
    }
    let secret = totp::generate_secret();
    sqlx::query("UPDATE users SET totp_pending_secret = ? WHERE id = ?")
        .bind(&secret)
        .bind(viewer.id())
        .execute(&state.db)
        .await?;
    let otpauth_url = totp::otpauth_url(&secret, &state.config.auth.totp_issuer, &user.username)?;
    Ok(TotpSetup { secret, otpauth_url })
}

/// Confirm enrolment with a current code from the pending secret.
pub async fn totp_confirm(state: &AppState, viewer: &CurrentUser, code: &str) -> AppResult<User> {
    let user = reload(state, viewer).await?;
    let Some(pending) = user.totp_pending_secret.as_deref() else {
        return Err(AppError::bad_request("no TOTP enrolment in progress"));
    };
    state.limiters.check_login(viewer.ip, &user.username)?;
    let Some(step) = totp::verify(pending, code, now() as u64, None) else {
        return Err(AppError::bad_request("invalid TOTP code"));
    };
    sqlx::query(
        "UPDATE users SET totp_secret = totp_pending_secret, totp_pending_secret = NULL, totp_last_step = ? WHERE id = ?",
    )
    .bind(step)
    .bind(viewer.id())
    .execute(&state.db)
    .await?;
    db::audit(
        &state.db,
        Some(viewer.id()),
        "totp.enable",
        Some(viewer.id()),
        viewer.ip_string().as_deref(),
        None,
    )
    .await;
    Ok(reload(state, viewer).await?.to_api())
}

/// Disable TOTP (needs a current code).
pub async fn totp_disable(state: &AppState, viewer: &CurrentUser, code: &str) -> AppResult<User> {
    let user = reload(state, viewer).await?;
    let Some(secret) = user.totp_secret.as_deref() else {
        return Err(AppError::bad_request("TOTP is not enabled"));
    };
    state.limiters.check_login(viewer.ip, &user.username)?;
    if totp::verify(secret, code, now() as u64, user.totp_last_step).is_none() {
        return Err(AppError::bad_request("invalid TOTP code"));
    }
    sqlx::query("UPDATE users SET totp_secret = NULL, totp_pending_secret = NULL, totp_last_step = NULL WHERE id = ?")
        .bind(viewer.id())
        .execute(&state.db)
        .await?;
    db::audit(
        &state.db,
        Some(viewer.id()),
        "totp.disable",
        Some(viewer.id()),
        viewer.ip_string().as_deref(),
        None,
    )
    .await;
    Ok(reload(state, viewer).await?.to_api())
}

pub async fn create_token(
    state: &AppState,
    viewer: &CurrentUser,
    name: &str,
    expires_at: Option<i64>,
) -> AppResult<TokenCreated> {
    let created = tokens::create(&state.db, &viewer.user, name, expires_at).await?;
    db::audit(
        &state.db,
        Some(viewer.id()),
        "token.create",
        Some(&created.info.id),
        viewer.ip_string().as_deref(),
        Some(serde_json::json!({"name": created.info.name, "expires_at": created.info.expires_at})),
    )
    .await;
    Ok(created)
}

pub async fn list_tokens(state: &AppState, viewer: &CurrentUser) -> AppResult<Vec<ApiToken>> {
    tokens::list(&state.db, viewer.id()).await
}

pub async fn revoke_token(state: &AppState, viewer: &CurrentUser, token_id: &str) -> AppResult<()> {
    let tok = tokens::revoke(&state.db, viewer.id(), token_id).await?;
    db::audit(
        &state.db,
        Some(viewer.id()),
        "token.revoke",
        Some(token_id),
        viewer.ip_string().as_deref(),
        Some(serde_json::json!({"name": tok.name})),
    )
    .await;
    Ok(())
}
