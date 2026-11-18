use super::*;

const MIB: u64 = 1024 * 1024;

fn info(variant: Variant, size_bytes: u64) -> VariantInfo {
    VariantInfo {
        variant,
        mime: String::new(),
        width: 512,
        height: 512,
        size_bytes,
    }
}

fn item(kind: MediaKind, format: MediaFormat, variants: &[(Variant, u64)]) -> ItemSummary {
    ItemSummary {
        id: "i1".into(),
        set_id: "s1".into(),
        set_slug: "cats".into(),
        set_name: "Cats".into(),
        owner_username: "alice".into(),
        name: "cat".into(),
        keywords: vec![],
        emojis: vec![],
        vibe: None,
        tags: vec![],
        text: String::new(),
        kind,
        format,
        width: 512,
        height: 512,
        quality: None,
        likes: 0,
        copies: 0,
        media_id: "m1".into(),
        sha256: "0".repeat(64),
        phash: "0".repeat(16),
        variants: variants.iter().map(|(v, size)| info(*v, *size)).collect(),
        updated_at: 0,
    }
}

fn types(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

/// (variant, MIME type, sticker) of the choice, for short assertions.
fn pick(
    item: &ItemSummary,
    target: TargetApp,
    accepted: &[&str],
    sticker: bool,
) -> Option<(Variant, &'static str, bool)> {
    variant_for(item, target, &types(accepted), sticker).map(|c| (c.variant.variant, c.mime_type, c.sticker))
}

fn png_still() -> ItemSummary {
    item(
        MediaKind::Still,
        MediaFormat::Png,
        &[
            (Variant::Original, 1000),
            (Variant::Thumb, 100),
            (Variant::TgSticker, 100_000),
        ],
    )
}

fn gif_animation() -> ItemSummary {
    item(
        MediaKind::Animation,
        MediaFormat::Gif,
        &[
            (Variant::Original, 2 * MIB),
            (Variant::Thumb, 100),
            (Variant::DiscordGif, MIB),
            (Variant::Mp4, 200_000),
            (Variant::TgVideoSticker, 200_000),
        ],
    )
}

fn webp_animation() -> ItemSummary {
    item(
        MediaKind::Animation,
        MediaFormat::Webp,
        &[
            (Variant::Original, 2 * MIB),
            (Variant::Thumb, 100),
            (Variant::DiscordGif, MIB),
            (Variant::Mp4, 200_000),
        ],
    )
}

fn mp4_video() -> ItemSummary {
    item(
        MediaKind::Video,
        MediaFormat::Mp4,
        &[
            (Variant::Original, 3 * MIB),
            (Variant::Thumb, 100),
            (Variant::DiscordGif, 4 * MIB),
            (Variant::TgVideoSticker, 200_000),
        ],
    )
}

// -- target_for_package ---------------------------------------------------------------------------

#[test]
fn every_known_package_maps_to_its_app() {
    use TargetApp::*;
    let expected = [
        ("org.telegram.messenger", Telegram),
        ("org.telegram.messenger.web", Telegram),
        ("org.telegram.messenger.beta", Telegram),
        ("org.thunderdog.challegram", Telegram),
        ("tw.nekomimi.nekogram", Telegram),
        ("xyz.nextalone.nagram", Telegram),
        ("com.radolyn.ayugram", Telegram),
        ("org.telegram.plus", Telegram),
        ("org.forkgram.messenger", Telegram),
        ("uz.unnarsx.cherrygram", Telegram),
        ("com.discord", Discord),
        ("org.thoughtcrime.securesms", Signal),
        ("im.molly.app", Signal),
        ("com.whatsapp", Whatsapp),
        ("com.whatsapp.w4b", Whatsapp),
        ("com.Slack", Slack),
    ];
    for (package, target) in expected {
        assert_eq!(target_for_package(package), target, "{package}");
    }
    assert_eq!(PACKAGES.len(), expected.len(), "a table entry without a test");
}

#[test]
fn package_table_has_no_duplicates_and_no_generic_rows() {
    for (i, (name, target)) in PACKAGES.iter().enumerate() {
        assert_ne!(*target, TargetApp::Generic, "{name}: Generic is the default, not a row");
        assert!(!PACKAGES[..i].iter().any(|(n, _)| n == name), "{name} twice");
    }
}

#[test]
fn unknown_and_empty_packages_are_generic() {
    for package in [
        "",
        "com.android.chrome",
        "org.mozilla.firefox",
        "com.google.android.apps.messaging",
    ] {
        assert_eq!(target_for_package(package), TargetApp::Generic, "{package:?}");
    }
}

#[test]
fn near_misses_are_generic() {
    for package in [
        "org.telegram.messengerx",
        "org.telegram.messenger.",
        "org.telegram",
        "org.telegram.messenger.web2",
        "telegram",
        "com.discord.beta",
        "com.whatsapp.w4",
        " com.discord",
        "com.discord ",
        "im.molly",
    ] {
        assert_eq!(target_for_package(package), TargetApp::Generic, "{package:?}");
    }
}

#[test]
fn package_names_are_case_sensitive() {
    assert_eq!(target_for_package("com.Slack"), TargetApp::Slack);
    for package in [
        "com.slack",
        "COM.SLACK",
        "Com.Discord",
        "org.Telegram.Messenger",
        "COM.WHATSAPP",
    ] {
        assert_eq!(target_for_package(package), TargetApp::Generic, "{package:?}");
    }
}

// -- MIME matching --------------------------------------------------------------------------------

#[test]
fn exact_mime_type_matches() {
    assert_eq!(
        pick(&png_still(), TargetApp::Generic, &["image/png"], false),
        Some((Variant::Original, "image/png", false))
    );
}

#[test]
fn type_wildcard_matches() {
    let jpeg = item(MediaKind::Still, MediaFormat::Jpeg, &[(Variant::Original, 1000)]);
    assert_eq!(
        pick(&jpeg, TargetApp::Signal, &["image/*"], false),
        Some((Variant::Original, "image/jpeg", false))
    );
    // `image/*` covers no video.
    assert_eq!(
        pick(&mp4_video(), TargetApp::Generic, &["image/*"], false).unwrap().1,
        "image/gif"
    );
    assert_eq!(pick(&mp4_video(), TargetApp::Generic, &["text/*"], false), None);
}

#[test]
fn full_wildcard_matches() {
    assert_eq!(
        pick(&mp4_video(), TargetApp::Generic, &["*/*"], false),
        Some((Variant::Original, "video/mp4", false))
    );
}

#[test]
fn case_and_parameters_are_ignored() {
    let still = png_still();
    for accepted in [
        "image/png; charset=binary",
        "IMAGE/PNG",
        "Image/Png;q=1",
        " image/png ",
        "image/*; x=y",
        "*/*;q=0.1",
    ] {
        assert_eq!(
            pick(&still, TargetApp::Generic, &[accepted], false),
            Some((Variant::Original, "image/png", false)),
            "{accepted:?}"
        );
    }
}

#[test]
fn malformed_entries_accept_nothing() {
    let still = png_still();
    for accepted in [
        "",
        "*",
        "image",
        "png",
        "*/png",
        "image/",
        "/png",
        "image/png/x",
        "image / png",
        ";image/png",
    ] {
        assert_eq!(
            pick(&still, TargetApp::Generic, &[accepted], false),
            None,
            "{accepted:?}"
        );
    }
}

#[test]
fn empty_list_gives_none() {
    for target in TargetApp::ALL {
        for sticker in [false, true] {
            assert_eq!(pick(&png_still(), target, &[], sticker), None);
            assert_eq!(pick(&gif_animation(), target, &[], sticker), None);
        }
    }
}

#[test]
fn nothing_fits_gives_none() {
    assert_eq!(pick(&png_still(), TargetApp::Telegram, &["video/mp4"], false), None);
    assert_eq!(pick(&png_still(), TargetApp::Telegram, &["image/gif"], true), None);
    assert_eq!(
        pick(
            &gif_animation(),
            TargetApp::Discord,
            &["image/png", "image/jpeg"],
            false
        ),
        None
    );
}

#[test]
fn video_with_only_png_accepted_gives_none() {
    for target in TargetApp::ALL {
        for sticker in [false, true] {
            assert_eq!(pick(&mp4_video(), target, &["image/png"], sticker), None, "{target:?}");
        }
    }
}

#[test]
fn video_falls_back_to_the_gif_for_an_image_only_field() {
    assert_eq!(
        pick(&mp4_video(), TargetApp::Discord, &["image/png", "image/gif"], false),
        Some((Variant::DiscordGif, "image/gif", false))
    );
}

// -- preferences ----------------------------------------------------------------------------------

#[test]
fn telegram_sticker_flag_picks_the_sticker_rendition() {
    let still = png_still();
    let accepted = ["image/png", "image/webp"];
    assert_eq!(
        pick(&still, TargetApp::Telegram, &accepted, true),
        Some((Variant::TgSticker, "image/webp", true))
    );
    assert_eq!(
        pick(&still, TargetApp::Telegram, &accepted, false),
        Some((Variant::Original, "image/png", false))
    );
    // Without image/webp the sticker cannot go in: the normal rendition does.
    assert_eq!(
        pick(&still, TargetApp::Telegram, &["image/png"], true),
        Some((Variant::Original, "image/png", false))
    );
    // Other apps have no stickers: the flag changes nothing.
    assert_eq!(
        pick(&still, TargetApp::Discord, &accepted, true),
        Some((Variant::Original, "image/png", false))
    );
}

#[test]
fn telegram_sticker_falls_back_without_a_sticker_variant() {
    let still = item(MediaKind::Still, MediaFormat::Png, &[(Variant::Original, 1000)]);
    assert_eq!(
        pick(&still, TargetApp::Telegram, &["image/webp", "image/png"], true),
        Some((Variant::Original, "image/png", false))
    );
}

#[test]
fn telegram_oversized_sticker_loses_to_the_normal_rendition() {
    let still = item(
        MediaKind::Still,
        MediaFormat::Png,
        &[(Variant::Original, 1000), (Variant::TgSticker, 3 * MIB)],
    );
    assert_eq!(
        pick(&still, TargetApp::Telegram, &["image/webp", "image/png"], true),
        Some((Variant::Original, "image/png", false))
    );
    // Still the only thing the field takes: inserted anyway.
    assert_eq!(
        pick(&still, TargetApp::Telegram, &["image/webp"], true),
        Some((Variant::TgSticker, "image/webp", true))
    );
}

#[test]
fn telegram_animated_sticker_is_the_video_sticker() {
    assert_eq!(
        pick(
            &gif_animation(),
            TargetApp::Telegram,
            &["video/webm", "image/gif"],
            true
        ),
        Some((Variant::TgVideoSticker, "video/webm", true))
    );
    // Not without the sticker flag: a GIF instead.
    assert_eq!(
        pick(
            &gif_animation(),
            TargetApp::Telegram,
            &["video/webm", "image/gif"],
            false
        ),
        Some((Variant::Original, "image/gif", false))
    );
}

#[test]
fn discord_animation_prefers_gif() {
    let accepted = ["video/mp4", "image/gif"];
    assert_eq!(
        pick(&gif_animation(), TargetApp::Discord, &accepted, false),
        Some((Variant::Original, "image/gif", false))
    );
    // An animated WebP original the field does not take: Discord gets the
    // GIF rendition, Telegram the MP4.
    assert_eq!(
        pick(&webp_animation(), TargetApp::Discord, &accepted, false),
        Some((Variant::DiscordGif, "image/gif", false))
    );
    assert_eq!(
        pick(&webp_animation(), TargetApp::Telegram, &accepted, false),
        Some((Variant::Mp4, "video/mp4", false))
    );
}

#[test]
fn missing_preferred_variant_falls_to_the_next() {
    // Telegram animations: original, MP4, GIF. No MP4 here, and the field
    // does not take the WebP original.
    let animation = item(
        MediaKind::Animation,
        MediaFormat::Webp,
        &[(Variant::Original, MIB), (Variant::DiscordGif, MIB)],
    );
    assert_eq!(
        pick(&animation, TargetApp::Telegram, &["video/mp4", "image/gif"], false),
        Some((Variant::DiscordGif, "image/gif", false))
    );
    // A JPEG still for a PNG-only field without a PNG rendition yet: the
    // sticker WebP is the last resort, and it does not fit either.
    let jpeg = item(MediaKind::Still, MediaFormat::Jpeg, &[(Variant::Original, 1000)]);
    assert_eq!(pick(&jpeg, TargetApp::Generic, &["image/png"], false), None);
    let jpeg = item(
        MediaKind::Still,
        MediaFormat::Jpeg,
        &[(Variant::Original, 1000), (Variant::Png, 2000)],
    );
    assert_eq!(
        pick(&jpeg, TargetApp::Generic, &["image/png"], false),
        Some((Variant::Png, "image/png", false))
    );
}

#[test]
fn the_apps_profile_breaks_ties_between_accepted_renditions() {
    // WhatsApp and Slack take no WebP still (profiles.rs): the PNG goes in
    // when the field takes both; Telegram keeps the original.
    let webp = item(
        MediaKind::Still,
        MediaFormat::Webp,
        &[(Variant::Original, 1000), (Variant::Png, 2000)],
    );
    for target in [TargetApp::Whatsapp, TargetApp::Slack] {
        assert_eq!(
            pick(&webp, target, &["image/*"], false),
            Some((Variant::Png, "image/png", false)),
            "{target:?}"
        );
    }
    assert_eq!(
        pick(&webp, TargetApp::Telegram, &["image/*"], false),
        Some((Variant::Original, "image/webp", false))
    );
    // A field that takes only WebP still gets the original.
    assert_eq!(
        pick(&webp, TargetApp::Whatsapp, &["image/webp"], false),
        Some((Variant::Original, "image/webp", false))
    );
}

#[test]
fn an_original_over_the_apps_limit_loses_to_a_smaller_rendition() {
    let big = item(
        MediaKind::Animation,
        MediaFormat::Gif,
        &[(Variant::Original, 30 * MIB), (Variant::DiscordGif, 9 * MIB)],
    );
    assert_eq!(
        pick(&big, TargetApp::Discord, &["image/gif"], false),
        Some((Variant::DiscordGif, "image/gif", false))
    );
    let only_original = item(MediaKind::Animation, MediaFormat::Gif, &[(Variant::Original, 30 * MIB)]);
    assert_eq!(
        pick(&only_original, TargetApp::Discord, &["image/gif"], false),
        Some((Variant::Original, "image/gif", false))
    );
}

#[test]
fn the_thumbnail_is_never_inserted() {
    let thumb_only = item(
        MediaKind::Still,
        MediaFormat::Png,
        &[(Variant::Original, 1000), (Variant::Thumb, 10)],
    );
    for target in TargetApp::ALL {
        assert_eq!(pick(&thumb_only, target, &["image/webp"], true), None);
    }
}

#[test]
fn the_choice_names_the_file_format() {
    let c = variant_for(&gif_animation(), TargetApp::Telegram, &types(&["video/mp4"]), false).unwrap();
    assert_eq!(c.format, MediaFormat::Mp4);
    assert_eq!(c.format.extension(), "mp4");
    assert_eq!(c.variant, info(Variant::Mp4, 200_000));
}

#[test]
fn preference_table_covers_every_combination_once() {
    for target in TargetApp::ALL {
        for kind in [MediaKind::Still, MediaKind::Animation, MediaKind::Video] {
            for sticker in [false, true] {
                let rows = PREFERENCES
                    .iter()
                    .filter(|(t, k, s, _)| *t == target && *k == kind && *s == sticker)
                    .count();
                assert_eq!(rows, 1, "{target:?} {kind:?} sticker={sticker}");
                let order = preference_order(target, kind, sticker);
                assert!(!order.is_empty());
                assert!(!order.contains(&Variant::Thumb));
                for (i, v) in order.iter().enumerate() {
                    assert!(!order[..i].contains(v), "{target:?} {kind:?} {sticker}: {v:?} twice");
                }
                if !(sticker && profile(target).sticker.is_some()) {
                    assert!(
                        !order.contains(&Variant::TgVideoSticker),
                        "{target:?} {kind:?} {sticker}"
                    );
                }
            }
        }
    }
    assert_eq!(PREFERENCES.len(), TargetApp::ALL.len() * 3 * 2);
}

/// The sticker flag is also what a tap in the Android keyboard sends with
/// "Prefer stickers" on, as "Send as a sticker" in the details always did.
/// For an app without stickers the flag changes nothing; for one with
/// stickers it only puts the sticker renditions first, the others in the
/// usual order after them, so whatever goes in without the flag can still
/// go in with it (the keyboard's offline marks count on that).
#[test]
fn the_sticker_flag_only_puts_the_sticker_renditions_first() {
    for target in TargetApp::ALL {
        for kind in [MediaKind::Still, MediaKind::Animation, MediaKind::Video] {
            let usual = preference_order(target, kind, false);
            let flagged = preference_order(target, kind, true);
            if profile(target).sticker.is_none() {
                assert_eq!(flagged, usual, "{target:?} {kind:?}");
                continue;
            }
            let stickers = flagged.iter().take_while(|v| is_sticker_rendition(**v)).count();
            assert!(stickers > 0, "{target:?} {kind:?}");
            let others = |order: &[Variant]| -> Vec<Variant> {
                order.iter().copied().filter(|v| !is_sticker_rendition(*v)).collect()
            };
            assert_eq!(others(&flagged[stickers..]), others(usual), "{target:?} {kind:?}");
            assert!(usual.iter().all(|v| flagged.contains(v)), "{target:?} {kind:?}");
        }
    }
}

// -- property: the result is always something the field listed ------------------------------------

/// Independent of `field_accepts`: lower-case everything, drop parameters.
fn listed(accepted: &[String], mime: &str) -> bool {
    accepted.iter().any(|a| {
        let a = a.split(';').next().unwrap().trim().to_ascii_lowercase();
        a == "*/*" || a == mime || (a.ends_with("/*") && a.len() > 2 && mime.starts_with(&a[..a.len() - 1]))
    })
}

fn subsets(all: &[Variant]) -> Vec<Vec<Variant>> {
    (0..1u32 << all.len())
        .map(|mask| {
            all.iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, v)| *v)
                .collect()
        })
        .collect()
}

#[test]
fn the_result_is_always_a_type_the_field_accepts() {
    use MediaFormat as F;
    use Variant as V;
    let shapes: [(MediaKind, &[MediaFormat], &[Variant]); 3] = [
        (
            MediaKind::Still,
            &[F::Png, F::Jpeg, F::Webp, F::Gif],
            &[V::Original, V::Thumb, V::Png, V::TgSticker],
        ),
        (
            MediaKind::Animation,
            &[F::Gif, F::Webp],
            &[V::Original, V::Thumb, V::DiscordGif, V::Mp4, V::TgVideoSticker],
        ),
        (
            MediaKind::Video,
            &[F::Mp4, F::Webm],
            &[V::Original, V::Thumb, V::DiscordGif, V::Mp4, V::TgVideoSticker],
        ),
    ];
    let lists: Vec<Vec<String>> = [
        &[][..],
        &["image/png"],
        &["image/gif"],
        &["image/webp"],
        &["image/jpeg"],
        &["video/mp4"],
        &["video/webm"],
        &["image/*"],
        &["video/*"],
        &["*/*"],
        &["image/gif", "image/png"],
        &["image/png", "image/gif", "image/webp"],
        &["IMAGE/GIF; foo=bar", "Video/MP4"],
        &["image/webp", "video/webm"],
        &["text/plain"],
        &["*", "image", "*/png"],
        &["image/*", "video/mp4"],
        &["application/octet-stream", "image/jpeg;q=1"],
    ]
    .iter()
    .map(|xs| types(xs))
    .collect();
    let mut checked = 0;
    let mut found = 0;
    for (kind, formats, variants) in shapes {
        for format in formats {
            for subset in subsets(variants) {
                for size in [1000, 30 * MIB] {
                    let sized: Vec<(Variant, u64)> = subset.iter().map(|v| (*v, size)).collect();
                    let it = item(kind, *format, &sized);
                    for target in TargetApp::ALL {
                        for sticker in [false, true] {
                            for accepted in &lists {
                                checked += 1;
                                let order = preference_order(target, kind, sticker);
                                let fits = |v: &VariantInfo| {
                                    order.contains(&v.variant)
                                        && listed(accepted, format_of_variant(v, *format).unwrap().mime())
                                };
                                let Some(c) = variant_for(&it, target, accepted, sticker) else {
                                    assert!(
                                        !it.variants.iter().any(fits),
                                        "None although something fits: {target:?} {kind:?} {format:?} \
                                         {subset:?} {accepted:?} sticker={sticker}"
                                    );
                                    continue;
                                };
                                found += 1;
                                let ctx = format!("{target:?} {kind:?} {format:?} {subset:?} {accepted:?} {sticker}");
                                assert!(listed(accepted, c.mime_type), "{} not listed: {ctx}", c.mime_type);
                                assert!(it.variants.contains(&c.variant), "{ctx}");
                                assert!(fits(&c.variant), "{ctx}");
                                assert_ne!(c.variant.variant, V::Thumb, "{ctx}");
                                assert_eq!(c.mime_type, c.format.mime(), "{ctx}");
                                assert_eq!(Some(c.format), format_of_variant(&c.variant, *format), "{ctx}");
                                if c.sticker {
                                    assert!(sticker && target == TargetApp::Telegram, "{ctx}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(found > 1000 && checked > found, "{found} of {checked}");
}
