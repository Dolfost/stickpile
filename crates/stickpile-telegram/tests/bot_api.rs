//! Telegram import/export against the in-process mock Bot API.

mod mock_bot;

use std::time::{Duration, Instant};

use mock_bot::{start, unique_id, BOT, BOT_ID, OWNER, TOKEN};
use stickpile_telegram::{
    export_pack, import_pack, BotApi, ExportItem, PackMapping, StickerFormat, TelegramError, DEFAULT_EMOJI,
    TGS_SKIP_REASON,
};

const PACK: &str = "cats_by_stickpile_test_bot";

fn api(base: &str) -> BotApi {
    BotApi::new(base, TOKEN).with_max_retry_wait(Duration::from_millis(20))
}

fn item(key: &str, content: &str, format: StickerFormat, emojis: &[&str], keywords: &[&str]) -> ExportItem {
    ExportItem {
        key: key.to_string(),
        content_key: content.to_string(),
        bytes: format!("bytes of {content}").into_bytes(),
        format,
        emojis: emojis.iter().map(|s| s.to_string()).collect(),
        keywords: keywords.iter().map(|s| s.to_string()).collect(),
    }
}

fn fuid(it: &ExportItem) -> String {
    unique_id(&it.bytes)
}

// ---------------------------------------------------------------------------
// Client basics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_me_and_errors() {
    let (base, _state) = start().await;
    let me = api(&base).get_me().await.unwrap();
    assert_eq!(me.id, BOT_ID);
    assert_eq!(me.username.as_deref(), Some(BOT));

    let err = api(&base)
        .get_sticker_set("nope_by_stickpile_test_bot")
        .await
        .unwrap_err();
    assert!(err.is_missing_sticker_set(), "{err}");
    assert!(
        matches!(&err, TelegramError::Api { code: 400, description, .. } if description.contains("STICKERSET_INVALID"))
    );

    let err = BotApi::new(&base, "999:wrong").get_me().await.unwrap_err();
    assert!(matches!(err, TelegramError::Api { code: 401, .. }), "{err}");
    assert!(!err.is_missing_sticker_set());
}

#[tokio::test]
async fn rate_limit_is_retried_with_capped_wait() {
    let (base, state) = start().await;
    {
        let mut st = state.lock().unwrap();
        st.rate_limit.insert("getMe".into(), 2);
        st.retry_after = 5; // capped to 20 ms by the client
    }
    let started = Instant::now();
    assert_eq!(api(&base).get_me().await.unwrap().id, BOT_ID);
    assert!(started.elapsed() < Duration::from_secs(2), "retry_after must be capped");
    assert_eq!(state.lock().unwrap().count("getMe"), 3);

    // Retries are bounded: 1 call + 3 retries, then the 429 surfaces.
    state.lock().unwrap().rate_limit.insert("getMe".into(), 10);
    let err = api(&base).get_me().await.unwrap_err();
    assert!(
        matches!(
            err,
            TelegramError::Api {
                code: 429,
                retry_after: Some(5),
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(state.lock().unwrap().count("getMe"), 3 + 4);
}

#[tokio::test]
async fn token_is_never_exposed() {
    let (base, state) = start().await;
    let client = api(&base);
    assert!(!format!("{client:?}").contains("SECRET"));

    // Connection failure: reqwest errors would normally include the URL.
    let unreachable = BotApi::new("http://127.0.0.1:1", TOKEN);
    let err = unreachable.get_me().await.unwrap_err();
    for text in [err.to_string(), format!("{err:?}")] {
        assert!(
            !text.contains("SECRET") && !text.contains(TOKEN),
            "token leaked: {text}"
        );
    }
    let err = unreachable.download_file("x", 10).await.unwrap_err();
    assert!(!format!("{err} {err:?}").contains("SECRET"));

    // Failed download from a reachable server.
    state
        .lock()
        .unwrap()
        .seed_pack("x_by_someone", "X", 1, vec![("static", b"webp".to_vec(), "😀")]);
    let file_id = state.lock().unwrap().pack("x_by_someone").stickers[0].file_id.clone();
    state.lock().unwrap().fail_downloads = true;
    let err = client.download_file(&file_id, 1000).await.unwrap_err();
    let text = format!("{err} {err:?}");
    assert!(text.contains("404") && !text.contains("SECRET"), "{text}");
}

#[tokio::test]
async fn download_file_respects_the_cap() {
    let (base, state) = start().await;
    state
        .lock()
        .unwrap()
        .seed_pack("x_by_someone", "X", 1, vec![("static", vec![7; 100], "😀")]);
    let file_id = state.lock().unwrap().pack("x_by_someone").stickers[0].file_id.clone();
    let client = api(&base);
    assert_eq!(client.download_file(&file_id, 100).await.unwrap(), vec![7; 100]);
    let err = client.download_file(&file_id, 99).await.unwrap_err();
    assert!(err.to_string().contains("larger than 99"), "{err}");
    let err = client.download_file("FILE_UNKNOWN", 100).await.unwrap_err();
    assert!(matches!(err, TelegramError::Api { code: 400, .. }), "{err}");
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[tokio::test]
async fn import_downloads_in_order_and_skips_tgs() {
    let (base, state) = start().await;
    let webp = b"RIFF\x10\0\0\0WEBPVP8 static".to_vec();
    let tgs = b"\x1f\x8b gzipped lottie".to_vec();
    let webm = b"\x1a\x45\xdf\xa3 webm".to_vec();
    state.lock().unwrap().seed_pack(
        "memes_by_someone_bot",
        "Memes",
        77,
        vec![
            ("static", webp.clone(), "😼"),
            ("animated", tgs.clone(), "🎉"),
            ("video", webm.clone(), "🙅"),
        ],
    );

    let pack = import_pack(&api(&base), "memes_by_someone_bot", 1 << 20).await.unwrap();
    assert_eq!(pack.name, "memes_by_someone_bot");
    assert_eq!(pack.title, "Memes");
    assert_eq!(pack.stickers.len(), 2);
    assert_eq!(pack.stickers[0].bytes, webp);
    assert_eq!(pack.stickers[0].format, StickerFormat::Static);
    assert_eq!(pack.stickers[0].emojis, ["😼"]);
    assert_eq!(pack.stickers[0].file_unique_id, unique_id(&webp));
    assert_eq!(pack.stickers[1].bytes, webm);
    assert_eq!(pack.stickers[1].format, StickerFormat::Video);
    assert_eq!(pack.stickers[1].emojis, ["🙅"]);
    assert_eq!(pack.skipped, vec![(unique_id(&tgs), TGS_SKIP_REASON.to_string())]);
    assert_eq!(
        TGS_SKIP_REASON,
        "animated .tgs stickers are not supported (stretch goal)"
    );
    assert_eq!(
        state.lock().unwrap().count("getFile"),
        2,
        "the .tgs file is not downloaded"
    );
}

#[tokio::test]
async fn import_skips_oversized_files_and_reports_missing_packs() {
    let (base, state) = start().await;
    state.lock().unwrap().seed_pack(
        "big_by_someone_bot",
        "Big",
        77,
        vec![("static", vec![1; 10], "1️⃣"), ("static", vec![2; 100], "2️⃣")],
    );
    let pack = import_pack(&api(&base), "big_by_someone_bot", 50).await.unwrap();
    assert_eq!(pack.stickers.len(), 1);
    assert_eq!(pack.skipped.len(), 1);
    assert!(pack.skipped[0].1.contains("larger than 50"), "{:?}", pack.skipped);

    let err = import_pack(&api(&base), "missing_by_someone_bot", 50)
        .await
        .unwrap_err();
    assert!(err.is_missing_sticker_set());
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

#[tokio::test]
async fn first_export_creates_the_pack() {
    let (base, state) = start().await;
    let items = vec![
        item("a", "A", StickerFormat::Static, &["😼", "🐱"], &["cat", "stare"]),
        item("b", "B", StickerFormat::Video, &["🙅"], &[]),
        item("c", "C", StickerFormat::Static, &[], &["sigh"]),
    ];
    let report = export_pack(&api(&base), OWNER, PACK, "Cats", &items, &PackMapping::new())
        .await
        .unwrap();
    assert!(report.created_pack);
    assert_eq!((report.added, report.removed, report.updated), (3, 0, 0));
    assert_eq!(report.mapping.len(), 3);
    for it in &items {
        assert_eq!(report.mapping[&it.key], (it.content_key.clone(), fuid(it)));
    }

    let st = state.lock().unwrap();
    assert_eq!(st.count("createNewStickerSet"), 1);
    assert_eq!(st.count("addStickerToSet"), 0);
    let pack = st.pack(PACK);
    assert_eq!(pack.title, "Cats");
    assert_eq!(pack.owner, OWNER);
    let fuids: Vec<&str> = pack.stickers.iter().map(|s| s.fuid.as_str()).collect();
    assert_eq!(fuids, items.iter().map(fuid).collect::<Vec<_>>());
    assert_eq!(pack.stickers[0].emoji_list, ["😼", "🐱"]);
    assert_eq!(pack.stickers[0].keywords, ["cat", "stare"]);
    assert_eq!(pack.stickers[0].format, "static");
    assert_eq!(pack.stickers[1].format, "video");
    assert_eq!(
        pack.stickers[2].emoji_list,
        [DEFAULT_EMOJI],
        "items without emoji get the default"
    );
}

#[tokio::test]
async fn re_export_adds_removes_replaces_reorders_and_updates() {
    let (base, state) = start().await;
    let client = api(&base);
    let a = item("a", "A", StickerFormat::Static, &["😼"], &["cat"]);
    let b = item("b", "B", StickerFormat::Static, &["🐶"], &[]);
    let c = item("c", "C", StickerFormat::Video, &["🙅"], &[]);
    let d = item("d", "D", StickerFormat::Static, &["😴"], &[]);
    let first = export_pack(
        &client,
        OWNER,
        PACK,
        "Cats",
        &[a.clone(), b.clone(), c.clone(), d.clone()],
        &PackMapping::new(),
    )
    .await
    .unwrap();

    // b removed, c's media changed, e added, a's emoji/keywords edited, new order.
    let e = item("e", "E", StickerFormat::Static, &["🆕"], &[]);
    let c2 = item("c", "C2", StickerFormat::Video, &["🙅"], &[]);
    let a2 = item("a", "A", StickerFormat::Static, &["😾", "🐱"], &["grumpy"]);
    let items = vec![e.clone(), c2.clone(), a2.clone(), d.clone()];
    state.lock().unwrap().calls.clear();
    let report = export_pack(&client, OWNER, PACK, "Cats", &items, &first.mapping)
        .await
        .unwrap();
    assert!(!report.created_pack);
    assert_eq!(report.removed, 2, "b and the old c");
    assert_eq!(report.added, 2, "e and the new c");
    assert_eq!(report.updated, 2, "a and d keep their stickers");
    assert!(report.reordered);
    for it in &items {
        assert_eq!(report.mapping[&it.key], (it.content_key.clone(), fuid(it)));
    }
    assert!(!report.mapping.contains_key("b"));
    {
        let st = state.lock().unwrap();
        assert_eq!(st.count("createNewStickerSet"), 0);
        assert_eq!(st.count("deleteStickerFromSet"), 2);
        assert_eq!(st.count("addStickerToSet"), 2);
        assert_eq!(st.count("setStickerSetTitle"), 0);
        let pack = st.pack(PACK);
        let fuids: Vec<&str> = pack.stickers.iter().map(|s| s.fuid.as_str()).collect();
        assert_eq!(
            fuids,
            items.iter().map(fuid).collect::<Vec<_>>(),
            "pack order matches the set"
        );
        assert_eq!(pack.stickers[2].emoji_list, ["😾", "🐱"]);
        assert_eq!(pack.stickers[2].keywords, ["grumpy"]);
        assert_eq!(pack.stickers[1].format, "video");
    }

    // Unchanged set: no uploads, no deletions, no moves; metadata re-sent.
    state.lock().unwrap().calls.clear();
    let again = export_pack(&client, OWNER, PACK, "Cats", &items, &report.mapping)
        .await
        .unwrap();
    assert_eq!(
        (again.added, again.removed, again.updated, again.reordered),
        (0, 0, 4, false)
    );
    assert_eq!(again.mapping, report.mapping);
    {
        let st = state.lock().unwrap();
        assert_eq!(st.count("addStickerToSet") + st.count("deleteStickerFromSet"), 0);
        assert_eq!(st.count("setStickerPositionInSet"), 0);
        assert_eq!(st.count("getStickerSet"), 1, "no re-read when nothing was uploaded");
    }

    // Title change.
    export_pack(&client, OWNER, PACK, "Cats & co", &items, &again.mapping)
        .await
        .unwrap();
    let st = state.lock().unwrap();
    assert_eq!(st.count("setStickerSetTitle"), 1);
    assert_eq!(st.pack(PACK).title, "Cats & co");
}

#[tokio::test]
async fn large_packs_are_created_in_batches() {
    let (base, state) = start().await;
    let items: Vec<ExportItem> = (0..55)
        .map(|i| item(&format!("k{i}"), &format!("C{i}"), StickerFormat::Static, &["⭐"], &[]))
        .collect();
    let report = export_pack(&api(&base), OWNER, PACK, "Many", &items, &PackMapping::new())
        .await
        .unwrap();
    assert!(report.created_pack);
    assert_eq!(report.added, 55);
    let st = state.lock().unwrap();
    assert_eq!(st.count("createNewStickerSet"), 1);
    assert_eq!(st.count("addStickerToSet"), 5);
    let fuids: Vec<&str> = st.pack(PACK).stickers.iter().map(|s| s.fuid.as_str()).collect();
    assert_eq!(fuids, items.iter().map(fuid).collect::<Vec<_>>());
}

#[tokio::test]
async fn lost_mapping_rebuilds_the_pack_without_emptying_it() {
    let (base, state) = start().await;
    let client = api(&base);
    let a = item("a", "A", StickerFormat::Static, &["🅰️"], &[]);
    let b = item("b", "B", StickerFormat::Static, &["🅱️"], &[]);
    export_pack(&client, OWNER, PACK, "AB", &[a.clone(), b.clone()], &PackMapping::new())
        .await
        .unwrap();

    // Same content, no mapping: everything is unknown and rebuilt. The last
    // old sticker (b) is identical to the first item; Telegram's dedup must
    // not lose it.
    for order in [vec![b.clone(), a.clone()], vec![a.clone(), b.clone()]] {
        let report = export_pack(&client, OWNER, PACK, "AB", &order, &PackMapping::new())
            .await
            .unwrap();
        assert_eq!(report.mapping.len(), 2);
        let st = state.lock().unwrap();
        let fuids: Vec<&str> = st.pack(PACK).stickers.iter().map(|s| s.fuid.as_str()).collect();
        assert_eq!(fuids, order.iter().map(fuid).collect::<Vec<_>>());
    }

    // A pack edited by hand (unknown sticker) is brought back in line.
    state.lock().unwrap().calls.clear();
    let mapping = export_pack(&client, OWNER, PACK, "AB", &[a.clone(), b.clone()], &PackMapping::new())
        .await
        .unwrap()
        .mapping;
    {
        let mut st = state.lock().unwrap();
        let stray = st.new_sticker(b"stray".to_vec(), "static", vec!["❓".into()], vec![]);
        st.packs.get_mut(PACK).unwrap().stickers.insert(1, stray);
    }
    let report = export_pack(&client, OWNER, PACK, "AB", &[a.clone(), b.clone()], &mapping)
        .await
        .unwrap();
    assert_eq!((report.added, report.removed), (0, 1));
    let st = state.lock().unwrap();
    assert_eq!(st.pack(PACK).stickers.len(), 2);
}

#[tokio::test]
async fn export_survives_rate_limits() {
    let (base, state) = start().await;
    {
        let mut st = state.lock().unwrap();
        st.rate_limit.insert("getStickerSet".into(), 1);
        st.rate_limit.insert("createNewStickerSet".into(), 2);
    }
    let items = vec![item("a", "A", StickerFormat::Static, &["😼"], &[])];
    let report = export_pack(&api(&base), OWNER, PACK, "Cats", &items, &PackMapping::new())
        .await
        .unwrap();
    assert!(report.created_pack);
    let st = state.lock().unwrap();
    assert_eq!(st.count("createNewStickerSet"), 3);
    assert_eq!(st.pack(PACK).stickers.len(), 1);
}

#[tokio::test]
async fn invalid_exports_make_no_calls() {
    let (base, state) = start().await;
    let client = api(&base);
    let too_many: Vec<ExportItem> = (0..121)
        .map(|i| item(&format!("k{i}"), &format!("C{i}"), StickerFormat::Static, &["⭐"], &[]))
        .collect();
    let err = export_pack(&client, OWNER, PACK, "T", &too_many, &PackMapping::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("120"), "{err}");

    let dup = vec![
        item("a", "A", StickerFormat::Static, &[], &[]),
        item("a", "B", StickerFormat::Static, &[], &[]),
    ];
    assert!(export_pack(&client, OWNER, PACK, "T", &dup, &PackMapping::new())
        .await
        .is_err());
    assert!(export_pack(&client, OWNER, PACK, "T", &[], &PackMapping::new())
        .await
        .is_err());
    assert!(state.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn identical_content_is_exported_once() {
    let (base, state) = start().await;
    let a = item("a", "A", StickerFormat::Static, &["😼"], &[]);
    let mut twin = a.clone();
    twin.key = "twin".into();
    let report = export_pack(&api(&base), OWNER, PACK, "T", &[a, twin], &PackMapping::new())
        .await
        .unwrap();
    assert_eq!(report.mapping.keys().collect::<Vec<_>>(), ["a"]);
    assert_eq!(state.lock().unwrap().pack(PACK).stickers.len(), 1);
}

#[tokio::test]
async fn export_to_a_pack_of_another_bot_fails_cleanly() {
    let (base, _state) = start().await;
    let items = vec![item("a", "A", StickerFormat::Static, &["😼"], &[])];
    let err = export_pack(
        &api(&base),
        OWNER,
        "cats_by_other_bot",
        "T",
        &items,
        &PackMapping::new(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, TelegramError::Api { code: 400, .. }), "{err}");
}
