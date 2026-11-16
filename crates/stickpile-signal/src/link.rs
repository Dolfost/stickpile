//! Sticker pack links.
//!
//! `https://signal.art/addstickers/#pack_id=<32 hex>&pack_key=<64 hex>` (the
//! parameters in the fragment, so they never reach signal.art) and the app
//! form `sgnl://addstickers/?pack_id=...&pack_key=...` (the parameters in the
//! query). The trailing slash is optional in both.
//!
//! Source: Signal-Desktop `ts/util/signalRoutes.std.ts`
//! (`artAddStickersRoute`: patterns `https://signal.art/addstickers{/}?` with
//! the parameters in the hash and `sgnl://addstickers{/}?` with them in the
//! search) and `ts/util/Stickers.std.ts` (`isPackIdValid`: 32 hex digits).
//! The pack id is 16 random bytes in lower-case hex (Signal-Server
//! `StickerController.generatePackId`); the pack key is 32 bytes in hex.

use rand::RngCore;

use crate::crypto::PACK_KEY_LEN;
use crate::SignalError;

/// A pack key: whoever has it can read the pack. Not printable; written
/// out only by [`PackLink::url`].
#[derive(Clone, PartialEq, Eq)]
pub struct PackKey([u8; PACK_KEY_LEN]);

impl std::fmt::Debug for PackKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PackKey(<redacted>)")
    }
}

impl PackKey {
    /// A fresh random key from the OS random source.
    pub fn random() -> PackKey {
        let mut key = [0u8; PACK_KEY_LEN];
        rand::rngs::OsRng.fill_bytes(&mut key);
        PackKey(key)
    }

    pub fn from_bytes(bytes: [u8; PACK_KEY_LEN]) -> PackKey {
        PackKey(bytes)
    }

    /// 64 hex digits (either case).
    pub fn from_hex(text: &str) -> Result<PackKey, SignalError> {
        let mut key = [0u8; PACK_KEY_LEN];
        if text.len() != 2 * PACK_KEY_LEN || hex::decode_to_slice(text, &mut key).is_err() {
            return Err(SignalError::InvalidLink(
                "pack_key must be 64 hexadecimal digits".into(),
            ));
        }
        Ok(PackKey(key))
    }

    pub fn as_bytes(&self) -> &[u8; PACK_KEY_LEN] {
        &self.0
    }

    fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

/// A pack id and its key.
#[derive(Clone, PartialEq, Eq)]
pub struct PackLink {
    /// 32 lower-case hex digits.
    pub pack_id: String,
    pub key: PackKey,
}

impl std::fmt::Debug for PackLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackLink")
            .field("pack_id", &self.pack_id)
            .field("key", &self.key)
            .finish()
    }
}

/// `true` for 32 hex digits.
pub fn valid_pack_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The last 3 characters of a pack id, for logs (as Signal-Desktop's
/// `redactPackId` does).
pub fn redact_pack_id(id: &str) -> String {
    let tail: String = id.chars().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect();
    format!("[REDACTED]{tail}")
}

impl PackLink {
    pub fn new(pack_id: &str, key: PackKey) -> Result<PackLink, SignalError> {
        if !valid_pack_id(pack_id) {
            return Err(SignalError::InvalidLink("pack_id must be 32 hexadecimal digits".into()));
        }
        Ok(PackLink {
            pack_id: pack_id.to_ascii_lowercase(),
            key,
        })
    }

    /// Parses either link form. Errors never contain the key.
    pub fn parse(link: &str) -> Result<PackLink, SignalError> {
        let bad = |why: &str| SignalError::InvalidLink(why.to_string());
        let url = url::Url::parse(link.trim()).map_err(|_| bad("not a URL"))?;
        let params = match url.scheme() {
            "https" if url.host_str() == Some("signal.art") && matches!(url.path(), "/addstickers" | "/addstickers/") => {
                url.fragment().unwrap_or_default().to_string()
            }
            "sgnl" if url.host_str() == Some("addstickers") && matches!(url.path(), "" | "/") => {
                url.query().unwrap_or_default().to_string()
            }
            _ => {
                return Err(bad(
                    "expected https://signal.art/addstickers/#pack_id=...&pack_key=... or sgnl://addstickers/?pack_id=...&pack_key=...",
                ))
            }
        };
        let mut pack_id = None;
        let mut pack_key = None;
        // Like URLSearchParams.get: the first occurrence counts.
        for (k, v) in url::form_urlencoded::parse(params.as_bytes()) {
            match k.as_ref() {
                "pack_id" if pack_id.is_none() => pack_id = Some(v.into_owned()),
                "pack_key" if pack_key.is_none() => pack_key = Some(v.into_owned()),
                _ => {}
            }
        }
        let pack_id = pack_id
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("pack_id is missing"))?;
        let pack_key = pack_key
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("pack_key is missing"))?;
        PackLink::new(&pack_id, PackKey::from_hex(&pack_key)?)
    }

    /// `https://signal.art/addstickers/#pack_id=...&pack_key=...`
    pub fn url(&self) -> String {
        format!(
            "https://signal.art/addstickers/#pack_id={}&pack_key={}",
            self.pack_id,
            self.key.to_hex()
        )
    }

    /// `sgnl://addstickers/?pack_id=...&pack_key=...`
    pub fn app_url(&self) -> String {
        format!(
            "sgnl://addstickers/?pack_id={}&pack_key={}",
            self.pack_id,
            self.key.to_hex()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "c8c83285b547872ac4c589d64a6edd6a";
    const KEY: &str = "59bb3a8860f0e6a5a83a5337a015c8d55ecd2193f82d77202f3b8112a845636e";

    #[test]
    fn both_link_forms() {
        for link in [
            format!("https://signal.art/addstickers/#pack_id={ID}&pack_key={KEY}"),
            format!("https://signal.art/addstickers#pack_id={ID}&pack_key={KEY}"),
            format!("  https://signal.art/addstickers/#pack_key={KEY}&pack_id={ID}\n"),
            format!("sgnl://addstickers/?pack_id={ID}&pack_key={KEY}"),
            format!("sgnl://addstickers?pack_id={ID}&pack_key={KEY}"),
            format!(
                "https://SIGNAL.ART/addstickers/#pack_id={}&pack_key={}",
                ID.to_uppercase(),
                KEY.to_uppercase()
            ),
        ] {
            let parsed = PackLink::parse(&link).unwrap_or_else(|e| panic!("{link}: {e}"));
            assert_eq!(parsed.pack_id, ID, "{link}");
            assert_eq!(hex::encode(parsed.key.as_bytes()), KEY, "{link}");
        }
        let link = PackLink::parse(&format!("sgnl://addstickers/?pack_id={ID}&pack_key={KEY}")).unwrap();
        assert_eq!(
            link.url(),
            format!("https://signal.art/addstickers/#pack_id={ID}&pack_key={KEY}")
        );
        assert_eq!(
            link.app_url(),
            format!("sgnl://addstickers/?pack_id={ID}&pack_key={KEY}")
        );
        assert_eq!(PackLink::parse(&link.url()).unwrap(), link);
    }

    #[test]
    fn bad_links() {
        for link in [
            String::new(),
            "signal.art/addstickers/".to_string(),
            "https://signal.art/addstickers/".to_string(),
            "https://signal.art/addstickers/#".to_string(),
            format!("https://signal.art/addstickers/#pack_id={ID}"),
            format!("https://signal.art/addstickers/#pack_id={ID}&pack_key="),
            format!("https://signal.art/addstickers/#pack_key={KEY}"),
            format!("https://signal.art/addstickers/#pack_id=garbage&pack_key={KEY}"),
            format!("https://signal.art/addstickers/#pack_id[]={ID}&pack_key={KEY}"),
            format!("https://signal.art/addstickers/#pack_id={ID}&pack_key[]={KEY}"),
            format!("https://signal.art/addstickers/#pack_id={ID}&pack_key={}", &KEY[..62]),
            format!("https://signal.art/addstickers/#pack_id={ID}&pack_key={}zz", &KEY[..62]),
            format!("https://signal.art/addstickers/#pack_id={}&pack_key={KEY}", &ID[..30]),
            format!("https://signal.art/addstickers/?pack_id={ID}&pack_key={KEY}"),
            format!("https://evil.example/addstickers/#pack_id={ID}&pack_key={KEY}"),
            format!("http://signal.art/addstickers/#pack_id={ID}&pack_key={KEY}"),
            format!("https://signal.art/other/#pack_id={ID}&pack_key={KEY}"),
            format!("sgnl://addstickers/#pack_id={ID}&pack_key={KEY}"),
            format!("sgnl://signal.group/?pack_id={ID}&pack_key={KEY}"),
        ] {
            let err = PackLink::parse(&link).expect_err(&link);
            assert!(matches!(err, SignalError::InvalidLink(_)), "{link}: {err}");
            let text = format!("{err} {err:?}");
            assert!(!text.contains(&KEY[..20]), "the key leaked: {text}");
        }
    }

    #[test]
    fn keys_are_not_printed() {
        let link = PackLink::parse(&format!("https://signal.art/addstickers/#pack_id={ID}&pack_key={KEY}")).unwrap();
        let text = format!("{link:?} {:?}", link.key);
        assert!(text.contains(ID) && !text.contains(&KEY[..16]), "{text}");
        assert_ne!(PackKey::random(), PackKey::random());
        assert_eq!(redact_pack_id(ID), "[REDACTED]d6a");
    }
}
