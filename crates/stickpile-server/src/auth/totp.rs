//! TOTP (RFC 6238): SHA1, 6 digits, 30 s steps, accepting ±1 step. A code's
//! step is remembered so the same code cannot be replayed.

use totp_rs::{Algorithm, Secret, TOTP};

use crate::error::{AppError, AppResult};

pub const STEP: u64 = 30;

pub fn generate_secret() -> String {
    match Secret::generate_secret().to_encoded() {
        Secret::Encoded(s) => s,
        Secret::Raw(_) => unreachable!("to_encoded returns Encoded"),
    }
}

fn totp(secret_b32: &str, issuer: &str, account: &str) -> AppResult<TOTP> {
    let bytes = Secret::Encoded(secret_b32.to_string())
        .to_bytes()
        .map_err(|e| AppError::internal(format!("totp secret: {e:?}")))?;
    let issuer = issuer.replace(':', "");
    let account = account.replace(':', "");
    TOTP::new(Algorithm::SHA1, 6, 1, STEP, bytes, Some(issuer), account)
        .map_err(|e| AppError::internal(format!("totp: {e:?}")))
}

pub fn otpauth_url(secret_b32: &str, issuer: &str, account: &str) -> AppResult<String> {
    Ok(totp(secret_b32, issuer, account)?.get_url())
}

/// Code for `time` (tests and tools).
pub fn code_at(secret_b32: &str, time: u64) -> AppResult<String> {
    Ok(totp(secret_b32, "x", "x")?.generate(time))
}

/// If `code` is valid at `now` (±1 step) and its step is newer than
/// `last_step`, returns the matched step.
pub fn verify(secret_b32: &str, code: &str, now: u64, last_step: Option<i64>) -> Option<i64> {
    let code = code.trim();
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let t = totp(secret_b32, "x", "x").ok()?;
    let current = now / STEP;
    let mut matched = None;
    for step in [current.saturating_sub(1), current, current + 1] {
        // Evaluate every candidate so timing does not depend on which matched.
        if crate::ids::ct_eq(&t.generate(step * STEP), code) && matched.is_none() {
            matched = Some(step as i64);
        }
    }
    let step = matched?;
    match last_step {
        Some(last) if step <= last => None,
        _ => Some(step),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_with_skew_and_rejects_replay() {
        let s = generate_secret();
        let now = 1_700_000_000u64;
        let code = code_at(&s, now).unwrap();
        let step = verify(&s, &code, now, None).unwrap();
        assert_eq!(step as u64, now / STEP);
        assert_eq!(verify(&s, &code, now, Some(step)), None, "replay");
        let prev = code_at(&s, now - STEP).unwrap();
        assert!(verify(&s, &prev, now, None).is_some());
        let old = code_at(&s, now - 3 * STEP).unwrap();
        assert!(old == code || verify(&s, &old, now, None).is_none());
        assert_eq!(verify(&s, "abcdef", now, None), None);
        assert!(otpauth_url(&s, "stickpile", "alice")
            .unwrap()
            .starts_with("otpauth://totp/"));
    }
}
