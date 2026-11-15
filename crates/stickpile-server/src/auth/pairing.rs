//! One-time pairing codes (docs/security.md#pairing): a signed-in user
//! creates one (Settings, "Add a device", or `POST /api/v1/pairings`) and a
//! new device exchanges it for an API token without a password
//! (`POST /api/v1/auth/pair`).
//!
//! * A code is 60 random bits from the OS CSPRNG, written as 12 characters
//!   of Crockford's base32 ([`stickpile_api::pairing`]). Only the sha256 of
//!   the canonical form is stored.
//! * It works for [`TTL_SECS`] and once: the exchange claims it with one
//!   conditional `UPDATE`, so of two concurrent exchanges only one succeeds.
//! * Every exchange counts against the per-IP login rate limit
//!   ([`crate::security::RateLimiters::check_pairing`]). A wrong, expired or
//!   used code, a code of a disabled account and input that cannot be a code
//!   all get the same `401` ([`invalid`]); the audit log tells them apart.

use std::net::IpAddr;

use rand::RngCore;
use sqlx::SqlitePool;
use stickpile_api::{pairing, PairRequest, PairingCreated, TokenCreated};

use super::{tokens, CurrentUser};
use crate::db;
use crate::error::{AppError, AppResult};
use crate::ids::{new_id, now, sha256_hex};
use crate::state::AppState;

/// How long a code works: five minutes.
pub const TTL_SECS: i64 = 5 * 60;
/// Rows are kept this long after they expire, so a late attempt with a used
/// or expired code is still audited as such; then they are deleted.
pub const KEEP_SECS: i64 = 60 * 60;
/// Longest input that is looked at; anything longer cannot be a code.
const MAX_INPUT_CHARS: usize = 64;

/// The one answer to every failed exchange.
pub fn invalid() -> AppError {
    AppError::unauthorized("invalid or expired pairing code")
}

/// A new random code in canonical form.
pub fn generate() -> String {
    pairing::code_from_bits(rand::rngs::OsRng.next_u64())
}

/// What the database keeps of a code: the sha256 (hex) of its canonical form.
pub fn hash(canonical: &str) -> String {
    sha256_hex(canonical.as_bytes())
}

/// Create a code for the caller's own account. The code is only in the
/// returned value; creating one is audited (`pairing.create`).
pub async fn create(state: &AppState, viewer: &CurrentUser) -> AppResult<PairingCreated> {
    purge(&state.db).await?;
    let code = generate();
    let id = new_id();
    let t = now();
    let expires_at = t + TTL_SECS;
    sqlx::query("INSERT INTO pairing_codes (id, code_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?, ?)")
        .bind(&id)
        .bind(hash(&code))
        .bind(viewer.id())
        .bind(t)
        .bind(expires_at)
        .execute(&state.db)
        .await?;
    db::audit(
        &state.db,
        Some(viewer.id()),
        "pairing.create",
        Some(&id),
        viewer.ip_string().as_deref(),
        Some(serde_json::json!({"expires_at": expires_at})),
    )
    .await;
    let shown = pairing::display(&code);
    Ok(PairingCreated {
        pairing_url: pairing::link(&state.config.server.public_url, &shown),
        code: shown,
        expires_at,
    })
}

/// Exchange a code for a new API token of the user who created it.
/// Successes are audited like a password login (`login` with method
/// `pairing`, then `token.create`), failures as `login_failed` with the
/// reason (`unknown_code`, `expired_code`, `used_code`, `disabled`).
pub async fn exchange(state: &AppState, req: &PairRequest, ip: Option<IpAddr>) -> AppResult<TokenCreated> {
    state.limiters.check_pairing(ip)?;
    // Before the code is spent: a bad name must not cost the user the code.
    let name = tokens::validate_name(&req.token_name)?;
    let ip_s = ip.map(|i| i.to_string());
    let fail = |reason: &'static str, actor: Option<String>, pairing_id: Option<String>| {
        let db = state.db.clone();
        let ip_s = ip_s.clone();
        async move {
            db::audit(
                &db,
                actor.as_deref(),
                "login_failed",
                actor.as_deref(),
                ip_s.as_deref(),
                Some(serde_json::json!({"method": "pairing", "reason": reason, "pairing": pairing_id})),
            )
            .await;
        }
    };
    // Input that cannot be a code is hashed as it is: it matches no stored
    // hash (those are of canonical codes) and takes the unknown code's path.
    let input: String = req.code.chars().take(MAX_INPUT_CHARS).collect();
    let code_hash = hash(&pairing::normalize(&input).unwrap_or(input));
    let t = now();
    // Claim it: one statement, so two concurrent exchanges cannot both win.
    let claimed: Option<(String, String)> = sqlx::query_as(
        "UPDATE pairing_codes SET used_at = ?1 WHERE code_hash = ?2 AND used_at IS NULL AND expires_at > ?1 \
         RETURNING id, user_id",
    )
    .bind(t)
    .bind(&code_hash)
    .fetch_optional(&state.db)
    .await?;
    let Some((pairing_id, user_id)) = claimed else {
        let known: Option<(String, String, Option<i64>)> =
            sqlx::query_as("SELECT id, user_id, used_at FROM pairing_codes WHERE code_hash = ?")
                .bind(&code_hash)
                .fetch_optional(&state.db)
                .await?;
        match known {
            None => fail("unknown_code", None, None).await,
            Some((id, user, used_at)) => {
                let reason = if used_at.is_some() { "used_code" } else { "expired_code" };
                fail(reason, Some(user), Some(id)).await;
            }
        }
        return Err(invalid());
    };
    let user = match db::user_by_id(&state.db, &user_id).await? {
        Some(u) if !u.disabled => u,
        _ => {
            fail("disabled", Some(user_id), Some(pairing_id)).await;
            return Err(invalid());
        }
    };
    let created = tokens::create(&state.db, &user, &name, None).await?;
    sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
        .bind(t)
        .bind(&user.id)
        .execute(&state.db)
        .await?;
    db::audit(
        &state.db,
        Some(&user.id),
        "login",
        Some(&user.id),
        ip_s.as_deref(),
        Some(serde_json::json!({"method": "pairing", "pairing": pairing_id})),
    )
    .await;
    db::audit(
        &state.db,
        Some(&user.id),
        "token.create",
        Some(&created.info.id),
        ip_s.as_deref(),
        Some(serde_json::json!({"name": created.info.name, "via": "pairing", "pairing": pairing_id})),
    )
    .await;
    Ok(created)
}

/// Delete codes that expired more than [`KEEP_SECS`] ago (used ones too).
/// Called when a code is created and by the periodic cleanup.
pub async fn purge(db: &SqlitePool) -> AppResult<u64> {
    Ok(sqlx::query("DELETE FROM pairing_codes WHERE expires_at <= ?")
        .bind(now() - KEEP_SECS)
        .execute(db)
        .await?
        .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_codes_are_canonical_and_differ() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let c = generate();
            assert_eq!(c.len(), pairing::CODE_LEN);
            assert_eq!(pairing::normalize(&c).as_deref(), Some(c.as_str()));
            assert!(seen.insert(c), "60 random bits do not repeat in 1000 draws");
        }
    }

    #[test]
    fn hashes_are_of_the_canonical_form() {
        let c = "7KQ2M9XA4FTR";
        assert_eq!(hash(c).len(), 64);
        assert_eq!(hash(c), hash(&pairing::normalize("7kq2-m9xa-4ftr").unwrap()));
        assert_ne!(hash(c), hash("7KQ2-M9XA-4FTR"), "only the canonical form is hashed");
    }
}
