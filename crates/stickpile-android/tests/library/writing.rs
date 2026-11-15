//! `record_use`, `toggle_like`, `upload` and the pairing input helpers.

use stickpile_android::{LibraryErrorKind, PairingLink, Target, UploadFile};
use stickpile_api::{TargetApp, Variant};

use super::*;

#[tokio::test]
async fn record_use_feeds_recent_and_the_usage_queue() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    lib.sync_now().await.unwrap();

    // Online: sent in the background.
    lib.record_use(items[1].id.clone(), "mp4".into(), "org.telegram.messenger".into())
        .unwrap();
    assert!(wait_until(|| server.state().usage.len() == 1).await, "background flush");
    assert!(wait_until(|| lib.pending_usage() == 0).await);
    let ev = server.state().usage[0].clone();
    assert_eq!(ev.item_id, items[1].id);
    assert_eq!(ev.target, Some(TargetApp::Telegram));
    assert_eq!(ev.variant, Some(Variant::Mp4));
    assert_eq!(ev.client, Some(stickpile_android::client_name()));

    // Offline: queued, and first in the recent items.
    server.stop();
    lib.record_use(items[2].id.clone(), "original".into(), "com.discord".into())
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(lib.pending_usage(), 1);
    assert_eq!(lib.sync_state().pending_usage, 1);
    assert_eq!(ids(&lib.recent(5))[0], items[2].id);

    // Back online: the next sync sends it.
    server.restart();
    lib.sync_now().await.unwrap();
    assert_eq!(lib.pending_usage(), 0);
    let st = server.state();
    let ev = st.usage.iter().find(|e| e.item_id == items[2].id).expect("sent");
    assert_eq!(ev.target, Some(TargetApp::Discord));
}

#[tokio::test]
async fn record_use_checks_its_input() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let bad = lib.record_use(items[0].id.clone(), "huge".into(), "com.discord".into());
    assert_eq!(invalid_field(bad), "variant");
    // Neither shown nor cached.
    let unknown = lib.record_use(items[0].id.clone(), "original".into(), "com.discord".into());
    assert_eq!(kind(unknown), LibraryErrorKind::NotFound);
    // Shown by a list: known.
    lib.pack_items(items[0].set_id.clone()).await.unwrap();
    lib.record_use(items[0].id.clone(), "original".into(), "com.discord".into())
        .unwrap();
    assert_eq!(ids(&lib.recent(5)), vec![items[0].id.clone()]);
}

#[tokio::test]
async fn record_use_signed_out_keeps_the_use_here() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    lib.sync_now().await.unwrap();
    std::fs::remove_file(token_path(dir.path())).unwrap();
    let lib = open(dir.path(), &server.url);
    lib.record_use(items[0].id.clone(), "original".into(), "".into())
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(lib.pending_usage(), 1, "nothing is sent without a token");
    assert_eq!(ids(&lib.recent(5)), vec![items[0].id.clone()]);
}

#[tokio::test]
async fn toggle_like_flips_and_needs_the_server() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    assert!(lib.toggle_like(items[4].id.clone()).await.unwrap());
    assert_eq!(server.state().likes.len(), 1);
    assert!(!lib.toggle_like(items[4].id.clone()).await.unwrap());
    assert!(server.state().likes.is_empty());
    assert_eq!(kind(lib.toggle_like("nope".into()).await), LibraryErrorKind::NotFound);

    server.stop();
    assert_eq!(
        kind(lib.toggle_like(items[4].id.clone()).await),
        LibraryErrorKind::Unreachable
    );

    server.restart();
    let out = tempfile::tempdir().unwrap();
    let lib = open(out.path(), &server.url);
    assert_eq!(
        kind(lib.toggle_like(items[4].id.clone()).await),
        LibraryErrorKind::NotSignedIn
    );
}

#[tokio::test]
async fn upload_adds_one_file_to_a_pack() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let memes = items[3].set_id.clone();
    let file = dir.path().join("upload-1234.tmp.png");
    std::fs::write(&file, png_bytes(30, 30, 9)).unwrap();
    let upload = UploadFile {
        path: file.display().to_string(),
        name: Some(" shrug ".into()),
        keywords: vec!["meh".into(), " ".into()],
        emojis: vec!["🤷".into()],
    };

    let item = lib.upload(memes.clone(), upload.clone()).await.unwrap();
    assert_eq!(item.name, "shrug");
    assert_eq!(item.set_id, memes);
    assert_eq!(item.set_name, "Memes");
    assert_eq!(item.keywords, vec!["meh"]);
    assert_eq!((item.width, item.height), (30, 30));
    let found = lib.search("shrug".into(), Target::Generic, 5).await.unwrap();
    assert_eq!(ids(&found.items), vec![item.id.clone()]);
    // The same content again: the item already there.
    let again = lib.upload(memes.clone(), upload.clone()).await.unwrap();
    assert_eq!(again.id, item.id);
    // No name: the server names it after the file.
    std::fs::write(&file, gif_bytes(2, 9)).unwrap();
    let unnamed = UploadFile {
        name: None,
        keywords: vec![],
        emojis: vec![],
        ..upload.clone()
    };
    assert_eq!(
        lib.upload(memes.clone(), unnamed).await.unwrap().name,
        "upload-1234.tmp"
    );

    // Not a picture.
    let junk = dir.path().join("notes.txt");
    std::fs::write(&junk, "hello").unwrap();
    let junk = UploadFile {
        path: junk.display().to_string(),
        ..upload.clone()
    };
    assert_eq!(invalid_field(lib.upload(memes.clone(), junk).await), "file");
    // No such file.
    let missing = UploadFile {
        path: dir.path().join("gone.png").display().to_string(),
        ..upload.clone()
    };
    assert_eq!(kind(lib.upload(memes.clone(), missing).await), LibraryErrorKind::Io);
    // Someone else's pack, or none.
    let dogs = items[4].set_id.clone();
    assert_eq!(kind(lib.upload(dogs, upload.clone()).await), LibraryErrorKind::NotFound);
    assert_eq!(
        kind(lib.upload("nope".into(), upload.clone()).await),
        LibraryErrorKind::NotFound
    );

    server.stop();
    assert_eq!(
        kind(lib.upload(memes.clone(), upload.clone()).await),
        LibraryErrorKind::Unreachable
    );

    server.restart();
    let out = tempfile::tempdir().unwrap();
    let lib = open(out.path(), &server.url);
    assert_eq!(kind(lib.upload(memes, upload).await), LibraryErrorKind::NotSignedIn);
}

#[tokio::test]
async fn pairing_input_needs_no_network() {
    let dir = tempfile::tempdir().unwrap();
    let lib = open(dir.path(), &stickpile_core::mock::dead_url());
    assert_eq!(
        lib.parse_pairing_link("stickpile://pair?url=https%3A%2F%2Fs.example%2F&code=7kq2m9xa4ftr".into()),
        Some(PairingLink {
            server_url: "https://s.example".into(),
            code: "7KQ2-M9XA-4FTR".into(),
        })
    );
    let made = stickpile_api::pairing::link("http://10.0.0.2:8080", "7KQ2-M9XA-4FTR");
    assert_eq!(lib.parse_pairing_link(made).unwrap().server_url, "http://10.0.0.2:8080");
    for not_a_link in [
        "",
        "https://s.example",
        "stickpile://pair?url=https%3A%2F%2Fs.example",
        "stickpile://pair?url=ftp%3A%2F%2Fs.example&code=7KQ2M9XA4FTR",
        "stickpile://other?url=https%3A%2F%2Fs.example&code=7KQ2M9XA4FTR",
    ] {
        assert_eq!(lib.parse_pairing_link(not_a_link.into()), None, "{not_a_link:?}");
    }

    assert_eq!(
        lib.normalize_pairing_code("7kq2 m9xa 4ftr".into()).as_deref(),
        Some("7KQ2-M9XA-4FTR")
    );
    assert_eq!(
        lib.normalize_pairing_code("o1il-0000-0000".into()).as_deref(),
        Some("0111-0000-0000")
    );
    assert_eq!(lib.normalize_pairing_code("7KQ2-M9XA".into()), None, "not whole yet");
    assert_eq!(lib.normalize_pairing_code("7KQ2-M9XA-4FTR-X".into()), None, "too long");
    assert_eq!(
        lib.normalize_pairing_code("7KQ2-M9XA-4FTU".into()),
        None,
        "no U in the alphabet"
    );
}
