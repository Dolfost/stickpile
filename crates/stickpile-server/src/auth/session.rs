//! Web sessions: a random 256-bit cookie (`stickpile_session`), stored as its
//! sha256 with an expiry and a per-session CSRF token.
//!
//! The web UI uses [`create`] after a successful [`super::login::password_login`],
//! sets the returned cookie, and [`destroy`]s it on logout. Templates get the
//! CSRF token from [`super::CurrentUser::csrf_token`].

use axum::http::HeaderValue;
use sqlx::SqlitePool;

use crate::db::{self, UserRow};
use crate::error::AppResult;
use crate::ids::{now, random_base32, sha256_hex};
use crate::state::AppState;

pub const COOKIE_NAME: &str = "stickpile_session";
/// `last_seen_at` (and the sliding expiry) is written at most this often.
const TOUCH_INTERVAL_SECS: i64 = 600;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SessionRow {
    pub id: String,
    pub user_id: String,
    pub csrf_token: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub last_seen_at: i64,
}

/// A freshly created session. `cookie` is the full `Set-Cookie` value.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub session_id: String,
    pub secret: String,
    pub csrf_token: String,
    pub cookie: HeaderValue,
}

fn ttl_secs(state: &AppState) -> i64 {
    state.config.auth.session_ttl_days as i64 * 86_400
}

/// `Set-Cookie` value for a session secret: HttpOnly, SameSite=Lax, Path=/,
/// Secure per `server.secure_cookies`.
pub fn cookie_header(state: &AppState, secret: &str, max_age: i64) -> HeaderValue {
    let secure = if state.config.server.secure_cookies {
        "; Secure"
    } else {
        ""
    };
    let v = format!("{COOKIE_NAME}={secret}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{secure}");
    HeaderValue::from_str(&v).expect("cookie is ascii")
}

/// `Set-Cookie` value that deletes the session cookie.
pub fn clear_cookie_header(state: &AppState) -> HeaderValue {
    cookie_header(state, "", 0)
}

/// Create a session for `user` (after password + TOTP, or tailnet identity).
pub async fn create(
    state: &AppState,
    user: &UserRow,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> AppResult<NewSession> {
    let secret = random_base32(32);
    let id = sha256_hex(secret.as_bytes());
    let csrf = random_base32(32);
    let t = now();
    let ttl = ttl_secs(state);
    sqlx::query(
        "INSERT INTO sessions (id, user_id, csrf_token, created_at, expires_at, last_seen_at, ip, user_agent) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&csrf)
    .bind(t)
    .bind(t + ttl)
    .bind(t)
    .bind(ip)
    .bind(user_agent.map(|u| u.chars().take(300).collect::<String>()))
    .execute(&state.db)
    .await?;
    let cookie = cookie_header(state, &secret, ttl);
    Ok(NewSession {
        session_id: id,
        secret,
        csrf_token: csrf,
        cookie,
    })
}

/// Delete a session by id (the sha256 of the cookie secret).
pub async fn destroy(db: &SqlitePool, session_id: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(session_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Delete every session of a user, optionally keeping one.
pub async fn destroy_all(db: &SqlitePool, user_id: &str, keep: Option<&str>) -> AppResult<()> {
    sqlx::query("DELETE FROM sessions WHERE user_id = ? AND id != ?")
        .bind(user_id)
        .bind(keep.unwrap_or(""))
        .execute(db)
        .await?;
    Ok(())
}

/// Result of resolving a session cookie.
pub struct Resolved {
    pub session: SessionRow,
    pub user: UserRow,
    /// New `Set-Cookie` value when the sliding expiry moved.
    pub refreshed_cookie: Option<HeaderValue>,
}

/// Resolve a cookie secret. Expired sessions and disabled users yield `None`.
pub async fn resolve(state: &AppState, secret: &str) -> AppResult<Option<Resolved>> {
    if secret.is_empty() || secret.len() > 128 {
        return Ok(None);
    }
    let id = sha256_hex(secret.as_bytes());
    let Some(session) = sqlx::query_as::<_, SessionRow>(
        "SELECT id, user_id, csrf_token, created_at, expires_at, last_seen_at FROM sessions WHERE id = ?",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(None);
    };
    let t = now();
    if session.expires_at <= t {
        destroy(&state.db, &id).await?;
        return Ok(None);
    }
    let Some(user) = db::user_by_id(&state.db, &session.user_id).await? else {
        return Ok(None);
    };
    if user.disabled {
        return Ok(None);
    }
    let mut refreshed_cookie = None;
    if t - session.last_seen_at >= TOUCH_INTERVAL_SECS {
        let ttl = ttl_secs(state);
        sqlx::query("UPDATE sessions SET last_seen_at = ?, expires_at = ? WHERE id = ?")
            .bind(t)
            .bind(t + ttl)
            .bind(&id)
            .execute(&state.db)
            .await?;
        refreshed_cookie = Some(cookie_header(state, secret, ttl));
    }
    Ok(Some(Resolved {
        session,
        user,
        refreshed_cookie,
    }))
}

/// Read the session secret from a `Cookie` header.
pub fn secret_from_cookies(headers: &axum::http::HeaderMap) -> Option<String> {
    for value in headers.get_all(axum::http::header::COOKIE) {
        let Ok(s) = value.to_str() else { continue };
        for part in s.split(';') {
            if let Some((k, v)) = part.trim().split_once('=') {
                if k == COOKIE_NAME && !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// Remove expired sessions (periodic cleanup).
pub async fn purge_expired(db: &SqlitePool) -> AppResult<u64> {
    Ok(sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now())
        .execute(db)
        .await?
        .rows_affected())
}
