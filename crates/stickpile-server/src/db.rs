//! SQLite pool, embedded migrations, row types and small conversions.
//! Queries are runtime-checked (`sqlx::query*`), so no DATABASE_URL is needed
//! at build time.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use stickpile_api::{FieldSource, Role, Theme, User, Visibility};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Open (creating if needed) the database with WAL, foreign keys and a busy
/// timeout, and run the embedded migrations.
pub async fn connect(path: &Path) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(10));
    let pool = SqlitePoolOptions::new().max_connections(8).connect_with(opts).await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// Serialise a unit enum to its serde string (`Visibility::Public` -> `public`).
pub fn enum_str<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

/// Parse a serde string back into a unit enum.
pub fn parse_enum<T: DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

pub fn json_list(s: &str) -> Vec<String> {
    serde_json::from_str(s).unwrap_or_default()
}

pub fn to_json_list(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

pub fn source(s: &str) -> FieldSource {
    if s == "ai" {
        FieldSource::Ai
    } else {
        FieldSource::User
    }
}

pub fn source_str(s: FieldSource) -> &'static str {
    match s {
        FieldSource::Ai => "ai",
        FieldSource::User => "user",
    }
}

pub fn visibility(s: &str) -> Visibility {
    parse_enum(s).unwrap_or(Visibility::Private)
}

/// u64 hashes are stored as the i64 with the same bit pattern.
pub fn u64_to_i64(v: u64) -> i64 {
    v as i64
}
pub fn i64_to_u64(v: i64) -> u64 {
    v as u64
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

pub const USER_COLUMNS: &str = "id, username, display_name, password_hash, totp_secret, totp_pending_secret, \
     totp_last_step, role, tailscale_login, telegram_user_id, disabled, created_at, last_login_at, theme";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub password_hash: Option<String>,
    pub totp_secret: Option<String>,
    pub totp_pending_secret: Option<String>,
    pub totp_last_step: Option<i64>,
    pub role: String,
    pub tailscale_login: Option<String>,
    pub telegram_user_id: Option<i64>,
    pub disabled: bool,
    pub created_at: i64,
    pub last_login_at: Option<i64>,
    /// `system`, `light` or `dark` (see [`UserRow::theme`]).
    pub theme: String,
}

impl UserRow {
    pub fn role(&self) -> Role {
        if self.role == "admin" {
            Role::Admin
        } else {
            Role::User
        }
    }
    pub fn is_admin(&self) -> bool {
        self.role() == Role::Admin
    }
    /// The web UI theme (`system` for anything unexpected).
    pub fn theme(&self) -> Theme {
        Theme::parse(&self.theme).unwrap_or_default()
    }
    pub fn to_api(&self) -> User {
        User {
            id: self.id.clone(),
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            role: self.role(),
            totp_enabled: self.totp_secret.is_some(),
            tailscale_login: self.tailscale_login.clone(),
            created_at: self.created_at,
            telegram_user_id: self.telegram_user_id,
            theme: self.theme(),
        }
    }
}

pub async fn user_by_id(db: &SqlitePool, id: &str) -> sqlx::Result<Option<UserRow>> {
    sqlx::query_as::<_, UserRow>(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn user_by_username(db: &SqlitePool, username: &str) -> sqlx::Result<Option<UserRow>> {
    sqlx::query_as::<_, UserRow>(&format!("SELECT {USER_COLUMNS} FROM users WHERE username = ?"))
        .bind(username)
        .fetch_optional(db)
        .await
}

pub const SET_COLUMNS: &str = "id, owner_id, slug, name, description, visibility, aspect_w, aspect_h, recommended_px, \
     version, source, git_url, git_branch, git_subdir, git_mode, git_last_commit, git_last_pull_at, git_last_error, \
     telegram_pack, cover_item_id, cover_media_id, extra_toml, created_at, updated_at";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SetRow {
    pub id: String,
    pub owner_id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub visibility: String,
    pub aspect_w: Option<i64>,
    pub aspect_h: Option<i64>,
    pub recommended_px: Option<i64>,
    pub version: i64,
    pub source: String,
    pub git_url: Option<String>,
    pub git_branch: Option<String>,
    pub git_subdir: Option<String>,
    pub git_mode: Option<String>,
    pub git_last_commit: Option<String>,
    pub git_last_pull_at: Option<i64>,
    pub git_last_error: Option<String>,
    pub telegram_pack: Option<String>,
    /// Chosen cover; only valid while that item is in the set (see
    /// `service::sets`).
    pub cover_item_id: Option<String>,
    /// The set's own cover image (a `media` row no item needs); it wins over
    /// `cover_item_id` (see `service::cover`).
    pub cover_media_id: Option<String>,
    pub extra_toml: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl SetRow {
    pub fn visibility(&self) -> Visibility {
        visibility(&self.visibility)
    }
    /// A git pack bound in read-only mode: nobody edits it in stickpile.
    pub fn read_only(&self) -> bool {
        self.source == "git" && self.git_mode.as_deref() != Some("push")
    }
    pub fn aspect(&self) -> Option<(u32, u32)> {
        match (self.aspect_w, self.aspect_h) {
            (Some(w), Some(h)) if w > 0 && h > 0 => Some((w as u32, h as u32)),
            _ => None,
        }
    }
}

pub async fn set_by_id(db: &SqlitePool, id: &str) -> sqlx::Result<Option<SetRow>> {
    sqlx::query_as::<_, SetRow>(&format!("SELECT {SET_COLUMNS} FROM sets WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

pub const MEDIA_COLUMNS: &str = "id, sha256, original_sha256, kind, format, width, height, duration_ms, frame_count, \
     fps, has_alpha, size_bytes, phash, dhash, quality, quality_report, embedding_model, ai_keywords, ai_emojis, \
     ai_vibe, variants_done, created_at";

/// Media row without the embedding blob.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MediaRow {
    pub id: String,
    pub sha256: String,
    pub original_sha256: String,
    pub kind: String,
    pub format: String,
    pub width: i64,
    pub height: i64,
    pub duration_ms: Option<i64>,
    pub frame_count: Option<i64>,
    pub fps: Option<f64>,
    pub has_alpha: bool,
    pub size_bytes: i64,
    pub phash: i64,
    pub dhash: i64,
    pub quality: Option<i64>,
    pub quality_report: Option<String>,
    pub embedding_model: Option<String>,
    pub ai_keywords: Option<String>,
    pub ai_emojis: Option<String>,
    pub ai_vibe: Option<String>,
    pub variants_done: bool,
    pub created_at: i64,
}

impl MediaRow {
    pub fn kind(&self) -> stickpile_api::MediaKind {
        parse_enum(&self.kind).unwrap_or(stickpile_api::MediaKind::Still)
    }
    pub fn format(&self) -> stickpile_api::MediaFormat {
        parse_enum(&self.format).unwrap_or(stickpile_api::MediaFormat::Png)
    }
    pub fn probe(&self) -> stickpile_media::Probe {
        stickpile_media::Probe {
            format: self.format(),
            kind: self.kind(),
            width: self.width as u32,
            height: self.height as u32,
            frame_count: self.frame_count.map(|v| v as u32),
            duration_ms: self.duration_ms.map(|v| v as u64),
            fps: self.fps.map(|v| v as f32),
            has_alpha: self.has_alpha,
        }
    }
}

pub async fn media_by_id(db: &SqlitePool, id: &str) -> sqlx::Result<Option<MediaRow>> {
    sqlx::query_as::<_, MediaRow>(&format!("SELECT {MEDIA_COLUMNS} FROM media WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct VariantRow {
    pub media_id: String,
    pub variant: String,
    pub mime: String,
    pub width: i64,
    pub height: i64,
    pub size_bytes: i64,
    pub sha256: String,
    pub path: String,
}

impl VariantRow {
    pub fn variant(&self) -> Option<stickpile_api::Variant> {
        stickpile_api::Variant::parse(&self.variant)
    }
    pub fn to_api(&self) -> Option<stickpile_api::VariantInfo> {
        Some(stickpile_api::VariantInfo {
            variant: self.variant()?,
            mime: self.mime.clone(),
            width: self.width as u32,
            height: self.height as u32,
            size_bytes: self.size_bytes as u64,
        })
    }
}

/// Append an audit log entry. Failures are logged, never propagated: auditing
/// must not turn a successful action into an error.
pub async fn audit(
    db: &SqlitePool,
    actor_id: Option<&str>,
    action: &str,
    target: Option<&str>,
    ip: Option<&str>,
    details: Option<serde_json::Value>,
) {
    let res =
        sqlx::query("INSERT INTO audit_log (at, actor_id, action, target, ip, details) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(crate::ids::now())
            .bind(actor_id)
            .bind(action)
            .bind(target)
            .bind(ip)
            .bind(details.map(|d| d.to_string()))
            .execute(db)
            .await;
    if let Err(e) = res {
        tracing::error!("writing audit log entry {action}: {e}");
    }
}
