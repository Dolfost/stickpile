//! Clipboard and drag payloads (display-free).
//!
//! [`provider_specs`] turns `PreparedCopy::offers` into the list of
//! `(mime, bytes)` pairs the GTK layer wraps in
//! `gdk::ContentProvider::for_bytes` and unions. The file-reference helpers
//! build the drag-and-drop payloads for the materialised file.

use std::path::Path;

use stickpile_core::clipboard::ClipOffer;

pub const URI_LIST: &str = "text/uri-list";
pub const GNOME_COPIED_FILES: &str = "x-special/gnome-copied-files";
pub const TEXT_PLAIN: &str = "text/plain";
pub const TEXT_PLAIN_UTF8: &str = "text/plain;charset=utf-8";

/// MIME types the lazy drag provider offers. Plain text is left out on
/// purpose: chat inputs would insert the path instead of attaching the file.
pub const DRAG_MIME_TYPES: [&str; 2] = [URI_LIST, GNOME_COPIED_FILES];

/// One `gdk::ContentProvider::for_bytes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferSpec {
    pub mime: String,
    pub data: Vec<u8>,
}

/// Offers in order, without duplicates or empty payloads. `text/plain` is
/// also offered as `text/plain;charset=utf-8`, which is what GTK and Qt
/// clients ask for on Wayland (GDK maps it to `UTF8_STRING` on X11).
pub fn provider_specs(offers: &[ClipOffer]) -> Vec<OfferSpec> {
    let mut out: Vec<OfferSpec> = Vec::with_capacity(offers.len() + 1);
    let mut push = |mime: &str, data: &[u8]| {
        if !data.is_empty() && !out.iter().any(|o| o.mime.eq_ignore_ascii_case(mime)) {
            out.push(OfferSpec {
                mime: mime.to_string(),
                data: data.to_vec(),
            });
        }
    };
    for o in offers {
        let mime = o.mime.trim();
        push(mime, &o.data);
        if mime.eq_ignore_ascii_case(TEXT_PLAIN) {
            push(TEXT_PLAIN_UTF8, &o.data);
        }
    }
    out
}

/// `file:///…` with percent-encoding. `None` for relative paths.
pub fn file_uri(path: &Path) -> Option<String> {
    url::Url::from_file_path(path).ok().map(String::from)
}

/// `text/uri-list` body (RFC 2483: CRLF-terminated lines).
pub fn uri_list(path: &Path) -> Option<String> {
    Some(format!("{}\r\n", file_uri(path)?))
}

/// `x-special/gnome-copied-files` body (Nautilus format: operation, then URIs).
pub fn gnome_copied_files(path: &Path) -> Option<String> {
    Some(format!("copy\n{}", file_uri(path)?))
}

/// Payload of a file-reference MIME type for `path`, if it is one.
pub fn file_ref_payload(mime: &str, path: &Path) -> Option<Vec<u8>> {
    let mime = mime.trim().to_ascii_lowercase();
    let text = match mime.as_str() {
        URI_LIST => uri_list(path)?,
        GNOME_COPIED_FILES => gnome_copied_files(path)?,
        TEXT_PLAIN | TEXT_PLAIN_UTF8 => path.to_str()?.to_string(),
        _ => return None,
    };
    Some(text.into_bytes())
}

/// Short human summary for logs: `image/png (12 KiB), text/uri-list (54 B)`.
pub fn summary(specs: &[OfferSpec]) -> String {
    specs
        .iter()
        .map(|s| {
            let n = s.data.len();
            if n >= 1024 {
                format!("{} ({} KiB)", s.mime, n / 1024)
            } else {
                format!("{} ({n} B)", s.mime)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn offer(mime: &str, data: &[u8]) -> ClipOffer {
        ClipOffer {
            mime: mime.into(),
            data: data.to_vec(),
        }
    }

    #[test]
    fn specs_keep_order_and_drop_duplicates_and_empties() {
        let specs = provider_specs(&[
            offer("image/png", b"PNG"),
            offer("text/uri-list", b"file:///a.png\r\n"),
            offer("image/png", b"OTHER"),
            offer("image/gif", b""),
        ]);
        let mimes: Vec<_> = specs.iter().map(|s| s.mime.as_str()).collect();
        assert_eq!(mimes, ["image/png", "text/uri-list"]);
        assert_eq!(specs[0].data, b"PNG");
    }

    #[test]
    fn text_plain_gets_utf8_alias() {
        let specs = provider_specs(&[offer("text/plain", b"/tmp/a.png")]);
        let mimes: Vec<_> = specs.iter().map(|s| s.mime.as_str()).collect();
        assert_eq!(mimes, ["text/plain", "text/plain;charset=utf-8"]);
        assert_eq!(specs[1].data, b"/tmp/a.png");
        // An explicit utf-8 offer is not duplicated.
        let specs = provider_specs(&[offer("text/plain;charset=utf-8", b"x"), offer("text/plain", b"x")]);
        assert_eq!(specs.len(), 2);
    }

    #[test]
    fn empty_offers() {
        assert!(provider_specs(&[]).is_empty());
    }

    #[test]
    fn file_references_are_percent_encoded() {
        let p = PathBuf::from("/home/me/.cache/stickpile/files/naahh cat #1.gif");
        assert_eq!(
            file_uri(&p).unwrap(),
            "file:///home/me/.cache/stickpile/files/naahh%20cat%20%231.gif"
        );
        assert_eq!(
            uri_list(&p).unwrap(),
            "file:///home/me/.cache/stickpile/files/naahh%20cat%20%231.gif\r\n"
        );
        assert_eq!(
            gnome_copied_files(&p).unwrap(),
            "copy\nfile:///home/me/.cache/stickpile/files/naahh%20cat%20%231.gif"
        );
        assert!(file_uri(Path::new("relative.png")).is_none());
    }

    #[test]
    fn unicode_file_names() {
        let p = PathBuf::from("/c/кіт.png");
        assert_eq!(file_uri(&p).unwrap(), "file:///c/%D0%BA%D1%96%D1%82.png");
    }

    #[test]
    fn drag_payloads() {
        let p = PathBuf::from("/c/a.webp");
        assert_eq!(file_ref_payload("text/uri-list", &p).unwrap(), b"file:///c/a.webp\r\n");
        assert_eq!(
            file_ref_payload("x-special/gnome-copied-files", &p).unwrap(),
            b"copy\nfile:///c/a.webp"
        );
        assert_eq!(file_ref_payload("text/plain;charset=utf-8", &p).unwrap(), b"/c/a.webp");
        assert_eq!(file_ref_payload("image/png", &p), None);
        for m in DRAG_MIME_TYPES {
            assert!(file_ref_payload(m, &p).is_some());
        }
    }

    #[test]
    fn summary_text() {
        let specs = vec![
            OfferSpec {
                mime: "image/png".into(),
                data: vec![0; 3000],
            },
            OfferSpec {
                mime: "text/uri-list".into(),
                data: vec![0; 20],
            },
        ];
        assert_eq!(summary(&specs), "image/png (2 KiB), text/uri-list (20 B)");
    }
}
