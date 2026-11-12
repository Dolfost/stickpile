//! `stickpile-server backup`: consistent while the server runs and writes,
//! and the set directories round-trip into a fresh server.

mod common;

use std::path::Path;

use common::{png, Opts, TestServer, TestUser};
use stickpile_api::{routes, ImportReport, Visibility};
use stickpile_server::service::backup::{self, BackupInfo};
use stickpile_setfmt::SetBundle;

const CATS_TOML: &str = r#"version = 4
name = "My cats"
slug = "cats"
description = "Cats, mostly staring."
visibility = "public"
aspect_ratio = "1:1"
recommended_px = 512
cover_image = "cover.png"
author = "alice"

[items."001-cat.png"]
name = "cat"
keywords = ["cat", "stare"]
keywords_source = "ai"
emojis = ["😼"]
vibe = "smug"
vibe_source = "ai"
tags = ["reaction"]

[items."001-cat.png".credits]
by = "someone"

[items."002-dog.png"]
name = "not a cat"
emojis = ["🐶"]
"#;

/// Import a set directory over HTTP (what the CLI and web UI do).
async fn import_dir(srv: &TestServer, u: &TestUser, dir: &Path) -> ImportReport {
    let mut form = reqwest::multipart::Form::new();
    let top = dir.file_name().unwrap().to_string_lossy().into_owned();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        let bytes = std::fs::read(entry.path()).unwrap();
        form = form.part(
            name.clone(),
            reqwest::multipart::Part::bytes(bytes).file_name(format!("{top}/{name}")),
        );
    }
    let resp = srv
        .client
        .post(srv.url(routes::SETS_IMPORT))
        .bearer_auth(&u.token)
        .multipart(form)
        .send()
        .await
        .unwrap();
    common::read(resp).await.expect(200).json()
}

async fn export(srv: &TestServer, u: &TestUser, set_id: &str) -> SetBundle {
    let tar = srv
        .get(u, &routes::path(routes::SET_EXPORT, &[set_id]))
        .await
        .expect(200);
    stickpile_setfmt::read_tar(&tar.body, 1 << 30).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backup_while_running_round_trips() {
    let srv = TestServer::start_with(
        Opts {
            workers: true,
            ..Default::default()
        },
        |_| {},
    )
    .await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let src = tempfile::tempdir().unwrap();
    let cats_dir = src.path().join("cats");
    std::fs::create_dir_all(&cats_dir).unwrap();
    std::fs::write(cats_dir.join("set.toml"), CATS_TOML).unwrap();
    std::fs::write(cats_dir.join("001-cat.png"), png(1)).unwrap();
    std::fs::write(cats_dir.join("002-dog.png"), png(2)).unwrap();
    std::fs::write(cats_dir.join("cover.png"), png(4)).unwrap();
    let cats = import_dir(&srv, &alice, &cats_dir).await.set.unwrap();
    let dogs = srv.create_set(&bob, "dogs", Visibility::Private).await;
    srv.upload_one(&bob, &dogs.id, "rex.png", png(3)).await;
    let busy = srv.create_set(&bob, "busy", Visibility::Private).await;

    // Keep writing while the backup runs (WAL: neither blocks the other).
    let writer = {
        let base = srv.base.clone();
        let token = bob.token.clone();
        let set_id = busy.id.clone();
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            for i in 0..8u64 {
                let form = reqwest::multipart::Form::new().part(
                    "file",
                    reqwest::multipart::Part::bytes(png(100 + i)).file_name(format!("{i}.png")),
                );
                let r = client
                    .post(format!("{base}{}", routes::path(routes::SET_ITEMS, &[&set_id])))
                    .bearer_auth(&token)
                    .multipart(form)
                    .send()
                    .await
                    .unwrap();
                assert!(r.status().is_success());
            }
        })
    };
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("backup");
    let info = backup::run(&srv.state.config, &target).await.unwrap();
    writer.await.unwrap();

    assert!(info.errors.is_empty(), "{:?}", info.errors);
    assert_eq!(info.users, 2);
    assert_eq!(info.sets, 3);
    assert!(info.items >= 3);
    assert_eq!(info.format, backup::FORMAT);
    assert!(info.schema_version >= 2);
    let written: BackupInfo = serde_json::from_slice(&std::fs::read(target.join(backup::MANIFEST)).unwrap()).unwrap();
    assert_eq!(written, info);
    for rel in [
        "sets/alice/cats/set.toml",
        "sets/alice/cats/cover.png",
        "sets/bob/dogs/001-rex.png",
        "sets/bob/busy/set.toml",
    ] {
        assert!(target.join(rel).is_file(), "{rel}");
    }
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&target), 0o700, "the backup holds secrets");
        assert_eq!(mode(&target.join(backup::DB_FILE)), 0o600);
    }
    // The snapshot is one self-contained, consistent SQLite file.
    assert!(!target.join("stickpile.db-wal").exists());
    let snap = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=ro", target.join(backup::DB_FILE).display()))
        .await
        .unwrap();
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&snap)
        .await
        .unwrap();
    assert_eq!(users, 2);
    let busy_items: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items WHERE set_id = ?")
        .bind(&busy.id)
        .fetch_one(&snap)
        .await
        .unwrap();
    let on_disk = std::fs::read_dir(target.join("sets/bob/busy"))
        .unwrap()
        .filter(|e| e.as_ref().unwrap().file_name() != "set.toml")
        .count() as i64;
    assert_eq!(busy_items, on_disk, "directories are exported from the snapshot");
    let check: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&snap)
        .await
        .unwrap();
    assert_eq!(check, "ok");
    snap.close().await;

    // Round trip: a backed-up directory imported into a fresh server
    // exports the same manifest data and identical files.
    let original = export(&srv, &alice, &cats.id).await;
    assert_eq!(original.files.len(), 2);
    assert!(original.cover_image.is_some(), "the cover image is no item");
    let fresh = TestServer::start().await;
    let carol = fresh.user("carol").await;
    let report = import_dir(&fresh, &carol, &target.join("sets/alice/cats")).await;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let again = export(&fresh, &carol, &report.set.unwrap().id).await;
    assert_eq!(again, original);
    assert_eq!(again.manifest.extra["author"].as_str(), Some("alice"));
    assert_eq!(
        again.manifest.items["001-cat.png"].keywords_source,
        stickpile_api::FieldSource::Ai
    );

    // A second backup into the same directory replaces the first; a
    // directory with other content is refused.
    let info2 = backup::run(&srv.state.config, &target).await.unwrap();
    assert_eq!(info2.sets, 3);
    let foreign = out.path().join("foreign");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::write(foreign.join("important.txt"), "keep me").unwrap();
    let err = backup::run(&srv.state.config, &foreign).await.unwrap_err();
    assert!(err.to_string().contains("refusing"), "{err}");
    assert!(foreign.join("important.txt").exists());

    // The CLI subcommand, against the running server's data directory.
    let cfg_path = out.path().join("config.toml");
    std::fs::write(&cfg_path, toml::to_string(&srv.state.config).unwrap()).unwrap();
    let cli_target = out.path().join("cli-backup");
    let bin = env!("CARGO_BIN_EXE_stickpile-server");
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(bin)
            .arg("--config")
            .arg(&cfg_path)
            .arg("backup")
            .arg(&cli_target)
            .env_remove("STICKPILE_CONFIG")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Backed up 3 sets"), "{stdout}");
    assert!(out.path().join("cli-backup/sets/alice/cats/001-cat.png").is_file());

    // Restore into a fresh data directory: accounts, tokens, likes and
    // content come back; thumbnails at once, other variants via jobs.
    let restored_dir = out.path().join("restored");
    let mut cfg = srv.state.config.clone();
    cfg.server.data_dir = restored_dir.clone();
    let r = backup::restore(&cfg, &target).await.unwrap();
    assert!(r.missing.is_empty(), "{:?}", r.missing);
    assert_eq!(r.sets, 3);
    assert!(r.variant_jobs >= 3);
    // Restoring over an existing database is refused.
    let err = backup::restore(&cfg, &target).await.unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
    let restored = TestServer::with_config(|c| c.server.data_dir = restored_dir.clone()).await;
    let alice_again = TestUser {
        token: alice.token.clone(),
        id: alice.id.clone(),
        username: alice.username.clone(),
        password: alice.password.clone(),
    };
    let same = export(&restored, &alice_again, &cats.id).await;
    assert_eq!(same, original, "the old token works and the set is intact");
    let d: stickpile_api::SetDetail = restored
        .get(&alice_again, &routes::path(routes::SET, &[&cats.id]))
        .await
        .expect(200)
        .json();
    let first = &d.items[0].id;
    restored
        .get(&alice_again, &routes::path(routes::ITEM_FILE, &[first, "thumb"]))
        .await
        .expect(200);
    let cover = d.set.cover_image.as_ref().expect("cover image");
    restored.get(&alice_again, &cover.thumb_url).await.expect(200);
    // The other variants come from `variants` jobs on the restored server:
    // run them (retries made runnable at once) until the rendition exists.
    let sticker = routes::path(routes::ITEM_FILE, &[first, "tg_sticker"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        sqlx::query("UPDATE jobs SET run_after = 0 WHERE status = 'queued'")
            .execute(&restored.state.db)
            .await
            .unwrap();
        restored.run_jobs().await;
        let r = restored.get(&alice_again, &sticker).await;
        if r.status == 200 {
            break;
        }
        if std::time::Instant::now() > deadline {
            let jobs: Vec<(String, String, Option<String>)> =
                sqlx::query_as("SELECT kind, status, error FROM jobs ORDER BY created_at")
                    .fetch_all(&restored.state.db)
                    .await
                    .unwrap();
            panic!("no tg_sticker after restore ({}): jobs {jobs:?}", r.status);
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let hits: stickpile_api::SearchResults = restored
        .get(&alice_again, &format!("{}?q=stare", routes::SEARCH))
        .await
        .expect(200)
        .json();
    assert_eq!(hits.hits.len(), 1, "the search index is rebuilt from the database");

    // No database at all: a clear error instead of an empty backup.
    let mut cfg = srv.state.config.clone();
    cfg.server.data_dir = out.path().join("nothing-here");
    let err = backup::run(&cfg, &out.path().join("x")).await.unwrap_err();
    assert!(err.to_string().contains("no database"), "{err}");
}
