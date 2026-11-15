//! Which rendition of a moving item the grids play
//! ([`crate::StickpileLibrary::motion`]). The thumbnail is a still of the
//! first frame; to move, a tile needs a file Android can play on its own:
//!
//! - An animation plays as a picture (`ImageDecoder` animates a GIF or an
//!   animated WebP, transparency kept): the original when it is a GIF or a
//!   WebP, or the Discord GIF, whichever is smaller. The MP4 (flattened on
//!   white) only when there is neither.
//! - A video plays in a media player, without sound: the MP4 the server
//!   made when the original is a WebM or not plain H.264 (what browsers
//!   play, and so does every Android), else the original.

use stickpile_api::{ItemSummary, MediaKind, Variant, VariantInfo};

use crate::types::MotionKind;

/// The rendition to play for `item` and how; `None` for a still, or a
/// moving item without a playable rendition.
pub fn choose(item: &ItemSummary) -> Option<(&VariantInfo, MotionKind)> {
    let find = |v: Variant| item.variants.iter().find(|i| i.variant == v);
    let picture = |i: &&VariantInfo| matches!(i.mime.as_str(), "image/gif" | "image/webp");
    let video = |i: &&VariantInfo| matches!(i.mime.as_str(), "video/mp4" | "video/webm");
    match item.kind {
        MediaKind::Still => None,
        MediaKind::Animation => [find(Variant::Original), find(Variant::DiscordGif)]
            .into_iter()
            .flatten()
            .filter(picture)
            .min_by_key(|i| i.size_bytes)
            .map(|i| (i, MotionKind::Picture))
            .or_else(|| find(Variant::Mp4).filter(video).map(|i| (i, MotionKind::Video))),
        MediaKind::Video => find(Variant::Mp4)
            .or_else(|| find(Variant::Original))
            .filter(video)
            .map(|i| (i, MotionKind::Video)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stickpile_api::MediaFormat;

    fn info(variant: Variant, mime: &str, size_bytes: u64) -> VariantInfo {
        VariantInfo {
            variant,
            mime: mime.into(),
            width: 512,
            height: 512,
            size_bytes,
        }
    }

    fn item(kind: MediaKind, format: MediaFormat, variants: Vec<VariantInfo>) -> ItemSummary {
        ItemSummary {
            kind,
            format,
            variants,
            ..crate::hints::tests::summary()
        }
    }

    fn chosen(item: &ItemSummary) -> Option<(Variant, MotionKind)> {
        choose(item).map(|(i, k)| (i.variant, k))
    }

    #[test]
    fn a_still_does_not_move() {
        let still = item(
            MediaKind::Still,
            MediaFormat::Png,
            vec![
                info(Variant::Original, "image/png", 10),
                info(Variant::Thumb, "image/webp", 5),
            ],
        );
        assert_eq!(chosen(&still), None);
    }

    #[test]
    fn an_animation_plays_its_smaller_picture() {
        let gif = |original: u64, discord: Option<u64>| {
            let mut v = vec![
                info(Variant::Original, "image/gif", original),
                info(Variant::Thumb, "image/webp", 1),
                info(Variant::Mp4, "video/mp4", 2),
            ];
            v.extend(discord.map(|d| info(Variant::DiscordGif, "image/gif", d)));
            item(MediaKind::Animation, MediaFormat::Gif, v)
        };
        assert_eq!(
            chosen(&gif(300, None)),
            Some((Variant::Original, MotionKind::Picture)),
            "the MP4 is smaller but loses the transparency"
        );
        assert_eq!(
            chosen(&gif(9_000, Some(4_000))),
            Some((Variant::DiscordGif, MotionKind::Picture))
        );

        let webp = item(
            MediaKind::Animation,
            MediaFormat::Webp,
            vec![
                info(Variant::Original, "image/webp", 200),
                info(Variant::DiscordGif, "image/gif", 900),
                info(Variant::Mp4, "video/mp4", 50),
            ],
        );
        assert_eq!(chosen(&webp), Some((Variant::Original, MotionKind::Picture)));

        let only_mp4 = item(
            MediaKind::Animation,
            MediaFormat::Webp,
            vec![
                info(Variant::Mp4, "video/mp4", 50),
                info(Variant::Thumb, "image/webp", 1),
            ],
        );
        assert_eq!(chosen(&only_mp4), Some((Variant::Mp4, MotionKind::Video)));
        assert_eq!(chosen(&item(MediaKind::Animation, MediaFormat::Gif, vec![])), None);
    }

    #[test]
    fn a_video_plays_its_mp4_else_the_original() {
        let webm = item(
            MediaKind::Video,
            MediaFormat::Webm,
            vec![
                info(Variant::Original, "video/webm", 100),
                info(Variant::Mp4, "video/mp4", 300),
                info(Variant::DiscordGif, "image/gif", 50),
            ],
        );
        assert_eq!(chosen(&webm), Some((Variant::Mp4, MotionKind::Video)));
        let mp4 = item(
            MediaKind::Video,
            MediaFormat::Mp4,
            vec![
                info(Variant::Original, "video/mp4", 100),
                info(Variant::Thumb, "image/webp", 1),
            ],
        );
        assert_eq!(chosen(&mp4), Some((Variant::Original, MotionKind::Video)));
        let odd = item(
            MediaKind::Video,
            MediaFormat::Mp4,
            vec![info(Variant::Original, "application/octet-stream", 100)],
        );
        assert_eq!(chosen(&odd), None, "only what a player takes");
    }
}
