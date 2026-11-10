//! Creating and looking up users (bootstrap admin, admin API, registration,
//! tailnet auto-create, `create-user` subcommand).

use sqlx::SqlitePool;
use stickpile_api::Role;

use crate::auth::password;
use crate::db::{self, enum_str, UserRow};
use crate::error::{AppError, AppResult};
use crate::ids::{new_id, now};

pub struct NewUser {
    pub username: String,
    pub display_name: Option<String>,
    pub password: Option<String>,
    pub role: Role,
    pub tailscale_login: Option<String>,
}

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
            password::validate(&p)?;
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
