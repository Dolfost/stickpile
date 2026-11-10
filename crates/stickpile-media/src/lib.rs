//! Media pipeline: sniff, validate and sanitise uploads, generate variants,
//! perceptual hashes and the quality score. Everything here is blocking; the
//! server calls it from `spawn_blocking`.
//!
//! Security rules (docs/security.md): the format comes from magic bytes, never
//! the extension; every still/animation is fully decoded under [`Limits`]
//! before it is accepted; the stored original is rewritten (PNG/GIF
//! re-encoded losslessly, JPEG/WebP rebuilt from whitelisted segments/chunks)
//! so no metadata or trailing payload survives; ffmpeg/ffprobe only ever run
//! with fixed arguments on server-generated paths, a timeout, and
//! `-protocol_whitelist file`.

use std::path::{Path, PathBuf};
use std::time::Duration;

pub use stickpile_api::{MediaFormat, MediaKind, QualityComponent, QualityReport, Variant};

pub mod hash;
pub mod quality;

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
        Limits { max_bytes: 50 << 20, max_pixels: 40_000_000, max_frames: 3000, max_video_ms: 120_000 }
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
pub fn sniff(bytes: &[u8]) -> Option<MediaFormat> {
    let _ = bytes;
    todo!("media agent")
}

/// Validate, sanitise and analyse an upload.
pub fn ingest(bytes: &[u8], opts: &IngestOptions, tools: &Tools) -> Result<Ingested, MediaError> {
    let _ = (bytes, opts, tools);
    todo!("media agent")
}

#[derive(Debug, Clone)]
pub struct VariantOptions {
    pub discord_gif_max_bytes: u64,
}

/// Generate every variant except `Original` and `Thumb` that applies to the
/// item's kind (see [`Variant`] docs). `original` is the stored sanitised file.
/// Variants that cannot meet their constraints are skipped with a warning,
/// not an error.
pub fn generate_variants(original: &Path, probe: &Probe, opts: &VariantOptions, tools: &Tools) -> Result<Vec<Rendition>, MediaError> {
    let _ = (original, probe, opts, tools);
    todo!("media agent")
}

/// Decode a still (or first frame) for re-processing (e.g. Telegram import of
/// a WebP sticker). Enforces `limits`.
pub fn decode_frame(bytes: &[u8], limits: &Limits) -> Result<image::RgbaImage, MediaError> {
    let _ = (bytes, limits);
    todo!("media agent")
}
