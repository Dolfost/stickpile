//! Media pipeline: sniff, validate and sanitise uploads, generate variants,
//! perceptual hashes and the quality score. Everything here is blocking; the
//! server calls it from `spawn_blocking`.
//!
//! Security rules (docs/security.md, details in docs/media.md): the format
//! comes from magic bytes, never the extension; every still/animation is
//! fully decoded under [`Limits`] before it is accepted; the stored original
//! is rewritten (PNG/GIF re-encoded losslessly, JPEG/WebP rebuilt from
//! whitelisted segments/chunks, videos remuxed without metadata) so no
//! metadata or trailing payload survives; ffmpeg/ffprobe only ever run with
//! fixed arguments on server-generated paths inside [`Tools::tmp_dir`], a
//! forced demuxer, `-protocol_whitelist file` (`pipe` for raw frames we
//! decoded ourselves), a cleared environment and a timeout.
//!
//! The quality score is documented in docs/quality.md.

use std::path::{Path, PathBuf};
use std::time::Duration;

pub use stickpile_api::{MediaFormat, MediaKind, QualityComponent, QualityReport, Variant};

pub mod hash;
pub mod quality;
pub mod signature;

mod grey;
mod jpeg;
mod raster;
mod sniff;
mod tools;
mod variants;
mod video;
mod webp;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("unsupported or unrecognised file type")]
    Unsupported,
    #[error("file too large: {0}")]
    TooLarge(String),
    #[error("corrupt or invalid media: {0}")]
    Invalid(String),
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub max_bytes: u64,
    /// width * height of a decoded frame.
    pub max_pixels: u64,
    pub max_frames: u32,
    pub max_video_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_bytes: 50 << 20,
            max_pixels: 40_000_000,
            max_frames: 3000,
            max_video_ms: 120_000,
        }
    }
}

/// External tools and scratch space.
#[derive(Debug, Clone)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub timeout: Duration,
    /// Scratch directory for ffmpeg inputs/outputs (server: `<data_dir>/tmp`).
    pub tmp_dir: PathBuf,
}

/// Facts about a decoded/probed file.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub format: MediaFormat,
    pub kind: MediaKind,
    pub width: u32,
    pub height: u32,
    pub frame_count: Option<u32>,
    pub duration_ms: Option<u64>,
    pub fps: Option<f32>,
    pub has_alpha: bool,
}

/// One encoded rendition.
#[derive(Debug, Clone)]
pub struct Rendition {
    pub variant: Variant,
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

/// Result of [`ingest`].
#[derive(Debug, Clone)]
pub struct Ingested {
    /// sha256 (lowercase hex) of the bytes as uploaded: the dedup key.
    pub sha256: String,
    pub probe: Probe,
    /// Sanitised original to store (variant `Original`).
    pub original: Vec<u8>,
    pub phash: u64,
    pub dhash: u64,
    pub quality: QualityReport,
    /// Grid thumbnail (variant `Thumb`), always produced synchronously.
    pub thumb: Rendition,
    /// Representative frame (first frame for animations, ~10% in for videos)
    /// for AI embedding.
    pub frame: image::RgbaImage,
}

#[derive(Debug, Clone)]
pub struct IngestOptions {
    pub limits: Limits,
    pub thumb_px: u32,
    /// Recommended longest side of the target set, for the resolution score.
    pub recommended_px: u32,
}

/// Magic-byte sniffing. `None` for anything not in [`MediaFormat`].
///
/// MP4 means an ISO-BMFF `ftyp` box with a video brand (HEIF/AVIF brands are
/// rejected); WebM means an EBML header whose DocType is `webm` (generic
/// Matroska is rejected).
pub fn sniff(bytes: &[u8]) -> Option<MediaFormat> {
    sniff::sniff(bytes)
}

/// Validate, sanitise and analyse an upload.
///
/// Order: size limit, sha256 of the bytes as uploaded, magic-byte sniff,
/// format-specific sanitising (which fully decodes the stored result under
/// `opts.limits`), hashes and quality on the representative frame, thumbnail.
pub fn ingest(bytes: &[u8], opts: &IngestOptions, tools: &Tools) -> Result<Ingested, MediaError> {
    let limits = &opts.limits;
    if bytes.len() as u64 > limits.max_bytes {
        return Err(MediaError::TooLarge(format!(
            "{} bytes, limit {}",
            bytes.len(),
            limits.max_bytes
        )));
    }
    let sha256 = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes));
    let format = sniff(bytes).ok_or(MediaError::Unsupported)?;
    let (original, probe, frame) = match format {
        MediaFormat::Mp4 | MediaFormat::Webm => {
            let v = video::ingest(bytes, format, limits, tools)?;
            (v.original, v.probe, v.frame)
        }
        _ => {
            let s = raster::sanitise(bytes, format, limits)?;
            (s.original, s.probe, s.frame)
        }
    };
    let phash = hash::phash(&frame);
    let dhash = hash::dhash(&frame);
    let quality = quality::assess(bytes, &probe, &frame, opts.recommended_px);
    let thumb = variants::thumb(&frame, opts.thumb_px)?;
    Ok(Ingested {
        sha256,
        probe,
        original,
        phash,
        dhash,
        quality,
        thumb,
        frame,
    })
}

#[derive(Debug, Clone)]
pub struct VariantOptions {
    pub discord_gif_max_bytes: u64,
}

/// Generate every variant except `Original` and `Thumb` that applies to the
/// item's kind (see [`Variant`] docs). `original` is the stored sanitised file.
/// Variants that cannot meet their constraints are skipped with a warning,
/// not an error.
///
/// Renditions come back sorted by [`Variant`]. See docs/media.md for the
/// constraints of each variant.
pub fn generate_variants(
    original: &Path,
    probe: &Probe,
    opts: &VariantOptions,
    tools: &Tools,
) -> Result<Vec<Rendition>, MediaError> {
    variants::generate(original, probe, opts, tools)
}

pub use variants::{PACK_THUMBNAIL_PX, SHORT_VIDEO_MS};

/// A Telegram sticker set thumbnail from a decoded still (e.g. the cover
/// item's grid thumbnail through [`decode_frame`]): exactly
/// [`PACK_THUMBNAIL_PX`] square, transparent padding, PNG.
pub fn pack_thumbnail(frame: &image::RgbaImage) -> Result<Vec<u8>, MediaError> {
    variants::pack_thumbnail(frame)
}

/// Decode a still (or first frame) for re-processing (e.g. Telegram import of
/// a WebP sticker). Enforces `limits` (`max_bytes`, `max_pixels`; the
/// dimensions are read from the header before decoding). JPEG EXIF
/// orientation is applied.
pub fn decode_frame(bytes: &[u8], limits: &Limits) -> Result<image::RgbaImage, MediaError> {
    if bytes.len() as u64 > limits.max_bytes {
        return Err(MediaError::TooLarge(format!(
            "{} bytes, limit {}",
            bytes.len(),
            limits.max_bytes
        )));
    }
    let format = sniff(bytes).ok_or(MediaError::Unsupported)?;
    match format {
        MediaFormat::Mp4 | MediaFormat::Webm => Err(MediaError::Unsupported),
        MediaFormat::Gif | MediaFormat::Webp => raster::first_frame(bytes, format, limits),
        MediaFormat::Jpeg => {
            let mut img = raster::decode_still(bytes, format, limits)?;
            let orientation = jpeg::parse(bytes).ok().and_then(|p| jpeg::exif_orientation(&p));
            if let Some(o) = orientation.and_then(image::metadata::Orientation::from_exif) {
                img.apply_orientation(o);
            }
            Ok(img.to_rgba8())
        }
        MediaFormat::Png => Ok(raster::decode_still(bytes, format, limits)?.to_rgba8()),
    }
}
