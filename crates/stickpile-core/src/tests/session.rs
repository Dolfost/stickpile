use std::time::Duration;

use stickpile_api::{MediaKind, TargetApp, Variant};

use super::*;
use crate::session::{clip_file_stem, pick_variant};

async fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test]
async fn open_without_token_or_server() {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::open(config_for(&crate::mock::dead_url(), dir.path()))
        .await
        .unwrap();
    assert!(!session.is_logged_in());
    let out = session.search("cat", None, 5).await.unwrap();
    assert!(out.offline && out.hits.is_empty());
    assert!(session.recent(5).is_empty());
    assert!(dir.path().join("cache").join(crate::cache::DB_FILE).exists());
}

#[tokio::test]
async fn open_refuses_world_readable_token() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state/token");
    crate::token::write_token(&path, "spk_x").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let err = Session::open(config_for("http://localhost:1", dir.path()))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("chmod 600"), "{err}");
}

#[tokio::test]
async fn search_online_then_offline_fallback() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await;
    session.sync().await.unwrap();
    assert_eq!(session.cache().me().unwrap().unwrap().username, "alice");

    let online = session.search("cat", Some(TargetApp::Discord), 10).await.unwrap();
    assert!(!online.offline);
    assert_eq!(online.hits.len(), 2);

    // Slow server: the timeout falls back to the cache.
    server.state().search_delay = Some(Duration::from_secs(4));
    let slow = session.search("naahh", None, 10).await.unwrap();
    assert!(slow.offline);
    assert_eq!(slow.hits[0].item.id, items[1].id);

    // Server gone entirely.
    let url = server.url.clone();
    drop(server);
    let offline = session.search("cat", Some(TargetApp::Discord), 10).await.unwrap();
    assert!(offline.offline);
    let ids: Vec<_> = offline.hits.iter().map(|h| h.item.id.clone()).collect();
    assert!(ids.contains(&items[0].id) && ids.contains(&items[1].id), "{ids:?}");
    assert!(session.sync().await.unwrap_err().is_offline());

    // A new session on the same cache (and URL) also works offline.
    drop(session);
    let again = Session::open(config_for(&url, dir.path())).await.unwrap();
    let hits = again.search("fire", None, 10).await.unwrap();
    assert!(hits.offline);
    assert_eq!(hits.hits[0].item.id, items[3].id);
    assert_eq!(again.item_summary(&items[3].id).await.unwrap().name, "this is fine");
}

#[tokio::test]
async fn offline_index_is_rebuilt_after_sync() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await;
    session.sync().await.unwrap();
    assert!(session.search_offline("brandnew", None, 5).unwrap().hits.is_empty());
    let set_id = items[0].set_id.clone();
    server.state().add_item(&set_id, "brandnew", &[], png_bytes(4, 4, 99));
    session.sync().await.unwrap();
    assert_eq!(session.search_offline("brandnew", None, 5).unwrap().hits.len(), 1);
}

#[tokio::test]
async fn prepare_copy_materialises_named_file_with_offers() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await;
    let still = &items[0];

    let p = session.prepare_copy(still, TargetApp::Discord, false).await.unwrap();
    assert_eq!(p.variant, Variant::Original);
    assert!(!p.sticker);
    assert!(p.warning.is_none());
    assert_eq!(p.file.file_name().unwrap(), "grumpy cat.png");
    assert!(p.file.starts_with(dir.path().join("cache/clip")));
    assert_eq!(std::fs::read(&p.file).unwrap(), png_bytes(20, 20, 1));
    let mimes: Vec<_> = p.offers.iter().map(|o| o.mime.as_str()).collect();
    assert_eq!(mimes, ["image/png", "text/uri-list", "x-special/gnome-copied-files"]);
    assert_eq!(p.offers[0].data, png_bytes(20, 20, 1));
    assert!(String::from_utf8_lossy(&p.offers[1].data).contains("grumpy%20cat.png"));

    // Second copy reuses the cached file (no new download).
    let downloads = |s: &crate::mock::MockServer| s.state().requests.iter().filter(|r| r.contains("/file/")).count();
    let before = downloads(&server);
    let p2 = session.prepare_copy(still, TargetApp::Discord, false).await.unwrap();
    assert_eq!(p2.file, p.file);
    assert_eq!(downloads(&server), before);

    // Animated GIF: GIF bytes on the clipboard.
    let gif = session
        .prepare_copy(&items[1], TargetApp::Generic, false)
        .await
        .unwrap();
    assert_eq!(gif.offers[0].mime, "image/gif");
    assert_eq!(gif.offers[0].data, gif_bytes(3, 2));
    assert_eq!(gif.file.extension().unwrap(), "gif");

    // WebP still: converted to PNG data.
    let webp = session
        .prepare_copy(&items[2], TargetApp::Generic, false)
        .await
        .unwrap();
    assert_eq!(webp.offers[0].mime, "image/png");
    assert_eq!(&webp.offers[0].data[..8], b"\x89PNG\r\n\x1a\n");
}

#[tokio::test]
async fn sticker_and_incompatible_choices_warn_but_copy() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await;

    // No sticker rendition yet: falls back to a normal copy with a warning.
    let p = session
        .prepare_copy(&items[0], TargetApp::Telegram, true)
        .await
        .unwrap();
    assert!(!p.sticker);
    assert_eq!(p.variant, Variant::Original);
    assert!(p.warning.as_deref().unwrap().contains("sticker"), "{:?}", p.warning);

    server
        .state()
        .add_variant(&items[0].id, Variant::TgSticker, "image/webp", webp_bytes(32, 32, 1));
    let mut item = items[0].clone();
    item.variants = server.state().items[0].item.media.variants.clone();
    let p = session.prepare_copy(&item, TargetApp::Telegram, true).await.unwrap();
    assert!(p.sticker);
    assert_eq!(p.variant, Variant::TgSticker);
    assert!(p.warning.is_none());
    assert_eq!(p.file.file_name().unwrap(), "grumpy cat.webp");
    let mimes: Vec<_> = p.offers.iter().map(|o| o.mime.as_str()).collect();
    assert_eq!(
        mimes,
        [
            "text/uri-list",
            "x-special/gnome-copied-files",
            "application/x-sticker",
            "application/x-no-dialog"
        ],
        "sticker = file reference plus the sticker and no-dialog hints"
    );

    // WebP still to WhatsApp: no compatible variant -> original + warning.
    let p = session
        .prepare_copy(&items[2], TargetApp::Whatsapp, false)
        .await
        .unwrap();
    assert_eq!(p.variant, Variant::Original);
    assert!(p.warning.as_deref().unwrap().contains("WhatsApp"), "{:?}", p.warning);

    // Oversized video to Discord: copied anyway, with the limit in the warning.
    let set_id = items[0].set_id.clone();
    let video = server
        .state()
        .add_item(&set_id, "long clip", &[], mp4_bytes(11 * 1024 * 1024));
    assert_eq!(video.kind, MediaKind::Video);
    let p = session.prepare_copy(&video, TargetApp::Discord, false).await.unwrap();
    assert!(p.warning.as_deref().unwrap().contains("10 MiB"), "{:?}", p.warning);
    let mimes: Vec<_> = p.offers.iter().map(|o| o.mime.as_str()).collect();
    assert_eq!(mimes, ["text/uri-list", "x-special/gnome-copied-files"]);
}

#[test]
fn pick_variant_prefers_profiles_choice() {
    let (_server, items) = seeded_server();
    let (v, sticker, warn) = pick_variant(&items[1], TargetApp::Discord, false).unwrap();
    assert_eq!((v.variant, sticker, warn), (Variant::Original, false, None));
    let mut none = items[1].clone();
    none.variants.retain(|v| v.variant == Variant::Thumb);
    assert!(
        pick_variant(&none, TargetApp::Discord, false).is_err(),
        "thumb is never copied"
    );
}

#[tokio::test]
async fn record_copy_queues_flushes_and_feeds_recent() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await.with_client_name("test/1");
    for item in [&items[0], &items[1], &items[0]] {
        let p = session.prepare_copy(item, TargetApp::Discord, false).await.unwrap();
        session.record_copy(item, &p).unwrap();
    }
    let recent: Vec<String> = session.recent(10).into_iter().map(|i| i.id).collect();
    assert_eq!(
        recent,
        vec![items[0].id.clone(), items[1].id.clone()],
        "distinct, newest first"
    );
    assert!(wait_until(|| server.state().usage.len() == 3).await, "background flush");
    assert!(wait_until(|| session.pending_usage() == 0).await);
    let ev = server.state().usage[0].clone();
    assert_eq!(ev.client.as_deref(), Some("test/1"));
    assert_eq!(ev.target, Some(TargetApp::Discord));
    assert_eq!(ev.variant, Some(Variant::Original));
    assert_eq!(ev.id.len(), 26);
}

#[tokio::test]
async fn fetch_variant_materialises_the_given_rendition() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await;
    server
        .state()
        .add_variant(&items[0].id, Variant::TgSticker, "image/webp", webp_bytes(32, 32, 1));
    let mut item = items[0].clone();
    item.variants = server.state().items[0].item.media.variants.clone();

    // The sticker rendition, though prepare_copy would pick the original here.
    let sticker = session.fetch_variant(&item, Variant::TgSticker).await.unwrap();
    assert_eq!(sticker.file_name().unwrap(), "grumpy cat.webp");
    assert!(sticker.starts_with(dir.path().join("cache/clip")));
    assert_eq!(std::fs::read(&sticker).unwrap(), webp_bytes(32, 32, 1));
    // The original is the very file prepare_copy uses for it.
    let original = session.fetch_variant(&item, Variant::Original).await.unwrap();
    assert_eq!(original.file_name().unwrap(), "grumpy cat.png");
    let p = session.prepare_copy(&item, TargetApp::Discord, false).await.unwrap();
    assert_eq!(p.file, original);
    // A rendition the item does not have.
    let err = session.fetch_variant(&item, Variant::Mp4).await.unwrap_err();
    assert!(err.to_string().contains("no mp4 rendition"), "{err}");

    // Cached: no download, also with the server gone.
    let downloads = |s: &crate::mock::MockServer| s.state().requests.iter().filter(|r| r.contains("/file/")).count();
    let before = downloads(&server);
    server.stop();
    assert_eq!(session.fetch_variant(&item, Variant::TgSticker).await.unwrap(), sticker);
    assert_eq!(downloads(&server), before);
    // Not cached and the server gone: offline.
    assert!(session
        .fetch_variant(&items[1], Variant::Original)
        .await
        .unwrap_err()
        .is_offline());
}

#[tokio::test]
async fn record_use_records_target_and_variant() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await.with_client_name("test/1");
    session
        .record_use(&items[1], TargetApp::Telegram, Variant::Mp4)
        .unwrap();
    assert_eq!(session.recent(5)[0].id, items[1].id);
    assert!(wait_until(|| server.state().usage.len() == 1).await, "background flush");
    let ev = server.state().usage[0].clone();
    assert_eq!(ev.item_id, items[1].id);
    assert_eq!(ev.target, Some(TargetApp::Telegram));
    assert_eq!(ev.variant, Some(Variant::Mp4));
    assert_eq!(ev.client.as_deref(), Some("test/1"));

    // Offline: kept in the queue and in the history.
    server.stop();
    session
        .record_use(&items[2], TargetApp::Generic, Variant::Original)
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(session.pending_usage(), 1);
    assert_eq!(session.recent(5)[0].id, items[2].id);
}

#[tokio::test]
async fn copy_works_offline_from_cache_and_queues_usage() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await;
    session.sync().await.unwrap();
    let first = session
        .prepare_copy(&items[1], TargetApp::Discord, false)
        .await
        .unwrap();
    let thumb = session.thumbnail(&items[1]).await.unwrap();
    assert!(thumb.exists());
    drop(server);

    let p = session
        .prepare_copy(&items[1], TargetApp::Discord, false)
        .await
        .unwrap();
    assert_eq!(p.file, first.file);
    assert_eq!(session.thumbnail(&items[1]).await.unwrap(), thumb);
    session.record_copy(&items[1], &p).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(session.pending_usage(), 1, "kept for later");
    assert!(session.flush_usage().await.unwrap_err().is_offline());
    assert_eq!(session.recent(5)[0].id, items[1].id);
    // Not cached: offline error.
    assert!(session
        .prepare_copy(&items[0], TargetApp::Discord, false)
        .await
        .unwrap_err()
        .is_offline());
}

#[tokio::test]
async fn usage_recorded_while_down_is_sent_when_the_server_is_back() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await;
    session.sync().await.unwrap();
    let p = session
        .prepare_copy(&items[2], TargetApp::Generic, false)
        .await
        .unwrap();
    server.stop();
    session.record_copy(&items[2], &p).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(session.pending_usage(), 1);
    assert!(session.search("sleepy", None, 5).await.unwrap().offline);

    server.restart();
    let r = session.flush_usage().await.unwrap();
    assert_eq!(r.accepted, 1);
    assert_eq!(session.pending_usage(), 0);
    assert_eq!(server.state().usage[0].item_id, items[2].id);
    assert!(!session.search("sleepy", None, 5).await.unwrap().offline);
}

#[tokio::test]
async fn thumbnails_are_downloaded_once() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await;
    let a = session.thumbnail(&items[0]).await.unwrap();
    let b = session.thumbnail(&items[0]).await.unwrap();
    assert_eq!(a, b);
    assert!(a.starts_with(dir.path().join("cache/files/thumb")));
    let n = server
        .state()
        .requests
        .iter()
        .filter(|r| r.ends_with("/file/thumb"))
        .count();
    assert_eq!(n, 1);
    image::load_from_memory(&std::fs::read(&a).unwrap()).unwrap();
}

#[tokio::test]
async fn login_pairing_stores_the_token_like_a_password_login() {
    use std::os::unix::fs::PermissionsExt;
    let server = MockServer::start();
    let dir = tempfile::tempdir().unwrap();
    let session = Session::open(config_for(&server.url, dir.path())).await.unwrap();
    assert!(!session.is_logged_in());
    let code = server.state().add_pairing("alice");

    // As typed on a phone: lower case, spaces instead of dashes.
    let typed = code.to_lowercase().replace('-', " ");
    let user = session.login_pairing(&typed, "Pixel 8").await.unwrap();
    assert_eq!(user.username, "alice");
    assert!(session.is_logged_in());
    // The token file: the issued token, mode 0600.
    let path = dir.path().join("state/token");
    let stored = crate::token::read_token(&path).unwrap().expect("token stored");
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    {
        let st = server.state();
        let issued = st.tokens.iter().find(|t| t.info.name == "Pixel 8").expect("issued");
        assert_eq!(issued.secret, stored);
        assert!(st.pairings.iter().all(|p| p.used));
    }
    // The session uses it from now on, and the cache knows the account.
    assert_eq!(session.client().token().as_deref(), Some(stored.as_str()));
    assert_eq!(session.client().me().await.unwrap().username, "alice");
    assert_eq!(session.cache().me().unwrap().unwrap().username, "alice");
    // A reopened session finds it.
    let again = Session::open(config_for(&server.url, dir.path())).await.unwrap();
    assert!(again.is_logged_in());

    // A used code fails and leaves the stored token alone.
    let err = session.login_pairing(&code, "Pixel 8").await.unwrap_err();
    assert_eq!(err.status(), Some(401), "{err:?}");
    assert_eq!(crate::token::read_token(&path).unwrap(), Some(stored));
}

#[test]
fn clip_file_names_are_safe() {
    assert_eq!(clip_file_stem("grumpy cat", "id"), "grumpy cat");
    assert_eq!(clip_file_stem("../../etc/passwd", "id"), "_.._etc_passwd");
    assert_eq!(clip_file_stem("  .hidden ", "id"), "hidden");
    assert_eq!(clip_file_stem("a\nb\0c", "id"), "a_b_c");
    assert_eq!(clip_file_stem("", "abc"), "abc");
    assert_eq!(clip_file_stem("котик 😼", "id"), "котик 😼");
    let long = "é".repeat(80);
    let s = clip_file_stem(&long, "id");
    assert!(s.len() <= 100 && s.chars().all(|c| c == 'é'));
}
