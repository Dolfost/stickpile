//! Clipboard offers: which MIME types carry which bytes for an item (driven by
//! `ClientConfig::clipboard`, provisional until docs/clipboard.md is filled),
//! and CLI clipboard backends. The GTK client sets offers through GDK itself.
//!
//! [`build_offers`] turns the configured MIME list into concrete payloads:
//!
//! | MIME | payload |
//! |---|---|
//! | `image/png`, `image/gif`, `image/webp`, `image/jpeg` | the file bytes when the file already has that type, else a conversion with the `image` crate (WebP still → PNG, ...). Animated sources only convert to `image/gif`; other conversions of animations are skipped (debug log). Videos never convert. |
//! | `text/uri-list` | `file:///…\r\n` (percent-encoded) |
//! | `x-special/gnome-copied-files` | `copy\nfile:///…` |
//! | `text/plain` | the absolute path |
//!
//! [`serve`] (features `wayland` / `x11`) keeps offers on the clipboard from a
//! detached helper process after the CLI exits.

use std::io::Cursor;
use std::path::Path;

use image::{AnimationDecoder, ImageFormat};

use crate::{CoreError, Result};

pub mod serve;
#[cfg(feature = "wayland")]
mod wayland;
#[cfg(feature = "x11")]
pub mod x11;

/// One MIME representation offered on the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipOffer {
    pub mime: String,
    pub data: Vec<u8>,
}

/// Build the offers for `file` (of type `file_mime`) in `offers_cfg` order.
/// Unknown MIME types and impossible conversions are skipped (logged), so the
/// result may be shorter than the configuration.
pub fn build_offers(offers_cfg: &[String], file: &Path, file_mime: &str) -> Result<Vec<ClipOffer>> {
    let abs = absolute(file)?;
    let mut bytes: Option<Vec<u8>> = None;
    let mut out: Vec<ClipOffer> = Vec::with_capacity(offers_cfg.len());
    for mime in offers_cfg {
        if out.iter().any(|o| &o.mime == mime) {
            continue;
        }
        let data = match mime.as_str() {
            "text/uri-list" => uri_list(&abs).into_bytes(),
            "x-special/gnome-copied-files" => gnome_copied_files(&abs).into_bytes(),
            "text/plain" => abs.to_string_lossy().into_owned().into_bytes(),
            "image/png" | "image/gif" | "image/webp" | "image/jpeg" => {
                if bytes.is_none() {
                    bytes = Some(std::fs::read(&abs).map_err(|e| CoreError::Other(format!("{}: {e}", abs.display())))?);
                }
                let src = bytes.as_deref().unwrap_or_default();
                if normalise_mime(file_mime) == mime.as_str() {
                    src.to_vec()
                } else {
                    match convert_image(src, file_mime, mime) {
                        Ok(Some(data)) => data,
                        Ok(None) => continue,
                        Err(e) => {
                            tracing::debug!("skipping clipboard offer {mime} for {}: {e}", abs.display());
                            continue;
                        }
                    }
                }
            }
            other => {
                tracing::debug!("skipping unsupported clipboard offer {other}");
                continue;
            }
        };
        out.push(ClipOffer {
            mime: mime.clone(),
            data,
        });
    }
    Ok(out)
}

fn absolute(p: &Path) -> Result<std::path::PathBuf> {
    if p.is_absolute() {
        Ok(p.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(p))
    }
}

fn normalise_mime(m: &str) -> &str {
    match m {
        "image/jpg" | "image/pjpeg" => "image/jpeg",
        other => other,
    }
}

fn image_format(mime: &str) -> Option<ImageFormat> {
    Some(match normalise_mime(mime) {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/webp" => ImageFormat::WebP,
        "image/gif" => ImageFormat::Gif,
        _ => return None,
    })
}

/// True if the image has more than one frame (animated GIF or WebP).
pub fn is_animated(bytes: &[u8], format: ImageFormat) -> bool {
    match format {
        ImageFormat::Gif => image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
            .map(|d| d.into_frames().take(2).count() > 1)
            .unwrap_or(false),
        ImageFormat::WebP => image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
            .map(|d| d.has_animation())
            .unwrap_or(false),
        _ => false,
    }
}

/// Convert `src` (of `from` MIME) into `to`. `Ok(None)` when the conversion is
/// not possible by design (animation into a still format, video).
fn convert_image(src: &[u8], from: &str, to: &str) -> Result<Option<Vec<u8>>, String> {
    let Some(from_fmt) = image_format(from) else {
        tracing::debug!("cannot convert {from} into {to}");
        return Ok(None);
    };
    let to_fmt = image_format(to).ok_or_else(|| format!("unsupported target {to}"))?;
    if is_animated(src, from_fmt) {
        if to_fmt == ImageFormat::Gif {
            return animation_to_gif(src, from_fmt).map(Some);
        }
        tracing::debug!("animated {from} cannot become {to} data; skipping offer");
        return Ok(None);
    }
    let img = image::load_from_memory_with_format(src, from_fmt).map_err(|e| e.to_string())?;
    let mut out = Cursor::new(Vec::new());
    match to_fmt {
        ImageFormat::Jpeg => {
            // JPEG has no alpha channel: flatten onto white.
            let rgba = img.to_rgba8();
            let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
            for (dst, p) in rgb.pixels_mut().zip(rgba.pixels()) {
                let a = p[3] as u32;
                for c in 0..3 {
                    dst[c] = ((p[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
                }
            }
            rgb.write_to(&mut out, ImageFormat::Jpeg).map_err(|e| e.to_string())?;
        }
        ImageFormat::WebP => {
            // The image crate encodes lossless WebP from RGB(A)8 only.
            img.to_rgba8()
                .write_to(&mut out, ImageFormat::WebP)
                .map_err(|e| e.to_string())?;
        }
        ImageFormat::Gif => {
            img.to_rgba8()
                .write_to(&mut out, ImageFormat::Gif)
                .map_err(|e| e.to_string())?;
        }
        _ => img.write_to(&mut out, to_fmt).map_err(|e| e.to_string())?,
    }
    Ok(Some(out.into_inner()))
}

fn animation_to_gif(src: &[u8], from: ImageFormat) -> Result<Vec<u8>, String> {
    let frames = match from {
        ImageFormat::WebP => image::codecs::webp::WebPDecoder::new(Cursor::new(src))
            .map_err(|e| e.to_string())?
            .into_frames()
            .collect_frames()
            .map_err(|e| e.to_string())?,
        ImageFormat::Gif => return Ok(src.to_vec()),
        other => return Err(format!("{other:?} animations are not supported")),
    };
    let mut out = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
        enc.set_repeat(image::codecs::gif::Repeat::Infinite)
            .map_err(|e| e.to_string())?;
        enc.encode_frames(frames).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// `file:///…` with every byte outside RFC 3986 unreserved characters and `/`
/// percent-encoded (spaces, `#`, `%`, non-ASCII, ...).
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// `text/uri-list` payload: one URI, CRLF-terminated (RFC 2483).
pub fn uri_list(path: &Path) -> String {
    format!("{}\r\n", file_uri(path))
}

/// `x-special/gnome-copied-files` payload (Nautilus/GTK file managers).
pub fn gnome_copied_files(path: &Path) -> String {
    format!("copy\n{}", file_uri(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn s(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    pub(crate) fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([x as u8, y as u8, 128, 200]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    pub(crate) fn webp_still(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([x as u8, y as u8, 10, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::WebP).unwrap();
        out.into_inner()
    }

    pub(crate) fn animated_gif(frames: usize) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
            enc.set_repeat(image::codecs::gif::Repeat::Infinite).unwrap();
            for i in 0..frames {
                let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([(i * 60) as u8, 0, 0, 255]));
                enc.encode_frame(image::Frame::new(img)).unwrap();
            }
        }
        out
    }

    #[test]
    fn uri_encoding_of_spaces_and_unicode() {
        let p = PathBuf::from("/tmp/my stickers/котик 😼#1%.gif");
        let uri = file_uri(&p);
        assert_eq!(
            uri,
            "file:///tmp/my%20stickers/%D0%BA%D0%BE%D1%82%D0%B8%D0%BA%20%F0%9F%98%BC%231%25.gif"
        );
        assert_eq!(uri_list(&p), format!("{uri}\r\n"));
        assert_eq!(gnome_copied_files(&p), format!("copy\n{uri}"));
        // round trip through the url crate
        let parsed = url::Url::parse(&uri).unwrap();
        assert_eq!(parsed.to_file_path().unwrap(), p);
    }

    #[test]
    fn webp_still_becomes_png_data() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("cat.webp");
        std::fs::write(&f, webp_still(16, 12)).unwrap();
        let offers = build_offers(&s(&["image/png", "text/uri-list", "image/webp"]), &f, "image/webp").unwrap();
        assert_eq!(
            offers.iter().map(|o| o.mime.as_str()).collect::<Vec<_>>(),
            ["image/png", "text/uri-list", "image/webp"]
        );
        let png = image::load_from_memory_with_format(&offers[0].data, ImageFormat::Png).unwrap();
        assert_eq!((png.width(), png.height()), (16, 12));
        assert_eq!(offers[2].data, std::fs::read(&f).unwrap());
    }

    #[test]
    fn gif_bytes_are_passed_through_and_not_converted_to_png() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("dance.gif");
        let gif = animated_gif(3);
        std::fs::write(&f, &gif).unwrap();
        let offers = build_offers(
            &s(&[
                "image/gif",
                "image/png",
                "text/uri-list",
                "x-special/gnome-copied-files",
            ]),
            &f,
            "image/gif",
        )
        .unwrap();
        let mimes: Vec<_> = offers.iter().map(|o| o.mime.as_str()).collect();
        assert_eq!(mimes, ["image/gif", "text/uri-list", "x-special/gnome-copied-files"]);
        assert_eq!(offers[0].data, gif);
        let uri = file_uri(&f);
        assert_eq!(offers[1].data, format!("{uri}\r\n").into_bytes());
        assert_eq!(offers[2].data, format!("copy\n{uri}").into_bytes());
    }

    #[test]
    fn single_frame_gif_converts_to_png_and_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("still.gif");
        std::fs::write(&f, animated_gif(1)).unwrap();
        let offers = build_offers(&s(&["image/png", "image/jpeg"]), &f, "image/gif").unwrap();
        assert_eq!(offers.len(), 2);
        image::load_from_memory_with_format(&offers[0].data, ImageFormat::Png).unwrap();
        image::load_from_memory_with_format(&offers[1].data, ImageFormat::Jpeg).unwrap();
    }

    #[test]
    fn png_converts_to_gif_and_webp() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.png");
        std::fs::write(&f, png_bytes(10, 10)).unwrap();
        let offers = build_offers(&s(&["image/gif", "image/webp", "image/png"]), &f, "image/png").unwrap();
        assert_eq!(offers.len(), 3);
        image::load_from_memory_with_format(&offers[0].data, ImageFormat::Gif).unwrap();
        image::load_from_memory_with_format(&offers[1].data, ImageFormat::WebP).unwrap();
        assert_eq!(offers[2].data, std::fs::read(&f).unwrap());
    }

    #[test]
    fn video_offers_only_file_references_and_path() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("clip one.mp4");
        std::fs::write(&f, b"\0\0\0\x18ftypmp42not really").unwrap();
        let offers = build_offers(
            &s(&[
                "image/gif",
                "image/png",
                "text/uri-list",
                "x-special/gnome-copied-files",
                "text/plain",
                "text/plain",
            ]),
            &f,
            "video/mp4",
        )
        .unwrap();
        let mimes: Vec<_> = offers.iter().map(|o| o.mime.as_str()).collect();
        assert_eq!(mimes, ["text/uri-list", "x-special/gnome-copied-files", "text/plain"]);
        assert_eq!(offers[2].data, f.to_string_lossy().as_bytes());
        assert!(String::from_utf8(offers[0].data.clone())
            .unwrap()
            .contains("clip%20one.mp4"));
    }

    #[test]
    fn detects_animation() {
        assert!(is_animated(&animated_gif(2), ImageFormat::Gif));
        assert!(!is_animated(&animated_gif(1), ImageFormat::Gif));
        assert!(!is_animated(&webp_still(4, 4), ImageFormat::WebP));
    }
}
