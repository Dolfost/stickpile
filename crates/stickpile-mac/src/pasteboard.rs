//! The clipboard offers of a copy (MIME types, `[clipboard]` in the config)
//! as macOS pasteboard types on one `NSPasteboardItem`.
//!
//! | MIME | pasteboard type |
//! |---|---|
//! | `image/png`, `image/gif`, `image/webp`, `image/jpeg` | `public.png`, `com.compuserve.gif`, `org.webmproject.webp`, `public.jpeg` |
//! | `text/uri-list` | `public.file-url` (the first URL, as Finder writes a copied file) |
//! | `text/plain` | `public.utf8-plain-text` |
//! | `x-special/gnome-copied-files` | left out (GNOME file managers only) |
//! | anything else (the paste hints) | Qt's encoding of a MIME type, `com.trolltech.anymime.` with `/` as `--`, so Qt apps (Telegram Desktop, AyuGram) read it back as the same MIME type |

use stickpile_core::clipboard::ClipOffer;
use stickpile_desktop::offers;

use crate::bridge::PasteboardData;

/// The pasteboard type for a MIME type; `None`: not offered on macOS.
pub fn pasteboard_type(mime: &str) -> Option<String> {
    let mime = mime.trim().to_ascii_lowercase();
    Some(
        match mime.as_str() {
            "image/png" => "public.png",
            "image/gif" => "com.compuserve.gif",
            "image/webp" => "org.webmproject.webp",
            "image/jpeg" => "public.jpeg",
            "text/uri-list" => "public.file-url",
            "text/plain" => "public.utf8-plain-text",
            // `text/plain` above already; the GNOME list is for file managers.
            "text/plain;charset=utf-8" | "x-special/gnome-copied-files" => return None,
            other => return Some(format!("com.trolltech.anymime.{}", other.replace('/', "--"))),
        }
        .to_string(),
    )
}

/// What to write for a copy, in offer order, without duplicates.
pub fn items(offers_in: &[ClipOffer]) -> Vec<PasteboardData> {
    let mut out: Vec<PasteboardData> = Vec::new();
    for spec in offers::provider_specs(offers_in) {
        let Some(kind) = pasteboard_type(&spec.mime) else {
            continue;
        };
        let data = if kind == "public.file-url" {
            // A URI list: the first entry, without the CRLF.
            let text = String::from_utf8_lossy(&spec.data);
            match text
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with('#'))
            {
                Some(url) => url.as_bytes().to_vec(),
                None => continue,
            }
        } else {
            spec.data
        };
        if !out.iter().any(|o| o.kind == kind) {
            out.push(PasteboardData { kind, data });
        }
    }
    out
}

/// One line for the log: the types and their sizes.
pub fn summary(items: &[PasteboardData]) -> String {
    items
        .iter()
        .map(|i| format!("{} ({} B)", i.kind, i.data.len()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(mime: &str, data: &[u8]) -> ClipOffer {
        ClipOffer {
            mime: mime.into(),
            data: data.to_vec(),
        }
    }

    #[test]
    fn maps_the_default_offers() {
        let got = items(&[
            offer("image/png", b"PNG"),
            offer(
                "text/uri-list",
                b"file:///Users/me/Library/Caches/stickpile/cat%20face.png\r\n",
            ),
            offer("x-special/gnome-copied-files", b"copy\nfile:///x.png"),
            offer("application/x-sticker", b"1"),
            offer("text/plain", b"/x.png"),
        ]);
        let kinds: Vec<&str> = got.iter().map(|i| i.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "public.png",
                "public.file-url",
                "com.trolltech.anymime.application--x-sticker",
                "public.utf8-plain-text"
            ]
        );
        assert_eq!(
            got[1].data,
            b"file:///Users/me/Library/Caches/stickpile/cat%20face.png".to_vec()
        );
    }

    #[test]
    fn skips_empty_and_duplicate_offers() {
        let got = items(&[offer("image/gif", b""), offer("text/uri-list", b"\r\n")]);
        assert!(got.is_empty());
        assert_eq!(pasteboard_type("IMAGE/WEBP").as_deref(), Some("org.webmproject.webp"));
    }
}
