//! `whatsapp_packs`, `whatsapp_progress` and `whatsapp_unpacked`: the
//! server's build of a pack, downloaded and unpacked (the mock serves real
//! stored `.wastickers` ZIPs, as the server does).

use std::path::{Path, PathBuf};

use stickpile_android::{LibraryError, WhatsappPart, WhatsappStep};
use stickpile_api::Variant;
use stickpile_core::mock::stored_zip;
use stickpile_core::ApiClient;

use super::*;

/// The id of alice's public pack `cats` (three items) in [`seeded_server`].
fn cats(items: &[ItemSummary]) -> String {
    items[0].set_id.clone()
}

/// Every file under `dir`, recursively.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn whatsapp_dir(dir: &Path) -> PathBuf {
    dir.join("cache/whatsapp")
}

fn thumb(server: &MockServer, item_id: &str) -> Vec<u8> {
    server
        .state()
        .items
        .iter()
        .find(|i| i.item.id == item_id)
        .unwrap()
        .files[&Variant::Thumb]
        .clone()
}

/// Build the pack's WhatsApp packs on the server as alice, outside the
/// library (so that a test can swap the files before the library
/// downloads them).
async fn build_on_server(server: &MockServer, set_id: &str) {
    let token = server.state().issue_token("alice", "builder");
    let client = ApiClient::new(&server.url, Duration::from_secs(5))
        .unwrap()
        .with_token(Some(token));
    let job = client.whatsapp_build(set_id).await.unwrap();
    assert_eq!(job.status, stickpile_api::JobStatus::Done);
}

fn requests_to(server: &MockServer, set_id: &str) -> (usize, usize, usize) {
    (
        requests(server, &format!("GET /api/v1/sets/{set_id}/whatsapp")),
        requests(server, &format!("POST /api/v1/sets/{set_id}/whatsapp")),
        requests(server, &format!("GET /api/v1/sets/{set_id}/whatsapp/packs/1")),
    )
}

fn part_files(part: &WhatsappPart) -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(&part.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    files
}

#[tokio::test]
async fn a_pack_is_built_downloaded_and_unpacked() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let id = cats(&items);
    {
        let mut st = server.state();
        let it = st.items.iter_mut().find(|i| i.item.id == items[0].id).unwrap();
        it.item.emojis = vec!["\u{1f63e}".into(), "x".into(), "\u{2764}".into(), "\u{1f525}".into()];
    }

    let packs = lib.whatsapp_packs(id.clone()).await.unwrap();
    assert!(!packs.offline);
    assert_eq!(packs.skipped, 0);
    assert_eq!(packs.parts.len(), 1);
    let part = &packs.parts[0];
    assert_eq!(part.pack_id, id);
    assert_eq!(part.number, 1);
    assert_eq!(part.identifier, format!("{id}-1"));
    assert_eq!((part.name.as_str(), part.publisher.as_str()), ("Cats", "alice"));
    assert!(!part.animated);
    assert_eq!(part.version.len(), 16, "{}", part.version);
    assert_eq!(
        Path::new(&part.dir),
        whatsapp_dir(dir.path()).join(&id).join(format!("1-{}", part.version))
    );
    assert_eq!(part.tray_file, "cover.png");
    let files: Vec<&str> = part.stickers.iter().map(|s| s.file.as_str()).collect();
    assert_eq!(files, ["01.webp", "02.webp", "03.webp"]);
    // The first three emoji as the server sent them; the server's default
    // for an item without.
    assert_eq!(part.stickers[0].emojis, ["\u{1f63e}", "x", "\u{2764}"]);
    assert_eq!(part.stickers[1].emojis, ["\u{1f642}"]);

    // Only the pictures are unpacked, byte for byte.
    assert_eq!(part_files(part), ["01.webp", "02.webp", "03.webp", "cover.png"]);
    for (sticker, item) in part.stickers.iter().zip(&items) {
        assert_eq!(
            std::fs::read(Path::new(&part.dir).join(&sticker.file)).unwrap(),
            thumb(&server, &item.id)
        );
    }
    let tray = image::load_from_memory(&std::fs::read(Path::new(&part.dir).join("cover.png")).unwrap()).unwrap();
    assert_eq!((tray.width(), tray.height()), (96, 96));

    assert_eq!(requests_to(&server, &id), (1, 1, 1));
    assert_eq!(lib.whatsapp_unpacked(), packs.parts, "what the provider serves");
    assert_eq!(lib.whatsapp_progress(id.clone()), None, "not running");
    // The index sits next to the part; nothing is left over.
    let mut left: Vec<PathBuf> = files_under(&whatsapp_dir(dir.path()));
    left.sort();
    assert_eq!(left.len(), 5, "{left:?}");
    assert!(left.contains(&whatsapp_dir(dir.path()).join(&id).join("index.json")));
}

#[tokio::test]
async fn an_unchanged_pack_is_reused_and_a_changed_one_gets_a_new_version() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let id = cats(&items);
    let first = lib.whatsapp_packs(id.clone()).await.unwrap();

    // Unchanged: the server's build is current and the files are here.
    let again = lib.whatsapp_packs(id.clone()).await.unwrap();
    assert_eq!(again, first);
    assert_eq!(requests_to(&server, &id), (2, 1, 1), "no new build, no download");

    // Another library on the same files (the app restarted) needs no download either.
    let restarted = open(dir.path(), &server.url);
    assert_eq!(restarted.whatsapp_packs(id.clone()).await.unwrap(), first);
    assert_eq!(requests_to(&server, &id), (3, 1, 1));

    // The pack changed: a new build, a new version in a new directory,
    // the old one gone.
    server.state().add_item(&id, "fourth", &["cat"], png_bytes(30, 30, 7));
    let rebuilt = lib.whatsapp_packs(id.clone()).await.unwrap();
    assert_eq!(requests_to(&server, &id), (4, 2, 2));
    let (old, new) = (&first.parts[0], &rebuilt.parts[0]);
    assert_ne!(new.version, old.version);
    assert_eq!(new.identifier, old.identifier, "WhatsApp sees the same pack, updated");
    assert_ne!(new.dir, old.dir);
    assert_eq!(new.stickers.len(), 4);
    assert!(!Path::new(&old.dir).exists());
    assert_eq!(part_files(new).len(), 5);
    assert_eq!(lib.whatsapp_unpacked(), rebuilt.parts);
}

#[tokio::test]
async fn a_big_pack_comes_in_parts() {
    let (server, _) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let id = {
        let mut st = server.state();
        let big = st.add_set("alice", "big", "Big", Visibility::Private);
        for i in 0..31u8 {
            st.add_item(&big.id, &format!("s{i}"), &[], png_bytes(8, 8, i));
        }
        big.id
    };
    let packs = lib.whatsapp_packs(id.clone()).await.unwrap();
    let parts: Vec<(u32, &str, &str, usize)> = packs
        .parts
        .iter()
        .map(|p| (p.number, p.identifier.as_str(), p.name.as_str(), p.stickers.len()))
        .collect();
    let (one, two) = (format!("{id}-1"), format!("{id}-2"));
    assert_eq!(
        parts,
        [(1, one.as_str(), "Big (1/2)", 16), (2, two.as_str(), "Big (2/2)", 15)]
    );
    assert_eq!(lib.whatsapp_unpacked().len(), 2);
}

#[tokio::test]
async fn offline_the_parts_unpacked_before_answer() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let id = cats(&items);
    let memes = items[3].set_id.clone();
    let online = lib.whatsapp_packs(id.clone()).await.unwrap();

    server.stop();
    let offline = lib.whatsapp_packs(id.clone()).await.unwrap();
    assert!(offline.offline);
    assert_eq!(offline.parts, online.parts);
    // Also for a library opened offline.
    let restarted = open(dir.path(), &server.url);
    assert_eq!(restarted.whatsapp_packs(id.clone()).await.unwrap(), offline);
    assert_eq!(restarted.whatsapp_unpacked(), online.parts);
    // Never unpacked: nothing to answer with.
    assert_eq!(kind(lib.whatsapp_packs(memes).await), LibraryErrorKind::Unreachable);
}

#[tokio::test]
async fn hostile_packs_are_refused_and_the_old_parts_stay() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let id = cats(&items);
    let good = lib.whatsapp_packs(id.clone()).await.unwrap();

    // A new build on the server (the pack changed), whose file is then
    // swapped for a hostile one before the library downloads it.
    server.state().add_item(&id, "fourth", &[], png_bytes(9, 9, 9));
    build_on_server(&server, &id).await;
    let sticker = thumb(&server, &items[0].id);
    let tray = {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(96, 96)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    };
    let big_webp = {
        let mut b = b"RIFF\0\0\0\0WEBP".to_vec();
        b.resize(100 * 1024 + 1, 0);
        b
    };
    let many: Vec<String> = (0..65).map(|i| format!("x{i}.txt")).collect();
    let fine = ["01.webp", "02.webp", "03.webp", "04.webp"];
    let pack = |extra: &[(&str, &[u8])], leave_out: &str| -> Vec<u8> {
        let mut entries: Vec<(&str, &[u8])> = vec![("title.txt", &b"Cats"[..]), ("cover.png", tray.as_slice())];
        entries.extend(
            fine.iter()
                .filter(|f| **f != leave_out)
                .map(|f| (*f, sticker.as_slice())),
        );
        entries.extend_from_slice(extra);
        stored_zip(entries)
    };
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("a parent directory", pack(&[("../evil.webp", &sticker)], "")),
        ("parent of a sticker", pack(&[("../01.webp", &sticker)], "01.webp")),
        ("an absolute path", pack(&[("/tmp/evil.webp", &sticker)], "")),
        ("a subdirectory", pack(&[("sub/01.webp", &sticker)], "01.webp")),
        ("a hidden file", pack(&[(".evil", &sticker)], "")),
        ("a backslash", pack(&[("..\\evil.webp", &sticker)], "")),
        ("a directory entry", pack(&[("dir/", b"")], "")),
        ("a duplicate", pack(&[("01.webp", &sticker)], "")),
        ("an oversized sticker", pack(&[("01.webp", &big_webp)], "01.webp")),
        ("a PNG named .webp", pack(&[("01.webp", &tray)], "01.webp")),
        ("a missing sticker", pack(&[], "04.webp")),
        (
            "too many entries",
            pack(&many.iter().map(|n| (n.as_str(), &b"x"[..])).collect::<Vec<_>>(), ""),
        ),
        ("not a ZIP", b"PK\x03\x04 this is not a zip".to_vec()),
    ];
    for (what, bytes) in cases {
        server.state().whatsapp_packs.get_mut(&id).unwrap()[0] = bytes;
        let e = match lib.whatsapp_packs(id.clone()).await {
            Ok(packs) => panic!("{what}: unpacked {packs:?}"),
            Err(e) => e,
        };
        assert!(
            matches!(&e, LibraryError::Other { detail } if detail.contains("cannot be used")),
            "{what}: {e:?}"
        );
        // The parts unpacked before are still served, and nothing landed
        // outside their directory.
        assert_eq!(lib.whatsapp_unpacked(), good.parts, "{what}");
        let mut stray: Vec<PathBuf> = files_under(dir.path())
            .into_iter()
            .filter(|f| {
                let name = f.file_name().unwrap().to_string_lossy();
                name.contains("evil") || (name.ends_with(".webp") && !f.starts_with(&good.parts[0].dir))
            })
            .collect();
        stray.retain(|f| {
            !f.starts_with(dir.path().join("cache/files")) && !f.starts_with(dir.path().join("cache/clip"))
        });
        assert!(stray.is_empty(), "{what}: {stray:?}");
        assert!(!dir.path().parent().unwrap().join("evil.webp").exists());
        assert!(!Path::new("/tmp/evil.webp").exists());
    }
    let left = std::fs::read_dir(whatsapp_dir(dir.path()).join(&id)).unwrap().count();
    assert_eq!(left, 2, "the index and the good part");

    // The right file again: the new build is unpacked.
    {
        let mut st = server.state();
        st.whatsapp.remove(&id);
        st.whatsapp_packs.remove(&id);
    }
    let rebuilt = lib.whatsapp_packs(id.clone()).await.unwrap();
    assert_eq!(rebuilt.parts[0].stickers.len(), 4);
}

#[tokio::test]
async fn failures_say_what_went_wrong() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let id = cats(&items);
    let memes = items[3].set_id.clone();

    // Fewer than 3 stickers.
    assert_eq!(invalid_field(lib.whatsapp_packs(memes).await), "pack");
    // Not a pack id at all, and not a pack.
    assert_eq!(invalid_field(lib.whatsapp_packs("../x".into()).await), "pack_id");
    assert_eq!(
        kind(lib.whatsapp_packs("01nosuchpack".into()).await),
        LibraryErrorKind::NotFound
    );
    // A server without WhatsApp packs answers 404 on their routes; the
    // pack is there all the same.
    server.state().whatsapp_unsupported = true;
    let e = lib.whatsapp_packs(id.clone()).await.unwrap_err();
    assert!(matches!(e, LibraryError::Server { status: 404, .. }), "{e:?}");
    assert_eq!(
        kind(lib.whatsapp_packs("01nosuchpack".into()).await),
        LibraryErrorKind::NotFound
    );
    // Signed out.
    let out = tempfile::tempdir().unwrap();
    server.state().whatsapp_unsupported = false;
    assert_eq!(
        kind(open(out.path(), &server.url).whatsapp_packs(id).await),
        LibraryErrorKind::NotSignedIn
    );
}

#[tokio::test]
async fn a_queued_build_is_waited_for_and_the_wait_can_stop() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let id = cats(&items);
    server.state().whatsapp_queued = true;

    // Stopped while waiting: nothing is kept, the progress goes.
    let stopped = tokio::time::timeout(Duration::from_millis(100), lib.whatsapp_packs(id.clone())).await;
    assert!(stopped.is_err());
    assert!(wait_until(|| lib.whatsapp_progress(id.clone()).is_none()).await);
    assert!(lib.whatsapp_unpacked().is_empty());

    // Waited for, the build the stopped call asked for (no second one):
    // the progress says the server is building meanwhile.
    let call = {
        let (lib, id) = (lib.clone(), id.clone());
        tokio::spawn(async move { lib.whatsapp_packs(id).await })
    };
    assert!(
        wait_until(|| lib
            .whatsapp_progress(id.clone())
            .is_some_and(|p| p.step == WhatsappStep::Building))
        .await
    );
    let packs = call.await.unwrap().unwrap();
    assert_eq!(packs.parts.len(), 1);
    let polls = server
        .state()
        .requests
        .iter()
        .filter(|r| r.starts_with("GET /api/v1/jobs/"))
        .count();
    assert!(polls >= 2, "{polls}");
    assert_eq!(requests_to(&server, &id).1, 1, "one build");
    assert_eq!(lib.whatsapp_progress(id), None);
}

#[tokio::test]
async fn the_packs_go_with_the_account() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let id = cats(&items);
    lib.whatsapp_packs(id).await.unwrap();
    assert_eq!(lib.whatsapp_unpacked().len(), 1);
    // They are not the cache's files.
    lib.clear_cache().unwrap();
    assert_eq!(lib.whatsapp_unpacked().len(), 1, "clear_cache leaves them");

    lib.logout().await.unwrap();
    assert!(lib.whatsapp_unpacked().is_empty());
    assert!(!whatsapp_dir(dir.path()).exists());
}
