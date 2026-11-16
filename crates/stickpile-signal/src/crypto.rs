//! Sticker pack encryption.
//!
//! Every file of a pack (the manifest and each sticker) is encrypted the same
//! way with keys derived from the 32-byte pack key:
//!
//! * HKDF-SHA256 with a salt of 32 zero bytes and info `"Sticker Pack"`
//!   gives 64 bytes: the AES-256 key (first 32) and the HMAC-SHA256 key
//!   (last 32).
//! * The blob is `IV (16) | AES-256-CBC ciphertext (PKCS#7 padding) |
//!   HMAC-SHA256(IV | ciphertext) (32)`.
//!
//! Sources: Signal-Desktop `ts/Crypto.node.ts` (`deriveStickerPackKey`:
//! salt `getZeroes(32)`, info `'Sticker Pack'`; `decryptAttachmentV1`: IV,
//! ciphertext, MAC, MAC checked before decrypting) and
//! `sticker-creator/src/util/crypto.ts` (`encryptAttachment`: random IV,
//! AES-CBC, HMAC over IV and ciphertext); signalstickers-client
//! `classes/signalcrypto.py` (the same layout, PKCS#7, HKDF without salt,
//! which RFC 5869 defines as 32 zero bytes).

use aes::Aes256;
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;

pub const PACK_KEY_LEN: usize = 32;
pub const IV_LEN: usize = 16;
pub const MAC_LEN: usize = 32;
const INFO: &[u8] = b"Sticker Pack";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    #[error("the encrypted file is too short")]
    TooShort,
    /// Wrong pack key, or the file was changed.
    #[error("the MAC does not match (wrong pack key or a damaged file)")]
    BadMac,
    #[error("the decrypted data has invalid padding")]
    BadPadding,
}

/// The AES and HMAC keys of one pack. Not printable.
#[derive(Clone)]
pub struct PackKeys {
    aes: [u8; 32],
    mac: [u8; 32],
}

impl std::fmt::Debug for PackKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PackKeys(<redacted>)")
    }
}

/// HKDF-SHA256(pack key, salt 32 zero bytes, info "Sticker Pack") -> 64
/// bytes: AES key, then HMAC key.
pub fn derive_keys(pack_key: &[u8; PACK_KEY_LEN]) -> PackKeys {
    let hk = Hkdf::<Sha256>::new(Some(&[0u8; 32]), pack_key);
    let mut okm = [0u8; 64];
    hk.expand(INFO, &mut okm)
        .expect("64 bytes is a valid HKDF-SHA256 length");
    let mut aes = [0u8; 32];
    let mut mac = [0u8; 32];
    aes.copy_from_slice(&okm[..32]);
    mac.copy_from_slice(&okm[32..]);
    PackKeys { aes, mac }
}

fn mac_of(keys: &PackKeys) -> Hmac<Sha256> {
    <Hmac<Sha256> as Mac>::new_from_slice(&keys.mac).expect("HMAC takes any key length")
}

/// Encrypts with the given IV (tests); see [`encrypt`].
pub fn encrypt_with_iv(keys: &PackKeys, plaintext: &[u8], iv: [u8; IV_LEN]) -> Vec<u8> {
    let ciphertext =
        cbc::Encryptor::<Aes256>::new(&keys.aes.into(), &iv.into()).encrypt_padded_vec_mut::<Pkcs7>(plaintext);
    let mut out = Vec::with_capacity(IV_LEN + ciphertext.len() + MAC_LEN);
    out.extend_from_slice(&iv);
    out.extend_from_slice(&ciphertext);
    let mut mac = mac_of(keys);
    mac.update(&out);
    out.extend_from_slice(&mac.finalize().into_bytes());
    out
}

/// `IV | AES-256-CBC(plaintext) | HMAC-SHA256(IV | ciphertext)` with a
/// fresh random IV.
pub fn encrypt(keys: &PackKeys, plaintext: &[u8]) -> Vec<u8> {
    let mut iv = [0u8; IV_LEN];
    rand::rngs::OsRng.fill_bytes(&mut iv);
    encrypt_with_iv(keys, plaintext, iv)
}

/// Checks the MAC (in constant time), then decrypts.
pub fn decrypt(keys: &PackKeys, blob: &[u8]) -> Result<Vec<u8>, CryptoError> {
    // At least one block of ciphertext: PKCS#7 always adds padding.
    if blob.len() < IV_LEN + 16 + MAC_LEN {
        return Err(CryptoError::TooShort);
    }
    let (body, their_mac) = blob.split_at(blob.len() - MAC_LEN);
    let mut mac = mac_of(keys);
    mac.update(body);
    mac.verify_slice(their_mac).map_err(|_| CryptoError::BadMac)?;
    let (iv, ciphertext) = body.split_at(IV_LEN);
    if ciphertext.len() % 16 != 0 {
        return Err(CryptoError::BadPadding);
    }
    let iv: [u8; IV_LEN] = iv.try_into().expect("split at IV_LEN");
    cbc::Decryptor::<Aes256>::new(&keys.aes.into(), &iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
        .map_err(|_| CryptoError::BadPadding)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> [u8; 32] {
        [n; 32]
    }

    #[test]
    fn round_trips() {
        let keys = derive_keys(&key(7));
        for len in [0usize, 1, 15, 16, 17, 31, 32, 1000, 300 * 1024] {
            let plain: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
            let blob = encrypt(&keys, &plain);
            assert_eq!(blob.len(), IV_LEN + (len / 16 + 1) * 16 + MAC_LEN, "len {len}");
            assert_eq!(decrypt(&keys, &blob).unwrap(), plain, "len {len}");
        }
        // Fresh IVs: the same plaintext encrypts differently.
        assert_ne!(encrypt(&keys, b"same"), encrypt(&keys, b"same"));
    }

    #[test]
    fn tampering_is_rejected_before_decryption() {
        let keys = derive_keys(&key(1));
        let blob = encrypt_with_iv(&keys, b"hello sticker", [9; 16]);
        for i in [0, IV_LEN, blob.len() - MAC_LEN - 1, blob.len() - 1] {
            let mut bad = blob.clone();
            bad[i] ^= 1;
            assert_eq!(decrypt(&keys, &bad), Err(CryptoError::BadMac), "byte {i}");
        }
        assert_eq!(decrypt(&derive_keys(&key(2)), &blob), Err(CryptoError::BadMac));
        assert_eq!(decrypt(&keys, &blob[..40]), Err(CryptoError::TooShort));
        assert_eq!(decrypt(&keys, &[]), Err(CryptoError::TooShort));
    }

    #[test]
    fn a_valid_mac_over_bad_padding_is_caught() {
        // Encrypt a block whose last byte is not valid PKCS#7 padding, then
        // MAC it properly: the MAC passes, the padding check must not.
        let keys = derive_keys(&key(3));
        let iv = [0u8; 16];
        let mut block = [0u8; 16];
        block[15] = 0x20;
        let mut buf = block.to_vec();
        cbc::Encryptor::<Aes256>::new(&keys.aes.into(), &iv.into())
            .encrypt_padded_mut::<cbc::cipher::block_padding::NoPadding>(&mut buf, 16)
            .unwrap();
        let mut blob = iv.to_vec();
        blob.extend(&buf);
        let mut mac = mac_of(&keys);
        mac.update(&blob);
        blob.extend(mac.finalize().into_bytes());
        assert_eq!(decrypt(&keys, &blob), Err(CryptoError::BadPadding));
    }

    #[test]
    fn hkdf_matches_rfc5869_with_zero_salt() {
        // RFC 5869 test case 3 uses an empty salt (= HashLen zero bytes) and
        // empty info; check the same HKDF with this module's salt handling.
        let ikm = [0x0bu8; 22];
        let hk = Hkdf::<Sha256>::new(Some(&[0u8; 32]), &ikm);
        let mut okm = [0u8; 42];
        hk.expand(&[], &mut okm).unwrap();
        assert_eq!(
            hex::encode(okm),
            "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"
        );
        // The derivation is deterministic and depends on the key.
        let a = derive_keys(&key(5));
        let b = derive_keys(&key(5));
        assert_eq!((a.aes, a.mac), (b.aes, b.mac));
        assert_ne!(a.aes, derive_keys(&key(6)).aes);
        assert_ne!(a.aes, a.mac);
        assert_eq!(format!("{a:?}"), "PackKeys(<redacted>)");
    }
}
