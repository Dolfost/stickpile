//! What goes on the clipboard: MIME types, their bytes for a given file, and
//! the offer combinations of the docs/clipboard.md matrix.

use std::path::Path;

/// Every MIME type the spike can offer, in the order the checkboxes show.
pub const MIME_TYPES: [&str; 6] = [
    "image/png",
    "image/gif",
    "image/webp",
    "text/uri-list",
    "x-special/gnome-copied-files",
    "text/plain",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Png,
    Jpeg,
    Webp,
    Gif,
    Mp4,
    Webm,
    Other,
}

impl Kind {
    pub fn mime(self) -> Option<&'static str> {
        Some(match self {
            Kind::Png => "image/png",
            Kind::Jpeg => "image/jpeg",
            Kind::Webp => "image/webp",
            Kind::Gif => "image/gif",
            Kind::Mp4 => "video/mp4",
            Kind::Webm => "video/webm",
            Kind::Other => return None,
        })
    }

    pub fn is_video(self) -> bool {
        matches!(self, Kind::Mp4 | Kind::Webm)
    }
}

/// Sniff the file type from its first bytes (never the extension).
pub fn sniff(bytes: &[u8]) -> Kind {
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Kind::Mp4;
    }
    if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        return Kind::Webm;
    }
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => Kind::Png,
        Ok(image::ImageFormat::Jpeg) => Kind::Jpeg,
        Ok(image::ImageFormat::WebP) => Kind::Webp,
        Ok(image::ImageFormat::Gif) => Kind::Gif,
        _ => Kind::Other,
    }
}

pub fn file_uri(path: &Path) -> Option<String> {
    url::Url::from_file_path(path).ok().map(String::from)
}

/// One offered representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub mime: String,
    pub data: Vec<u8>,
}

/// Bytes for each requested MIME type. Image types are offered when the
/// file already is that format; `image/png` is also produced by converting
/// any decodable still or GIF (first frame). Returns the offers and the MIME
/// types that could not be produced.
pub fn build(path: &Path, mimes: &[&str]) -> std::io::Result<(Vec<Offer>, Vec<String>)> {
    let path = std::path::absolute(path)?;
    let bytes = std::fs::read(&path)?;
    let kind = sniff(&bytes);
    let uri = file_uri(&path).unwrap_or_default();
    let mut offers = Vec::new();
    let mut skipped = Vec::new();
    for &mime in mimes {
        let data = match mime {
            m if Some(m) == kind.mime() => Some(bytes.clone()),
            "image/png" if !kind.is_video() => to_png(&bytes),
            "text/uri-list" => Some(format!("{uri}\r\n").into_bytes()),
            "x-special/gnome-copied-files" => Some(format!("copy\n{uri}").into_bytes()),
            "text/plain" | "text/plain;charset=utf-8" => Some(path.display().to_string().into_bytes()),
            _ => None,
        };
        match data {
            Some(d) => offers.push(Offer {
                mime: mime.to_string(),
                data: d,
            }),
            None => skipped.push(mime.to_string()),
        }
    }
    Ok((offers, skipped))
}

fn to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Offer combinations from the matrix in docs/clipboard.md.
pub const PRESETS: [(&str, &[&str]); 9] = [
    ("PNG data", &["image/png"]),
    ("PNG data + uri-list", &["image/png", "text/uri-list"]),
    ("uri-list only", &["text/uri-list"]),
    ("GIF data", &["image/gif"]),
    ("GIF data + uri-list", &["image/gif", "text/uri-list"]),
    ("WebP data + uri-list", &["image/webp", "text/uri-list"]),
    ("gnome-copied-files only", &["x-special/gnome-copied-files"]),
    (
        "uri-list + gnome-copied-files",
        &["text/uri-list", "x-special/gnome-copied-files"],
    ),
    ("path as text/plain", &["text/plain"]),
];

/// The provisional default of the real client (stickpile-core
/// `ClipboardOffers::default`) for a file of this kind.
pub fn provisional_default(kind: Kind, sticker: bool) -> &'static [&'static str] {
    if sticker {
        return &["text/uri-list", "x-special/gnome-copied-files"];
    }
    match kind {
        Kind::Gif => &["image/gif", "text/uri-list", "x-special/gnome-copied-files"],
        Kind::Mp4 | Kind::Webm => &["text/uri-list", "x-special/gnome-copied-files"],
        _ => &["image/png", "text/uri-list", "x-special/gnome-copied-files"],
    }
}

/// `image/png (12 KiB), text/uri-list (54 B)`
pub fn summary(offers: &[Offer]) -> String {
    offers
        .iter()
        .map(|o| {
            let n = o.data.len();
            if n >= 1024 {
                format!("{} ({} KiB)", o.mime, n / 1024)
            } else {
                format!("{} ({n} B)", o.mime)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn sniffing() {
        assert_eq!(sniff(&fixtures::png_bytes()), Kind::Png);
        assert_eq!(sniff(&fixtures::webp_sticker_bytes()), Kind::Webp);
        assert_eq!(sniff(&fixtures::gif_bytes().unwrap()), Kind::Gif);
        assert_eq!(sniff(b"\0\0\0\x18ftypisom\0\0\0\0"), Kind::Mp4);
        assert_eq!(sniff(&[0x1a, 0x45, 0xdf, 0xa3, 0]), Kind::Webm);
        assert_eq!(sniff(b"hello"), Kind::Other);
    }

    #[test]
    fn png_offers() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "a b.png", &fixtures::png_bytes());
        let (offers, skipped) = build(&p, &MIME_TYPES).unwrap();
        let mimes: Vec<_> = offers.iter().map(|o| o.mime.as_str()).collect();
        assert_eq!(
            mimes,
            [
                "image/png",
                "text/uri-list",
                "x-special/gnome-copied-files",
                "text/plain"
            ]
        );
        assert_eq!(skipped, ["image/gif", "image/webp"]);
        assert_eq!(offers[0].data, fixtures::png_bytes(), "PNG passed through untouched");
        let uri = String::from_utf8(offers[1].data.clone()).unwrap();
        assert!(uri.starts_with("file:///") && uri.ends_with("a%20b.png\r\n"), "{uri}");
        assert!(String::from_utf8(offers[2].data.clone())
            .unwrap()
            .starts_with("copy\nfile:///"));
    }

    #[test]
    fn gif_and_webp_also_offer_png_conversion() {
        let dir = tempfile::tempdir().unwrap();
        let g = write(dir.path(), "a.gif", &fixtures::gif_bytes().unwrap());
        let (offers, _) = build(&g, &["image/gif", "image/png"]).unwrap();
        assert!(offers[0].data.starts_with(b"GIF8"));
        assert!(offers[1].data.starts_with(b"\x89PNG"));
        let w = write(dir.path(), "s.webp", &fixtures::webp_sticker_bytes());
        let (offers, skipped) = build(&w, &["image/webp", "image/gif"]).unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(skipped, ["image/gif"]);
    }

    #[test]
    fn video_offers_only_file_references() {
        let dir = tempfile::tempdir().unwrap();
        let v = write(dir.path(), "v.mp4", b"\0\0\0\x18ftypisom\0\0\0\0rest");
        let (offers, skipped) = build(&v, &["image/png", "text/uri-list"]).unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(skipped, ["image/png"]);
    }

    #[test]
    fn presets_use_known_mime_types() {
        for (_, mimes) in PRESETS {
            assert!(mimes.iter().all(|m| MIME_TYPES.contains(m)));
        }
        assert_eq!(provisional_default(Kind::Png, false)[0], "image/png");
        assert_eq!(provisional_default(Kind::Gif, false)[0], "image/gif");
        assert_eq!(provisional_default(Kind::Mp4, false)[0], "text/uri-list");
        assert_eq!(provisional_default(Kind::Webp, true)[0], "text/uri-list");
    }

    #[test]
    fn summary_text() {
        let o = [
            Offer {
                mime: "image/png".into(),
                data: vec![0; 2048],
            },
            Offer {
                mime: "text/uri-list".into(),
                data: vec![0; 9],
            },
        ];
        assert_eq!(summary(&o), "image/png (2 KiB), text/uri-list (9 B)");
    }
}
