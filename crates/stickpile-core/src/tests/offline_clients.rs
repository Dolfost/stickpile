//! What the clients do with a kept sticker, against what `offline::needs`
//! keeps (ported from the behaviour review of the offline round, branch
//! `review-offline-behaviour`): a copy on the desktop and an insert by the
//! Android keyboard use offline what they use online.
//!
//! The items are shaped like the server's renditions
//! (`stickpile-media/src/variants.rs`: a PNG still has no `png` rendition, a
//! GIF within the Discord limit no `discord_gif`, an H.264 MP4 video no
//! `mp4`), with sizes as measured on the demo library. The keyboard is
//! modelled as the Android bridge works: with the server unreachable,
//! `prepare` and `available_offline` are `variant_for` over the renditions
//! on the phone (`library::on_device`), here those `needs` keeps.
//!
//! Run with `--nocapture` for the tables.

use std::collections::BTreeSet;

use stickpile_api::{ItemSummary, MediaFormat, MediaKind, TargetApp, Variant, VariantInfo};

use crate::offline::{needs, skip_reason, Device, FileRole, OfflineOptions, INSERT_FIELDS};
use crate::session::pick_variant;
use crate::{target_for_package, variant_for};

fn v(variant: Variant, mime: &str, kb: u64) -> VariantInfo {
    VariantInfo {
        variant,
        mime: mime.into(),
        width: 512,
        height: 512,
        size_bytes: kb * 1000,
    }
}

fn item(name: &str, kind: MediaKind, format: MediaFormat, variants: Vec<VariantInfo>) -> ItemSummary {
    let id = name.replace(' ', "-");
    ItemSummary {
        id: id.clone(),
        set_id: "s".into(),
        set_slug: "s".into(),
        set_name: "S".into(),
        owner_username: "alice".into(),
        name: name.into(),
        keywords: Vec::new(),
        emojis: Vec::new(),
        vibe: None,
        tags: Vec::new(),
        text: String::new(),
        kind,
        format,
        width: 512,
        height: 512,
        quality: None,
        likes: 0,
        copies: 0,
        media_id: format!("m-{id}"),
        sha256: String::new(),
        phash: String::new(),
        variants,
        updated_at: 0,
    }
}

/// One item of each shape the server makes.
fn shapes() -> Vec<ItemSummary> {
    use MediaFormat as F;
    use MediaKind::{Animation, Still, Video};
    use Variant::*;
    let thumb = || v(Thumb, "image/webp", 8);
    vec![
        item(
            "still png",
            Still,
            F::Png,
            vec![v(Original, "image/png", 300), thumb(), v(TgSticker, "image/webp", 60)],
        ),
        item(
            "still webp",
            Still,
            F::Webp,
            vec![
                v(Original, "image/webp", 120),
                thumb(),
                v(Png, "image/png", 900),
                v(TgSticker, "image/webp", 60),
            ],
        ),
        item(
            "gif animation",
            Animation,
            F::Gif,
            vec![
                v(Original, "image/gif", 400),
                thumb(),
                v(Mp4, "video/mp4", 150),
                v(TgVideoSticker, "video/webm", 200),
            ],
        ),
        item(
            "webp animation",
            Animation,
            F::Webp,
            vec![
                v(Original, "image/webp", 300),
                thumb(),
                v(Mp4, "video/mp4", 150),
                v(DiscordGif, "image/gif", 900),
                v(TgVideoSticker, "video/webm", 200),
            ],
        ),
        item(
            "mp4 video",
            Video,
            F::Mp4,
            vec![
                v(Original, "video/mp4", 2000),
                thumb(),
                v(DiscordGif, "image/gif", 1500),
                v(TgVideoSticker, "video/webm", 230),
            ],
        ),
        item(
            "webm video",
            Video,
            F::Webm,
            vec![
                v(Original, "video/webm", 1000),
                thumb(),
                v(Mp4, "video/mp4", 1200),
                v(DiscordGif, "image/gif", 1500),
                v(TgVideoSticker, "video/webm", 230),
            ],
        ),
    ]
}

/// Chat apps by package, and one the table does not know (Google
/// Messages: `Generic`).
const PACKAGES: &[&str] = &[
    "org.telegram.messenger",
    "com.discord",
    "com.whatsapp",
    "org.thoughtcrime.securesms",
    "com.Slack",
    "com.google.android.apps.messaging",
];

/// Fields as a keyboard may see them (`EditorInfoCompat.getContentMimeTypes`):
/// the common shapes, the usual chat field's list among them.
const FIELDS: &[&[&str]] = &[
    &["image/png"],
    &["image/gif", "image/png"],
    &["image/gif", "image/*", "image/jpg", "image/png", "image/webp"],
    &["image/*"],
    &["image/*", "video/*"],
];

/// `InsertFlow.SHARE_TYPES`.
const SHARE: &[&str] = &["image/*", "video/*"];

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// The renditions (not thumbnails) `needs` keeps for `item` on `device`,
/// with the per-kind options applied unless the sticker is kept by itself.
fn kept(item: &ItemSummary, opts: &OfflineOptions, device: &Device, explicit: bool) -> BTreeSet<Variant> {
    needs(item, opts, device)
        .into_iter()
        .filter(|f| !f.roles.iter().all(|r| *r == FileRole::Thumbnail))
        .filter(|f| skip_reason(item, f, opts, explicit).is_none())
        .map(|f| f.variant)
        .collect()
}

/// Their bytes.
fn kept_bytes(item: &ItemSummary, here: &BTreeSet<Variant>) -> u64 {
    item.variants
        .iter()
        .filter(|v| here.contains(&v.variant))
        .map(|v| v.size_bytes)
        .sum()
}

/// `item` with only `here` (what `library::on_device` gives the bridge).
fn only(item: &ItemSummary, here: &BTreeSet<Variant>) -> ItemSummary {
    let mut out = item.clone();
    out.variants.retain(|v| here.contains(&v.variant));
    out
}

fn chosen(item: &ItemSummary, target: TargetApp, field: &[String], sticker: bool) -> Option<Variant> {
    variant_for(item, target, field, sticker).map(|c| c.variant.variant)
}

fn name(v: Option<Variant>) -> &'static str {
    v.map_or("-", |v| v.as_str())
}

/// The Android phone as the bridge opens it: `default_target` is
/// `Generic` (no config file), plus the chat apps of the copy history; the
/// grids play, the keyboard inserts.
fn phone(history: &[TargetApp]) -> Device {
    let mut targets = vec![TargetApp::Generic];
    for t in history {
        if !targets.contains(t) {
            targets.push(*t);
        }
    }
    Device {
        targets,
        plays_motion: true,
        inserts: true,
    }
}

/// The desktop's rule: a copy's rendition (`pick_variant`, what
/// `prepare_copy` and `available_offline` use) is among what `needs` keeps
/// for each target of the device, with and without sticker mode.
#[test]
fn needs_keeps_what_a_desktop_copy_uses() {
    let opts = OfflineOptions::default();
    for item in shapes() {
        for target in TargetApp::ALL {
            let device = Device {
                targets: vec![target],
                plays_motion: false,
                inserts: false,
            };
            let here = kept(&item, &opts, &device, true);
            for sticker in [false, true] {
                let (info, _, _) = pick_variant(&item, target, sticker).unwrap();
                assert!(
                    here.contains(&info.variant),
                    "{} to {target:?} sticker={sticker}: {:?} not in {here:?}",
                    item.name,
                    info.variant
                );
            }
        }
    }
}

/// What the Android keyboard inserts into a field with the server reachable
/// (`variant_for` over all renditions) against what it inserts with the
/// server unreachable and only the kept files on the phone, for a sticker
/// kept by itself on a phone that has sent to that app before (so the app
/// is among the device's targets): the same, for every field shape of
/// `FIELDS` (each covers what one of [`INSERT_FIELDS`] covers), with and
/// without the sticker flag. Prints what the inserts cost in kept bytes
/// over what the copies alone keep.
#[test]
fn needs_keeps_what_the_android_keyboard_inserts() {
    let opts = OfflineOptions::default();
    let mut differs = Vec::new();
    println!(
        "{:<15} {:<34} {:<50} {:<7} {:<15} {:<15} mark",
        "item", "package", "field", "sticker", "online", "offline"
    );
    let mut costs = Vec::new();
    for item in shapes() {
        let (mut copies_only, mut with_inserts) = (0u64, 0u64);
        for package in PACKAGES {
            let target = target_for_package(package);
            let device = phone(&[target]);
            let kept_here = kept(&item, &opts, &device, true);
            with_inserts += kept_bytes(&item, &kept_here);
            copies_only += kept_bytes(
                &item,
                &kept(
                    &item,
                    &opts,
                    &Device {
                        inserts: false,
                        ..device.clone()
                    },
                    true,
                ),
            );
            let here = only(&item, &kept_here);
            for field in FIELDS {
                let field = strings(field);
                let mut with_share = field.clone();
                with_share.extend(strings(SHARE));
                for sticker in [false, true] {
                    let online = chosen(&item, target, &field, sticker);
                    let offline = chosen(&here, target, &field, sticker);
                    let mark = [false, true]
                        .iter()
                        .any(|&s| chosen(&here, target, &with_share, s).is_some());
                    println!(
                        "{:<15} {:<34} {:<50} {:<7} {:<15} {:<15} {}",
                        item.name,
                        package,
                        field.join(","),
                        sticker,
                        name(online),
                        name(offline),
                        if mark { "available" } else { "unavailable" }
                    );
                    if online.is_some() && offline != online {
                        differs.push(format!(
                            "{} into {package} [{}] sticker={sticker}: online {}, offline {}",
                            item.name,
                            field.join(","),
                            name(online),
                            name(offline),
                        ));
                    }
                }
            }
            // The share sheet (the keyboard's fallback) too.
            for sticker in [false, true] {
                let share = strings(SHARE);
                let online = chosen(&item, target, &share, sticker);
                let offline = chosen(&here, target, &share, sticker);
                if online.is_some() && offline != online {
                    differs.push(format!(
                        "{} shared for {package} sticker={sticker}: online {}, offline {}",
                        item.name,
                        name(online),
                        name(offline)
                    ));
                }
            }
        }
        costs.push((item.name.clone(), copies_only, with_inserts));
    }
    println!("kept bytes, summed over the {} apps:", PACKAGES.len());
    for (item, copies, inserts) in &costs {
        println!("  {item:<15} copies and played only {copies:>9}, with the inserts {inserts:>9}");
    }
    let (copies, inserts) = costs.iter().fold((0, 0), |(a, b), (_, c, i)| (a + c, b + i));
    println!(
        "  all             copies and played only {copies:>9}, with the inserts {inserts:>9} (+{:.0}%)",
        (inserts as f64 / copies as f64 - 1.0) * 100.0
    );
    assert!(
        differs.is_empty(),
        "a kept sticker inserts differently offline ({}):\n  {}",
        differs.len(),
        differs.join("\n  ")
    );
}

/// The keyboard's marks: a sticker that can only be sent as a sticker
/// offline is not dimmed. A video kept with its pack under
/// `offline.max_file_mb = 1` keeps only the Telegram video sticker (the
/// other renditions are too large); "Send as a sticker" shares it, so the
/// tile is marked available (`ImeLibrary.availableOffline` asks with both
/// flags).
#[test]
fn needs_a_dimmed_tile_can_be_sent_as_a_sticker() {
    let opts = OfflineOptions {
        videos: true,
        max_file_mb: 1,
        ..OfflineOptions::default()
    };
    let video = shapes().into_iter().find(|i| i.name == "mp4 video").unwrap();
    let device = phone(&[TargetApp::Telegram]);
    let kept = kept(&video, &opts, &device, false);
    assert_eq!(kept, BTreeSet::from([Variant::TgVideoSticker]));
    let here = only(&video, &kept);
    let field = strings(&["image/gif", "image/*", "image/jpg", "image/png", "image/webp"]);
    let mut with_share = field.clone();
    with_share.extend(strings(SHARE));
    let as_sticker = chosen(&here, TargetApp::Telegram, &strings(SHARE), true);
    assert_eq!(
        as_sticker,
        Some(Variant::TgVideoSticker),
        "the share step of a sticker send"
    );
    let plain = chosen(&here, TargetApp::Telegram, &with_share, false).is_some();
    let either = plain || chosen(&here, TargetApp::Telegram, &with_share, true).is_some();
    assert!(!plain, "without the sticker flag nothing goes");
    assert!(
        either,
        "marked available: Send as a sticker shares {}",
        name(as_sticker)
    );
}

/// `INSERT_FIELDS` names each kind of field once: no two entries cover the
/// same MIME types.
#[test]
fn insert_fields_differ() {
    let probe = [
        "image/png",
        "image/gif",
        "image/webp",
        "image/jpeg",
        "video/mp4",
        "video/webm",
    ];
    let covers = |field: &[&str]| -> Vec<bool> {
        probe
            .iter()
            .map(|m| {
                field
                    .iter()
                    .any(|f| *f == *m || f.strip_suffix("/*").is_some_and(|t| m.starts_with(&format!("{t}/"))))
            })
            .collect()
    };
    let all: BTreeSet<Vec<bool>> = INSERT_FIELDS.iter().map(|f| covers(f)).collect();
    assert_eq!(all.len(), INSERT_FIELDS.len());
    for field in FIELDS {
        assert!(all.contains(&covers(field)), "{field:?} is covered by INSERT_FIELDS");
    }
}
