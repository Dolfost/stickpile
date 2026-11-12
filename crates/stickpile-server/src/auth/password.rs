//! argon2id password hashing. Hashing and verification are CPU-bound and run
//! on the blocking pool.

use std::sync::OnceLock;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

use crate::error::{AppError, AppResult};

/// Shortest minimum `auth.min_password_length` may set.
pub const MIN_LEN_FLOOR: usize = crate::config::MIN_PASSWORD_LENGTH_FLOOR;
/// Default of `auth.min_password_length`.
pub const DEFAULT_MIN_LEN: usize = crate::config::DEFAULT_MIN_PASSWORD_LENGTH;
pub const MAX_LEN: usize = 1024;

/// At least `min_len` characters (`auth.min_password_length`, never below
/// [`MIN_LEN_FLOOR`]) and at most [`MAX_LEN`] bytes.
pub fn validate(password: &str, min_len: usize) -> AppResult<()> {
    let min = min_len.max(MIN_LEN_FLOOR);
    let n = password.chars().count();
    if n < min {
        return Err(AppError::bad_request(format!(
            "password must be at least {min} characters"
        )));
    }
    if password.len() > MAX_LEN {
        return Err(AppError::bad_request("password too long"));
    }
    Ok(())
}

/// argon2id (v19, default parameters: m = 19 MiB, t = 2, p = 1) PHC string.
pub fn hash_blocking(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("argon2: {e}"))?
        .to_string())
}

pub async fn hash(password: String) -> AppResult<String> {
    Ok(tokio::task::spawn_blocking(move || hash_blocking(&password)).await??)
}

/// Constant-time verification against a PHC string.
pub fn verify_blocking(password: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok(),
        Err(_) => false,
    }
}

/// Verify `password` against `phc`, or against a dummy hash when the user does
/// not exist / has no password, so response timing does not reveal which.
pub async fn verify(password: String, phc: Option<String>) -> bool {
    let res = tokio::task::spawn_blocking(move || match phc {
        Some(phc) => verify_blocking(&password, &phc),
        None => {
            let _ = verify_blocking(&password, dummy_hash());
            false
        }
    })
    .await;
    res.unwrap_or(false)
}

fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| hash_blocking("stickpile-dummy-password").unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let h = hash_blocking("correct horse").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_blocking("correct horse", &h));
        assert!(!verify_blocking("wrong", &h));
        assert!(!verify_blocking("x", "not a hash"));
    }

    #[test]
    fn length_rules() {
        assert!(validate("short", DEFAULT_MIN_LEN).is_err());
        assert!(validate("long enough", DEFAULT_MIN_LEN).is_ok());
        // A configured minimum of 4, and never below the floor.
        assert!(validate("1234", 4).is_ok());
        assert!(validate("abc", 4).is_err());
        assert!(validate("abc", 1).is_err());
        assert!(validate("ab\u{e9}d", 4).is_ok(), "characters, not bytes");
        assert!(validate(&"x".repeat(MAX_LEN + 1), 4).is_err());
    }
}
