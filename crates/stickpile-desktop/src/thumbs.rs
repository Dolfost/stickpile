//! Thumbnail decoding (display-free). Thumbnails are decoded in Rust on the
//! tokio side instead of through gdk-pixbuf loaders, because the WebP loader
//! is often missing (and is not part of GTK). The GTK layer wraps the pixels
//! in a `gdk::MemoryTexture` (R8G8B8A8).

use std::path::Path;

/// Straight-alpha RGBA8 pixels, rows packed (`stride == width * 4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Rgba {
    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }
}

/// Decode the first frame of an image file and shrink it so its longest side
/// is at most `max_px` (aspect ratio kept, never enlarged).
pub fn decode_file(path: &Path, max_px: u32) -> Result<Rgba, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode_bytes(&bytes, max_px)
}

pub fn decode_bytes(bytes: &[u8], max_px: u32) -> Result<Rgba, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let max_px = max_px.max(1);
    let img = if img.width() > max_px || img.height() > max_px {
        img.thumbnail(max_px, max_px)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Ok(Rgba {
        width: rgba.width(),
        height: rgba.height(),
        pixels: rgba.into_raw(),
    })
}

/// Pixel size to decode for a cell of `thumb_px` logical pixels (2x for HiDPI).
pub fn decode_px(thumb_px: u32) -> u32 {
    thumb_px.saturating_mul(2).clamp(64, 1024)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image::{ImageFormat, RgbaImage};
    use std::io::Cursor;

    pub(crate) fn encoded(w: u32, h: u32, format: ImageFormat) -> Vec<u8> {
        let img = RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 200]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    #[test]
    fn large_png_is_shrunk_keeping_aspect() {
        let t = decode_bytes(&encoded(600, 300, ImageFormat::Png), 256).unwrap();
        assert_eq!((t.width, t.height), (256, 128));
        assert_eq!(t.pixels.len(), t.stride() * t.height as usize);
    }

    #[test]
    fn small_image_is_not_enlarged() {
        let t = decode_bytes(&encoded(40, 20, ImageFormat::Png), 256).unwrap();
        assert_eq!((t.width, t.height), (40, 20));
        assert_eq!(&t.pixels[..4], &[0, 0, 128, 200]);
    }

    #[test]
    fn webp_and_gif_decode() {
        let t = decode_bytes(&encoded(512, 512, ImageFormat::WebP), 128).unwrap();
        assert_eq!((t.width, t.height), (128, 128));
        let t = decode_bytes(&encoded(64, 32, ImageFormat::Gif), 128).unwrap();
        assert_eq!((t.width, t.height), (64, 32));
    }

    #[test]
    fn garbage_and_missing_files_fail() {
        assert!(decode_bytes(b"not an image", 128).is_err());
        assert!(decode_file(Path::new("/nonexistent/stickpile.png"), 128).is_err());
    }

    #[test]
    fn decode_file_reads_disk() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.png");
        std::fs::write(&p, encoded(10, 10, ImageFormat::Png)).unwrap();
        assert_eq!(decode_file(&p, 128).unwrap().width, 10);
    }

    #[test]
    fn decode_px_for_hidpi() {
        assert_eq!(decode_px(128), 256);
        assert_eq!(decode_px(16), 64);
        assert_eq!(decode_px(4000), 1024);
    }
}
