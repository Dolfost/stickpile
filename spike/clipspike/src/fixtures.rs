//! Built-in test media, generated on demand so the spike needs no assets.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, ImageFormat, Rgba, RgbaImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Fixture {
    /// 640×480 PNG still with transparency.
    Png,
    /// 512×512 lossless WebP with transparency: a Telegram static sticker.
    Webp,
    /// 240×240 animated GIF, 12 frames, loops.
    Gif,
    /// 2 s 320×240 H.264 MP4 without audio (needs ffmpeg).
    Mp4,
}

impl Fixture {
    pub const ALL: [Fixture; 4] = [Fixture::Png, Fixture::Webp, Fixture::Gif, Fixture::Mp4];

    pub fn file_name(self) -> &'static str {
        match self {
            Fixture::Png => "clipspike-still.png",
            Fixture::Webp => "clipspike-sticker-512.webp",
            Fixture::Gif => "clipspike-animation.gif",
            Fixture::Mp4 => "clipspike-video.mp4",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Fixture::Png => "PNG still (640×480)",
            Fixture::Webp => "WebP sticker (512×512)",
            Fixture::Gif => "Animated GIF (240×240)",
            Fixture::Mp4 => "MP4 video (2 s, needs ffmpeg)",
        }
    }
}

/// `$XDG_CACHE_HOME/clipspike` (falls back to `~/.cache/clipspike`). Not
/// /tmp: some chat apps have a private /tmp (systemd PrivateTmp, Flatpak).
pub fn default_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("clipspike")
}

fn pattern(w: u32, h: u32, phase: u32) -> RgbaImage {
    let (cx, cy, r) = (w as i64 / 2, h as i64 / 2, (w.min(h) / 3) as i64);
    RgbaImage::from_fn(w, h, |x, y| {
        let (dx, dy) = (x as i64 - cx, y as i64 - cy);
        let inside = dx * dx + dy * dy <= r * r;
        let shift = (phase * 20) as i64;
        if inside {
            Rgba([255, ((x as i64 + shift) % 256) as u8, 64, 255])
        } else if (x / 32 + y / 32) % 2 == 0 {
            Rgba([40, 120, ((y as i64 + shift) % 256) as u8, 255])
        } else {
            Rgba([0, 0, 0, 0])
        }
    })
}

pub fn png_bytes() -> Vec<u8> {
    encode(&pattern(640, 480, 0), ImageFormat::Png)
}

pub fn webp_sticker_bytes() -> Vec<u8> {
    encode(&pattern(512, 512, 3), ImageFormat::WebP)
}

pub fn gif_bytes() -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new_with_speed(&mut out, 10);
        enc.set_repeat(Repeat::Infinite)?;
        for phase in 0..12 {
            let frame = Frame::from_parts(pattern(240, 240, phase), 0, 0, Delay::from_numer_denom_ms(80, 1));
            enc.encode_frame(frame)?;
        }
    }
    Ok(out)
}

fn encode(img: &RgbaImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, format).expect("encoding an in-memory image");
    out.into_inner()
}

/// Fixed ffmpeg arguments for the MP4 fixture (no user input involved).
pub fn ffmpeg_args(out: &Path, codec: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x240:rate=15:duration=2",
        "-an",
        "-pix_fmt",
        "yuv420p",
        "-c:v",
        codec,
        "-movflags",
        "+faststart",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(out.display().to_string());
    args
}

fn mp4(out: &Path) -> Result<()> {
    let mut last = String::new();
    for codec in ["libx264", "mpeg4"] {
        match Command::new("ffmpeg").args(ffmpeg_args(out, codec)).output() {
            Ok(o) if o.status.success() => return Ok(()),
            Ok(o) => last = String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => bail!("ffmpeg not found ({e}); pass your own MP4 file instead"),
        }
    }
    bail!("ffmpeg failed: {last}")
}

/// Write the fixture into `dir` (created if needed) and return its path.
pub fn generate(fixture: Fixture, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(fixture.file_name());
    match fixture {
        Fixture::Png => std::fs::write(&path, png_bytes())?,
        Fixture::Webp => std::fs::write(&path, webp_sticker_bytes())?,
        Fixture::Gif => std::fs::write(&path, gif_bytes()?)?,
        Fixture::Mp4 => mp4(&path)?,
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::AnimationDecoder;

    #[test]
    fn png_fixture_decodes() {
        let img = image::load_from_memory(&png_bytes()).unwrap();
        assert_eq!((img.width(), img.height()), (640, 480));
        assert!(img.color().has_alpha());
    }

    #[test]
    fn webp_fixture_is_a_512_sticker() {
        let bytes = webp_sticker_bytes();
        assert_eq!(image::guess_format(&bytes).unwrap(), ImageFormat::WebP);
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (512, 512));
        assert!(bytes.len() <= 512 * 1024, "Telegram static sticker limit");
    }

    #[test]
    fn gif_fixture_is_animated() {
        let bytes = gif_bytes().unwrap();
        let frames = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
            .unwrap()
            .into_frames();
        assert_eq!(frames.count(), 12);
    }

    #[test]
    fn ffmpeg_arguments_are_fixed() {
        let args = ffmpeg_args(Path::new("/x/out.mp4"), "libx264");
        assert_eq!(args.last().unwrap(), "/x/out.mp4");
        assert!(args.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert!(args.contains(&"-an".to_string()));
    }

    #[test]
    fn generate_writes_files() {
        let dir = tempfile::tempdir().unwrap();
        for f in [Fixture::Png, Fixture::Webp, Fixture::Gif] {
            let p = generate(f, dir.path()).unwrap();
            assert_eq!(p.file_name().unwrap(), f.file_name());
            assert!(std::fs::metadata(&p).unwrap().len() > 100);
        }
    }
}
