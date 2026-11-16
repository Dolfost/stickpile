//! WhatsApp sticker renditions (docs/whatsapp.md): stickers are WebP of
//! exactly 512x512, static ones at most 100 KB, animated ones at most
//! 500 KB, at most 10 s long and with frames of at least 8 ms; the pack's
//! tray icon is a 96x96 PNG of at most 50 KB. The limits are those that
//! WhatsApp's sample sticker app checks (`StickerPackValidator.java` in
//! github.com/WhatsApp/stickers).
//!
//! An item that is not square is scaled so its longer side is 512 and
//! centred on a transparent square. Static stickers are lossless when that
//! fits and re-encoded lossy (highest quality that fits) otherwise.
//! Animated stickers are encoded with libwebp (animations are decoded in
//! Rust, videos by ffmpeg), trying lower frame rates and qualities until one
//! fits.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use image::imageops::FilterType;
use image::RgbaImage;
use stickpile_api::{MediaFormat, MediaKind};

use crate::anim_webp::AnimWriter;
use crate::tools::{self, file_arg};
use crate::variants::{fit_exact, squeeze_webp, trusted_limits, webp_lossless};
use crate::video::{input_args, needs_libvpx, representative_frame};
use crate::{raster, MediaError, Probe, Tools};

/// Width and height of every sticker.
pub const SIDE: u32 = 512;
/// Largest static sticker (100 KB of 1024 bytes, as WhatsApp counts).
pub const STATIC_MAX_BYTES: u64 = 100 * 1024;
/// Largest animated sticker.
pub const ANIMATED_MAX_BYTES: u64 = 500 * 1024;
/// Shortest frame of an animated sticker.
pub const MIN_FRAME_MS: u32 = 8;
/// Longest animated sticker; longer ones are cut.
pub const MAX_DURATION_MS: u64 = 10_000;
/// Width and height of the tray icon.
pub const TRAY_SIDE: u32 = 96;
pub const TRAY_MAX_BYTES: u64 = 50 * 1024;
/// Stickers per pack.
pub const PACK_MIN_STICKERS: usize = 3;
pub const PACK_MAX_STICKERS: usize = 30;
/// Emoji per sticker (at least one).
pub const MAX_EMOJIS: usize = 3;
/// Characters of a pack's name, publisher and identifier.
pub const MAX_NAME_CHARS: usize = 128;

/// (frame rate cap, lossy quality) for animated stickers, tried in order
/// until one fits [`ANIMATED_MAX_BYTES`].
const ANIMATED_LADDER: [(u32, f32); 6] = [(15, 75.0), (12, 60.0), (10, 45.0), (8, 30.0), (6, 20.0), (5, 10.0)];

/// `frame` scaled so its longer side is exactly [`SIDE`] and centred on a
/// transparent [`SIDE`] x [`SIDE`] square.
pub fn square(frame: &RgbaImage) -> RgbaImage {
    pad_square(frame, SIDE)
}

fn pad_square(frame: &RgbaImage, side: u32) -> RgbaImage {
    let (w, h) = fit_exact(frame.width().max(1), frame.height().max(1), side);
    let (w, h) = (w.clamp(1, side), h.clamp(1, side));
    if (w, h) == (side, side) && frame.dimensions() == (side, side) {
        return frame.clone();
    }
    let scaled = image::imageops::resize(frame, w, h, FilterType::Lanczos3);
    let mut out = RgbaImage::new(side, side);
    image::imageops::overlay(&mut out, &scaled, i64::from((side - w) / 2), i64::from((side - h) / 2));
    out
}

/// A static sticker from a decoded frame: lossless WebP when it fits
/// [`STATIC_MAX_BYTES`], else lossy at the highest quality that fits.
/// `None` when even the lowest quality is too large.
pub fn still(frame: &RgbaImage) -> Result<Option<Vec<u8>>, MediaError> {
    let img = square(frame);
    let lossless = webp_lossless(&img)?;
    if lossless.len() as u64 <= STATIC_MAX_BYTES {
        return Ok(Some(lossless));
    }
    squeeze_webp(&img, STATIC_MAX_BYTES)
}

/// The tray icon from a decoded frame: [`TRAY_SIDE`] square PNG, the frame
/// centred on transparency. A 96x96 PNG is at most about 37 KB, well below
/// [`TRAY_MAX_BYTES`].
pub fn tray(frame: &RgbaImage) -> Result<Vec<u8>, MediaError> {
    crate::variants::square_png(frame, TRAY_SIDE)
}

/// The frame a still sticker is made from: the image, the first frame of
/// an animation, or a frame at about 10 % of a video (like the grid
/// thumbnail; needs ffmpeg).
pub fn first_frame(original: &Path, probe: &Probe, tools: &Tools) -> Result<RgbaImage, MediaError> {
    match probe.kind {
        MediaKind::Still | MediaKind::Animation => {
            let bytes = std::fs::read(original)?;
            crate::decode_frame(&bytes, &trusted_limits(probe))
        }
        MediaKind::Video => {
            let dir = tools::scratch(tools)?;
            let input = dir.path().join(format!("input.{}", probe.format.extension()));
            std::fs::copy(original, &input)?;
            let info = tools::ffprobe(tools, dir.path(), probe.format, &input)?;
            let duration = probe.duration_ms.or_else(|| info.duration_ms()).unwrap_or(0);
            representative_frame(
                &input,
                probe.format,
                duration,
                needs_libvpx(probe.format, &info),
                &trusted_limits(probe),
                tools,
                dir.path(),
            )
        }
    }
}

/// Outcome of [`animated`].
#[derive(Debug, Clone, PartialEq)]
pub enum Animated {
    /// An animated WebP within every limit. `cut`: the source was longer
    /// than [`MAX_DURATION_MS`] and only its beginning is in the sticker.
    Webp { bytes: Vec<u8>, cut: bool },
    /// The frames all look the same: it can only be a still sticker.
    NotAnimated,
    /// Larger than [`ANIMATED_MAX_BYTES`] even at the lowest frame rate and
    /// quality.
    TooLarge,
}

/// An animated sticker from a stored animation (decoded here) or video
/// (decoded by ffmpeg). Stills are [`Animated::NotAnimated`].
pub fn animated(original: &Path, probe: &Probe, tools: &Tools) -> Result<Animated, MediaError> {
    match probe.kind {
        MediaKind::Still => Ok(Animated::NotAnimated),
        MediaKind::Animation => from_animation(original, probe),
        MediaKind::Video => from_video(original, probe, tools),
    }
}

/// Output frames at `fps` for a source of `total_ms`, at most
/// [`MAX_DURATION_MS`] long.
fn frame_plan(fps: f64, total_ms: u64) -> (u64, f64) {
    let step = 1000.0 / fps;
    let max = (MAX_DURATION_MS as f64 / step).floor() as u64;
    let wanted = (total_ms as f64 / step).ceil() as u64;
    (wanted.clamp(1, max.max(1)), step)
}

/// Duration of output frame `k` at `step` ms: whole milliseconds that add
/// up to the frame timestamps.
fn frame_ms(k: u64, step: f64) -> u32 {
    let at = |i: u64| (i as f64 * step).round() as u64;
    (at(k + 1) - at(k)).max(u64::from(MIN_FRAME_MS)) as u32
}

/// The last rung makes files about 6 times smaller than the first one; a
/// first attempt this many times over the limit cannot fit.
const HOPELESS: u64 = 12;

/// The encoded animation, if it fits, or why not; `None`: too large.
fn judge(bytes: Vec<u8>, cut: bool) -> Result<Option<Animated>, MediaError> {
    let (_, layout) = crate::webp::rebuild(&bytes)?;
    if layout.durations.len() <= 1 {
        return Ok(Some(Animated::NotAnimated));
    }
    if bytes.len() as u64 <= ANIMATED_MAX_BYTES {
        return Ok(Some(Animated::Webp { bytes, cut }));
    }
    Ok(None)
}

fn from_animation(original: &Path, probe: &Probe) -> Result<Animated, MediaError> {
    let bytes = std::fs::read(original)?;
    let limits = trusted_limits(probe);
    let fmt = probe.format;
    let delays = raster::frame_delays(&bytes, fmt)?;
    let total: u64 = delays.iter().map(|d| u64::from(*d)).sum();
    if delays.len() <= 1 {
        return Ok(Animated::NotAnimated);
    }
    let cut = total > MAX_DURATION_MS;
    let shortest = delays.iter().copied().min().unwrap_or(100).max(1);
    let native = 1000.0 / f64::from(shortest);
    let mut tried = Vec::new();
    for (rung, (cap, quality)) in ANIMATED_LADDER.into_iter().enumerate() {
        let fps = native.min(f64::from(cap));
        // Several rungs can come down to the same rate for slow animations;
        // a lower quality at the same rate is still worth a try.
        tried.push(fps);
        let (frames, step) = frame_plan(fps, total);
        let mut writer = AnimWriter::new(SIDE, SIDE, quality)?;
        let mut written = 0u64;
        let mut src_end = 0f64;
        let mut last_src: Option<RgbaImage> = None;
        raster::for_each_frame(&bytes, fmt, &limits, |f| {
            src_end += f64::from(f.delay_ms);
            let mut sq: Option<RgbaImage> = None;
            while written < frames && (written as f64) * step < src_end {
                let img = sq.get_or_insert_with(|| square(&f.image));
                writer.add(img.as_raw(), frame_ms(written, step))?;
                written += 1;
            }
            last_src = Some(f.image);
            Ok(written < frames)
        })?;
        // Rounding can leave the tail a frame short.
        if written < frames {
            if let Some(src) = &last_src {
                let img = square(src);
                while written < frames {
                    writer.add(img.as_raw(), frame_ms(written, step))?;
                    written += 1;
                }
            }
        }
        let out = writer.finish()?;
        let hopeless = rung == 0 && out.len() as u64 > HOPELESS * ANIMATED_MAX_BYTES;
        match judge(out, cut)? {
            Some(done) => return Ok(done),
            None if hopeless => break,
            None => {}
        }
    }
    tracing::warn!("whatsapp animated sticker does not fit {ANIMATED_MAX_BYTES} bytes at rates {tried:?}");
    Ok(Animated::TooLarge)
}

fn from_video(original: &Path, probe: &Probe, tools: &Tools) -> Result<Animated, MediaError> {
    let dir = tools::scratch(tools)?;
    let input = dir.path().join(format!("input.{}", probe.format.extension()));
    std::fs::copy(original, &input)?;
    let info = tools::ffprobe(tools, dir.path(), probe.format, &input)?;
    let v = info
        .video()
        .ok_or_else(|| MediaError::Invalid("no video stream".into()))?;
    let libvpx = needs_libvpx(probe.format, &info);
    let (w, h) = (v.width.unwrap_or(probe.width), v.height.unwrap_or(probe.height));
    let (sw, sh) = fit_exact(w.max(1), h.max(1), SIDE);
    let duration_ms = probe.duration_ms.or_else(|| info.duration_ms()).unwrap_or(0);
    let cut = duration_ms > MAX_DURATION_MS;
    // Frames are extracted once at the highest rate tried; lower rungs pick
    // from them.
    let src_fps = probe.fps.or_else(|| v.fps()).map(f64::from).filter(|f| *f > 0.0);
    let top = f64::from(ANIMATED_LADDER[0].0);
    let fps = src_fps.map_or(top, |f| f.min(top));
    let raw = dir.path().join("frames.rgba");
    let limit = format!("{:.3}", MAX_DURATION_MS as f64 / 1000.0);
    let vf = format!("fps={fps:.4},scale={sw}:{sh}:flags=lanczos,format=rgba");
    let mut a = input_args(probe.format, &input, libvpx);
    for s in [
        "-map",
        "0:V:0",
        "-t",
        &limit,
        "-an",
        "-sn",
        "-dn",
        "-vf",
        &vf,
        "-pix_fmt",
        "rgba",
        "-map_metadata",
        "-1",
        "-fflags",
        "+bitexact",
        "-f",
        "rawvideo",
        "-y",
    ] {
        a.push(s.into());
    }
    a.push(file_arg(&raw));
    tools::ffmpeg(tools, dir.path(), a, None)?;

    let frame_bytes = sw as u64 * sh as u64 * 4;
    let len = std::fs::metadata(&raw)?.len();
    let count = len / frame_bytes;
    if count == 0 {
        return Err(MediaError::Invalid("video has no decodable frame".into()));
    }
    if count == 1 {
        return Ok(Animated::NotAnimated);
    }
    let total_ms = (count as f64 * 1000.0 / fps).round() as u64;
    let mut file = std::io::BufReader::new(std::fs::File::open(&raw)?);
    let mut buf = vec![0u8; frame_bytes as usize];
    for (rung, (cap, quality)) in ANIMATED_LADDER.into_iter().enumerate() {
        let rate = fps.min(f64::from(cap));
        let (frames, step) = frame_plan(rate, total_ms.min(MAX_DURATION_MS));
        let mut writer = AnimWriter::new(SIDE, SIDE, quality)?;
        let mut loaded: Option<(u64, RgbaImage)> = None;
        for k in 0..frames {
            let idx = ((k as f64 * step * fps / 1000.0).floor() as u64).min(count - 1);
            if loaded.as_ref().map(|(i, _)| *i) != Some(idx) {
                file.seek(SeekFrom::Start(idx * frame_bytes))?;
                file.read_exact(&mut buf)?;
                let img = RgbaImage::from_raw(sw, sh, buf.clone())
                    .ok_or_else(|| MediaError::Invalid("bad raw frame".into()))?;
                loaded = Some((idx, pad_square(&img, SIDE)));
            }
            let (_, img) = loaded.as_ref().expect("loaded above");
            writer.add(img.as_raw(), frame_ms(k, step))?;
        }
        let out = writer.finish()?;
        let hopeless = rung == 0 && out.len() as u64 > HOPELESS * ANIMATED_MAX_BYTES;
        match judge(out, cut)? {
            Some(done) => return Ok(done),
            None if hopeless => break,
            None => {}
        }
    }
    tracing::warn!("whatsapp animated sticker from a video does not fit {ANIMATED_MAX_BYTES} bytes");
    Ok(Animated::TooLarge)
}

/// What a sticker file is, for checks: size, canvas, frames.
#[derive(Debug, Clone, PartialEq)]
pub struct WebpFacts {
    pub width: u32,
    pub height: u32,
    pub animated: bool,
    /// Frame durations (ms) of an animation; empty for a still.
    pub durations: Vec<u32>,
}

/// Read the canvas and the frames of a WebP (as WhatsApp checks them).
pub fn inspect(bytes: &[u8]) -> Result<WebpFacts, MediaError> {
    if crate::sniff(bytes) != Some(MediaFormat::Webp) {
        return Err(MediaError::Unsupported);
    }
    let (_, layout) = crate::webp::rebuild(bytes)?;
    let dec = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|e| MediaError::Invalid(format!("webp: {e}")))?;
    let (width, height) = image::ImageDecoder::dimensions(&dec);
    Ok(WebpFacts {
        width,
        height,
        animated: layout.animated,
        durations: if layout.animated { layout.durations } else { Vec::new() },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_constant_rate_frames_within_ten_seconds() {
        let (n, step) = frame_plan(15.0, 2000);
        assert_eq!(n, 30);
        assert!((step - 66.666).abs() < 0.01);
        let total: u64 = (0..n).map(|k| u64::from(frame_ms(k, step))).sum();
        assert_eq!(total, 2000);
        // Long sources are cut at 10 s.
        let (n, step) = frame_plan(15.0, 60_000);
        assert_eq!(n, 150);
        let total: u64 = (0..n).map(|k| u64::from(frame_ms(k, step))).sum();
        assert!(total <= MAX_DURATION_MS, "{total}");
        assert!((0..n).all(|k| frame_ms(k, step) >= MIN_FRAME_MS));
    }

    #[test]
    fn squares_keep_the_aspect_ratio_on_transparency() {
        let wide = RgbaImage::from_pixel(300, 100, image::Rgba([10, 200, 30, 255]));
        let sq = square(&wide);
        assert_eq!(sq.dimensions(), (SIDE, SIDE));
        assert_eq!(sq.get_pixel(256, 256)[3], 255);
        assert_eq!(sq.get_pixel(256, 10)[3], 0, "transparent band above");
        let small = RgbaImage::from_pixel(20, 40, image::Rgba([1, 2, 3, 255]));
        let sq = square(&small);
        assert_eq!(sq.get_pixel(256, 256)[3], 255, "scaled up to fill the height");
        assert_eq!(sq.get_pixel(256, 0)[3], 255);
        assert_eq!(sq.get_pixel(10, 256)[3], 0);
    }

    fn noise(w: u32, h: u32, seed: u32) -> RgbaImage {
        let mut s = seed | 1;
        RgbaImage::from_fn(w, h, |_, _| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let b = s.to_le_bytes();
            image::Rgba([b[0], b[1], b[2], 255])
        })
    }

    #[test]
    fn still_stickers_fit_the_limit() {
        // A flat image stays lossless; noise is squeezed under 100 KB.
        let flat = RgbaImage::from_pixel(200, 120, image::Rgba([200, 30, 30, 255]));
        let a = still(&flat).unwrap().unwrap();
        let facts = inspect(&a).unwrap();
        assert_eq!((facts.width, facts.height, facts.animated), (SIDE, SIDE, false));
        let b = still(&noise(700, 700, 7)).unwrap().unwrap();
        assert!(b.len() as u64 <= STATIC_MAX_BYTES, "{}", b.len());
        assert_eq!(inspect(&b).unwrap().width, SIDE);
    }

    #[test]
    fn tray_is_a_small_square_png() {
        let png = tray(&noise(300, 150, 3)).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert!(png.len() as u64 <= TRAY_MAX_BYTES, "{}", png.len());
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!((img.width(), img.height()), (TRAY_SIDE, TRAY_SIDE));
    }
}
