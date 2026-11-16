//! Discord client, sync and import against the in-process mock.

mod mock_discord;

use std::time::{Duration, Instant};

use mock_discord::{gif, png, start, Shared, BOT_ID, GUILD, HUMAN_ID, TOKEN};
use stickpile_discord::{
    import_guild, sync_guild, BotToken, DiscordApi, DiscordError, ExportItem, ExpressionKind, GuildMapping,
    ImageFormat, ImportOptions, InputFile, NewSticker, StickerChanges, LOTTIE_SKIP_REASON,
};

async fn setup() -> (DiscordApi, Shared, String) {
    let (base, cdn, media, state) = start().await;
    let api = DiscordApi::with_base(&base, BotToken::new(TOKEN).unwrap())
        .with_cdn(&cdn, &media)
        .with_max_retry_wait(Duration::from_millis(20));
    (api, state, base)
}

fn item(key: &str, content: &str, name: &str, emojis: &[&str], bytes: Vec<u8>) -> ExportItem {
    ExportItem {
        key: key.into(),
        content_key: content.into(),
        name: name.into(),
        description: String::new(),
        emojis: emojis.iter().map(|s| s.to_string()).collect(),
        bytes,
    }
}

fn sticker_item(key: &str, seed: u8, name: &str) -> ExportItem {
    item(key, &format!("sha-{key}-{seed}"), name, &["😼"], png(320, 320, seed))
}

// ---------------------------------------------------------------------------
// Client basics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn token_is_never_exposed() {
    let (api, state, base) = setup().await;
    for text in [format!("{api:?}"), format!("{:?}", BotToken::new(TOKEN).unwrap())] {
        assert!(!text.contains("SECRET") && !text.contains(TOKEN), "{text}");
    }
    // The API gets the token, the CDN never does.
    api.current_user().await.unwrap();
    assert_eq!(state.lock().unwrap().auth_headers, [format!("Bot {TOKEN}")]);

    // Wrong token: Discord's 401 text, no token.
    let wrong = DiscordApi::with_base(&base, BotToken::new("other.SECRET-x").unwrap());
    let err = wrong.current_user().await.unwrap_err();
    assert_eq!(err.status(), Some(401));
    let text = format!("{err} {err:?}");
    assert!(!text.contains("SECRET"), "{text}");

    // Connection failure: no token in the error.
    let unreachable = DiscordApi::with_base("http://127.0.0.1:1/api/v10", BotToken::new(TOKEN).unwrap());
    let err = unreachable.guild(GUILD).await.unwrap_err();
    let text = format!("{err} {err:?}");
    assert!(!text.contains("SECRET") && !text.contains(TOKEN), "{text}");

    // Token files: errors name the file, never its content.
    let dir = std::env::temp_dir().join(format!("stickpile-discord-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("token");
    std::fs::write(&path, "SECRET with spaces\n").unwrap();
    let err = BotToken::from_file(&path).unwrap_err().to_string();
    assert!(err.contains("token") && !err.contains("SECRET"), "{err}");
    std::fs::write(&path, format!("{TOKEN}\n")).unwrap();
    let token = BotToken::from_file(&path).unwrap();
    assert!(!format!("{token:?}").contains("SECRET"));
    let err = BotToken::from_file(&dir.join("missing")).unwrap_err().to_string();
    assert!(err.contains("missing"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn rate_limits_are_retried_with_capped_waits() {
    let (api, state, _) = setup().await;
    {
        let mut st = state.lock().unwrap();
        st.rate_limit.insert("GET /guilds".into(), 2);
        st.retry_after = 5.0; // capped to 20 ms by the client
    }
    let started = Instant::now();
    assert_eq!(api.guild(GUILD).await.unwrap().name, "Cat Lounge");
    assert!(started.elapsed() < Duration::from_secs(2), "retry_after must be capped");
    assert_eq!(state.lock().unwrap().count("GET /guilds"), 3);

    // Bounded: 1 call + 3 retries, then the 429 surfaces with retry_after.
    state.lock().unwrap().rate_limit.insert("GET /guilds".into(), 10);
    let err = api.guild(GUILD).await.unwrap_err();
    assert!(
        matches!(err, DiscordError::Api { status: 429, retry_after: Some(r), .. } if r == 5.0),
        "{err:?}"
    );
    assert_eq!(state.lock().unwrap().count("GET /guilds"), 3 + 4);
}

#[tokio::test]
async fn rate_limit_headers_hold_back_the_next_request() {
    let (base, cdn, media, state) = start().await;
    let api = DiscordApi::with_base(&base, BotToken::new(TOKEN).unwrap()).with_cdn(&cdn, &media);
    state.lock().unwrap().exhaust.insert("GET /stickers".into(), 0.3);
    api.list_stickers(GUILD).await.unwrap();
    api.list_stickers(GUILD).await.unwrap();
    let st = state.lock().unwrap();
    let times: Vec<Instant> = st
        .call_times
        .iter()
        .filter(|(r, _)| r == "GET /stickers")
        .map(|(_, t)| *t)
        .collect();
    assert_eq!(times.len(), 2);
    assert!(
        times[1] - times[0] >= Duration::from_millis(250),
        "the second request must wait for X-RateLimit-Reset-After"
    );
}

#[tokio::test]
async fn a_global_rate_limit_holds_back_every_route() {
    let (base, cdn, media, state) = start().await;
    let api = DiscordApi::with_base(&base, BotToken::new(TOKEN).unwrap()).with_cdn(&cdn, &media);
    {
        let mut st = state.lock().unwrap();
        st.rate_limit.insert("GET /users/@me".into(), 1);
        st.retry_after = 0.3;
        st.global = true;
    }
    let started = Instant::now();
    api.current_user().await.unwrap();
    api.guild(GUILD).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(250));
    assert_eq!(state.lock().unwrap().count("GET /users/@me"), 2);
}

// ---------------------------------------------------------------------------
// Stickers and emoji
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stickers_are_created_updated_and_deleted() {
    let (api, state, _) = setup().await;
    let new = NewSticker {
        name: "grumpy".into(),
        description: "a grumpy cat".into(),
        tags: "😾".into(),
        file: InputFile {
            bytes: png(320, 320, 1),
            format: ImageFormat::Png,
        },
    };
    let s = api.create_sticker(GUILD, &new).await.unwrap();
    assert_eq!((s.name.as_str(), s.tags.as_str(), s.format_type), ("grumpy", "😾", 1));
    assert_eq!(s.user.as_ref().map(|u| u.id.as_str()), Some(BOT_ID));
    assert_eq!(state.lock().unwrap().sticker(&s.id).unwrap().bytes, png(320, 320, 1));

    let changed = api
        .modify_sticker(
            GUILD,
            &s.id,
            &StickerChanges {
                name: Some("grumpier".into()),
                description: Some(String::new()),
                tags: Some("😼".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(changed.name, "grumpier");
    assert_eq!(changed.description, None);
    assert_eq!(changed.tags, "😼");

    assert_eq!(api.list_stickers(GUILD).await.unwrap().len(), 1);
    api.delete_sticker(GUILD, &s.id).await.unwrap();
    assert!(api.list_stickers(GUILD).await.unwrap().is_empty());
    let err = api.delete_sticker(GUILD, &s.id).await.unwrap_err();
    assert_eq!((err.status(), err.code()), (Some(404), Some(10060)));
    assert_eq!(state.lock().unwrap().missing_audit_reason, 0);

    // Discord's per-field errors reach the message.
    let mut bad = new.clone();
    bad.name = "x".into();
    let err = api.create_sticker(GUILD, &bad).await.unwrap_err();
    assert_eq!(err.code(), Some(50035));
    assert!(
        err.to_string().contains("name: Must be between 2 and 30 in length."),
        "{err}"
    );
    // Ids are checked before they reach a URL.
    assert!(matches!(
        api.delete_sticker(GUILD, "../emojis").await,
        Err(DiscordError::Invalid(_))
    ));
}

#[tokio::test]
async fn emojis_are_created_renamed_and_deleted() {
    let (api, state, _) = setup().await;
    let still = InputFile {
        bytes: png(128, 128, 1),
        format: ImageFormat::Png,
    };
    let e = api.create_emoji(GUILD, "grumpy_cat", &still).await.unwrap();
    assert_eq!(e.name, "grumpy_cat");
    assert!(!e.animated);
    let anim = InputFile {
        bytes: gif(128, 128, 3, 2),
        format: ImageFormat::Gif,
    };
    let a = api.create_emoji(GUILD, "party", &anim).await.unwrap();
    assert!(a.animated);
    assert_eq!(state.lock().unwrap().emoji(&a.id).unwrap().bytes, gif(128, 128, 3, 2));

    let renamed = api.modify_emoji(GUILD, &e.id, "grumpier").await.unwrap();
    assert_eq!(renamed.name, "grumpier");
    assert_eq!(api.list_emojis(GUILD).await.unwrap().len(), 2);
    api.delete_emoji(GUILD, &e.id).await.unwrap();
    let left = api.list_emojis(GUILD).await.unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, a.id);

    let err = api.create_emoji(GUILD, "bad-name", &still).await.unwrap_err();
    assert_eq!(err.status(), Some(400));
    let err = api.guild("100000000000000999").await.unwrap_err();
    assert_eq!((err.status(), err.code()), (Some(404), Some(10004)));
}

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sticker_sync_adds_keeps_updates_and_removes() {
    let (api, state, _) = setup().await;
    let a = sticker_item("a", 1, "grumpy cat");
    let b = sticker_item("b", 2, "sleepy cat");
    let c = sticker_item("c", 3, "x"); // too short: the fallback name is used
    let first = sync_guild(
        &api,
        GUILD,
        ExpressionKind::Sticker,
        &[a.clone(), b.clone(), c.clone()],
        &GuildMapping::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        (first.added, first.removed, first.updated, first.unchanged),
        (3, 0, 0, 0)
    );
    assert!(first.skipped.is_empty(), "{:?}", first.skipped);
    assert_eq!(first.slots.used, 3);
    assert_eq!(first.slots.limit, 5);
    assert_eq!(first.guild_name, "Cat Lounge");
    assert_eq!(
        first.url.as_deref(),
        Some(&*format!("https://discord.com/channels/{GUILD}"))
    );
    {
        let st = state.lock().unwrap();
        let names: Vec<&str> = st.stickers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["grumpy cat", "sleepy cat", "sticker 3"]);
        assert!(st.stickers.iter().all(|s| s.tags == "😼"));
        for it in [&a, &b, &c] {
            let (content, id) = &first.mapping[&it.key];
            assert_eq!(content, &it.content_key);
            assert_eq!(st.sticker(id).unwrap().bytes, it.bytes);
        }
    }

    // a renamed with new emoji, b's picture changed, c removed, d added.
    let mut a2 = a.clone();
    a2.name = "grumpier cat".into();
    a2.emojis = vec!["word".into(), "😾".into()];
    a2.description = "very grumpy".into();
    let b2 = sticker_item("b", 9, "sleepy cat");
    let d = sticker_item("d", 4, "new cat");
    state.lock().unwrap().calls.clear();
    let second = sync_guild(
        &api,
        GUILD,
        ExpressionKind::Sticker,
        &[a2.clone(), b2.clone(), d.clone()],
        &first.mapping,
    )
    .await
    .unwrap();
    assert_eq!(second.added, 2, "b's new picture and d");
    assert_eq!(second.removed, 2, "b's old picture and c");
    assert_eq!((second.updated, second.unchanged), (1, 0));
    assert_eq!(second.mapping.keys().collect::<Vec<_>>(), ["a", "b", "d"]);
    assert_eq!(second.mapping["a"].1, first.mapping["a"].1, "a keeps its sticker");
    assert_ne!(second.mapping["b"].1, first.mapping["b"].1);
    {
        let st = state.lock().unwrap();
        let s = st.sticker(&second.mapping["a"].1).unwrap();
        assert_eq!(s.name, "grumpier cat");
        assert_eq!(s.tags, "😾");
        assert_eq!(s.description.as_deref(), Some("very grumpy"));
        assert_eq!(st.sticker(&second.mapping["b"].1).unwrap().bytes, b2.bytes);
        assert!(st.sticker(&first.mapping["c"].1).is_none());
        assert_eq!(st.count("DELETE /stickers"), 2);
        assert_eq!(st.count("PATCH /stickers"), 1);
        // Deletions run before uploads, so a full guild still has room.
        let first_post = st.calls.iter().position(|c| c == "POST /stickers").unwrap();
        let last_delete = st.calls.iter().rposition(|c| c == "DELETE /stickers").unwrap();
        assert!(last_delete < first_post);
    }

    // Nothing changed: no writes.
    state.lock().unwrap().calls.clear();
    let third = sync_guild(&api, GUILD, ExpressionKind::Sticker, &[a2, b2, d], &second.mapping)
        .await
        .unwrap();
    assert_eq!(
        (third.added, third.removed, third.updated, third.unchanged),
        (0, 0, 0, 3)
    );
    assert_eq!(third.mapping, second.mapping);
    let st = state.lock().unwrap();
    assert!(st.calls.iter().all(|c| c.starts_with("GET ")), "{:?}", st.calls);
}

#[tokio::test]
async fn sync_respects_slots_and_never_touches_foreign_stickers() {
    let (api, state, _) = setup().await;
    let (human, stray) = {
        let mut st = state.lock().unwrap();
        let human = st.seed_sticker("by a person", "🙂", 1, png(320, 320, 200), HUMAN_ID);
        // Made by this bot but unknown to the mapping (another set, or a
        // lost mapping): still left alone.
        let stray = st.seed_sticker("other set", "🙂", 1, png(320, 320, 201), BOT_ID);
        (human, stray)
    };
    let items: Vec<ExportItem> = (1..=5)
        .map(|i| sticker_item(&format!("k{i}"), i, &format!("cat {i}")))
        .collect();
    let report = sync_guild(&api, GUILD, ExpressionKind::Sticker, &items, &GuildMapping::new())
        .await
        .unwrap();
    assert_eq!(report.added, 3, "5 slots, 2 taken");
    assert_eq!(report.skipped.len(), 2);
    for s in &report.skipped {
        assert!(
            s.reason.contains("no free sticker slot") && s.reason.contains("all 5"),
            "{s:?}"
        );
    }
    assert_eq!(
        report.skipped.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(),
        ["k4", "k5"],
        "items are added in set order"
    );
    assert_eq!(report.slots.used, 5);
    assert_eq!(report.mapping.len(), 3);

    // Removing every item deletes only what stickpile made.
    let emptied = sync_guild(&api, GUILD, ExpressionKind::Sticker, &[], &report.mapping)
        .await
        .unwrap();
    assert_eq!(emptied.removed, 3);
    assert!(emptied.mapping.is_empty());
    let st = state.lock().unwrap();
    let ids: Vec<&str> = st.stickers.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, [human.as_str(), stray.as_str()]);
}

#[tokio::test]
async fn boosted_guilds_get_more_slots() {
    let (api, state, _) = setup().await;
    state.lock().unwrap().premium_tier = 1;
    let items: Vec<ExportItem> = (1..=16)
        .map(|i| sticker_item(&format!("k{i}"), i as u8, &format!("cat {i}")))
        .collect();
    let report = sync_guild(&api, GUILD, ExpressionKind::Sticker, &items, &GuildMapping::new())
        .await
        .unwrap();
    assert_eq!((report.added, report.skipped.len()), (15, 1));
    assert_eq!(report.premium_tier, 1);
    assert!(report.skipped[0].reason.contains("level 1"), "{:?}", report.skipped);
}

#[tokio::test]
async fn sync_skips_unusable_files_and_duplicates() {
    let (api, state, _) = setup().await;
    let good = sticker_item("good", 1, "good cat");
    let mut twin = good.clone();
    twin.key = "twin".into();
    let big = item("big", "big", "huge cat", &[], png(512, 512, 2));
    let webp = item(
        "webp",
        "webp",
        "webp cat",
        &[],
        b"RIFF\0\0\0\0WEBPVP8L\x05\0\0\0\x2f\0\0\0\0".to_vec(),
    );
    let long = item("long", "long", "slow cat", &[], {
        // A 6 s APNG.
        let mut apng = png(320, 320, 3);
        let iend = apng.len() - 12;
        let mut actl = b"\0\0\0\x08acTL".to_vec();
        actl.extend(6u32.to_be_bytes());
        actl.extend(0u32.to_be_bytes());
        actl.extend([0; 4]);
        for i in 0..6u32 {
            actl.extend(b"\0\0\0\x1afcTL");
            actl.extend(i.to_be_bytes());
            actl.extend(320u32.to_be_bytes());
            actl.extend(320u32.to_be_bytes());
            actl.extend([0; 8]);
            actl.extend(1u16.to_be_bytes());
            actl.extend(1u16.to_be_bytes());
            actl.extend([0, 0, 0, 0, 0, 0]);
        }
        apng.splice(iend..iend, actl);
        apng
    });
    let report = sync_guild(
        &api,
        GUILD,
        ExpressionKind::Sticker,
        &[good, twin, big, webp, long],
        &GuildMapping::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.added, 1);
    let reasons: Vec<(&str, &str)> = report
        .skipped
        .iter()
        .map(|s| (s.key.as_str(), s.reason.as_str()))
        .collect();
    assert_eq!(reasons.len(), 4, "{reasons:?}");
    assert!(
        reasons[0].0 == "twin" && reasons[0].1.contains("same picture"),
        "{reasons:?}"
    );
    assert!(reasons[1].0 == "big" && reasons[1].1.contains("320x320"), "{reasons:?}");
    assert!(
        reasons[2].0 == "webp" && reasons[2].1.contains("not WebP"),
        "{reasons:?}"
    );
    assert!(reasons[3].0 == "long" && reasons[3].1.contains("5 s"), "{reasons:?}");
    assert_eq!(state.lock().unwrap().count("POST /stickers"), 1);
}

#[tokio::test]
async fn emoji_sync_counts_static_and_animated_slots_and_avoids_name_clashes() {
    let (api, state, _) = setup().await;
    {
        let mut st = state.lock().unwrap();
        // 49 static emoji made by people: one static slot left.
        for i in 0..49 {
            st.seed_emoji(&format!("human_{i}"), false, png(128, 128, i), HUMAN_ID);
        }
        st.seed_emoji("grumpy", false, png(128, 128, 99), HUMAN_ID);
        st.emojis.remove(0); // back to 49 with "grumpy" among them
    }
    let items = vec![
        item("s1", "s1", "Grumpy!", &[], png(128, 128, 1)),
        item("s2", "s2", "second", &[], png(128, 128, 2)),
        item("a1", "a1", "party", &[], gif(128, 128, 3, 3)),
        item("a2", "a2", "party", &[], gif(128, 128, 3, 4)),
    ];
    let report = sync_guild(&api, GUILD, ExpressionKind::Emoji, &items, &GuildMapping::new())
        .await
        .unwrap();
    assert_eq!(report.added, 3, "{:?}", report.skipped);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].key, "s2");
    assert!(
        report.skipped[0].reason.contains("no free emoji slot"),
        "{:?}",
        report.skipped
    );
    assert_eq!(report.slots.used, 50);
    assert_eq!(report.animated_slots.unwrap().used, 2);
    let st = state.lock().unwrap();
    let name = |key: &str| st.emoji(&report.mapping[key].1).unwrap().name.clone();
    assert_eq!(name("s1"), "Grumpy_2", "a person's 'grumpy' keeps its name");
    assert_eq!(name("a1"), "party");
    assert_eq!(name("a2"), "party_2");
    assert!(st.emoji(&report.mapping["a1"].1).unwrap().animated);
}

#[tokio::test]
async fn emoji_sync_renames_and_replaces() {
    let (api, state, _) = setup().await;
    let a = item("a", "a1", "cat", &[], png(128, 128, 1));
    let b = item("b", "b1", "dog", &[], png(128, 128, 2));
    let first = sync_guild(&api, GUILD, ExpressionKind::Emoji, &[a, b], &GuildMapping::new())
        .await
        .unwrap();
    let a2 = item("a", "a1", "kitty", &[], png(128, 128, 1));
    let b2 = item("b", "b2", "dog", &[], gif(128, 128, 2, 5));
    let second = sync_guild(&api, GUILD, ExpressionKind::Emoji, &[a2, b2], &first.mapping)
        .await
        .unwrap();
    assert_eq!((second.added, second.removed, second.updated), (1, 1, 1));
    let st = state.lock().unwrap();
    assert_eq!(st.emoji(&second.mapping["a"].1).unwrap().name, "kitty");
    assert_eq!(second.mapping["a"].1, first.mapping["a"].1);
    let b = st.emoji(&second.mapping["b"].1).unwrap();
    assert!(b.animated && b.name == "dog");
    assert_eq!(st.emojis.len(), 2);
}

#[tokio::test]
async fn expressions_deleted_by_hand_are_uploaded_again() {
    let (api, state, _) = setup().await;
    let a = sticker_item("a", 1, "cat");
    let first = sync_guild(
        &api,
        GUILD,
        ExpressionKind::Sticker,
        std::slice::from_ref(&a),
        &GuildMapping::new(),
    )
    .await
    .unwrap();
    state.lock().unwrap().stickers.clear();
    let second = sync_guild(&api, GUILD, ExpressionKind::Sticker, &[a], &first.mapping)
        .await
        .unwrap();
    assert_eq!((second.added, second.removed), (1, 0));
    assert_ne!(second.mapping["a"].1, first.mapping["a"].1);
    assert_eq!(state.lock().unwrap().stickers.len(), 1);
}

#[tokio::test]
async fn a_damaged_mapping_never_shares_one_sticker_between_items() {
    let (api, state, _) = setup().await;
    let a = sticker_item("a", 1, "cat");
    let first = sync_guild(
        &api,
        GUILD,
        ExpressionKind::Sticker,
        std::slice::from_ref(&a),
        &GuildMapping::new(),
    )
    .await
    .unwrap();
    // b claims a's sticker with b's own content key.
    let b = sticker_item("b", 2, "dog");
    let mut damaged = first.mapping.clone();
    damaged.insert("b".into(), (b.content_key.clone(), first.mapping["a"].1.clone()));
    let report = sync_guild(&api, GUILD, ExpressionKind::Sticker, &[a, b.clone()], &damaged)
        .await
        .unwrap();
    assert_eq!((report.added, report.removed, report.unchanged), (1, 0, 1));
    assert_eq!(report.mapping["a"], first.mapping["a"]);
    assert_ne!(report.mapping["b"].1, first.mapping["a"].1);
    let st = state.lock().unwrap();
    assert_eq!(st.stickers.len(), 2);
    assert_eq!(st.sticker(&report.mapping["b"].1).unwrap().bytes, b.bytes);
}

#[tokio::test]
async fn a_failed_sync_returns_what_it_created() {
    let (api, state, _) = setup().await;
    let items: Vec<ExportItem> = (1..=3)
        .map(|i| sticker_item(&format!("k{i}"), i, &format!("cat {i}")))
        .collect();
    // Two uploads succeed, the third fails with a server error.
    state
        .lock()
        .unwrap()
        .fail_after
        .insert("POST /stickers".into(), (500, 2));
    let err = sync_guild(&api, GUILD, ExpressionKind::Sticker, &items, &GuildMapping::new())
        .await
        .unwrap_err();
    assert_eq!(err.error.status(), Some(500));
    assert!(err.to_string().contains("500"), "{err}");
    let mapping = err.partial.mapping.clone();
    assert_eq!(
        mapping.keys().collect::<Vec<_>>(),
        ["k1", "k2"],
        "the stickers that were created stay mapped"
    );
    assert_eq!(err.partial.added, 2);
    {
        let st = state.lock().unwrap();
        assert_eq!(st.stickers.len(), 2);
        for (key, (_, id)) in &mapping {
            assert!(st.sticker(id).is_some(), "{key}");
        }
    }

    // The next sync picks up from the partial mapping: only k3 is added.
    let report = sync_guild(&api, GUILD, ExpressionKind::Sticker, &items, &mapping)
        .await
        .unwrap();
    assert_eq!((report.added, report.removed, report.unchanged), (1, 0, 2));

    // A failing delete leaves the mapping as it was.
    state.lock().unwrap().fail.insert("DELETE /stickers".into(), (403, 1));
    let err = sync_guild(&api, GUILD, ExpressionKind::Sticker, &items[..1], &report.mapping)
        .await
        .unwrap_err();
    assert_eq!(err.error.status(), Some(403));
    assert_eq!(err.partial.mapping, report.mapping);

    // Duplicate keys: refused before any call.
    state.lock().unwrap().calls.clear();
    let dup = [items[0].clone(), items[0].clone()];
    let err = sync_guild(&api, GUILD, ExpressionKind::Sticker, &dup, &report.mapping)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("duplicate"), "{err}");
    assert_eq!(err.partial.mapping, report.mapping);
    assert!(state.lock().unwrap().calls.is_empty());
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[tokio::test]
async fn import_downloads_stickers_and_emojis_from_the_cdn() {
    let (api, state, _) = setup().await;
    let (png_id, gif_id, lottie_id, bot_id, static_id, anim_id) = {
        let mut st = state.lock().unwrap();
        let png_id = st.seed_sticker("wave", "👋, hello, wave", 1, png(320, 320, 1), HUMAN_ID);
        let gif_id = st.seed_sticker("dance", "💃", 4, gif(320, 320, 4, 2), HUMAN_ID);
        let lottie_id = st.seed_sticker("lottie", "🎉", 3, b"{\"v\":\"5\"}".to_vec(), HUMAN_ID);
        let bot_id = st.seed_sticker("mine", "😼", 1, png(320, 320, 3), BOT_ID);
        let static_id = st.seed_emoji("blob", false, png(128, 128, 4), HUMAN_ID);
        let anim_id = st.seed_emoji("party", true, gif(128, 128, 2, 5), BOT_ID);
        (png_id, gif_id, lottie_id, bot_id, static_id, anim_id)
    };
    let pack = import_guild(
        &api,
        GUILD,
        &ImportOptions {
            stickers: true,
            emojis: true,
            max_file_bytes: 1 << 20,
        },
    )
    .await
    .unwrap();
    assert_eq!(pack.guild_name, "Cat Lounge");
    let ids: Vec<&str> = pack.items.iter().map(|i| i.remote_id.as_str()).collect();
    assert_eq!(ids, [&png_id, &gif_id, &bot_id, &static_id, &anim_id]);
    assert_eq!(
        pack.skipped,
        vec![(ExpressionKind::Sticker, lottie_id, LOTTIE_SKIP_REASON.to_string())]
    );
    let wave = &pack.items[0];
    assert_eq!(wave.kind, ExpressionKind::Sticker);
    assert_eq!(wave.emojis, ["👋"]);
    assert_eq!(wave.keywords, ["hello", "wave"]);
    assert_eq!(wave.format, Some(ImageFormat::Png));
    assert_eq!(wave.bytes, png(320, 320, 1));
    assert!(!wave.created_by_bot);
    let dance = &pack.items[1];
    assert!(dance.animated);
    assert_eq!(dance.format, Some(ImageFormat::Gif));
    assert!(pack.items[2].created_by_bot);
    let blob = &pack.items[3];
    assert_eq!((blob.kind, blob.name.as_str()), (ExpressionKind::Emoji, "blob"));
    assert_eq!(blob.bytes, png(128, 128, 4));
    let party = &pack.items[4];
    assert!(party.animated && party.created_by_bot);

    let st = state.lock().unwrap();
    assert!(!st.cdn_saw_auth, "CDN downloads carry no token");
    assert!(st.cdn_calls.contains(&format!("/media/stickers/{gif_id}.gif")));
    assert!(st.cdn_calls.contains(&format!("/cdn/emojis/{anim_id}.webp")));
    assert!(st.cdn_calls.contains(&format!("/cdn/emojis/{static_id}.png")));
}

#[tokio::test]
async fn import_skips_large_files_and_can_pick_one_kind() {
    let (api, state, _) = setup().await;
    {
        let mut st = state.lock().unwrap();
        st.seed_sticker("small", "🙂", 1, png(320, 320, 1), HUMAN_ID);
        let mut big = png(320, 320, 2);
        big.resize(5000, 0);
        st.seed_sticker("big", "🙂", 1, big, HUMAN_ID);
        st.seed_emoji("blob", false, png(128, 128, 4), HUMAN_ID);
    }
    let only_stickers = import_guild(
        &api,
        GUILD,
        &ImportOptions {
            stickers: true,
            emojis: false,
            max_file_bytes: 1000,
        },
    )
    .await
    .unwrap();
    assert_eq!(only_stickers.items.len(), 1);
    assert_eq!(only_stickers.skipped.len(), 1);
    assert!(only_stickers.skipped[0].2.contains("larger than 1000"));

    let only_emojis = import_guild(
        &api,
        GUILD,
        &ImportOptions {
            stickers: false,
            emojis: true,
            max_file_bytes: 1000,
        },
    )
    .await
    .unwrap();
    assert_eq!(only_emojis.items.len(), 1);
    assert_eq!(only_emojis.items[0].kind, ExpressionKind::Emoji);
}
