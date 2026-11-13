//! Copy-time image adjustments (`[copy]` in the config).
//!
//! * `copy.square` pads non-square stills and static sticker renditions to a
//!   centred square of their longest side, with transparent or white margins.
//!   Nothing is scaled or cropped.
//! * `copy.background = "white"` flattens the transparent pixels of stills
//!   copied as images onto white. Stickers keep their alpha.
//!
//! [`crate::Session::prepare_copy`] runs them on the materialised variant
//! file, so the image data on the clipboard and the file that file references
//! point at carry the same pixels. An adjusted still becomes a PNG; an
//! adjusted sticker stays a WebP with its longest side unchanged (512),
//! encoded like the server's sticker rendition, so it is still a valid
//! Telegram sticker. Animations and videos are never touched.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{ImageEncoder, Rgba, RgbaImage};
use stickpile_api::MediaKind;

use crate::cache::Cache;
use crate::config::{Background, CopyConfig, SquarePad};
use crate::{CoreError, Result};

pub const TRANSPARENT: Rgba<u8> = Rgba([0, 0, 0, 0]);
pub const WHITE: Rgba<u8> = Rgba([255, 255, 255, 255]);

/// An adjusted image file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adjusted {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

/// `img` centred on a square canvas of its longest side filled with `fill`.
/// Only the added margins get `fill`; the image's own pixels (and alpha) are
/// copied as they are.
pub fn pad_square(img: &RgbaImage, fill: Rgba<u8>) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w == h {
        return img.clone();
    }
    let side = w.max(h);
    let mut out = RgbaImage::from_pixel(side, side, fill);
    image::imageops::replace(&mut out, img, i64::from((side - w) / 2), i64::from((side - h) / 2));
    out
}

/// Composite onto opaque white.
pub fn flatten_onto_white(img: &mut RgbaImage) {
    for p in img.pixels_mut() {
        let a = u32::from(p[3]);
        for c in 0..3 {
            p[c] = ((u32::from(p[c]) * a + 255 * (255 - a)) / 255) as u8;
        }
        p[3] = 255;
    }
}

/// Name of the option combination that applies to a copy, used in the cache
/// path of the adjusted file. `None` when no option applies.
pub fn adjust_tag(cfg: &CopyConfig, sticker: bool) -> Option<String> {
    let mut parts = Vec::new();
    match cfg.square {
        SquarePad::Off => {}
        SquarePad::Transparent => parts.push("sq-transparent"),
        SquarePad::White => parts.push("sq-white"),
    }
    if !sticker && cfg.background == Background::White {
        parts.push("bg-white");
    }
    (!parts.is_empty()).then(|| parts.join("."))
}

/// Cache path of the adjusted copy of the variant file `rel`
/// (`clip/<media>.<variant>/<name>.<ext>`): the directory gets the tag, so
/// every option combination has its own file, and the file keeps the item
/// name (Telegram and file managers show it).
pub fn adjusted_rel(rel: &str, tag: &str, ext: &str) -> String {
    let (dir, file) = rel.rsplit_once('/').unwrap_or(("", rel));
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    if dir.is_empty() {
        format!("{tag}/{stem}.{ext}")
    } else {
        format!("{dir}.{tag}/{stem}.{ext}")
    }
}

/// Apply `cfg` to a still image or, with `sticker`, a static sticker
/// rendition. `Ok(None)` when nothing would change (already square, nothing
/// transparent to fill, options off) or the image is animated.
pub fn adjust_image(bytes: &[u8], sticker: bool, cfg: &CopyConfig) -> Result<Option<Adjusted>, String> {
    if adjust_tag(cfg, sticker).is_none() {
        return Ok(None);
    }
    let format = image::guess_format(bytes).map_err(|e| e.to_string())?;
    if super::is_animated(bytes, format) {
        return Ok(None);
    }
    let limits = stickpile_media::Limits {
        max_bytes: u64::MAX,
        max_pixels: 200_000_000,
        ..Default::default()
    };
    let mut img = stickpile_media::decode_frame(bytes, &limits).map_err(|e| e.to_string())?;
    let flatten = !sticker && cfg.background == Background::White && img.pixels().any(|p| p[3] < 255);
    let fill = match cfg.square {
        SquarePad::Off => None,
        SquarePad::Transparent => Some(TRANSPARENT),
        SquarePad::White => Some(WHITE),
    };
    let fill = fill.filter(|_| img.width() != img.height());
    if !flatten && fill.is_none() {
        return Ok(None);
    }
    if flatten {
        flatten_onto_white(&mut img);
    }
    if let Some(fill) = fill {
        img = pad_square(&img, fill);
    }
    if sticker {
        let bytes = stickpile_media::encode_tg_sticker(&img)
            .map_err(|e| e.to_string())?
            .ok_or("the padded sticker does not fit Telegram's size limit")?;
        return Ok(Some(Adjusted {
            bytes,
            mime: "image/webp",
        }));
    }
    Ok(Some(Adjusted {
        bytes: encode_png(&img)?,
        mime: "image/png",
    }))
}

fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    let (w, h) = img.dimensions();
    let enc = image::codecs::png::PngEncoder::new(&mut out);
    let r = if img.pixels().all(|p| p[3] == 255) {
        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        enc.write_image(rgb.as_raw(), w, h, image::ExtendedColorType::Rgb8)
    } else {
        enc.write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
    };
    r.map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// The file and MIME type to offer for a materialised copy file (`rel` in the
/// cache, `file` on disk): the adjusted file, written to [`adjusted_rel`] on
/// first use, or `file` itself when no adjustment applies. A failed
/// adjustment is logged and the unadjusted file is copied.
pub(crate) async fn adjust_copy_file(
    cache: &Cache,
    rel: &str,
    file: PathBuf,
    mime: String,
    kind: MediaKind,
    sticker: bool,
    cfg: CopyConfig,
) -> Result<(PathBuf, String)> {
    if kind != MediaKind::Still {
        return Ok((file, mime));
    }
    let Some(tag) = adjust_tag(&cfg, sticker) else {
        return Ok((file, mime));
    };
    let (ext, out_mime) = if sticker {
        ("webp", "image/webp")
    } else {
        ("png", "image/png")
    };
    let out_rel = adjusted_rel(rel, &tag, ext);
    if let Some(p) = cache.lookup_file(&out_rel, None)? {
        // A re-downloaded source (new rendition) is newer than its old adjustment.
        if modified(&p) >= modified(&file) {
            return Ok((p, out_mime.into()));
        }
    }
    let out = cache.abs(&out_rel);
    let (src, dest) = (file.clone(), out.clone());
    let written = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let bytes = std::fs::read(&src).map_err(|e| e.to_string())?;
        match adjust_image(&bytes, sticker, &cfg)? {
            Some(a) => write_file(&dest, &a.bytes).map(|()| true),
            None => Ok(false),
        }
    })
    .await
    .map_err(|e| CoreError::Other(format!("adjusting the copy: {e}")))?;
    match written {
        Ok(true) => {
            cache.register_file(&out_rel)?;
            Ok((out, out_mime.into()))
        }
        Ok(false) => Ok((file, mime)),
        Err(e) => {
            tracing::warn!("copying {} unadjusted: {e}", file.display());
            Ok((file, mime))
        }
    }
}

fn modified(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn write_file(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = dest.parent().ok_or("no parent directory")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dest.with_file_name(format!(".{}.tmp", crate::new_id()));
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, dest))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("{}: {e}", dest.display())
        })
}

#[cfg(test)]
mod tests;
