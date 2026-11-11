//! Tailnet identity from `tailscale serve` (docs/security.md). Only consulted for requests
//! from the trusted proxy ([`crate::security::from_trusted_proxy`]); on every
//! other request the `Tailscale-User-*` headers are ignored.

use axum::http::HeaderMap;
use stickpile_api::Role;

use crate::db::{self, UserRow, USER_COLUMNS};
use crate::error::{AppError, AppResult};
use crate::service::users::{self, NewUser};
use crate::state::AppState;

pub const LOGIN_HEADER: &str = "tailscale-user-login";
pub const NAME_HEADER: &str = "tailscale-user-name";

pub struct Identity {
    pub login: String,
    pub name: Option<String>,
}

/// Read the identity headers (caller must have checked the proxy is trusted).
pub fn identity(headers: &HeaderMap) -> Option<Identity> {
    let login = headers.get(LOGIN_HEADER)?.to_str().ok()?.trim().to_string();
    if users::validate_tailscale_login(&login).is_err() {
        return None;
    }
    let name = headers
        .get(NAME_HEADER)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).trim().to_string())
        .filter(|n| !n.is_empty() && users::validate_display_name(n).is_ok());
    Some(Identity { login, name })
}

/// Outcome of mapping a tailnet login to a user.
pub enum Mapped {
    User(Box<UserRow>),
    /// Unknown login and auto-create is off: treated as no identity.
    Unknown,
    Disabled,
}

/// Map a login to its user, auto-creating it if `auth.tailscale_autocreate`,
/// and granting admin to logins in `auth.tailscale_admins`.
pub async fn resolve(state: &AppState, id: &Identity, ip: Option<&str>) -> AppResult<Mapped> {
    let is_admin_login = state
        .config
        .auth
        .tailscale_admins
        .iter()
        .any(|a| a.eq_ignore_ascii_case(&id.login));
    let user = match find(state, &id.login).await? {
        Some(u) => u,
        None if state.config.auth.tailscale_autocreate => {
            let username = users::unique_username_for_login(&state.db, &id.login).await?;
            let created = users::create(
                &state.db,
                NewUser {
                    username,
                    display_name: id.name.clone(),
                    password: None,
                    role: if is_admin_login { Role::Admin } else { Role::User },
                    tailscale_login: Some(id.login.clone()),
                },
            )
            .await;
            match created {
                Ok(u) => {
                    db::audit(
                        &state.db,
                        Some(&u.id),
                        "user.autocreate",
                        Some(&u.id),
                        ip,
                        Some(serde_json::json!({"tailscale_login": id.login, "username": u.username, "role": u.role})),
                    )
                    .await;
                    u
                }
                // Lost a race with a concurrent first request: use the winner.
                Err(e) if e.code == "conflict" => find(state, &id.login).await?.ok_or(e)?,
                Err(e) => return Err(e),
            }
        }
        None => return Ok(Mapped::Unknown),
    };
    if user.disabled {
        return Ok(Mapped::Disabled);
    }
    if is_admin_login && !user.is_admin() {
        sqlx::query("UPDATE users SET role = 'admin' WHERE id = ?")
            .bind(&user.id)
            .execute(&state.db)
            .await?;
        db::audit(
            &state.db,
            Some(&user.id),
            "user.promote",
            Some(&user.id),
            ip,
            Some(serde_json::json!({"reason": "auth.tailscale_admins", "tailscale_login": id.login})),
        )
        .await;
        return Ok(Mapped::User(Box::new(
            db::user_by_id(&state.db, &user.id)
                .await?
                .ok_or_else(AppError::not_found)?,
        )));
    }
    Ok(Mapped::User(Box::new(user)))
}

async fn find(state: &AppState, login: &str) -> AppResult<Option<UserRow>> {
    Ok(
        sqlx::query_as::<_, UserRow>(&format!("SELECT {USER_COLUMNS} FROM users WHERE tailscale_login = ?"))
            .bind(login)
            .fetch_optional(&state.db)
            .await?,
    )
}
