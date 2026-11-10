//! No-reference image quality score, 0..=100 with a per-component breakdown
//! (docs/quality.md): resolution vs recommended size, JPEG quantisation
//! estimate, recompression (generation) detection, blockiness, ringing,
//! sharpness (Laplacian variance), alpha presence, frame rate and palette size
//! for animations.

use crate::Probe;
use stickpile_api::QualityReport;

/// `input` is the file as uploaded (needed for JPEG quantisation tables);
/// `frame` is the representative decoded frame.
pub fn assess(input: &[u8], probe: &Probe, frame: &image::RgbaImage, recommended_px: u32) -> QualityReport {
    let _ = (input, probe, frame, recommended_px);
    todo!("media agent")
}

/// Estimated IJG quality (1..=100) from a JPEG's luminance quantisation table.
pub fn jpeg_quality_estimate(jpeg: &[u8]) -> Option<u8> {
    let _ = jpeg;
    todo!("media agent")
}
