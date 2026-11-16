//! Sticker renditions for Discord and Signal, made where they are needed
//! (the publish jobs and the Signal-ready zip) instead of being stored as
//! item variants (docs/discord.md and docs/signal.md, "Renditions").
//!
//! `stickpile_media::pack` renders a square of the platform's size within
//! its byte budget (stills: PNG or WebP; animations: APNG or GIF, cut to
//! the longest animation the platform takes, fewer frames and then a
//! smaller square until it fits). The platform crate then checks the file
//! with its own rules ([`stickpile_discord::check_file`],
//! [`stickpile_signal::check_sticker`]); its sizes and limits are the
//! source of truth for the [`PackTarget`]s here. Items that cannot be made
//! to fit are skipped with the reason.

use std::collections::HashMap;
use std::path::PathBuf;

use stickpile_api::{MediaKind, Variant};
use stickpile_discord::ExpressionKind;
use stickpile_media::pack::{AnimationFormat, PackTarget, StillFormat};

use crate::db::VariantRow;
use crate::files;
use crate::jobs::JobError;
use crate::service::hydrate::ItemFull;
use crate::service::whatsapp::probe_of;
use crate::state::AppState;

/// Discord gives emoji no animation limit; stickpile cuts them here.
pub const EMOJI_MAX_ANIMATION_MS: u64 = 10_000;

/// What a sticker is made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    DiscordSticker,
    DiscordEmoji,
    Signal,
}

impl Kind {
    pub fn discord(kind: ExpressionKind) -> Kind {
        match kind {
            ExpressionKind::Sticker => Kind::DiscordSticker,
            ExpressionKind::Emoji => Kind::DiscordEmoji,
        }
    }

    /// The platform's rules as a render target.
    pub fn target(self) -> PackTarget {
        match self {
            Kind::DiscordSticker => {
                let r = &stickpile_discord::STICKER_REQUIREMENTS;
                PackTarget {
                    side: r.recommended_px.0,
                    max_bytes: r.max_bytes,
                    max_animation_ms: r.max_animation_ms.map_or(EMOJI_MAX_ANIMATION_MS, u64::from),
                    still: StillFormat::Png,
                    animation: AnimationFormat::Apng,
                }
            }
            Kind::DiscordEmoji => {
                let r = &stickpile_discord::EMOJI_REQUIREMENTS;
                PackTarget {
                    side: r.recommended_px.0,
                    max_bytes: r.max_bytes,
                    max_animation_ms: r.max_animation_ms.map_or(EMOJI_MAX_ANIMATION_MS, u64::from),
                    still: StillFormat::Png,
                    animation: AnimationFormat::Gif,
                }
            }
            Kind::Signal => {
                let r = &stickpile_signal::STICKER_REQUIREMENTS;
                PackTarget {
                    side: r.side_px,
                    max_bytes: r.max_bytes,
                    max_animation_ms: u64::from(r.max_animation_ms),
                    still: StillFormat::Webp,
                    animation: AnimationFormat::Apng,
                }
            }
        }
    }

    /// The platform crate's check of a rendered file; `Err` is the reason.
    pub fn check(self, bytes: &[u8]) -> Result<(), String> {
        match self {
            Kind::DiscordSticker => stickpile_discord::check_file(ExpressionKind::Sticker, bytes).map(|_| ()),
            Kind::DiscordEmoji => stickpile_discord::check_file(ExpressionKind::Emoji, bytes).map(|_| ()),
            Kind::Signal => stickpile_signal::check_sticker(bytes).map(|_| ()),
        }
    }

    fn too_large(self, animated: bool) -> &'static str {
        match (self, animated) {
            (Kind::DiscordSticker, false) => "does not fit the 512 KB limit of Discord stickers",
            (Kind::DiscordSticker, true) => {
                "does not fit the 512 KB limit of Discord stickers, even smaller and at 5 frames per second"
            }
            (Kind::DiscordEmoji, false) => "does not fit the 256 KB limit of Discord emoji",
            (Kind::DiscordEmoji, true) => {
                "does not fit the 256 KB limit of Discord emoji, even smaller and at 5 frames per second"
            }
            (Kind::Signal, false) => "does not fit the 300 KB limit of Signal stickers",
            (Kind::Signal, true) => {
                "does not fit the 300 KB limit of Signal stickers, even smaller and at 5 frames per second"
            }
        }
    }

    fn cut_note(self) -> String {
        match self.target().max_animation_ms {
            3000 => "cut to the first 3 seconds".into(),
            5000 => "cut to the first 5 seconds".into(),
            10_000 => "cut to the first 10 seconds".into(),
            ms => format!("cut to the first {} ms", ms),
        }
    }
}

/// A sticker made for a platform.
#[derive(Debug, Clone)]
pub struct Made {
    pub bytes: Vec<u8>,
    pub animated: bool,
    /// What was changed to fit, e.g. a long animation cut.
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Rendered {
    Made(Made),
    /// Why the item cannot become a sticker.
    Skip(String),
}

fn retry(e: impl std::fmt::Display) -> JobError {
    JobError::Retry(e.to_string())
}

/// The stored original of `item`, if it is there.
pub fn original(state: &AppState, variants: &HashMap<String, Vec<VariantRow>>, item: &ItemFull) -> Option<PathBuf> {
    variants
        .get(&item.media_id)
        .and_then(|vs| vs.iter().find(|v| v.variant() == Some(Variant::Original)))
        .and_then(|v| files::resolve(&state.media_dir(), &v.path))
}

/// Render `item` (its stored original at `original`) for `kind`.
pub async fn render(
    state: &AppState,
    item: &ItemFull,
    original: Option<PathBuf>,
    kind: Kind,
) -> Result<Rendered, JobError> {
    let Some(abs) = original else {
        return Ok(Rendered::Skip("the original file is missing".into()));
    };
    if item.kind() == MediaKind::Video && !state.ffmpeg_available {
        return Ok(Rendered::Skip(
            "animated stickers from videos need ffmpeg on the server".into(),
        ));
    }
    let probe = probe_of(item);
    let tools = state.tools.clone();
    let target = kind.target();
    let out = tokio::task::spawn_blocking(move || stickpile_media::pack::sticker(&abs, &probe, &tools, &target))
        .await
        .map_err(retry)?;
    Ok(match out {
        Ok(Some(s)) => match kind.check(&s.bytes) {
            Err(why) => Rendered::Skip(why),
            Ok(()) => {
                let note = if s.cut {
                    Some(kind.cut_note())
                } else if item.kind() != MediaKind::Still && !s.animated {
                    Some("every frame looks the same: sent as a still sticker".into())
                } else {
                    None
                };
                Rendered::Made(Made {
                    bytes: s.bytes,
                    animated: s.animated,
                    note,
                })
            }
        },
        Ok(None) => Rendered::Skip(kind.too_large(item.kind() != MediaKind::Still).into()),
        Err(e) => Rendered::Skip(e.to_string()),
    })
}

/// A still sticker for `kind` from a decoded frame (a set's cover); `None`
/// when it does not fit or the platform refuses it.
pub async fn render_still(frame: image::RgbaImage, kind: Kind) -> Result<Option<Vec<u8>>, JobError> {
    let target = kind.target();
    let out = tokio::task::spawn_blocking(move || stickpile_media::pack::still(&frame, &target))
        .await
        .map_err(retry)?;
    Ok(match out {
        Ok(Some(bytes)) if kind.check(&bytes).is_ok() => Some(bytes),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_follow_the_platform_crates() {
        let t = Kind::DiscordSticker.target();
        assert_eq!((t.side, t.max_bytes, t.max_animation_ms), (320, 512 * 1024, 5000));
        assert_eq!(t.animation, AnimationFormat::Apng);
        let t = Kind::DiscordEmoji.target();
        assert_eq!((t.side, t.max_bytes, t.max_animation_ms), (128, 256 * 1024, 10_000));
        assert_eq!(t.animation, AnimationFormat::Gif);
        let t = Kind::Signal.target();
        assert_eq!((t.side, t.max_bytes, t.max_animation_ms), (512, 300 * 1024, 3000));
        assert_eq!(t.still, StillFormat::Webp);
        assert_eq!(Kind::Signal.cut_note(), "cut to the first 3 seconds");
    }
}
