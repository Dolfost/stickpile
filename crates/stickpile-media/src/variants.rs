//! Renditions: thumbnail, PNG, Telegram stickers, Discord GIF, MP4.

use std::borrow::Cow;
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::ChildStdin;

use image::imageops::FilterType;
use image::{ImageEncoder, RgbaImage};
use stickpile_api::profiles::{profile, StickerSpec};
use stickpile_api::{MediaFormat, MediaKind, TargetApp, Variant};
use tracing::warn;

use crate::tools::{self, file_arg, ProbeJson};
use crate::video::{input_args, needs_libvpx};
use crate::{raster, Limits, MediaError, Probe, Rendition, Tools, VariantOptions};

/// Longest side of MP4 renditions.
const MP4_MAX_SIDE: u32 = 1920;
/// Frame-rate cap of MP4 renditions of animations (GIF delays go down to 20 ms).
const MP4_MAX_FPS: u32 = 50;
const TG_MAX_FPS: u32 = 30;
/// Videos longer than this get no Discord GIF / Telegram video sticker.
pub const SHORT_VIDEO_MS: u64 = 10_000;
/// Discord GIF attempts: (longest side, fps cap), tried in order until one fits.
const GIF_LADDER: [(u32, u32); 6] = [(640, 30), (480, 25), (400, 20), (320, 15), (240, 12), (160, 10)];
/// Bitrate search attempts for the Telegram video sticker.
const VP9_ATTEMPTS: usize = 6;
const THUMB_WEBP_QUALITY: f32 = 80.0;
const STICKER_MAX_QUALITY: u32 = 95;

fn tg_spec() -> StickerSpec {
    profile(TargetApp::Telegram).sticker.expect("telegram has stickers")
}

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

/// Fit inside `max` (longest side), never upscaling.
pub(crate) fn fit_within(w: u32, h: u32, max: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max {
        return (w, h);
    }
    let s = f64::from(max) / f64::from(long);
    let scale = |v: u32| ((f64::from(v) * s).round() as u32).clamp(1, max);
    (scale(w), scale(h))
}

/// Scale so the longest side is exactly `side` (up or down).
pub(crate) fn fit_exact(w: u32, h: u32, side: u32) -> (u32, u32) {
    let long = w.max(h).max(1);
    let s = f64::from(side) / f64::from(long);
    let scale = |v: u32| ((f64::from(v) * s).round() as u32).clamp(1, side);
    if w >= h {
        (side, scale(h))
    } else {
        (scale(w), side)
    }
}

fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

/// Even dimensions for yuv420p, with the longest side exactly `side` when
/// `exact`, else bounded by it.
fn even_dims(w: u32, h: u32, side: u32, exact: bool) -> (u32, u32) {
    let (a, b) = if exact {
        fit_exact(w, h, side)
    } else {
        fit_within(w, h, side)
    };
    // `side` is even, so the exact side stays exact.
    (even(a), even(b))
}

fn rendition(variant: Variant, mime: &'static str, (width, height): (u32, u32), bytes: Vec<u8>) -> Rendition {
    Rendition {
        variant,
        mime,
        width,
        height,
        bytes,
    }
}

pub(crate) fn webp_lossy(img: &RgbaImage, quality: f32) -> Result<Vec<u8>, MediaError> {
    let mut config = webp_config()?;
    config.quality = quality;
    webp_encode(img, &config)
}

/// Lossless WebP. Colours under fully transparent pixels may change (libwebp
/// default); nothing visible does.
pub(crate) fn webp_lossless(img: &RgbaImage) -> Result<Vec<u8>, MediaError> {
    let mut config = webp_config()?;
    config.lossless = 1;
    config.alpha_compression = 0;
    // Effort, not fidelity: 75 is libwebp's default for lossless.
    config.quality = 75.0;
    webp_encode(img, &config)
}

/// Lossy WebP for stickers: like [`webp_lossy`] plus sharp RGB to YUV
/// conversion, which keeps coloured edges (text, outlines) from bleeding.
fn webp_sticker_lossy(img: &RgbaImage, quality: f32) -> Result<Vec<u8>, MediaError> {
    let mut config = webp_config()?;
    config.quality = quality;
    config.use_sharp_yuv = 1;
    webp_encode(img, &config)
}

fn webp_config() -> Result<webp::WebPConfig, MediaError> {
    webp::WebPConfig::new().map_err(|()| MediaError::Invalid("webp: bad libwebp version".into()))
}

/// Encode with `config`; opaque images go in as RGB (no alpha plane).
fn webp_encode(img: &RgbaImage, config: &webp::WebPConfig) -> Result<Vec<u8>, MediaError> {
    let (w, h) = img.dimensions();
    let encoded = if raster::has_transparency(img) {
        webp::Encoder::from_rgba(img.as_raw(), w, h)
            .encode_advanced(config)
            .map(|m| m.to_vec())
    } else {
        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        webp::Encoder::from_rgb(rgb.as_raw(), w, h)
            .encode_advanced(config)
            .map(|m| m.to_vec())
    };
    encoded.map_err(|e| MediaError::Invalid(format!("webp encode: {e:?}")))
}

/// Side of a Telegram sticker set thumbnail (`setStickerSetThumbnail`).
pub const PACK_THUMBNAIL_PX: u32 = 100;

/// Telegram sticker set thumbnail: `frame` scaled so its longer side is
/// exactly [`PACK_THUMBNAIL_PX`], centred on a transparent square of that
/// size (Telegram wants exactly 100×100), as PNG (a few KB; the limit is 128).
pub(crate) fn pack_thumbnail(frame: &RgbaImage) -> Result<Vec<u8>, MediaError> {
    let side = PACK_THUMBNAIL_PX;
    let (w, h) = fit_exact(frame.width().max(1), frame.height().max(1), side);
    let (w, h) = (w.clamp(1, side), h.clamp(1, side));
    let scaled = image::imageops::resize(frame, w, h, FilterType::Lanczos3);
    let mut square = RgbaImage::new(side, side);
    image::imageops::overlay(
        &mut square,
        &scaled,
        i64::from((side - w) / 2),
        i64::from((side - h) / 2),
    );
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(square.as_raw(), side, side, image::ExtendedColorType::Rgba8)
    .map_err(|e| MediaError::Invalid(format!("png encode: {e}")))?;
    Ok(out)
}

fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, MediaError> {
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    );
    let (w, h) = img.dimensions();
    let r = if raster::has_transparency(img) {
        enc.write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
    } else {
        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        enc.write_image(rgb.as_raw(), w, h, image::ExtendedColorType::Rgb8)
    };
    r.map_err(|e| MediaError::Invalid(format!("png encode: {e}")))?;
    Ok(out)
}

/// Grid thumbnail: longest side bounded by `px`, still lossy WebP (alpha kept).
pub(crate) fn thumb(frame: &RgbaImage, px: u32) -> Result<Rendition, MediaError> {
    let dims = fit_within(frame.width(), frame.height(), px.max(1));
    let img = if dims == frame.dimensions() {
        Cow::Borrowed(frame)
    } else {
        Cow::Owned(image::imageops::resize(frame, dims.0, dims.1, FilterType::Lanczos3))
    };
    Ok(rendition(
        Variant::Thumb,
        "image/webp",
        dims,
        webp_lossy(&img, THUMB_WEBP_QUALITY)?,
    ))
}

/// Telegram static sticker for chats: longest side exactly 512, encoded by
/// [`tg_sticker_webp`] (lossless). Packs take it through
/// [`tg_pack_sticker_webp`].
fn tg_sticker(frame: &RgbaImage) -> Result<Option<Rendition>, MediaError> {
    let dims = fit_exact(frame.width(), frame.height(), tg_spec().static_side_px);
    let img = image::imageops::resize(frame, dims.0, dims.1, FilterType::Lanczos3);
    Ok(tg_sticker_webp(&img)?.map(|b| rendition(Variant::TgSticker, "image/webp", dims, b)))
}

/// WebP of an image already at sticker size, for sending in a chat:
/// lossless, as long as it fits the chat limit (a 512 px image always does:
/// even white noise stays near its raw 1 MiB). Otherwise lossy, see
/// [`squeeze_webp`]. `None` when nothing fits.
pub(crate) fn tg_sticker_webp(img: &RgbaImage) -> Result<Option<Vec<u8>>, MediaError> {
    let max = tg_spec().static_chat_max_bytes;
    let lossless = webp_lossless(img)?;
    if lossless.len() as u64 <= max {
        return Ok(Some(lossless));
    }
    squeeze_webp(img, max)
}

/// A `tg_sticker` rendition (or any static sticker WebP) for a Telegram
/// sticker pack, which takes at most `static_max_bytes`: the bytes as they
/// are when they fit, else decoded and re-encoded lossy ([`squeeze_webp`]).
/// `None` when nothing fits.
pub(crate) fn tg_pack_sticker_webp(bytes: &[u8]) -> Result<Option<Vec<u8>>, MediaError> {
    let max = tg_spec().static_max_bytes;
    if bytes.len() as u64 <= max {
        return Ok(Some(bytes.to_vec()));
    }
    let img = crate::decode_frame(bytes, &Limits::default())?;
    squeeze_webp(&img, max)
}

/// Lossy sticker WebP at the highest quality <= 95 that fits `max` bytes
/// (binary search); `None` when even quality 1 does not fit.
fn squeeze_webp(img: &RgbaImage, max: u64) -> Result<Option<Vec<u8>>, MediaError> {
    let fits = |b: &Vec<u8>| b.len() as u64 <= max;
    let top = webp_sticker_lossy(img, STICKER_MAX_QUALITY as f32)?;
    if fits(&top) {
        return Ok(Some(top));
    }
    let (mut lo, mut hi, mut best) = (1u32, STICKER_MAX_QUALITY - 1, None);
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let b = webp_sticker_lossy(img, mid as f32)?;
        if fits(&b) {
            best = Some(b);
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    match best {
        Some(b) => Ok(Some(b)),
        None => {
            warn!("telegram sticker does not fit {max} bytes even at quality 1, skipped");
            Ok(None)
        }
    }
}

/// Limits for re-decoding an original that already passed ingest.
fn trusted_limits(probe: &Probe) -> Limits {
    let d = Limits::default();
    Limits {
        max_bytes: u64::MAX,
        max_pixels: (u64::from(probe.width) * u64::from(probe.height)).max(d.max_pixels),
        max_frames: u32::MAX,
        max_video_ms: u64::MAX,
    }
}

pub(crate) fn generate(
    original: &Path,
    probe: &Probe,
    opts: &VariantOptions,
    tools: &Tools,
) -> Result<Vec<Rendition>, MediaError> {
    let mut out = match probe.kind {
        MediaKind::Still => still(original, probe)?,
        MediaKind::Animation => animation(original, probe, opts, tools)?,
        MediaKind::Video => video(original, probe, opts, tools)?,
    };
    out.sort_by_key(|r| r.variant);
    Ok(out)
}

fn still(original: &Path, probe: &Probe) -> Result<Vec<Rendition>, MediaError> {
    let bytes = std::fs::read(original)?;
    let frame = crate::decode_frame(&bytes, &trusted_limits(probe))?;
    let mut out = Vec::new();
    if probe.format != MediaFormat::Png {
        out.push(rendition(
            Variant::Png,
            "image/png",
            frame.dimensions(),
            encode_png(&frame)?,
        ));
    }
    out.extend(tg_sticker(&frame)?);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Animations: frames are decoded in Rust (ffmpeg cannot decode animated WebP)
// and streamed to ffmpeg as raw video at a constant frame rate.
// ---------------------------------------------------------------------------

struct AnimSource<'a> {
    bytes: &'a [u8],
    fmt: MediaFormat,
    limits: Limits,
    width: u32,
    height: u32,
    delays: Vec<u32>,
    has_alpha: bool,
}

/// Output frame rate as a rational.
#[derive(Debug, Clone, Copy)]
struct Rate {
    num: u32,
    den: u32,
}

impl Rate {
    fn fps(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }
    fn arg(self) -> String {
        format!("{}/{}", self.num, self.den)
    }
}

impl AnimSource<'_> {
    fn total_ms(&self) -> u64 {
        self.delays.iter().map(|d| u64::from(*d)).sum()
    }

    /// The source's own rate (from its shortest frame), capped.
    fn rate(&self, cap: u32) -> Rate {
        let min = self.delays.iter().copied().min().unwrap_or(100).max(1);
        if 1000 <= u64::from(cap) * u64::from(min) {
            Rate { num: 1000, den: min }
        } else {
            Rate { num: cap, den: 1 }
        }
    }

    /// Number of output frames for `limit_ms` of the animation.
    fn frame_count(&self, rate: Rate, limit_ms: Option<u64>) -> u64 {
        let ms = limit_ms.map_or(self.total_ms(), |l| l.min(self.total_ms()));
        ((ms as f64 * rate.fps() / 1000.0).ceil() as u64).max(1)
    }

    fn input_args(&self, pix_fmt: &str, rate: Rate) -> Vec<OsString> {
        args(&[
            "-f",
            "rawvideo",
            "-pix_fmt",
            pix_fmt,
            "-video_size",
            &format!("{}x{}", self.width, self.height),
            "-framerate",
            &rate.arg(),
            "-protocol_whitelist",
            "pipe",
            "-i",
            "pipe:0",
        ])
    }

    /// Resample to `rate` and write `frames` raw frames (RGBA, or RGB
    /// flattened on white).
    fn feed(&self, pipe: &mut ChildStdin, rate: Rate, frames: u64, flatten: bool) -> Result<(), MediaError> {
        let step = 1000.0 / rate.fps();
        let mut written = 0u64;
        let mut src_end = 0f64;
        let mut last: Option<Vec<u8>> = None;
        let mut out = std::io::BufWriter::with_capacity(1 << 20, pipe);
        raster::for_each_frame(self.bytes, self.fmt, &self.limits, |f| {
            if f.image.dimensions() != (self.width, self.height) {
                return Err(MediaError::Invalid("frame size differs from canvas".into()));
            }
            src_end += f64::from(f.delay_ms);
            let raw = if flatten {
                flatten_on_white(&f.image)
            } else {
                f.image.into_raw()
            };
            while written < frames && (written as f64) * step < src_end {
                out.write_all(&raw)?;
                written += 1;
            }
            last = Some(raw);
            Ok(written < frames)
        })?;
        // Rounding can leave the tail one frame short.
        if let Some(raw) = last {
            while written < frames {
                out.write_all(&raw)?;
                written += 1;
            }
        }
        out.flush()?;
        Ok(())
    }

    /// Run ffmpeg with the animation on stdin.
    fn encode(
        &self,
        tools: &Tools,
        dir: &Path,
        rate: Rate,
        frames: u64,
        flatten: bool,
        output: Vec<OsString>,
    ) -> Result<(), MediaError> {
        let mut a = self.input_args(if flatten { "rgb24" } else { "rgba" }, rate);
        a.extend(output);
        let mut feed = |pipe: &mut ChildStdin| self.feed(pipe, rate, frames, flatten);
        tools::ffmpeg(tools, dir, a, Some(&mut feed))
    }
}

fn flatten_on_white(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.as_raw().len() / 4 * 3);
    for p in img.pixels() {
        let a = u32::from(p[3]);
        for c in &p.0[..3] {
            out.push(((u32::from(*c) * a + 255 * (255 - a) + 127) / 255) as u8);
        }
    }
    out
}

fn read_output(path: &Path) -> Result<Vec<u8>, MediaError> {
    let b = std::fs::read(path)?;
    if b.is_empty() {
        return Err(MediaError::Ffmpeg("ffmpeg produced an empty file".into()));
    }
    Ok(b)
}

fn gif_filter(w: u32, h: u32, alpha: bool, fps: Option<f64>) -> String {
    let fps = fps.map(|f| format!("fps={f:.4},")).unwrap_or_default();
    let reserve = if alpha { 1 } else { 0 };
    format!(
        "{fps}scale={w}:{h}:flags=lanczos,split[a][b];\
         [a]palettegen=max_colors=256:reserve_transparent={reserve}:stats_mode=diff[p];\
         [b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle:alpha_threshold=128"
    )
}

fn gif_output(dims: (u32, u32), alpha: bool, fps: Option<f64>, out: &Path) -> Vec<OsString> {
    let mut a = args(&[
        "-vf",
        &gif_filter(dims.0, dims.1, alpha, fps),
        "-loop",
        "0",
        "-f",
        "gif",
        "-y",
    ]);
    a.push(file_arg(out));
    a
}

fn vp9_output(dims: (u32, u32), alpha: bool, kbps: u32, extra_vf: &str, out: &Path) -> Vec<OsString> {
    let vf = format!("{extra_vf}scale={}:{}:flags=lanczos,setsar=1", dims.0, dims.1);
    let mut a = args(&[
        "-vf",
        &vf,
        "-c:v",
        "libvpx-vp9",
        "-pix_fmt",
        if alpha { "yuva420p" } else { "yuv420p" },
        "-b:v",
        &format!("{kbps}k"),
        "-deadline",
        "good",
        "-cpu-used",
        "2",
        "-row-mt",
        "1",
        "-an",
        "-sn",
        "-dn",
        "-map_metadata",
        "-1",
        "-fflags",
        "+bitexact",
        "-f",
        "webm",
        "-y",
    ]);
    a.push(file_arg(out));
    a
}

fn mp4_output(vf: &str, out: &Path) -> Vec<OsString> {
    let mut a = args(&[
        "-vf",
        vf,
        "-c:v",
        "libx264",
        "-preset",
        "medium",
        "-crf",
        "20",
        "-pix_fmt",
        "yuv420p",
        "-an",
        "-sn",
        "-dn",
        "-map_metadata",
        "-1",
        "-map_chapters",
        "-1",
        "-movflags",
        "+faststart",
        "-fflags",
        "+bitexact",
        "-f",
        "mp4",
        "-y",
    ]);
    a.push(file_arg(out));
    a
}

/// Scale (when shrinking) or crop the odd pixel (when not) to even dimensions.
fn even_vf(w: u32, h: u32, dims: (u32, u32)) -> String {
    if fit_within(w, h, MP4_MAX_SIDE) == (w, h) {
        format!("crop={}:{}:0:0,setsar=1", dims.0, dims.1)
    } else {
        format!("scale={}:{}:flags=lanczos,setsar=1", dims.0, dims.1)
    }
}

/// Initial VP9 bitrate (kbit/s) for a sticker of `ms` duration.
fn vp9_kbps(max_bytes: u64, ms: u64) -> u32 {
    let secs = (ms.max(100) as f64) / 1000.0;
    ((max_bytes as f64 * 8.0 * 0.85 / secs / 1000.0) as u32).clamp(24, 4000)
}

/// Encode with decreasing bitrate until the output fits `max_bytes`.
fn bitrate_search(
    max_bytes: u64,
    ms: u64,
    out: &Path,
    mut encode: impl FnMut(u32) -> Result<(), MediaError>,
) -> Result<Option<Vec<u8>>, MediaError> {
    let mut kbps = vp9_kbps(max_bytes, ms);
    for _ in 0..VP9_ATTEMPTS {
        encode(kbps)?;
        let b = read_output(out)?;
        if b.len() as u64 <= max_bytes {
            return Ok(Some(b));
        }
        let next = (f64::from(kbps) * max_bytes as f64 / b.len() as f64 * 0.85) as u32;
        if next < 16 {
            break;
        }
        kbps = next.min(kbps.saturating_sub(1));
    }
    warn!("telegram video sticker does not fit {max_bytes} bytes, skipped");
    Ok(None)
}

fn animation(
    original: &Path,
    probe: &Probe,
    opts: &VariantOptions,
    tools: &Tools,
) -> Result<Vec<Rendition>, MediaError> {
    let bytes = std::fs::read(original)?;
    let src = AnimSource {
        bytes: &bytes,
        fmt: probe.format,
        limits: trusted_limits(probe),
        width: probe.width,
        height: probe.height,
        delays: raster::frame_delays(&bytes, probe.format)?,
        has_alpha: probe.has_alpha,
    };
    let dir = tools::scratch(tools)?;
    let mut out = Vec::new();

    // MP4: H.264, alpha flattened on white, even dimensions.
    {
        let dims = even_dims(src.width, src.height, MP4_MAX_SIDE, false);
        let rate = src.rate(MP4_MAX_FPS);
        let path = dir.path().join("anim.mp4");
        let vf = even_vf(src.width, src.height, dims);
        src.encode(
            tools,
            dir.path(),
            rate,
            src.frame_count(rate, None),
            true,
            mp4_output(&vf, &path),
        )?;
        out.push(rendition(Variant::Mp4, "video/mp4", dims, read_output(&path)?));
    }

    // Discord GIF: unless the original already is a GIF within the limit.
    if !(probe.format == MediaFormat::Gif && bytes.len() as u64 <= opts.discord_gif_max_bytes) {
        let path = dir.path().join("discord.gif");
        let mut done = None;
        for (side, fps_cap) in ladder(src.width, src.height) {
            let dims = fit_within(src.width, src.height, side);
            let rate = src.rate(fps_cap);
            src.encode(
                tools,
                dir.path(),
                rate,
                src.frame_count(rate, None),
                false,
                gif_output(dims, src.has_alpha, None, &path),
            )?;
            let b = read_output(&path)?;
            if b.len() as u64 <= opts.discord_gif_max_bytes {
                done = Some(rendition(Variant::DiscordGif, "image/gif", dims, b));
                break;
            }
        }
        match done {
            Some(r) => out.push(r),
            None => warn!(
                "discord gif does not fit {} bytes at any size, skipped",
                opts.discord_gif_max_bytes
            ),
        }
    }

    // Telegram video sticker: first 3 s, <= 30 fps, 512 px, VP9.
    {
        let spec = tg_spec();
        let dims = even_dims(src.width, src.height, spec.static_side_px, true);
        let rate = src.rate(TG_MAX_FPS);
        let frames = ((spec.video_max_ms as f64 * rate.fps() / 1000.0).floor() as u64).min(src.frame_count(rate, None));
        let ms = (frames as f64 * 1000.0 / rate.fps()) as u64;
        let path = dir.path().join("sticker.webm");
        let alpha = src.has_alpha;
        let found = bitrate_search(spec.video_max_bytes, ms, &path, |kbps| {
            src.encode(
                tools,
                dir.path(),
                rate,
                frames,
                false,
                vp9_output(dims, alpha, kbps, "", &path),
            )
        })?;
        if let Some(b) = found {
            out.push(rendition(Variant::TgVideoSticker, "video/webm", dims, b));
        }
    }
    Ok(out)
}

fn ladder(w: u32, h: u32) -> Vec<(u32, u32)> {
    let native = w.max(h);
    let mut v: Vec<(u32, u32)> = GIF_LADDER.iter().map(|&(side, fps)| (side.min(native), fps)).collect();
    v.dedup();
    v
}

// ---------------------------------------------------------------------------
// Videos: ffmpeg reads a copy of the stored original inside tmp_dir.
// ---------------------------------------------------------------------------

/// Flatten alpha on white inside the filter graph.
const FLATTEN_VF: &str =
    "format=rgba,split[fg][bgsrc];[bgsrc]lutrgb=r=255:g=255:b=255:a=255[bg];[bg][fg]overlay=format=auto,";

fn video(original: &Path, probe: &Probe, opts: &VariantOptions, tools: &Tools) -> Result<Vec<Rendition>, MediaError> {
    let dir = tools::scratch(tools)?;
    let input = dir.path().join(format!("input.{}", probe.format.extension()));
    std::fs::copy(original, &input)?;
    let info: ProbeJson = tools::ffprobe(tools, dir.path(), probe.format, &input)?;
    let v = info
        .video()
        .ok_or_else(|| MediaError::Invalid("no video stream".into()))?;
    let libvpx = needs_libvpx(probe.format, &info);
    let alpha = libvpx;
    let (w, h) = (v.width.unwrap_or(probe.width), v.height.unwrap_or(probe.height));
    let duration_ms = probe.duration_ms.or_else(|| info.duration_ms()).unwrap_or(0);
    let src_fps = probe.fps.or_else(|| v.fps()).map(f64::from);
    let short = duration_ms <= SHORT_VIDEO_MS;
    let mut out = Vec::new();

    // Browsers play H.264 only as 8-bit 4:2:0.
    let playable =
        v.codec_name.as_deref() == Some("h264") && matches!(v.pix_fmt.as_deref(), Some("yuv420p" | "yuvj420p"));
    if probe.format == MediaFormat::Webm || !playable {
        let dims = even_dims(w, h, MP4_MAX_SIDE, false);
        let path = dir.path().join("video.mp4");
        let flatten = if alpha { FLATTEN_VF } else { "" };
        let vf = format!("{flatten}{},format=yuv420p", even_vf(w, h, dims));
        let mut a = input_args(probe.format, &input, libvpx);
        a.extend(args(&["-map", "0:V:0"]));
        a.extend(mp4_output(&vf, &path));
        tools::ffmpeg(tools, dir.path(), a, None)?;
        out.push(rendition(Variant::Mp4, "video/mp4", dims, read_output(&path)?));
    }

    if short {
        let path = dir.path().join("discord.gif");
        let mut done = None;
        for (side, fps_cap) in ladder(w, h) {
            let dims = fit_within(w, h, side);
            let fps = src_fps.map_or(f64::from(fps_cap), |f| f.min(f64::from(fps_cap)));
            let mut a = input_args(probe.format, &input, libvpx);
            a.extend(args(&["-map", "0:V:0", "-an", "-sn", "-dn", "-map_metadata", "-1"]));
            a.extend(gif_output(dims, alpha, Some(fps), &path));
            tools::ffmpeg(tools, dir.path(), a, None)?;
            let b = read_output(&path)?;
            if b.len() as u64 <= opts.discord_gif_max_bytes {
                done = Some(rendition(Variant::DiscordGif, "image/gif", dims, b));
                break;
            }
        }
        match done {
            Some(r) => out.push(r),
            None => warn!(
                "discord gif does not fit {} bytes at any size, skipped",
                opts.discord_gif_max_bytes
            ),
        }

        let spec = tg_spec();
        let dims = even_dims(w, h, spec.static_side_px, true);
        let path = dir.path().join("sticker.webm");
        let ms = duration_ms.min(spec.video_max_ms);
        let fps_vf = match src_fps {
            Some(f) if f <= f64::from(TG_MAX_FPS) => String::new(),
            _ => format!("fps={TG_MAX_FPS},"),
        };
        let limit = format!("{:.3}", spec.video_max_ms as f64 / 1000.0);
        let found = bitrate_search(spec.video_max_bytes, ms, &path, |kbps| {
            let mut a = input_args(probe.format, &input, libvpx);
            a.extend(args(&["-map", "0:V:0", "-t", &limit]));
            a.extend(vp9_output(dims, alpha, kbps, &fps_vf, &path));
            tools::ffmpeg(tools, dir.path(), a, None)
        })?;
        if let Some(b) = found {
            out.push(rendition(Variant::TgVideoSticker, "video/webm", dims, b));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizing_helpers() {
        assert_eq!(fit_within(100, 50, 256), (100, 50));
        assert_eq!(fit_within(1000, 500, 256), (256, 128));
        assert_eq!(fit_within(10, 5000, 256), (1, 256));
        assert_eq!(fit_exact(100, 50, 512), (512, 256));
        assert_eq!(fit_exact(300, 900, 512), (171, 512));
        assert_eq!(fit_exact(2000, 2000, 512), (512, 512));
        assert_eq!(even_dims(300, 900, 512, true), (170, 512));
        assert_eq!(even_dims(101, 51, 1920, false), (100, 50));
        assert_eq!(even_dims(1, 1, 1920, false), (2, 2));
    }

    #[test]
    fn pack_thumbnail_is_an_exact_square_png() {
        for (w, h) in [(300, 150), (40, 90), (512, 512), (1, 1)] {
            let frame = RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 255]));
            let png = pack_thumbnail(&frame).unwrap();
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
            assert!(png.len() <= 128 * 1024);
            let back = image::load_from_memory(&png).unwrap().to_rgba8();
            assert_eq!(back.dimensions(), (PACK_THUMBNAIL_PX, PACK_THUMBNAIL_PX));
            // The image is centred; a wide one leaves transparent bands.
            assert_eq!(back.get_pixel(50, 50)[3], 255);
            if w == 300 {
                assert_eq!(back.get_pixel(50, 2)[3], 0);
            }
        }
    }

    #[test]
    fn rate_follows_shortest_frame() {
        let src = AnimSource {
            bytes: &[],
            fmt: MediaFormat::Gif,
            limits: Limits::default(),
            width: 1,
            height: 1,
            delays: vec![100, 40, 100],
            has_alpha: false,
        };
        let r = src.rate(30);
        assert_eq!((r.num, r.den), (1000, 40));
        assert_eq!(src.frame_count(r, None), 6);
        let r = src.rate(20);
        assert_eq!((r.num, r.den), (20, 1));
        assert_eq!(src.frame_count(r, Some(100)), 2);
    }

    #[test]
    fn flatten_blends_on_white() {
        let img = RgbaImage::from_vec(2, 1, vec![0, 0, 0, 0, 10, 20, 30, 255]).unwrap();
        assert_eq!(flatten_on_white(&img), vec![255, 255, 255, 10, 20, 30]);
    }

    #[test]
    fn png_and_webp_encoders_keep_alpha() {
        let img = RgbaImage::from_fn(20, 10, |x, _| image::Rgba([200, 10, 10, if x < 10 { 0 } else { 255 }]));
        let png = encode_png(&img).unwrap();
        assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8(), img);
        let webp = webp_lossy(&img, 90.0).unwrap();
        let back = image::load_from_memory(&webp).unwrap().to_rgba8();
        assert_eq!(back.get_pixel(2, 2)[3], 0);
        assert_eq!(back.get_pixel(15, 2)[3], 255);
    }
}
