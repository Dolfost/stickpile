//! What a keyboard inserts into a chat app's text field (the Android input
//! method; see docs/android.md, "Choosing what to insert").
//!
//! The input method knows the focused field's package
//! (`EditorInfo.packageName`) and the MIME types the field accepts
//! (`EditorInfoCompat.getContentMimeTypes`, e.g. `["image/gif",
//! "image/png"]`, `["image/*"]` or nothing), and inserts with
//! `commitContent` only a rendition of a type the field listed.
//!
//! * [`target_for_package`] maps the package to a [`TargetApp`] with the
//!   `PACKAGES` table.
//! * [`variant_for`] picks the rendition: the item's variants in the order of
//!   the `PREFERENCES` table for (target, item kind, sticker), keeping only
//!   those whose MIME type the field accepts. Among those, the first one the
//!   desktop's choice for the target would also send
//!   ([`stickpile_api::profiles::choose_variant`]: a format the app takes,
//!   within its size limits) wins; when none would, the first one the field
//!   accepts is used anyway, since the field said it takes that type.
//!
//! Both tables are data, so results observed by hand change one place.

use stickpile_api::profiles::{choose_variant, format_of_variant, profile, ItemShape};
use stickpile_api::{ItemSummary, MediaFormat, MediaKind, TargetApp, Variant, VariantInfo};

/// Chat apps by Android package name (`applicationId`), an exact,
/// case-sensitive match. Each id was checked on 2026-11-15 against the
/// source named next to it; ids that could not be confirmed are left out.
const PACKAGES: &[(&str, TargetApp)] = &[
    // Telegram. github.com/DrKLO/Telegram (master, commit of 2026-11-12):
    // gradle.properties `APP_PACKAGE=org.telegram.messenger`; in
    // TMessagesProj_App/build.gradle the `standalone` build type adds `.web`
    // and the `debug` build type `.beta`. Google Play:
    // id=org.telegram.messenger. Telegram FOSS uses the same id
    // (github.com/Telegram-FOSS-Team/Telegram-FOSS gradle.properties, and
    // IzzyOnDroid's archive repository,
    // apt.izzysoft.de/fdroid/index/apk/org.telegram.messenger), and so do
    // AyuGram's "fcm" builds, ".web" for "fcmweb" (AyuGram FAQ,
    // teletype.in/@ayugramfaq/conflict-pkg-en, updated 2026-01-30).
    ("org.telegram.messenger", TargetApp::Telegram),
    ("org.telegram.messenger.web", TargetApp::Telegram),
    ("org.telegram.messenger.beta", TargetApp::Telegram),
    // Telegram X: F-Droid (f-droid.org/api/v1/packages/org.thunderdog.challegram)
    // and Google Play id=org.thunderdog.challegram.
    ("org.thunderdog.challegram", TargetApp::Telegram),
    // Nekogram: github.com/Nekogram/Nekogram gradle.properties
    // `APP_PACKAGE=tw.nekomimi.nekogram`; Google Play id=tw.nekomimi.nekogram.
    ("tw.nekomimi.nekogram", TargetApp::Telegram),
    // Nagram: github.com/NextAlone/Nagram TMessagesProj/build.gradle.kts
    // `applicationId = "xyz.nextalone.nagram"`.
    ("xyz.nextalone.nagram", TargetApp::Telegram),
    // AyuGram, the "vanilla" build: github.com/AyuGram/AyuGram4A
    // gradle.properties `APP_PACKAGE=com.radolyn.ayugram` (last pushed in
    // 2023) and the AyuGram FAQ above.
    ("com.radolyn.ayugram", TargetApp::Telegram),
    // Plus Messenger: Google Play id=org.telegram.plus.
    ("org.telegram.plus", TargetApp::Telegram),
    // Forkgram: F-Droid (f-droid.org/api/v1/packages/org.forkgram.messenger).
    ("org.forkgram.messenger", TargetApp::Telegram),
    // Cherrygram: github.com/arsLan4k1390/Cherrygram gradle.properties
    // `APP_PACKAGE=uz.unnarsx.cherrygram`.
    ("uz.unnarsx.cherrygram", TargetApp::Telegram),
    // Discord: Google Play id=com.discord.
    ("com.discord", TargetApp::Discord),
    // Signal: Google Play id=org.thoughtcrime.securesms.
    ("org.thoughtcrime.securesms", TargetApp::Signal),
    // Molly and Molly-FOSS share one id: github.com/mollyim/mollyim-android
    // app/gradle.properties `basePackageId=im.molly.app` (the `store` and
    // `website` flavours add no suffix), and both F-Droid repositories
    // (molly.im/fdroid/repo and molly.im/fdroid/foss/fdroid/repo) list
    // im.molly.app.
    ("im.molly.app", TargetApp::Signal),
    // WhatsApp and WhatsApp Business: Google Play id=com.whatsapp and
    // id=com.whatsapp.w4b.
    ("com.whatsapp", TargetApp::Whatsapp),
    ("com.whatsapp.w4b", TargetApp::Whatsapp),
    // Slack: Google Play id=com.Slack (capital S; id=com.slack is not found).
    ("com.Slack", TargetApp::Slack),
];

/// The chat app behind an Android package name (the focused field's
/// `EditorInfo.packageName`): an exact, case-sensitive match against a table
/// of known packages (Telegram and its forks, Discord, Signal and Molly,
/// WhatsApp and WhatsApp Business, Slack). Anything else, the empty string
/// included, is [`TargetApp::Generic`].
pub fn target_for_package(package: &str) -> TargetApp {
    PACKAGES
        .iter()
        .find(|(name, _)| *name == package)
        .map_or(TargetApp::Generic, |(_, target)| *target)
}

/// Renditions to try, best first, per (target, item kind, sticker asked).
/// Variants the item does not have, or whose MIME type the field does not
/// accept, are skipped. `Thumb` is never inserted.
///
/// PROVISIONAL (mostly UNVERIFIED on Android): one result was observed on
/// a phone (by the owner, 2026-11-18): a still with the sticker flag goes
/// into Telegram's field as its 512 px sticker WebP, and Telegram sends
/// that as a sticker. How each app treats a committed WebP, GIF, MP4 or
/// WebM is to be tried by hand and recorded in docs/android-input.md; the
/// results replace these rows. The orders come from the desktop's choice
/// (`stickpile_api::profiles`, its `candidates` and each app's profile),
/// the desktop's fallback (`stickpile_core::session::pick_variant`) and
/// docs/clipboard.md (Telegram Desktop observed on Wayland, Discord expected
/// from Chromium's behaviour), per target:
///
/// * Every target, stills: the original, then the PNG for an app or field
///   that does not take the original's format (profiles.rs: WhatsApp and
///   Slack take no WebP still), then the 512 px sticker WebP as the last
///   resort (as in `pick_variant`'s fallback).
/// * Every target, videos: the original, the MP4, then the GIF as the last
///   resort for a field that takes only images (as in `pick_variant`'s
///   fallback; profiles.rs lists it for Discord). The 3 s video sticker is
///   never a normal video's stand-in.
/// * Telegram: animations try the MP4 before the GIF (profiles.rs; Telegram
///   turns GIFs into MP4 animations). With the sticker flag the sticker
///   rendition comes first (profiles.rs: the 512 px WebP for a still, the
///   WebM video sticker otherwise), then the normal order, as
///   `pick_variant` falls back when no sticker fits. On the desktop a WebP
///   under 2 MiB becomes a sticker only when sent as a file, and a WebM
///   never does (docs/clipboard.md, 2.1, from the tdesktop source).
/// * Discord: animations try the GIF before the MP4 (profiles.rs; an
///   animated GIF upload is expected, docs/clipboard.md D5, D6). No
///   stickers: the sticker rows equal the normal ones.
/// * WhatsApp: as Telegram without stickers (profiles.rs: GIFs are sent as
///   MP4, WebP stills arrive as stickers or documents, so a WebP original
///   loses to the PNG through the profile). WhatsApp's sticker rows are a
///   candidate to try with the sticker WebP first.
/// * Signal, Slack and anything else: animations try the GIF before the MP4
///   (profiles.rs `candidates`); no stickers.
const PREFERENCES: &[(TargetApp, MediaKind, bool, &[Variant])] = {
    use MediaKind::{Animation, Still, Video};
    use TargetApp::{Discord, Generic, Signal, Slack, Telegram, Whatsapp};
    use Variant::{DiscordGif, Mp4, Original, Png, TgSticker, TgVideoSticker};
    &[
        (Telegram, Still, false, &[Original, Png, TgSticker]),
        (Telegram, Animation, false, &[Original, Mp4, DiscordGif]),
        (Telegram, Video, false, &[Original, Mp4, DiscordGif]),
        (Telegram, Still, true, &[TgSticker, Original, Png]),
        (Telegram, Animation, true, &[TgVideoSticker, Original, Mp4, DiscordGif]),
        (Telegram, Video, true, &[TgVideoSticker, Original, Mp4, DiscordGif]),
        (Discord, Still, false, &[Original, Png, TgSticker]),
        (Discord, Animation, false, &[Original, DiscordGif, Mp4]),
        (Discord, Video, false, &[Original, Mp4, DiscordGif]),
        (Discord, Still, true, &[Original, Png, TgSticker]),
        (Discord, Animation, true, &[Original, DiscordGif, Mp4]),
        (Discord, Video, true, &[Original, Mp4, DiscordGif]),
        (Signal, Still, false, &[Original, Png, TgSticker]),
        (Signal, Animation, false, &[Original, DiscordGif, Mp4]),
        (Signal, Video, false, &[Original, Mp4, DiscordGif]),
        (Signal, Still, true, &[Original, Png, TgSticker]),
        (Signal, Animation, true, &[Original, DiscordGif, Mp4]),
        (Signal, Video, true, &[Original, Mp4, DiscordGif]),
        (Whatsapp, Still, false, &[Original, Png, TgSticker]),
        (Whatsapp, Animation, false, &[Original, Mp4, DiscordGif]),
        (Whatsapp, Video, false, &[Original, Mp4, DiscordGif]),
        (Whatsapp, Still, true, &[Original, Png, TgSticker]),
        (Whatsapp, Animation, true, &[Original, Mp4, DiscordGif]),
        (Whatsapp, Video, true, &[Original, Mp4, DiscordGif]),
        (Slack, Still, false, &[Original, Png, TgSticker]),
        (Slack, Animation, false, &[Original, DiscordGif, Mp4]),
        (Slack, Video, false, &[Original, Mp4, DiscordGif]),
        (Slack, Still, true, &[Original, Png, TgSticker]),
        (Slack, Animation, true, &[Original, DiscordGif, Mp4]),
        (Slack, Video, true, &[Original, Mp4, DiscordGif]),
        (Generic, Still, false, &[Original, Png, TgSticker]),
        (Generic, Animation, false, &[Original, DiscordGif, Mp4]),
        (Generic, Video, false, &[Original, Mp4, DiscordGif]),
        (Generic, Still, true, &[Original, Png, TgSticker]),
        (Generic, Animation, true, &[Original, DiscordGif, Mp4]),
        (Generic, Video, true, &[Original, Mp4, DiscordGif]),
    ]
};

/// The rendition to insert into a text field, as chosen by [`variant_for`].
#[derive(Debug, Clone, PartialEq)]
pub struct InsertChoice {
    /// The rendition: one of the item's `variants`.
    pub variant: VariantInfo,
    /// Its container format; name the prepared file with
    /// [`MediaFormat::extension`].
    pub format: MediaFormat,
    /// The MIME type to commit it as: the rendition's own type
    /// ([`MediaFormat::mime`], lower case), always one the field accepts,
    /// directly or through a wildcard.
    pub mime_type: &'static str,
    /// True when this is the sticker rendition, chosen because a sticker was
    /// asked for and the target has stickers (Telegram). False otherwise,
    /// also when a sticker was asked for and a normal rendition went in.
    pub sticker: bool,
}

/// The best rendition of `item` for a text field of `target` that accepts
/// `accepted_mime_types` (as the field lists them: `image/png`, `image/*`,
/// `*/*`; case-insensitive, parameters such as `; charset=` ignored), or
/// `None` when the item has no rendition the field accepts. An empty list
/// accepts nothing. `sticker` is the user's "send as sticker" flag.
///
/// Candidates come from the provisional preference table for (target, item
/// kind, sticker) and are limited to the item's own variants. The first one
/// that both the field accepts and the desktop's choice for `target` would
/// send (a format the app takes, within its size limits) wins; failing
/// that, the first one the field accepts.
pub fn variant_for(
    item: &ItemSummary,
    target: TargetApp,
    accepted_mime_types: &[String],
    sticker: bool,
) -> Option<InsertChoice> {
    let target_has_stickers = profile(target).sticker.is_some();
    let candidates: Vec<InsertChoice> = preference_order(target, item.kind, sticker)
        .iter()
        .filter_map(|want| item.variants.iter().find(|v| v.variant == *want))
        .filter_map(|v| {
            let format = format_of_variant(v, item.format)?;
            let mime_type = format.mime();
            field_accepts(accepted_mime_types, mime_type).then(|| InsertChoice {
                variant: v.clone(),
                format,
                mime_type,
                sticker: sticker && target_has_stickers && is_sticker_rendition(v.variant),
            })
        })
        .collect();
    let suits = |c: &&InsertChoice| suits_target(item, target, &c.variant, c.sticker);
    candidates.iter().find(suits).or(candidates.first()).cloned()
}

/// The `PREFERENCES` row for (target, kind, sticker).
fn preference_order(target: TargetApp, kind: MediaKind, sticker: bool) -> &'static [Variant] {
    PREFERENCES
        .iter()
        .find(|(t, k, s, _)| *t == target && *k == kind && *s == sticker)
        .map_or(&[], |(_, _, _, order)| order)
}

fn is_sticker_rendition(variant: Variant) -> bool {
    matches!(variant, Variant::TgSticker | Variant::TgVideoSticker)
}

/// True when the desktop's choice for `target` would send `v` too: asked
/// with `v` as the only variant, `choose_variant` accepts it (the app takes
/// its format, as a sticker when `as_sticker`, within the size limits).
fn suits_target(item: &ItemSummary, target: TargetApp, v: &VariantInfo, as_sticker: bool) -> bool {
    let shape = ItemShape {
        kind: item.kind,
        format: item.format,
        duration_ms: None,
        variants: std::slice::from_ref(v),
    };
    choose_variant(target, &shape, as_sticker).is_ok()
}

/// True when one of the field's MIME types covers `mime` (a concrete,
/// lower-case `type/subtype`): equal to it, `type/*` or `*/*`, compared
/// case-insensitively with parameters ignored. Malformed entries (no `/`, an
/// empty part, `*/subtype`) cover nothing.
fn field_accepts(accepted: &[String], mime: &str) -> bool {
    let Some((ty, sub)) = mime.split_once('/') else {
        return false;
    };
    accepted.iter().any(|entry| {
        let essence = entry.split(';').next().unwrap_or_default().trim();
        let Some((t, s)) = essence.split_once('/') else {
            return false;
        };
        let token = |x: &str| !x.is_empty() && !x.contains(|c: char| c.is_whitespace() || c == '/');
        if !token(t) || !token(s) {
            return false;
        }
        match (t, s) {
            ("*", "*") => true,
            ("*", _) => false,
            (t, "*") => t.eq_ignore_ascii_case(ty),
            (t, s) => t.eq_ignore_ascii_case(ty) && s.eq_ignore_ascii_case(sub),
        }
    })
}

#[cfg(test)]
mod tests;
