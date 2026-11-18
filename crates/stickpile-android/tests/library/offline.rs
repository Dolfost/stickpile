//! Keeping stickers offline: keep and release, the estimate and the plan,
//! the run with its status, pause and cancel, clearing, the marks, and
//! inserting with the server stopped.

use std::path::Path;
use std::time::Duration;

use stickpile_android::{
    CacheCleared, KeepReason, LibraryErrorKind, OfflineEnd, OfflineOptions, OfflinePart, OfflineScope, OfflineState,
    StickpileLibrary,
};
use stickpile_api::Variant;

use super::*;

fn mimes(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn pack(items: &[ItemSummary], i: usize) -> OfflineScope {
    OfflineScope::Pack {
        pack_id: items[i].set_id.clone(),
    }
}

fn ids(items: &[ItemSummary]) -> Vec<String> {
    items.iter().map(|i| i.id.clone()).collect()
}

/// Signed in, synced, with the core's options but no automatic thumbnails
/// (so that only what a test keeps is kept).
async fn synced(server: &MockServer, dir: &Path) -> Arc<StickpileLibrary> {
    let lib = signed_in(server, dir);
    lib.set_offline_options(OfflineOptions {
        auto_thumbnails: false,
        ..OfflineOptions::default()
    });
    lib.sync_now().await.unwrap();
    lib
}

#[tokio::test]
async fn a_kept_pack_is_downloaded_and_inserts_with_the_server_stopped() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = synced(&server, dir.path()).await;
    let cats = pack(&items, 0);

    let estimate = lib.offline_estimate(cats.clone()).await.unwrap();
    assert_eq!(estimate.items, 3, "the three cats");
    assert!(estimate.totals.files >= 6, "thumbnails and renditions: {estimate:?}");
    assert_eq!(estimate.totals.present_files, 0);
    assert!(estimate.fits);
    assert_eq!(estimate.budget, Some(1024 * 1024 * 1024));

    lib.keep_offline(cats.clone()).await.unwrap();
    assert_eq!(lib.offline_keeps().unwrap(), vec![cats.clone()]);
    assert_eq!(
        lib.offline_reasons(items[0].id.clone()).unwrap(),
        vec![KeepReason::Pack]
    );
    assert_eq!(lib.offline_reasons(items[4].id.clone()).unwrap(), vec![]);
    let plan = lib.offline_plan().await.unwrap();
    assert_eq!(plan.items, 3);
    assert_eq!(plan.totals.present_files, 0);
    assert_eq!(plan.parts.len(), 1);
    assert_eq!(plan.parts[0].part, OfflinePart::Kept { scope: cats.clone() });
    assert_eq!(plan.parts[0].name.as_deref(), Some("Cats"));

    let outcome = lib.offline_run().await.unwrap();
    assert_eq!(outcome.end, OfflineEnd::Complete);
    assert!(outcome.complete, "{outcome:?}");
    assert_eq!(outcome.status.files_done, outcome.status.files_total);
    assert_eq!(outcome.status.files_total, plan.totals.files);
    assert!(!outcome.status.running);
    let status = lib.offline_status();
    assert_eq!(status.state, OfflineState::Idle);
    assert_eq!((status.files_done, status.running), (plan.totals.files, false));
    let usage = lib.offline_usage().unwrap();
    assert_eq!(usage.kept_files, plan.totals.files);
    assert_eq!(usage.kept_bytes, plan.totals.bytes);
    assert_eq!(
        lib.offline_plan().await.unwrap().totals.present_files,
        plan.totals.files
    );

    // Marks: what a share from the app would use is on the phone.
    let shared = mimes(&["image/*", "video/*"]);
    assert_eq!(
        lib.available_offline(ids(&items), String::new(), shared.clone(), false),
        vec![true, true, true, false, false]
    );
    assert!(!lib.server_unreachable());

    server.stop();
    // The kept stickers insert, the others say why they do not.
    let p = lib
        .prepare(items[0].id.clone(), "com.discord".into(), mimes(&["image/png"]), false)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&p.path).unwrap(), png_bytes(20, 20, 1));
    let gif = lib
        .prepare(items[1].id.clone(), String::new(), shared.clone(), false)
        .await
        .unwrap();
    assert_eq!(gif.mime_type, "image/gif");
    let other = lib
        .prepare(items[3].id.clone(), "com.discord".into(), mimes(&["image/png"]), false)
        .await;
    assert_eq!(kind(other), LibraryErrorKind::NotOnDevice);
    assert!(lib.server_unreachable());
    // The grid plays the kept GIF without the server.
    let motion = lib.motion(items[1].id.clone(), false).await.unwrap().expect("kept");
    assert_eq!(std::fs::read(&motion.path).unwrap(), gif_bytes(3, 2));

    // A new library (the app restarted) finds the same from the cache.
    let again = open(dir.path(), &server.url);
    assert_eq!(
        again.available_offline(ids(&items), String::new(), shared, false),
        vec![true, true, true, false, false]
    );
    assert_eq!(again.offline_keeps().unwrap(), vec![cats]);
}

#[tokio::test]
async fn offline_a_rendition_on_the_phone_that_the_field_takes_is_inserted() {
    let (mut server, items) = seeded_server();
    server
        .state()
        .add_variant(&items[0].id, Variant::TgSticker, "image/webp", webp_bytes(32, 32, 1));
    let dir = tempfile::tempdir().unwrap();
    let lib = synced(&server, dir.path()).await;
    // Sent once as a Telegram sticker: only its sticker WebP is here.
    let sticker = lib
        .prepare(
            items[0].id.clone(),
            "org.telegram.messenger".into(),
            mimes(&["image/webp", "image/png"]),
            true,
        )
        .await
        .unwrap();
    assert_eq!(sticker.variant, "tg_sticker");
    let id = vec![items[0].id.clone()];
    assert_eq!(
        lib.available_offline(
            id.clone(),
            "com.discord".into(),
            mimes(&["image/png", "image/webp"]),
            false
        ),
        vec![true]
    );
    assert_eq!(
        lib.available_offline(id.clone(), "com.discord".into(), mimes(&["image/png"]), false),
        vec![false],
        "the field takes no WebP"
    );

    server.stop();
    // Online Discord gets the original PNG; offline the WebP that is here.
    let p = lib
        .prepare(
            items[0].id.clone(),
            "com.discord".into(),
            mimes(&["image/png", "image/webp"]),
            false,
        )
        .await
        .unwrap();
    assert_eq!((p.variant.as_str(), p.mime_type.as_str()), ("tg_sticker", "image/webp"));
    assert_eq!(p.path, sticker.path);
    let none = lib
        .prepare(items[0].id.clone(), "com.discord".into(), mimes(&["image/png"]), false)
        .await;
    assert_eq!(kind(none), LibraryErrorKind::NotOnDevice);
}

#[tokio::test]
async fn release_clear_and_remove_kept_files() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = synced(&server, dir.path()).await;
    let grumpy = OfflineScope::Sticker {
        item_id: items[0].id.clone(),
    };
    lib.keep_offline(grumpy.clone()).await.unwrap();
    assert_eq!(
        lib.offline_reasons(items[0].id.clone()).unwrap(),
        vec![KeepReason::Sticker]
    );
    assert!(lib.offline_run().await.unwrap().complete);
    let kept = lib.offline_usage().unwrap();
    assert!(kept.kept_files >= 2, "the thumbnail and the original: {kept:?}");

    // "Clear cache" leaves the kept files and says how many stay.
    lib.thumbnail(items[3].id.clone()).await.unwrap();
    let cleared = lib.clear_cache().unwrap();
    assert_eq!(cleared.removed_files, 1, "the other thumbnail");
    assert_eq!(
        (cleared.kept_files, cleared.kept_bytes),
        (kept.kept_files, kept.kept_bytes)
    );
    assert_eq!(lib.offline_usage().unwrap().kept_files, kept.kept_files);

    // Released: the files stay, as ordinary cached files.
    lib.release_offline(grumpy.clone()).await.unwrap();
    assert_eq!(lib.offline_keeps().unwrap(), vec![]);
    assert_eq!(lib.offline_reasons(items[0].id.clone()).unwrap(), vec![]);
    let usage = lib.offline_usage().unwrap();
    assert_eq!((usage.kept_files, usage.other_files), (0, kept.kept_files));

    // Kept again: what is here is kept at once, nothing to download.
    lib.keep_offline(grumpy.clone()).await.unwrap();
    assert_eq!(lib.offline_usage().unwrap().kept_files, kept.kept_files);
    let downloads = requests(&server, &format!("GET /api/v1/items/{}/file/original", items[0].id));
    assert!(lib.offline_run().await.unwrap().complete);
    assert_eq!(
        requests(&server, &format!("GET /api/v1/items/{}/file/original", items[0].id)),
        downloads
    );

    // The confirmed "Remove kept files" takes them and the keeps.
    let removed = lib.remove_kept_files().unwrap();
    assert_eq!(
        removed,
        CacheCleared {
            removed_files: kept.kept_files,
            freed_bytes: kept.kept_bytes,
            kept_files: 0,
            kept_bytes: 0
        }
    );
    assert_eq!(lib.offline_keeps().unwrap(), vec![]);
    assert_eq!(lib.offline_usage().unwrap().kept_files, 0);
}

#[tokio::test]
async fn a_run_with_the_server_stopped_waits_for_the_network() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = synced(&server, dir.path()).await;
    lib.keep_offline(pack(&items, 0)).await.unwrap();
    server.stop();
    let outcome = lib.offline_run().await.unwrap();
    assert_eq!(outcome.end, OfflineEnd::WaitingForNetwork);
    assert!(!outcome.complete);
    assert_eq!(lib.offline_status().state, OfflineState::WaitingForNetwork);
    assert!(lib.offline_status().message.is_some());

    // The lists believe it away for a while; a run tries it all the same
    // (the app runs one when the platform says a network is there).
    server.restart();
    assert!(lib.server_unreachable());
    assert!(
        lib.offline_run().await.unwrap().complete,
        "goes on once the server is back"
    );
}

#[tokio::test]
async fn a_paused_run_waits_and_a_cancel_ends_it() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = synced(&server, dir.path()).await;
    lib.keep_offline(pack(&items, 0)).await.unwrap();

    let idle = lib.offline_status_change(None).await.unwrap();
    assert!(!idle.running && !idle.paused);
    lib.pause_offline();
    let paused = lib.offline_status_change(Some(idle.clone())).await.unwrap();
    assert!(paused.paused, "the pause is seen at once");

    let running = {
        let lib = lib.clone();
        tokio::spawn(async move { lib.offline_run().await })
    };
    let mut seen = paused;
    while !seen.running {
        seen = tokio::time::timeout(Duration::from_secs(10), lib.offline_status_change(Some(seen)))
            .await
            .expect("the run starts")
            .unwrap();
    }
    // Paused: nothing is downloaded while it waits.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(lib.offline_usage().unwrap().kept_files, 0);
    assert!(
        tokio::time::timeout(
            Duration::from_millis(200),
            lib.offline_status_change(Some(lib.offline_status()))
        )
        .await
        .is_err(),
        "no change while paused"
    );

    lib.cancel_offline();
    let outcome = tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("the cancel ends the run")
        .unwrap()
        .unwrap();
    assert_eq!(outcome.end, OfflineEnd::Cancelled);
    assert!(!lib.offline_status().running);

    lib.resume_offline();
    assert!(!lib.offline_status().paused);
    assert!(lib.offline_run().await.unwrap().complete, "the next run starts afresh");
}

#[tokio::test]
async fn the_options_reach_the_core_and_outlive_a_change_of_server() {
    let (server, _) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    assert_eq!(lib.offline_options(), OfflineOptions::default(), "the core's");
    let mine = OfflineOptions {
        videos: true,
        max_mb: 5,
        auto_liked: true,
        auto_used: true,
        auto_used_count: 50_000,
        mobile_data: true,
        ..OfflineOptions::default()
    };
    lib.set_offline_options(mine.clone());
    let used = OfflineOptions {
        auto_used_count: 2000,
        ..mine.clone()
    };
    assert_eq!(lib.offline_options(), used, "the count within 1 to 2000");
    assert_eq!(lib.offline_usage().unwrap().budget, Some(5 * 1024 * 1024));

    let other = MockServer::start();
    lib.set_server_url(other.url.clone()).await.unwrap();
    assert_eq!(lib.offline_options(), used, "a new server's session has them too");
}

#[tokio::test]
async fn a_sync_says_whether_it_went_through() {
    let (mut server, _) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    assert!(!lib.sync_state().synced);
    assert!(lib.sync_if_due().await.unwrap().synced, "due, and it went through");
    assert!(!lib.sync_if_due().await.unwrap().synced, "none was due");
    assert!(lib.sync_now().await.unwrap().synced);
    server.stop();
    assert_eq!(kind(lib.sync_now().await), LibraryErrorKind::Unreachable);
    assert!(!lib.sync_state().synced);
}

#[tokio::test]
async fn a_sticker_kept_before_it_is_cached_is_named_kept() {
    let (server, _) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let scope = OfflineScope::Sticker {
        item_id: "not-synced".into(),
    };
    lib.keep_offline(scope.clone()).await.unwrap();
    assert_eq!(
        lib.offline_reasons("not-synced".into()).unwrap(),
        vec![KeepReason::Sticker]
    );
    let plan = lib.offline_plan().await.unwrap();
    assert_eq!(plan.skipped.not_cached, 1);
    assert_eq!(
        lib.available_offline(vec!["not-synced".into()], String::new(), mimes(&["*/*"]), false),
        vec![false]
    );
}

// -- inserting a kept sticker offline as online (ported from the behaviour
// review, branch `review-offline-behaviour`) ------------------------------------

/// A file the mock takes for an MP4 video.
fn mp4_bytes(len: usize) -> Vec<u8> {
    let mut v = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom".to_vec();
    v.resize(len.max(v.len()), 7);
    v
}

/// A file the mock takes for a WebM video.
fn webm_bytes(len: usize) -> Vec<u8> {
    let mut v = vec![0x1A, 0x45, 0xDF, 0xA3];
    v.resize(len.max(v.len()), 9);
    v
}

/// alice's public pack `mixed`, its renditions shaped like the server's: a
/// PNG still with its Telegram sticker, and an MP4 video with its Discord
/// GIF and its video sticker (each `video_kb` long, for the size limit).
fn mixed(server: &MockServer, video_kb: usize) -> (ItemSummary, ItemSummary) {
    let mut st = server.state();
    let set = st.add_set("alice", "mixed", "Mixed", Visibility::Public);
    let still = st.add_item(&set.id, "still png", &["still"], png_bytes(20, 20, 1));
    st.add_variant(&still.id, Variant::TgSticker, "image/webp", webp_bytes(16, 16, 2));
    let video = st.add_item(&set.id, "mp4 video", &["video"], mp4_bytes(video_kb * 1024));
    let mut gif = gif_bytes(4, 5);
    gif.resize(gif.len().max(video_kb * 1024), 0);
    st.add_variant(&video.id, Variant::DiscordGif, "image/gif", gif);
    st.add_variant(&video.id, Variant::TgVideoSticker, "video/webm", webm_bytes(800));
    let summary = |id: &str| {
        let item = st.items.iter().find(|m| m.item.id == id).unwrap();
        st.summary(&item.item)
    };
    (summary(&still.id), summary(&video.id))
}

/// The keyboard's types: the field's and the share sheet's
/// (`ImeLibrary.availableOffline`).
fn keyboard(field: &[String]) -> Vec<String> {
    let mut out = field.to_vec();
    out.extend(mimes(&["image/*", "video/*"]));
    out
}

/// A video kept by itself, inserted online into Telegram's image field as
/// its Discord GIF: with the server stopped the keyboard inserts the same
/// GIF (kept for the inserts), not only the share sheet's MP4, and the
/// tile is marked available.
#[tokio::test]
async fn a_kept_video_inserts_into_an_image_field_offline_as_online() {
    let mut server = MockServer::start();
    let (_, video) = mixed(&server, 4);
    let dir = tempfile::tempdir().unwrap();
    let lib = synced(&server, dir.path()).await;
    let telegram = "org.telegram.messenger".to_string();
    let field = mimes(&["image/gif", "image/*", "image/jpg", "image/png", "image/webp"]);
    // Online: the Discord GIF; the use puts Telegram into the copy history.
    let online = lib
        .prepare(video.id.clone(), telegram.clone(), field.clone(), false)
        .await
        .unwrap();
    assert_eq!(online.variant, "discord_gif");
    lib.record_use(video.id.clone(), online.variant.clone(), telegram.clone())
        .unwrap();
    lib.keep_offline(OfflineScope::Sticker {
        item_id: video.id.clone(),
    })
    .await
    .unwrap();
    assert!(lib.offline_run().await.unwrap().complete);
    // What the insert fetched is kept: "Clear cache" leaves it.
    lib.clear_cache().unwrap();

    server.stop();
    let marked = lib.available_offline(vec![video.id.clone()], telegram.clone(), keyboard(&field), false);
    assert_eq!(marked, [true], "the keyboard marks it available");
    let shared = lib
        .prepare(
            video.id.clone(),
            telegram.clone(),
            mimes(&["image/*", "video/*"]),
            false,
        )
        .await
        .unwrap();
    assert_eq!(shared.variant, "original", "the share sheet gets the MP4");
    let inserted = lib
        .prepare(video.id.clone(), telegram.clone(), field.clone(), false)
        .await
        .unwrap();
    assert_eq!(inserted.variant, "discord_gif", "inserted into the field as online");
}

/// A still kept by itself on a phone that has not sent to Telegram yet
/// (only Generic's files are kept), inserted into Telegram as a sticker
/// with the server stopped: the kept original goes in (the sticker WebP is
/// not here), and the sticker is kept by itself.
#[tokio::test]
async fn a_kept_still_inserts_into_telegram_as_a_sticker_offline() {
    let mut server = MockServer::start();
    let (still, _) = mixed(&server, 4);
    let dir = tempfile::tempdir().unwrap();
    let lib = synced(&server, dir.path()).await;
    lib.keep_offline(OfflineScope::Sticker {
        item_id: still.id.clone(),
    })
    .await
    .unwrap();
    assert!(lib.offline_run().await.unwrap().complete);
    server.stop();
    let field = mimes(&["image/gif", "image/*", "image/jpg", "image/png", "image/webp"]);
    // Marked available either way, so a tap with "Prefer stickers" on works too.
    let id = vec![still.id.clone()];
    let telegram = "org.telegram.messenger".to_string();
    let plain = lib.available_offline(id.clone(), telegram.clone(), keyboard(&field), false);
    let as_sticker = lib.available_offline(id, telegram, keyboard(&field), true);
    assert_eq!((plain, as_sticker), (vec![true], vec![true]));
    let inserted = lib
        .prepare(still.id.clone(), "org.telegram.messenger".into(), field, true)
        .await
        .expect("the kept original goes in");
    assert_eq!(inserted.variant, "original");
    assert!(!inserted.sticker);
    assert_eq!(
        lib.offline_reasons(still.id.clone()).unwrap(),
        vec![KeepReason::Sticker]
    );
}

/// A video kept with its pack under `max_file_mb = 1`: only its video
/// sticker is small enough, so with the server stopped nothing goes into
/// the field or the share sheet without the sticker flag, and "Send as a
/// sticker" shares the video sticker. The keyboard marks the tile by both
/// flags (`ImeLibrary.availableOffline`), so it is not dimmed.
#[tokio::test]
async fn a_video_only_its_sticker_is_kept_of_sends_as_a_sticker_offline() {
    let mut server = MockServer::start();
    let (_, video) = mixed(&server, 1100);
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    lib.set_offline_options(OfflineOptions {
        auto_thumbnails: false,
        videos: true,
        max_file_mb: 1,
        ..OfflineOptions::default()
    });
    lib.sync_now().await.unwrap();
    let telegram = "org.telegram.messenger".to_string();
    // Sent to Telegram before: its files are among the device's.
    lib.record_use(video.id.clone(), "original".into(), telegram.clone())
        .unwrap();
    lib.keep_offline(OfflineScope::Pack {
        pack_id: video.set_id.clone(),
    })
    .await
    .unwrap();
    assert!(lib.offline_run().await.unwrap().complete);
    server.stop();
    let field = mimes(&["image/gif", "image/*", "image/jpg", "image/png", "image/webp"]);
    let id = vec![video.id.clone()];
    let plain = lib.available_offline(id.clone(), telegram.clone(), keyboard(&field), false);
    let as_sticker = lib.available_offline(id, telegram.clone(), keyboard(&field), true);
    assert_eq!((plain, as_sticker), (vec![false], vec![true]));
    let sent = lib
        .prepare(video.id.clone(), telegram, mimes(&["image/*", "video/*"]), true)
        .await
        .unwrap();
    assert_eq!((sent.variant.as_str(), sent.sticker), ("tg_video_sticker", true));
}
