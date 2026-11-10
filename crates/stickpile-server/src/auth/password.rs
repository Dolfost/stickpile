//! argon2id password hashing. Hashing and verification are CPU-bound and run
//! on the blocking pool.

use std::sync::OnceLock;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

use crate::error::{AppError, AppResult};

pub const MIN_LEN: usize = 8;
pub const MAX_LEN: usize = 1024;

pub fn validate(password: &str) -> AppResult<()> {
    let n = password.chars().count();
    if n < MIN_LEN {
        return Err(AppError::bad_request(format!(
            "password must be at least {MIN_LEN} characters"
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
        assert!(validate("short").is_err());
        assert!(validate("long enough").is_ok());
    }
}
