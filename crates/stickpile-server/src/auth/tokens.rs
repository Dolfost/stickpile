//! Long-lived API tokens (`spk_<52 base32 chars>`), stored as sha256.

use sqlx::SqlitePool;
use stickpile_api::{ApiToken, TokenCreated};

use crate::db::{self, UserRow};
use crate::error::{AppError, AppResult};
use crate::ids::{new_id, now, random_base32, sha256_hex};

pub const TOKEN_PREFIX: &str = "spk_";
/// `last_used_at` is written at most this often per token.
const LAST_USED_THROTTLE_SECS: i64 = 60;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TokenRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub prefix: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl TokenRow {
    pub fn to_api(&self) -> ApiToken {
        ApiToken {
            id: self.id.clone(),
            name: self.name.clone(),
            prefix: self.prefix.clone(),
            created_at: self.created_at,
            last_used_at: self.last_used_at,
            expires_at: self.expires_at,
            revoked: self.revoked_at.is_some(),
        }
    }
}

const COLUMNS: &str = "id, user_id, name, prefix, created_at, last_used_at, expires_at, revoked_at";

fn validate_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(AppError::bad_request("token name must be 1..=100 characters"));
    }
    Ok(name.to_string())
}

/// Create a token for `user`; the secret is only in the returned value.
pub async fn create(db: &SqlitePool, user: &UserRow, name: &str, expires_at: Option<i64>) -> AppResult<TokenCreated> {
    let name = validate_name(name)?;
    if let Some(exp) = expires_at {
        if exp <= now() {
            return Err(AppError::bad_request("expires_at must be in the future"));
        }
    }
    let secret = format!("{TOKEN_PREFIX}{}", random_base32(32));
    let prefix: String = secret.chars().take(TOKEN_PREFIX.len() + 8).collect();
    let row = TokenRow {
        id: new_id(),
        user_id: user.id.clone(),
        name,
        prefix,
        created_at: now(),
        last_used_at: None,
        expires_at,
        revoked_at: None,
    };
    sqlx::query(
        "INSERT INTO api_tokens (id, user_id, name, token_hash, prefix, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&row.id)
    .bind(&row.user_id)
    .bind(&row.name)
    .bind(sha256_hex(secret.as_bytes()))
    .bind(&row.prefix)
    .bind(row.created_at)
    .bind(row.expires_at)
    .execute(db)
    .await?;
    Ok(TokenCreated {
        token: secret,
        info: row.to_api(),
        user: user.to_api(),
    })
}

/// Resolve a bearer secret to (token, user). Revoked, expired, unknown tokens
/// and disabled users all yield `None`.
pub async fn authenticate(db: &SqlitePool, secret: &str) -> AppResult<Option<(TokenRow, UserRow)>> {
    if !secret.starts_with(TOKEN_PREFIX) || secret.len() > 200 {
        return Ok(None);
    }
    let hash = sha256_hex(secret.as_bytes());
    let Some(tok) = sqlx::query_as::<_, TokenRow>(&format!("SELECT {COLUMNS} FROM api_tokens WHERE token_hash = ?"))
        .bind(&hash)
        .fetch_optional(db)
        .await?
    else {
        return Ok(None);
    };
    let t = now();
    if tok.revoked_at.is_some() || tok.expires_at.is_some_and(|e| e <= t) {
        return Ok(None);
    }
    let Some(user) = db::user_by_id(db, &tok.user_id).await? else {
        return Ok(None);
    };
    if user.disabled {
        return Ok(None);
    }
    if tok.last_used_at.is_none_or(|l| t - l >= LAST_USED_THROTTLE_SECS) {
        sqlx::query("UPDATE api_tokens SET last_used_at = ? WHERE id = ?")
            .bind(t)
            .bind(&tok.id)
            .execute(db)
            .await?;
    }
    Ok(Some((tok, user)))
}

pub async fn list(db: &SqlitePool, user_id: &str) -> AppResult<Vec<ApiToken>> {
    let rows = sqlx::query_as::<_, TokenRow>(&format!(
        "SELECT {COLUMNS} FROM api_tokens WHERE user_id = ? ORDER BY created_at DESC, id"
    ))
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows.iter().map(TokenRow::to_api).collect())
}

/// Revoke one of `user_id`'s tokens. Someone else's token id -> 404.
pub async fn revoke(db: &SqlitePool, user_id: &str, token_id: &str) -> AppResult<TokenRow> {
    let tok = sqlx::query_as::<_, TokenRow>(&format!(
        "SELECT {COLUMNS} FROM api_tokens WHERE id = ? AND user_id = ?"
    ))
    .bind(token_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(AppError::not_found)?;
    sqlx::query("UPDATE api_tokens SET revoked_at = COALESCE(revoked_at, ?) WHERE id = ?")
        .bind(now())
        .bind(token_id)
        .execute(db)
        .await?;
    Ok(tok)
}
