//! Password (+TOTP) login and self-service registration. Shared by the JSON
//! API ([`crate::api`]) and the web UI's login/register forms.

use std::net::IpAddr;

use stickpile_api::{RegisterRequest, Role};

use crate::config::Registration;
use crate::db::{self, UserRow};
use crate::error::{AppError, AppResult};
use crate::ids::{now, sha256_hex};
use crate::service::users::{self, NewUser};
use crate::state::AppState;

use super::{password, totp};

/// How the login will be used (recorded in the audit log).
#[derive(Debug, Clone, Copy)]
pub enum LoginPurpose {
    ApiToken,
    WebSession,
}

/// Verify username + password (+ TOTP when enabled). Rate limited per IP and
/// per username; every attempt is audited. Unknown users, wrong passwords and
/// disabled accounts give the same `unauthorized` error; a correct password
/// without a required TOTP code gives `totp_required`.
pub async fn password_login(
    state: &AppState,
    username: &str,
    password_in: &str,
    totp_code: Option<&str>,
    ip: Option<IpAddr>,
    purpose: LoginPurpose,
) -> AppResult<UserRow> {
    state.limiters.check_login(ip, username)?;
    let ip_s = ip.map(|i| i.to_string());
    let user = if username.len() <= 64 {
        db::user_by_username(&state.db, username).await?
    } else {
        None
    };
    let ok = password::verify(
        password_in.to_string(),
        user.as_ref().and_then(|u| u.password_hash.clone()),
    )
    .await;
    let fail = |reason: &'static str, user: Option<&UserRow>| {
        let db = state.db.clone();
        let actor = user.map(|u| u.id.clone());
        let ip_s = ip_s.clone();
        let username = username.chars().take(64).collect::<String>();
        async move {
            db::audit(
                &db,
                actor.as_deref(),
                "login_failed",
                actor.as_deref(),
                ip_s.as_deref(),
                Some(serde_json::json!({"username": username, "reason": reason})),
            )
            .await;
        }
    };
    let user = match user {
        Some(u) if ok && !u.disabled => u,
        Some(u) if ok => {
            fail("disabled", Some(&u)).await;
            return Err(AppError::unauthorized("invalid credentials"));
        }
        u => {
            fail(if u.is_some() { "password" } else { "unknown_user" }, u.as_ref()).await;
            return Err(AppError::unauthorized("invalid credentials"));
        }
    };
    if let Some(secret) = user.totp_secret.as_deref() {
        let Some(code) = totp_code.filter(|c| !c.trim().is_empty()) else {
            return Err(AppError::totp_required());
        };
        let Some(step) = totp::verify(secret, code, now() as u64, user.totp_last_step) else {
            fail("totp", Some(&user)).await;
            return Err(AppError::unauthorized("invalid credentials"));
        };
        sqlx::query("UPDATE users SET totp_last_step = ? WHERE id = ?")
            .bind(step)
            .bind(&user.id)
            .execute(&state.db)
            .await?;
    }
    sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
        .bind(now())
        .bind(&user.id)
        .execute(&state.db)
        .await?;
    let method = match purpose {
        LoginPurpose::ApiToken => "api_token",
        LoginPurpose::WebSession => "web_session",
    };
    db::audit(
        &state.db,
        Some(&user.id),
        "login",
        Some(&user.id),
        ip_s.as_deref(),
        Some(serde_json::json!({"method": method})),
    )
    .await;
    Ok(user)
}

/// Self-service registration per `auth.registration`:
/// `closed` -> 403; `invite` -> needs an unused, unexpired invitation code
/// (the account gets the invitation's role); `open` -> role `user`.
pub async fn register(state: &AppState, req: RegisterRequest, ip: Option<IpAddr>) -> AppResult<UserRow> {
    state.limiters.check_login(ip, &req.username)?;
    let ip_s = ip.map(|i| i.to_string());
    let policy = state.config.auth.registration;
    if policy == Registration::Closed {
        return Err(AppError::forbidden("registration is closed"));
    }
    users::validate_username(&req.username)?;
    password::validate(&req.password)?;
    let mut role = Role::User;
    let mut invitation_id = None;
    if policy == Registration::Invite {
        let code = req.invite_code.as_deref().map(str::trim).filter(|c| !c.is_empty());
        let Some(code) = code else {
            return Err(AppError::forbidden("an invitation code is required"));
        };
        let row: Option<(String, String, Option<i64>, Option<String>)> =
            sqlx::query_as("SELECT id, role, expires_at, used_by FROM invitations WHERE code_hash = ?")
                .bind(invitation_hash(code))
                .fetch_optional(&state.db)
                .await?;
        match row {
            Some((id, r, exp, None)) if exp.is_none_or(|e| e > now()) => {
                role = if r == "admin" { Role::Admin } else { Role::User };
                invitation_id = Some(id);
            }
            _ => return Err(AppError::forbidden("invalid or expired invitation code")),
        }
    }
    let user = users::create(
        &state.db,
        NewUser {
            username: req.username.clone(),
            display_name: req.display_name.clone(),
            password: Some(req.password),
            role,
            tailscale_login: None,
        },
    )
    .await?;
    if let Some(inv) = &invitation_id {
        // Claim atomically; if someone used it in the meantime, undo.
        let claimed = sqlx::query("UPDATE invitations SET used_by = ?, used_at = ? WHERE id = ? AND used_by IS NULL")
            .bind(&user.id)
            .bind(now())
            .bind(inv)
            .execute(&state.db)
            .await?
            .rows_affected();
        if claimed == 0 {
            sqlx::query("DELETE FROM users WHERE id = ?")
                .bind(&user.id)
                .execute(&state.db)
                .await?;
            return Err(AppError::forbidden("invalid or expired invitation code"));
        }
    }
    db::audit(
        &state.db,
        Some(&user.id),
        "register",
        Some(&user.id),
        ip_s.as_deref(),
        Some(serde_json::json!({"username": user.username, "role": user.role, "invitation": invitation_id})),
    )
    .await;
    Ok(user)
}

/// Invitation codes are stored as sha256 (shown once at creation).
pub fn invitation_hash(code: &str) -> String {
    sha256_hex(code.trim().as_bytes())
}
