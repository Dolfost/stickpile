//! Target-app profiles: what each chat app accepts, and which stored variant a
//! client should put on the clipboard for it.
//!
//! Limits are deliberately conservative approximations of the free tiers as of
//! 2026 and are documented in `docs/profiles.md`; they drive warnings and
//! variant choice, never hard server-side rejections.

use serde::{Deserialize, Serialize};

use crate::model::{MediaFormat, MediaKind, TargetApp, Variant, VariantInfo};

const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StickerSpec {
    /// Static sticker: formats, exact longest side, max bytes.
    pub static_formats: Vec<MediaFormat>,
    pub static_side_px: u32,
    pub static_max_bytes: u64,
    /// Animated sticker as video, if supported.
    pub video_formats: Vec<MediaFormat>,
    pub video_max_bytes: u64,
    pub video_max_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TargetProfile {
    pub target: TargetApp,
    pub display_name: String,
    /// Upload limit for a single file.
    pub max_file_bytes: u64,
    /// Longest side the app accepts without rejecting/recompressing heavily.
    pub max_dimension: Option<u32>,
    pub still_formats: Vec<MediaFormat>,
    pub animation_formats: Vec<MediaFormat>,
    pub video_formats: Vec<MediaFormat>,
    /// Real sticker support via file drop/attach (see docs/clipboard.md).
    pub sticker: Option<StickerSpec>,
    pub notes: String,
}

impl TargetProfile {
    pub fn accepts(&self, kind: MediaKind, format: MediaFormat) -> bool {
        let list = match kind {
            MediaKind::Still => &self.still_formats,
            MediaKind::Animation => &self.animation_formats,
            MediaKind::Video => &self.video_formats,
        };
        list.contains(&format)
    }
}

pub fn profile(target: TargetApp) -> TargetProfile {
    use MediaFormat::*;
    match target {
        TargetApp::Telegram => TargetProfile {
            target,
            display_name: "Telegram".into(),
            max_file_bytes: 2048 * MIB,
            max_dimension: Some(10_000),
            still_formats: vec![Png, Jpeg, Webp],
            animation_formats: vec![Gif, Mp4],
            video_formats: vec![Mp4, Webm],
            sticker: Some(StickerSpec {
                static_formats: vec![Webp, Png],
                static_side_px: 512,
                static_max_bytes: 512 * 1024,
                video_formats: vec![Webm],
                video_max_bytes: 256 * 1024,
                video_max_ms: 3000,
            }),
            notes: "Pasted images are sent as photos; real stickers need a 512 px WebP file reference. \
                    GIFs are converted to MP4 animations by Telegram."
                .into(),
        },
        TargetApp::Discord => TargetProfile {
            target,
            display_name: "Discord".into(),
            max_file_bytes: 10 * MIB,
            max_dimension: None,
            still_formats: vec![Png, Jpeg, Webp, Gif],
            animation_formats: vec![Gif, Webp],
            video_formats: vec![Mp4, Webm],
            sticker: None,
            notes: "10 MiB upload limit without Nitro.".into(),
        },
        TargetApp::Signal => TargetProfile {
            target,
            display_name: "Signal".into(),
            max_file_bytes: 100 * MIB,
            max_dimension: None,
            still_formats: vec![Png, Jpeg, Webp],
            animation_formats: vec![Gif],
            video_formats: vec![Mp4],
            sticker: None,
            notes: "Sticker packs must be created in Signal itself.".into(),
        },
        TargetApp::Whatsapp => TargetProfile {
            target,
            display_name: "WhatsApp".into(),
            max_file_bytes: 64 * MIB,
            max_dimension: None,
            still_formats: vec![Png, Jpeg],
            animation_formats: vec![Mp4],
            video_formats: vec![Mp4],
            sticker: None,
            notes: "GIFs are sent as MP4; WebP stills arrive as stickers or documents.".into(),
        },
        TargetApp::Slack => TargetProfile {
            target,
            display_name: "Slack".into(),
            max_file_bytes: 1024 * MIB,
            max_dimension: None,
            still_formats: vec![Png, Jpeg, Gif],
            animation_formats: vec![Gif],
            video_formats: vec![Mp4, Webm],
            sticker: None,
            notes: "Animated GIFs above ~2 MB may not autoplay inline.".into(),
        },
        TargetApp::Generic => TargetProfile {
            target,
            display_name: "Other / browser".into(),
            max_file_bytes: 25 * MIB,
            max_dimension: None,
            still_formats: vec![Png, Jpeg, Webp, Gif],
            animation_formats: vec![Gif, Webp],
            video_formats: vec![Mp4, Webm],
            sticker: None,
            notes: "Browser chats generally accept PNG image data from the clipboard.".into(),
        },
    }
}

pub fn all_profiles() -> Vec<TargetProfile> {
    TargetApp::ALL.into_iter().map(profile).collect()
}

/// Facts about one item needed to choose a variant.
#[derive(Debug, Clone)]
pub struct ItemShape<'a> {
    pub kind: MediaKind,
    /// Format of the original.
    pub format: MediaFormat,
    pub duration_ms: Option<u64>,
    /// Variants that currently exist.
    pub variants: &'a [VariantInfo],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub variant: VariantInfo,
    /// True when the chosen variant is a sticker rendition.
    pub sticker: bool,
}

/// Why an item cannot be sent to a target.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Incompatible(pub String);

fn format_of_variant(v: &VariantInfo, original: MediaFormat) -> Option<MediaFormat> {
    match v.variant {
        Variant::Original => Some(original),
        Variant::Thumb | Variant::TgSticker => Some(MediaFormat::Webp),
        Variant::Png => Some(MediaFormat::Png),
        Variant::TgVideoSticker => Some(MediaFormat::Webm),
        Variant::DiscordGif => Some(MediaFormat::Gif),
        Variant::Mp4 => Some(MediaFormat::Mp4),
    }
}

/// Candidate variants in preference order.
fn candidates(target: TargetApp, kind: MediaKind, sticker: bool) -> &'static [Variant] {
    use Variant::*;
    match (target, kind, sticker) {
        (TargetApp::Telegram, MediaKind::Still, true) => &[TgSticker],
        (TargetApp::Telegram, _, true) => &[TgVideoSticker],
        (_, MediaKind::Still, _) => &[Original, Png],
        (TargetApp::Telegram | TargetApp::Whatsapp, MediaKind::Animation, _) => &[Original, Mp4, DiscordGif],
        (TargetApp::Discord, MediaKind::Animation, _) => &[Original, DiscordGif, Mp4],
        (_, MediaKind::Animation, _) => &[Original, DiscordGif, Mp4],
        (TargetApp::Discord, MediaKind::Video, _) => &[Original, Mp4, DiscordGif],
        (_, MediaKind::Video, _) => &[Original, Mp4],
    }
}

/// Pick the variant to send `item` to `target`. With `prefer_sticker` and a
/// target that supports stickers, only sticker variants are considered.
pub fn choose_variant(target: TargetApp, item: &ItemShape<'_>, prefer_sticker: bool) -> Result<Choice, Incompatible> {
    let p = profile(target);
    let sticker = prefer_sticker && p.sticker.is_some();
    let mut last_reason = None;
    for want in candidates(target, item.kind, sticker) {
        let Some(v) = item.variants.iter().find(|v| v.variant == *want) else {
            continue;
        };
        let Some(fmt) = format_of_variant(v, item.format) else { continue };
        if sticker {
            let spec = p.sticker.as_ref().expect("checked");
            let max = if *want == Variant::TgSticker { spec.static_max_bytes } else { spec.video_max_bytes };
            if v.size_bytes > max {
                last_reason = Some(format!("{} sticker is {} KB, limit {} KB", p.display_name, v.size_bytes / 1024, max / 1024));
                continue;
            }
            return Ok(Choice { variant: v.clone(), sticker: true });
        }
        let kind_for_format = match fmt {
            MediaFormat::Mp4 | MediaFormat::Webm if item.kind == MediaKind::Animation => MediaKind::Animation,
            MediaFormat::Gif if item.kind != MediaKind::Still => MediaKind::Animation,
            _ => item.kind,
        };
        if !p.accepts(kind_for_format, fmt) {
            last_reason = Some(format!("{} does not accept {:?} {:?}", p.display_name, kind_for_format, fmt));
            continue;
        }
        if v.size_bytes > p.max_file_bytes {
            last_reason = Some(format!(
                "{} limit is {} MiB, {} is {:.1} MiB",
                p.display_name,
                p.max_file_bytes / MIB,
                v.variant.as_str(),
                v.size_bytes as f64 / MIB as f64
            ));
            continue;
        }
        if let Some(maxd) = p.max_dimension {
            if v.width.max(v.height) > maxd {
                last_reason = Some(format!("{} max dimension is {maxd} px", p.display_name));
                continue;
            }
        }
        return Ok(Choice { variant: v.clone(), sticker: false });
    }
    Err(Incompatible(last_reason.unwrap_or_else(|| {
        if sticker {
            format!("no {} sticker variant generated yet", p.display_name)
        } else {
            format!("no variant suitable for {}", p.display_name)
        }
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(variant: Variant, size: u64) -> VariantInfo {
        VariantInfo { variant, mime: String::new(), width: 512, height: 512, size_bytes: size }
    }

    #[test]
    fn still_prefers_original() {
        let vs = [v(Variant::Original, 1000), v(Variant::Png, 2000)];
        let shape = ItemShape { kind: MediaKind::Still, format: MediaFormat::Jpeg, duration_ms: None, variants: &vs };
        let c = choose_variant(TargetApp::Discord, &shape, false).unwrap();
        assert_eq!(c.variant.variant, Variant::Original);
    }

    #[test]
    fn whatsapp_webp_still_falls_back_to_png() {
        let vs = [v(Variant::Original, 1000), v(Variant::Png, 2000)];
        let shape = ItemShape { kind: MediaKind::Still, format: MediaFormat::Webp, duration_ms: None, variants: &vs };
        let c = choose_variant(TargetApp::Whatsapp, &shape, false).unwrap();
        assert_eq!(c.variant.variant, Variant::Png);
    }

    #[test]
    fn big_gif_falls_back_for_discord() {
        let vs = [v(Variant::Original, 30 * MIB), v(Variant::DiscordGif, 9 * MIB), v(Variant::Mp4, MIB)];
        let shape = ItemShape { kind: MediaKind::Animation, format: MediaFormat::Gif, duration_ms: Some(2000), variants: &vs };
        let c = choose_variant(TargetApp::Discord, &shape, false).unwrap();
        assert_eq!(c.variant.variant, Variant::DiscordGif);
    }

    #[test]
    fn telegram_sticker_requires_variant() {
        let vs = [v(Variant::Original, 1000)];
        let shape = ItemShape { kind: MediaKind::Still, format: MediaFormat::Png, duration_ms: None, variants: &vs };
        assert!(choose_variant(TargetApp::Telegram, &shape, true).is_err());
        let vs = [v(Variant::Original, 1000), v(Variant::TgSticker, 100_000)];
        let shape = ItemShape { variants: &vs, ..shape };
        let c = choose_variant(TargetApp::Telegram, &shape, true).unwrap();
        assert!(c.sticker);
        assert_eq!(c.variant.variant, Variant::TgSticker);
    }

    #[test]
    fn oversized_video_is_incompatible_with_reason() {
        let vs = [v(Variant::Original, 50 * MIB), v(Variant::Mp4, 40 * MIB)];
        let shape = ItemShape { kind: MediaKind::Video, format: MediaFormat::Mp4, duration_ms: Some(60_000), variants: &vs };
        let err = choose_variant(TargetApp::Discord, &shape, false).unwrap_err();
        assert!(err.0.contains("10 MiB"), "{}", err.0);
    }

    #[test]
    fn every_target_has_a_profile() {
        assert_eq!(all_profiles().len(), TargetApp::ALL.len());
    }
}
