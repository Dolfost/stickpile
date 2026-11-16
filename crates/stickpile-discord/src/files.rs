//! What a file must look like to become a Discord guild sticker or emoji.
//!
//! The server renders the renditions; these helpers state the rules and
//! check a rendered file before any API call, so a file Discord would refuse
//! is reported as skipped with a reason instead of failing the export.
//!
//! Sources (Discord developer docs): resources/sticker "Create Guild Sticker"
//! (PNG, APNG, GIF or Lottie, max 512 KiB; "constrained to 5 seconds in
//! length for animated stickers, and 320 x 320 pixels"); resources/emoji
//! "Create Guild Emoji" (the 128x128 emoji image as image data, max 256 KiB;
//! "Emoji can be uploaded as JPEG, PNG, GIF, WebP, and AVIF formats");
//! reference "Image Data" (a data URI). Lottie stickers need a verified or
//! partnered guild and are never produced by stickpile.

use serde::Serialize;

use crate::image::{inspect, ImageFormat, ImageInfo};
use crate::ExpressionKind;

/// The rules for one kind of guild expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Requirements {
    /// Accepted containers. APNG counts as animated PNG.
    pub formats: &'static [ImageFormat],
    pub max_bytes: u64,
    /// Neither side may exceed this (None: no documented limit).
    pub max_side_px: Option<u32>,
    /// The size to render at.
    pub recommended_px: (u32, u32),
    /// Longest animation.
    pub max_animation_ms: Option<u32>,
}

/// Guild stickers: PNG, APNG or GIF, at most 512 KiB, at most 320x320
/// (render exactly 320x320, padded with transparency), animations at most
/// 5 s.
pub const STICKER_REQUIREMENTS: Requirements = Requirements {
    formats: &[ImageFormat::Png, ImageFormat::Apng, ImageFormat::Gif],
    max_bytes: 512 * 1024,
    max_side_px: Some(320),
    recommended_px: (320, 320),
    max_animation_ms: Some(5000),
};

/// Guild emoji: PNG, GIF, JPEG or WebP, at most 256 KiB, shown at 128x128
/// (render 128x128; Discord scales larger files down). Animated GIF or
/// animated WebP makes an animated emoji, which uses the separate animated
/// emoji slots.
pub const EMOJI_REQUIREMENTS: Requirements = Requirements {
    formats: &[ImageFormat::Png, ImageFormat::Gif, ImageFormat::Jpeg, ImageFormat::Webp],
    max_bytes: 256 * 1024,
    max_side_px: None,
    recommended_px: (128, 128),
    max_animation_ms: None,
};

pub fn requirements(kind: ExpressionKind) -> &'static Requirements {
    match kind {
        ExpressionKind::Sticker => &STICKER_REQUIREMENTS,
        ExpressionKind::Emoji => &EMOJI_REQUIREMENTS,
    }
}

/// Checks `bytes` against the rules for `kind`. `Err` is a reason fit for
/// the export report.
pub fn check_file(kind: ExpressionKind, bytes: &[u8]) -> Result<ImageInfo, String> {
    let req = requirements(kind);
    let what = match kind {
        ExpressionKind::Sticker => "sticker",
        ExpressionKind::Emoji => "emoji",
    };
    if bytes.len() as u64 > req.max_bytes {
        return Err(format!(
            "the {what} file is {} KiB, Discord takes at most {} KiB",
            bytes.len().div_ceil(1024),
            req.max_bytes / 1024
        ));
    }
    let Some(info) = inspect(bytes) else {
        return Err(format!("the {what} file is not a PNG, APNG, GIF, JPEG or WebP image"));
    };
    if !req.formats.contains(&info.format) {
        let list: Vec<&str> = req.formats.iter().map(|f| f.as_str()).collect();
        return Err(format!(
            "Discord {what}s must be {}, not {}",
            list.join(", "),
            info.format.as_str()
        ));
    }
    if let Some(max) = req.max_side_px {
        if info.width > max || info.height > max {
            return Err(format!(
                "the {what} is {}x{} px, Discord takes at most {max}x{max}",
                info.width, info.height
            ));
        }
    }
    if info.width == 0 || info.height == 0 {
        return Err(format!("the {what} image has no pixels"));
    }
    if let (Some(max), Some(ms)) = (req.max_animation_ms, info.duration_ms) {
        if ms > max {
            return Err(format!(
                "the animation lasts {:.1} s, Discord {what}s may last at most {} s",
                f64::from(ms) / 1000.0,
                max / 1000
            ));
        }
    }
    Ok(info)
}

/// `data:<mime>;base64,<data>`: the "image data" Discord takes for emoji.
pub fn data_uri(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine;
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::tests::{gif, png};

    #[test]
    fn sticker_files() {
        assert_eq!(
            check_file(ExpressionKind::Sticker, &png(320, 320, None))
                .unwrap()
                .format,
            ImageFormat::Png
        );
        assert!(check_file(ExpressionKind::Sticker, &png(320, 320, Some((5, 0, 1000)))).is_ok());
        let err = check_file(ExpressionKind::Sticker, &png(320, 320, Some((6, 0, 1000)))).unwrap_err();
        assert!(err.contains("6.0 s"), "{err}");
        let err = check_file(ExpressionKind::Sticker, &png(512, 512, None)).unwrap_err();
        assert!(err.contains("512x512"), "{err}");
        assert!(check_file(ExpressionKind::Sticker, &gif(320, 320, 3, 10, true)).is_ok());
        let mut big = png(320, 320, None);
        big.resize(600 * 1024, 0);
        assert!(check_file(ExpressionKind::Sticker, &big)
            .unwrap_err()
            .contains("512 KiB"));
        let webp = b"RIFF\0\0\0\0WEBPVP8L\x05\0\0\0\x2f\x3f\x3f\0\0".to_vec();
        let err = check_file(ExpressionKind::Sticker, &webp).unwrap_err();
        assert!(err.contains("not WebP"), "{err}");
        assert!(check_file(ExpressionKind::Sticker, b"hello").is_err());
    }

    #[test]
    fn emoji_files() {
        assert!(check_file(ExpressionKind::Emoji, &png(1024, 1024, None)).is_ok());
        assert!(
            check_file(ExpressionKind::Emoji, &gif(128, 128, 2, 10, true))
                .unwrap()
                .animated
        );
        assert!(check_file(ExpressionKind::Emoji, &png(128, 128, Some((2, 0, 100)))).is_err());
        let mut big = png(128, 128, None);
        big.resize(300 * 1024, 0);
        assert!(check_file(ExpressionKind::Emoji, &big).unwrap_err().contains("256 KiB"));
        assert_eq!(data_uri("image/png", b"\x89PNG"), "data:image/png;base64,iVBORw==");
    }
}
