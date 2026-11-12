//! Creating, renaming and looking up users (bootstrap admin, admin API,
//! registration, tailnet auto-create, `create-user` subcommand).

use sqlx::SqlitePool;
use stickpile_api::Role;

use crate::auth::password;
use crate::db::{self, enum_str, UserRow};
use crate::error::{AppError, AppResult};
use crate::ids::{new_id, now};
use crate::state::AppState;

pub struct NewUser {
    pub username: String,
    pub display_name: Option<String>,
    pub password: Option<String>,
    pub role: Role,
    pub tailscale_login: Option<String>,
}

/// [`validate_username`] as an HTML `pattern` for username inputs.
pub const USERNAME_PATTERN: &str = "[A-Za-z0-9][A-Za-z0-9_.\\-]{1,31}";

/// 2..=32 chars of `[A-Za-z0-9_.-]`, starting with a letter or digit.
pub fn validate_username(name: &str) -> AppResult<()> {
    let ok = (2..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        && name.as_bytes()[0].is_ascii_alphanumeric();
    if ok {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "username must be 2..=32 characters of A-Z a-z 0-9 _ . - starting with a letter or digit",
        ))
    }
}

pub fn validate_display_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(AppError::bad_request(
            "display name must be at most 100 printable characters",
        ));
    }
    Ok(name.to_string())
}

pub fn validate_tailscale_login(login: &str) -> AppResult<String> {
    let login = login.trim();
    if login.is_empty() || login.len() > 256 || login.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(AppError::bad_request("invalid tailscale login"));
    }
    Ok(login.to_string())
}

/// Insert a user. Username clashes (case-insensitive) and tailscale login
/// clashes -> 409.
pub async fn create(db: &SqlitePool, new: NewUser) -> AppResult<UserRow> {
    validate_username(&new.username)?;
    let display_name = match new.display_name.as_deref() {
        Some(d) if !d.trim().is_empty() => validate_display_name(d)?,
        _ => new.username.clone(),
    };
    let tailscale_login = new
        .tailscale_login
        .as_deref()
        .map(validate_tailscale_login)
        .transpose()?;
    let password_hash = match new.password {
        Some(p) => {
            // The configured minimum is checked by the callers that take a
            // password from a person (they have the config).
            password::validate(&p, password::MIN_LEN_FLOOR)?;
            Some(password::hash(p).await?)
        }
        None => None,
    };
    let id = new_id();
    let res = sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, role, tailscale_login, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&new.username)
    .bind(&display_name)
    .bind(&password_hash)
    .bind(enum_str(&new.role))
    .bind(&tailscale_login)
    .bind(now())
    .execute(db)
    .await;
    match res {
        Ok(_) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(AppError::conflict("username or tailscale login already taken"));
        }
        Err(e) => return Err(e.into()),
    }
    db::user_by_id(db, &id)
        .await?
        .ok_or_else(|| AppError::internal("user vanished after insert"))
}

/// Derive a free username from a tailnet login: `alice@github` -> `alice`,
/// then `alice-2`, `alice-3`, ...
pub async fn unique_username_for_login(db: &SqlitePool, login: &str) -> AppResult<String> {
    let local = login.split('@').next().unwrap_or(login);
    let mut base: String = local
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    base = base.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_string();
    base.truncate(28);
    if base.len() < 2 {
        base = format!("user{base}");
    }
    for n in 1..10_000 {
        let candidate = if n == 1 { base.clone() } else { format!("{base}-{n}") };
        if db::user_by_username(db, &candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
    Err(AppError::conflict("could not derive a free username"))
}

/// Rename a user (self-service or admin). Returns the old username, or `None`
/// when the name is unchanged. A taken name (ignoring case) is a 409; a change
/// of case only is allowed.
///
/// The username is shown as the owner of sets and items and matched by the
/// `by:` search filter, so the search index is refreshed and the user's items
/// get a new `updated_at`, which makes clients refetch them into their
/// offline caches (like [`crate::service::sets::touch_items`]). A display
/// name still equal to the old username follows the rename. Sessions, tokens
/// and the tailnet login are bound to the user id and keep working.
pub async fn rename(state: &AppState, user_id: &str, new: &str) -> AppResult<Option<String>> {
    let new = new.trim();
    validate_username(new)?;
    let user = db::user_by_id(&state.db, user_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    if user.username == new {
        return Ok(None);
    }
    let mut tx = state.db.begin().await?;
    let res = sqlx::query(
        "UPDATE users SET username = ?1, \
         display_name = CASE WHEN display_name = ?2 THEN ?1 ELSE display_name END WHERE id = ?3",
    )
    .bind(new)
    .bind(&user.username)
    .bind(user_id)
    .execute(&mut *tx)
    .await;
    match res {
        Ok(_) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(AppError::conflict("that username is already taken"));
        }
        Err(e) => return Err(e.into()),
    }
    sqlx::query("UPDATE items SET updated_at = ? WHERE set_id IN (SELECT id FROM sets WHERE owner_id = ?)")
        .bind(now())
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let sets: Vec<String> = sqlx::query_scalar("SELECT id FROM sets WHERE owner_id = ?")
        .bind(user_id)
        .fetch_all(&state.db)
        .await?;
    for set_id in &sets {
        crate::index::refresh_set(state, set_id).await?;
    }
    Ok(Some(user.username))
}

/// Operator password reset (`stickpile-server reset-password`): the way back
/// in for a forgotten password when no admin can reset it in the admin panel
/// (or the only admin forgot theirs). Signs the user out of every web
/// session. API tokens are kept unless `revoke_tokens`: a forgotten password
/// says nothing about the CLI and desktop clients. `reset_totp` also turns
/// off two-step verification (lost authenticator).
pub async fn reset_password(
    db: &SqlitePool,
    username: &str,
    new_password: String,
    min_len: usize,
    revoke_tokens: bool,
    reset_totp: bool,
) -> AppResult<UserRow> {
    password::validate(&new_password, min_len)?;
    let user = db::user_by_username(db, username)
        .await?
        .ok_or_else(|| AppError::bad_request(format!("there is no user named {username:?}")))?;
    let hash = password::hash(new_password).await?;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(&user.id)
        .execute(db)
        .await?;
    if reset_totp {
        sqlx::query(
            "UPDATE users SET totp_secret = NULL, totp_pending_secret = NULL, totp_last_step = NULL WHERE id = ?",
        )
        .bind(&user.id)
        .execute(db)
        .await?;
    }
    crate::auth::session::destroy_all(db, &user.id, None).await?;
    let revoked = if revoke_tokens {
        crate::auth::tokens::revoke_all(db, &user.id).await?
    } else {
        0
    };
    db::audit(
        db,
        None,
        "user.password_reset",
        Some(&user.id),
        None,
        Some(serde_json::json!({"via": "cli", "tokens_revoked": revoked, "reset_totp": reset_totp})),
    )
    .await;
    Ok(user)
}

/// Create the configured initial admin when the database has no users.
pub async fn bootstrap_admin(db: &SqlitePool, config: &crate::Config) -> anyhow::Result<()> {
    let (Some(username), Some(pw_file)) = (
        config.auth.initial_admin_username.as_deref(),
        config.auth.initial_admin_password_file.as_deref(),
    ) else {
        return Ok(());
    };
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(db).await?;
    if count > 0 {
        return Ok(());
    }
    let password = crate::config::read_secret(pw_file)
        .map_err(|e| anyhow::anyhow!("reading auth.initial_admin_password_file {}: {e}", pw_file.display()))?;
    password::validate(&password, config.auth.min_password_length)
        .map_err(|e| anyhow::anyhow!("auth.initial_admin_password_file: {e}"))?;
    let user = create(
        db,
        NewUser {
            username: username.to_string(),
            display_name: None,
            password: Some(password),
            role: Role::Admin,
            tailscale_login: None,
        },
    )
    .await
    .map_err(|e| anyhow::anyhow!("creating initial admin: {e}"))?;
    db::audit(
        db,
        None,
        "user.bootstrap",
        Some(&user.id),
        None,
        Some(serde_json::json!({"username": user.username})),
    )
    .await;
    tracing::info!("created initial admin user {username:?}");
    Ok(())
}

/// Number of enabled admins (to refuse removing the last one).
pub async fn active_admin_count(db: &SqlitePool) -> AppResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled = 0")
            .fetch_one(db)
            .await?,
    )
}
