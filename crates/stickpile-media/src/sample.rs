//! Frames that stand for a whole picture, for reading the words in it.

use std::path::Path;

use image::RgbaImage;
use stickpile_api::MediaKind;

use crate::tools;
use crate::variants::trusted_limits;
use crate::video::{frame_at, needs_libvpx};
use crate::{raster, MediaError, Probe, Tools};

/// Indices of the first, middle and last of `n` frames, without repeats.
pub(crate) fn picks(n: usize) -> Vec<usize> {
    let mut v = vec![0, n / 2, n.saturating_sub(1)];
    v.dedup();
    v.retain(|i| *i < n.max(1));
    v
}

/// See [`crate::text_frames`].
pub(crate) fn text_frames(original: &Path, probe: &Probe, tools: &Tools) -> Result<Vec<RgbaImage>, MediaError> {
    let limits = trusted_limits(probe);
    match probe.kind {
        MediaKind::Still => {
            let bytes = std::fs::read(original)?;
            Ok(vec![crate::decode_frame(&bytes, &limits)?])
        }
        MediaKind::Animation => {
            let bytes = std::fs::read(original)?;
            let n = raster::frame_delays(&bytes, probe.format)?.len();
            let wanted = picks(n);
            let last = *wanted.last().unwrap_or(&0);
            let mut out = Vec::with_capacity(wanted.len());
            let mut i = 0usize;
            raster::for_each_frame(&bytes, probe.format, &limits, |f| {
                if wanted.contains(&i) {
                    out.push(f.image);
                }
                i += 1;
                Ok(i <= last)
            })?;
            if out.is_empty() {
                return Err(MediaError::Invalid("animation has no decodable frame".into()));
            }
            Ok(out)
        }
        MediaKind::Video => {
            let dir = tools::scratch(tools)?;
            let input = dir.path().join(format!("input.{}", probe.format.extension()));
            std::fs::copy(original, &input)?;
            let info = tools::ffprobe(tools, dir.path(), probe.format, &input)?;
            let duration = probe.duration_ms.or_else(|| info.duration_ms()).unwrap_or(0);
            let libvpx = needs_libvpx(probe.format, &info);
            // The last frame starts one frame before the end.
            let frame_ms = probe
                .fps
                .filter(|f| *f > 0.0)
                .map_or(100, |f| (1000.0 / f).ceil() as u64);
            let mut seeks = vec![0, duration / 2, duration.saturating_sub(frame_ms)];
            seeks.dedup();
            let mut out = Vec::with_capacity(seeks.len());
            for seek in seeks {
                if let Some(frame) = frame_at(&input, probe.format, seek, libvpx, &limits, tools, dir.path())? {
                    out.push(frame);
                }
            }
            if out.is_empty() {
                return Err(MediaError::Invalid("video has no decodable frame".into()));
            }
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::picks;

    #[test]
    fn first_middle_last() {
        assert_eq!(picks(0), vec![0]);
        assert_eq!(picks(1), vec![0]);
        assert_eq!(picks(2), vec![0, 1]);
        assert_eq!(picks(3), vec![0, 1, 2]);
        assert_eq!(picks(10), vec![0, 5, 9]);
    }
}
