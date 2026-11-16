//! Thumbnails, `motion` and `prepare`.

use std::path::Path;

use stickpile_android::{LibraryErrorKind, MotionKind, Prepared, Target};
use stickpile_api::Variant;

use super::*;

fn mimes(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn thumbnails_are_cached_and_work_offline() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());

    // Not shown before: the item is looked up on the server.
    let path = lib.thumbnail(items[0].id.clone()).await.unwrap();
    assert!(Path::new(&path).starts_with(dir.path().join("cache")));
    image::load_from_memory(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(lib.thumbnail(items[0].id.clone()).await.unwrap(), path);
    assert_eq!(
        requests(&server, &format!("GET /api/v1/items/{}/file/thumb", items[0].id)),
        1
    );
    assert_eq!(kind(lib.thumbnail("nope".into()).await), LibraryErrorKind::NotFound);

    lib.sync_now().await.unwrap();
    server.stop();
    assert_eq!(lib.thumbnail(items[0].id.clone()).await.unwrap(), path);
    // A new library knows the item from the cache only.
    let again = open(dir.path(), &server.url);
    assert_eq!(again.thumbnail(items[0].id.clone()).await.unwrap(), path);
    assert_eq!(
        kind(again.thumbnail(items[1].id.clone()).await),
        LibraryErrorKind::Unreachable,
        "not cached"
    );

    server.restart();
    let out = tempfile::tempdir().unwrap();
    let lib = open(out.path(), &server.url);
    assert_eq!(
        kind(lib.thumbnail(items[0].id.clone()).await),
        LibraryErrorKind::NotSignedIn
    );
}

#[tokio::test]
async fn pack_covers_are_the_cover_image_else_the_cover_item() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let cats = items[0].set_id.clone();
    let memes = items[3].set_id.clone();
    assert_eq!(lib.pack_cover(cats.clone()).await.unwrap(), None, "not listed yet");

    server.state().set_cover_image(&memes, png_bytes(30, 30, 9));
    let listed = lib.packs().await.unwrap();
    assert!(listed.packs.iter().any(|p| p.id == memes));
    let first = lib.pack_cover(cats.clone()).await.unwrap().expect("a cover");
    assert_eq!(
        first,
        lib.thumbnail(items[0].id.clone()).await.unwrap(),
        "the first item's thumbnail"
    );
    let own = lib.pack_cover(memes.clone()).await.unwrap().expect("a cover");
    assert_ne!(own, lib.thumbnail(items[3].id.clone()).await.unwrap(), "its own image");
    let img = image::load_from_memory(&std::fs::read(&own).unwrap()).unwrap();
    assert_eq!((img.width(), img.height()), (30, 30));
    assert_eq!(
        lib.pack_cover(memes.clone()).await.unwrap().as_deref(),
        Some(own.as_str()),
        "cached"
    );

    // Offline the cached files answer.
    lib.sync_now().await.unwrap();
    server.stop();
    assert!(lib.packs().await.unwrap().offline);
    assert_eq!(lib.pack_cover(cats).await.unwrap(), Some(first));
}

#[tokio::test]
async fn motion_plays_the_animated_original_and_waits_for_leave_to_download() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    assert_eq!(lib.motion(items[0].id.clone(), true).await.unwrap(), None, "a still");

    let gif = &items[1];
    assert_eq!(gif.kind, stickpile_api::MediaKind::Animation);
    let downloads = |s: &MockServer| requests(s, &format!("GET /api/v1/items/{}/file/original", gif.id));
    assert_eq!(lib.motion(gif.id.clone(), false).await.unwrap(), None, "not cached");
    assert_eq!(downloads(&server), 0, "nothing downloaded without leave");

    let m = lib
        .motion(gif.id.clone(), true)
        .await
        .unwrap()
        .expect("a moving picture");
    assert_eq!(m.kind, MotionKind::Picture);
    assert_eq!(std::fs::read(&m.path).unwrap(), gif_bytes(3, 2), "the original GIF");
    assert!(Path::new(&m.path).starts_with(dir.path().join("cache")));
    assert_eq!((m.width, m.height), (gif.width, gif.height));
    assert_eq!(
        lib.motion(gif.id.clone(), false).await.unwrap(),
        Some(m.clone()),
        "cached now"
    );
    assert_eq!(lib.motion(gif.id.clone(), true).await.unwrap(), Some(m.clone()));
    assert_eq!(downloads(&server), 1, "downloaded once");

    server.stop();
    assert_eq!(
        lib.motion(gif.id.clone(), true).await.unwrap(),
        Some(m),
        "offline: the cached file"
    );
}

#[tokio::test]
async fn prepare_picks_per_target_and_accepted_types() {
    let (server, items) = seeded_server();
    server
        .state()
        .add_variant(&items[0].id, Variant::TgSticker, "image/webp", webp_bytes(32, 32, 1));
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());

    // A PNG still into Discord's field that takes PNGs: the original.
    let p = lib
        .prepare(
            items[0].id.clone(),
            "com.discord".into(),
            mimes(&["image/png", "image/gif"]),
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        Prepared {
            path: p.path.clone(),
            ..p.clone()
        },
        Prepared {
            path: p.path.clone(),
            mime_type: "image/png".into(),
            display_name: "grumpy cat.png".into(),
            target: Target::Discord,
            variant: "original".into(),
            sticker: false,
        }
    );
    assert!(Path::new(&p.path).starts_with(dir.path().join("cache")));
    assert!(p.path.ends_with("/grumpy cat.png"), "{}", p.path);
    assert_eq!(std::fs::read(&p.path).unwrap(), png_bytes(20, 20, 1));

    // Telegram with the sticker flag: the sticker rendition.
    let s = lib
        .prepare(
            items[0].id.clone(),
            "org.telegram.messenger".into(),
            mimes(&["image/webp", "image/png"]),
            true,
        )
        .await
        .unwrap();
    assert_eq!(
        (s.variant.as_str(), s.mime_type.as_str(), s.sticker),
        ("tg_sticker", "image/webp", true)
    );
    assert_eq!(
        (s.target, s.display_name.as_str()),
        (Target::Telegram, "grumpy cat.webp")
    );
    assert_eq!(std::fs::read(&s.path).unwrap(), webp_bytes(32, 32, 1));
    // The same with a field that takes no WebP: the original, not a sticker.
    let s = lib
        .prepare(
            items[0].id.clone(),
            "org.telegram.messenger".into(),
            mimes(&["image/png"]),
            true,
        )
        .await
        .unwrap();
    assert_eq!((s.variant.as_str(), s.sticker), ("original", false));
    // The sticker flag means nothing for Discord.
    let d = lib
        .prepare(items[0].id.clone(), "com.discord".into(), mimes(&["image/*"]), true)
        .await
        .unwrap();
    assert_eq!((d.variant.as_str(), d.sticker), ("original", false));

    // An animated GIF into any image field of an unknown app.
    let g = lib
        .prepare(
            items[1].id.clone(),
            "org.example.chat".into(),
            mimes(&["image/*"]),
            false,
        )
        .await
        .unwrap();
    assert_eq!((g.mime_type.as_str(), g.target), ("image/gif", Target::Generic));
    assert!(g.path.ends_with("/naahh.gif"), "{}", g.path);

    // Nothing the field takes.
    let none = lib
        .prepare(items[0].id.clone(), "com.discord".into(), mimes(&["image/gif"]), false)
        .await;
    assert_eq!(kind(none), LibraryErrorKind::NothingFits);
    let none = lib
        .prepare(items[2].id.clone(), "com.whatsapp".into(), mimes(&["image/png"]), false)
        .await;
    assert_eq!(
        kind(none),
        LibraryErrorKind::NothingFits,
        "a WebP with no PNG rendition"
    );
    let none = lib
        .prepare(items[0].id.clone(), "com.discord".into(), vec![], false)
        .await;
    assert_eq!(
        kind(none),
        LibraryErrorKind::NothingFits,
        "a field that takes no content"
    );
    // ... but WhatsApp's field that lists WebP gets the WebP.
    let w = lib
        .prepare(
            items[2].id.clone(),
            "com.whatsapp".into(),
            mimes(&["image/webp"]),
            false,
        )
        .await
        .unwrap();
    assert_eq!((w.mime_type.as_str(), w.target), ("image/webp", Target::Whatsapp));

    assert_eq!(
        kind(lib.prepare("nope".into(), "".into(), mimes(&["*/*"]), false).await),
        LibraryErrorKind::NotFound
    );
}

#[tokio::test]
async fn prepare_works_offline_once_cached() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    lib.sync_now().await.unwrap();
    let first = lib
        .prepare(items[1].id.clone(), "com.discord".into(), mimes(&["image/gif"]), false)
        .await
        .unwrap();

    server.stop();
    let again = lib
        .prepare(items[1].id.clone(), "com.discord".into(), mimes(&["image/gif"]), false)
        .await
        .unwrap();
    assert_eq!(again, first);
    // A new library, the item from the cache.
    let fresh = open(dir.path(), &server.url);
    let cached = fresh
        .prepare(items[1].id.clone(), "com.discord".into(), mimes(&["image/gif"]), false)
        .await
        .unwrap();
    assert_eq!(cached, first);
    // Not cached.
    let missing = fresh
        .prepare(items[0].id.clone(), "com.discord".into(), mimes(&["image/png"]), false)
        .await;
    assert_eq!(kind(missing), LibraryErrorKind::Unreachable);

    server.restart();
    let out = tempfile::tempdir().unwrap();
    let lib = open(out.path(), &server.url);
    let signed_out = lib
        .prepare(items[0].id.clone(), "com.discord".into(), mimes(&["image/png"]), false)
        .await;
    assert_eq!(kind(signed_out), LibraryErrorKind::NotSignedIn);
}

#[tokio::test]
async fn the_cache_size_counts_files_and_clearing_keeps_the_items() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    assert_eq!(lib.cache_bytes().unwrap(), 0);
    lib.sync_now().await.unwrap();
    let thumb = lib.thumbnail(items[0].id.clone()).await.unwrap();
    let bytes = lib.cache_bytes().unwrap();
    assert!(bytes > 0, "the thumbnail is counted");

    assert_eq!(lib.clear_cache().unwrap(), bytes);
    assert_eq!(lib.cache_bytes().unwrap(), 0);
    assert!(!Path::new(&thumb).exists(), "the file is gone");
    assert_eq!(lib.clear_cache().unwrap(), 0, "nothing left to clear");

    // The item list stays: offline search still answers.
    server.stop();
    let found = lib.search(items[0].name.clone(), Target::Generic, 10).await.unwrap();
    assert!(found.offline);
    assert!(found.items.iter().any(|i| i.id == items[0].id));
}
