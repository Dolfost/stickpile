//! Identifiers, secrets, hashing and time helpers.

use rand::RngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// 26-char lowercase base32 (no padding) of 128 random bits.
pub fn new_id() -> String {
    random_base32(16)
}

/// Lowercase base32 (no padding) of `n` bytes from the OS CSPRNG.
pub fn random_base32(n: usize) -> String {
    let mut buf = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    data_encoding::BASE32_NOPAD.encode(&buf).to_ascii_lowercase()
}

/// True if `s` looks like an id produced by [`new_id`]. Used to reject
/// garbage early (and keep it out of logs and SQL).
pub fn is_id(s: &str) -> bool {
    s.len() == 26 && s.bytes().all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Constant-time string comparison (length is not secret).
pub fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// Unix seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_26_lowercase_base32() {
        let a = new_id();
        let b = new_id();
        assert_eq!(a.len(), 26);
        assert!(is_id(&a), "{a}");
        assert_ne!(a, b);
        assert!(!is_id("../etc/passwd"));
        assert!(!is_id(&a.to_uppercase()));
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
    }
}
