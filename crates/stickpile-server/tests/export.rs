//! The export page's downloads (ZIP, TAR, TAR.GZ), WhatsApp packs and
//! publishing to several platforms through the registry
//! (docs/web.md "Export and publish", docs/whatsapp.md).

mod common;
mod integrations_support;
mod web_common;

use std::collections::BTreeMap;
use std::io::Read;

use common::{png, TestServer, TestUser};
use integrations_support::mock_bot::{self, Shared, BOT};
use integrations_support::run_jobs_now;
use serde_json::{json, Value};
use stickpile_api::{
    routes, Job, JobKind, JobStatus, Publication, PublishJobs, Visibility, WhatsappBuild, WhatsappStatus,
};
use stickpile_media::whatsapp as wa;

async fn job(srv: &TestServer, u: &TestUser, id: &str) -> Job {
    srv.get(u, &routes::path(routes::JOB, &[id])).await.expect(200).json()
}

async fn run(srv: &TestServer, u: &TestUser, j: &Job) -> Job {
    run_jobs_now(srv).await;
    job(srv, u, &j.id).await
}

fn tar_entries(tar: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut a = tar::Archive::new(tar);
    let mut out = BTreeMap::new();
    for e in a.entries().unwrap() {
        let mut e = e.unwrap();
        if e.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let name = e.path().unwrap().to_string_lossy().into_owned();
        let mut buf = Vec::new();
        e.read_to_end(&mut buf).unwrap();
        out.insert(name, buf);
    }
    out
}

fn zip_entries(zip: &[u8]) -> BTreeMap<String, Vec<u8>> {
    stickpile_server::archive::entries(zip).unwrap().into_iter().collect()
}

#[tokio::test]
async fn zip_and_tar_gz_hold_the_same_files_as_the_tar() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let set = srv.create_set(&alice, "cats", Visibility::Unlisted).await;
    srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
    srv.upload_one(&alice, &set.id, "b.jpg", common::jpeg(2)).await;
    srv.upload_one(&alice, &set.id, "c.gif", common::gif_anim(3)).await;
    srv.put_cover(&alice, &set.id, "cover.png", png(9)).await.expect(200);

    // A reader who is not the owner downloads every format.
    let tar = srv
        .get(&bob, &routes::path(routes::SET_EXPORT, &[&set.id]))
        .await
        .expect(200);
    let zip = srv
        .get(&bob, &routes::path(routes::SET_EXPORT_ZIP, &[&set.id]))
        .await
        .expect(200);
    assert_eq!(zip.header("content-type").as_deref(), Some("application/zip"));
    assert_eq!(
        zip.header("content-disposition").as_deref(),
        Some("attachment; filename=\"cats.zip\"")
    );
    let tgz = srv
        .get(&bob, &routes::path(routes::SET_EXPORT_TGZ, &[&set.id]))
        .await
        .expect(200);
    assert_eq!(tgz.header("content-type").as_deref(), Some("application/gzip"));
    assert!(tgz.header("content-disposition").unwrap().contains("cats.tar.gz"));

    let from_tar = tar_entries(&tar.body);
    assert_eq!(
        from_tar.len(),
        5,
        "set.toml, three items, the cover: {:?}",
        from_tar.keys()
    );
    assert!(from_tar.contains_key("cats/set.toml"));
    assert_eq!(zip_entries(&zip.body), from_tar);
    let mut untarred = Vec::new();
    flate2::read::GzDecoder::new(&tgz.body[..])
        .read_to_end(&mut untarred)
        .unwrap();
    assert_eq!(untarred, tar.body, "the tar.gz is the tar, compressed");

    // Private sets: 404 for others, like the tar.
    let private = srv.create_set(&alice, "secret", Visibility::Private).await;
    for route in [routes::SET_EXPORT, routes::SET_EXPORT_ZIP, routes::SET_EXPORT_TGZ] {
        srv.get(&bob, &routes::path(route, &[&private.id])).await.expect(404);
        srv.get(&alice, &routes::path(route, &[&private.id])).await.expect(200);
    }
}

// ---------------------------------------------------------------------------
// WhatsApp
// ---------------------------------------------------------------------------

async fn build(srv: &TestServer, u: &TestUser, set_id: &str) -> Job {
    let j: Job = srv
        .post(u, &routes::path(routes::SET_WHATSAPP, &[set_id]), json!({}))
        .await
        .expect(200)
        .json();
    assert_eq!(j.kind, JobKind::WhatsappExport);
    run(srv, u, &j).await
}

async fn wa_status(srv: &TestServer, u: &TestUser, set_id: &str) -> WhatsappStatus {
    srv.get(u, &routes::path(routes::SET_WHATSAPP, &[set_id]))
        .await
        .expect(200)
        .json()
}

/// Check one `.wastickers` the way WhatsApp's sample app checks a pack.
fn check_pack(bytes: &[u8], animated: bool, title: &str, author: &str) -> BTreeMap<String, Vec<u8>> {
    let files = zip_entries(bytes);
    assert_eq!(files["title.txt"], title.as_bytes());
    assert_eq!(files["author.txt"], author.as_bytes());
    let tray = &files["cover.png"];
    assert!(tray.starts_with(b"\x89PNG"));
    assert!(tray.len() as u64 <= wa::TRAY_MAX_BYTES);
    let img = image::load_from_memory(tray).unwrap();
    assert_eq!((img.width(), img.height()), (wa::TRAY_SIDE, wa::TRAY_SIDE));
    let stickers: Vec<&String> = files.keys().filter(|k| k.ends_with(".webp")).collect();
    assert!(
        (wa::PACK_MIN_STICKERS..=wa::PACK_MAX_STICKERS).contains(&stickers.len()),
        "{}",
        stickers.len()
    );
    for name in stickers {
        let b = &files[name];
        let facts = wa::inspect(b).unwrap();
        assert_eq!((facts.width, facts.height), (wa::SIDE, wa::SIDE), "{name}");
        assert_eq!(facts.animated, animated, "{name}");
        if animated {
            assert!(b.len() as u64 <= wa::ANIMATED_MAX_BYTES);
            assert!(facts.durations.len() > 1);
            assert!(facts.durations.iter().all(|d| *d >= wa::MIN_FRAME_MS));
            assert!(facts.durations.iter().map(|d| u64::from(*d)).sum::<u64>() <= wa::MAX_DURATION_MS);
        } else {
            assert!(b.len() as u64 <= wa::STATIC_MAX_BYTES, "{name}: {}", b.len());
        }
    }
    files
}

#[tokio::test]
async fn whatsapp_packs_are_split_padded_and_within_the_limits() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let set = srv.create_set(&alice, "lots", Visibility::Public).await;

    // Nothing built yet.
    let st = wa_status(&srv, &bob, &set.id).await;
    assert!(st.build.is_none() && !st.current && st.job.is_none());
    srv.get(&bob, &routes::path(routes::SET_WHATSAPP_ZIP, &[&set.id]))
        .await
        .expect(404);

    // 31 stills (the first one wide, the second with emoji to filter) and
    // one animation: too few animations for an animated pack, so it goes in
    // as a still, and the 32 stills make two packs of 16.
    let wide = common::encode(&common::image(240, 80, 1, 0), image::ImageFormat::Png);
    let mut files = vec![("wide.png".to_string(), wide)];
    for i in 2..=31u64 {
        files.push((format!("s{i:02}.png"), png(i)));
    }
    files.push(("moving.gif".to_string(), common::gif_anim(40)));
    let meta: Vec<Value> = (0..files.len())
        .map(|i| match i {
            1 => json!({"emojis": ["😀", "smile", "🐱", "🐶", "🦊"]}),
            _ => json!({}),
        })
        .collect();
    srv.upload(
        &alice,
        &set.id,
        files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect(),
        Some(json!(meta)),
    )
    .await
    .expect(200);
    srv.patch(&alice, routes::ME, json!({"display_name": "Alice A."}))
        .await
        .expect(200);

    // A reader builds them.
    let j = build(&srv, &bob, &set.id).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    assert_eq!(r["reused"], false);
    let built: WhatsappBuild = serde_json::from_value(r).unwrap();
    assert_eq!(built.packs.len(), 2);
    assert_eq!(
        built.packs.iter().map(|p| p.stickers.len()).collect::<Vec<_>>(),
        [16, 16]
    );
    assert!(built.skipped.is_empty(), "{:?}", built.skipped);
    assert_eq!(built.packs[0].title, "lots (1/2)");
    assert_eq!(built.packs[1].identifier, "lots-2");
    assert_eq!(built.packs[0].publisher, "Alice A.");
    assert!(!built.packs[0].animated && !built.packs[1].animated);
    let first = &built.packs[0].stickers;
    assert_eq!(first[0].file, "01.webp");
    assert_eq!(first[1].emojis, ["😀", "🐱", "🐶"], "emoji only, at most 3");
    assert_eq!(first[0].emojis, [stickpile_telegram::DEFAULT_EMOJI]);
    let gif = built.packs[1].stickers.last().unwrap();
    assert_eq!(gif.name, "moving");
    assert!(!gif.animated);
    assert!(gif.note.as_deref().unwrap().contains("at least 3 animated"), "{gif:?}");

    // The download: both packs and contents.json.
    let zip = srv.get(&bob, &built.url).await.expect(200);
    assert_eq!(zip.header("content-type").as_deref(), Some("application/zip"));
    let outer = zip_entries(&zip.body);
    assert_eq!(
        outer.keys().collect::<Vec<_>>(),
        ["contents.json", "lots-1.wastickers", "lots-2.wastickers"]
    );
    let contents: Value = serde_json::from_slice(&outer["contents.json"]).unwrap();
    assert_eq!(contents["sticker_packs"][1]["identifier"], "lots-2");
    assert_eq!(contents["sticker_packs"][0]["tray_image_file"], "cover.png");
    assert_eq!(contents["sticker_packs"][0]["animated_sticker_pack"], false);
    assert_eq!(
        contents["sticker_packs"][0]["stickers"][1]["emojis"],
        json!(["😀", "🐱", "🐶"])
    );
    let pack1 = check_pack(&outer["lots-1.wastickers"], false, "lots (1/2)", "Alice A.");
    check_pack(&outer["lots-2.wastickers"], false, "lots (2/2)", "Alice A.");
    // The wide item is padded to a square: transparent above and below.
    let wide = image::load_from_memory(&pack1["01.webp"]).unwrap().to_rgba8();
    assert_eq!(wide.get_pixel(256, 10)[3], 0);
    assert_eq!(wide.get_pixel(256, 256)[3], 255);
    // One pack alone, as listed.
    let one = srv.get(&bob, &built.packs[1].url).await.expect(200);
    assert_eq!(one.body, outer["lots-2.wastickers"]);
    assert!(one.header("content-disposition").unwrap().contains("lots-2.wastickers"));
    srv.get(&bob, &routes::path(routes::SET_WHATSAPP_PACK, &[&set.id, "3"]))
        .await
        .expect(404);

    // Unchanged: the build is reused. A new emoji makes it out of date.
    let st = wa_status(&srv, &alice, &set.id).await;
    assert!(st.current);
    let again = build(&srv, &alice, &set.id).await;
    assert_eq!(again.result.unwrap()["reused"], true);
    let item = &built.packs[0].stickers[0].item_id;
    srv.patch(&alice, &routes::path(routes::ITEM, &[item]), json!({"emojis": ["🎉"]}))
        .await
        .expect(200);
    assert!(!wa_status(&srv, &bob, &set.id).await.current);
    let rebuilt = build(&srv, &bob, &set.id).await;
    let r = rebuilt.result.unwrap();
    assert_eq!(r["reused"], false);
    assert_eq!(r["packs"][0]["stickers"][0]["emojis"], json!(["🎉"]));
    assert!(wa_status(&srv, &bob, &set.id).await.current);
}

#[tokio::test]
async fn whatsapp_animated_pack_leaves_out_lonely_stills() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let set = srv.create_set(&alice, "moving", Visibility::Private).await;
    let still = srv.upload_one(&alice, &set.id, "still.png", png(1)).await;
    for i in 0..3 {
        srv.upload_one(&alice, &set.id, &format!("m{i}.gif"), common::gif_anim(10 + i))
            .await;
    }
    let j = build(&srv, &alice, &set.id).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let built: WhatsappBuild = serde_json::from_value(j.result.unwrap()).unwrap();
    assert_eq!(built.packs.len(), 1);
    assert!(built.packs[0].animated);
    assert_eq!(built.packs[0].file, "moving.wastickers");
    assert_eq!(built.packs[0].title, "moving");
    assert_eq!(built.skipped.len(), 1);
    assert_eq!(built.skipped[0].item_id, still.id);
    assert!(built.skipped[0].reason.contains("cannot mix"), "{:?}", built.skipped);
    let pack = srv.get(&alice, &built.packs[0].url).await.expect(200);
    check_pack(&pack.body, true, "moving", "alice");

    // Only the owner (and readers) see it; a private set is 404 to others.
    let bob = srv.user("bob").await;
    srv.get(&bob, &routes::path(routes::SET_WHATSAPP, &[&set.id]))
        .await
        .expect(404);
    srv.post(&bob, &routes::path(routes::SET_WHATSAPP, &[&set.id]), json!({}))
        .await
        .expect(404);
    srv.get(&bob, &built.url).await.expect(404);
    srv.get(&bob, &built.packs[0].url).await.expect(404);
}

#[tokio::test]
async fn whatsapp_needs_three_stickers() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let set = srv.create_set(&alice, "two", Visibility::Private).await;
    srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
    srv.upload_one(&alice, &set.id, "b.png", png(2)).await;
    let j = build(&srv, &alice, &set.id).await;
    assert_eq!(j.status, JobStatus::Failed);
    assert_eq!(j.attempts, 1);
    assert!(j.error.as_deref().unwrap().contains("at least 3 stickers"), "{j:?}");
    assert!(wa_status(&srv, &alice, &set.id).await.build.is_none());
}

#[tokio::test]
async fn a_queued_build_is_reused_and_the_export_of_a_deleted_set_is_removed() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let set = srv.create_set(&alice, "three", Visibility::Private).await;
    for i in 1..=3 {
        srv.upload_one(&alice, &set.id, &format!("{i}.png"), png(i)).await;
    }
    let path = routes::path(routes::SET_WHATSAPP, &[&set.id]);
    let a: Job = srv.post(&alice, &path, json!({})).await.expect(200).json();
    let b: Job = srv.post(&alice, &path, json!({})).await.expect(200).json();
    assert_eq!(a.id, b.id, "the queued build is returned");
    assert_eq!(wa_status(&srv, &alice, &set.id).await.job.unwrap().id, a.id);
    assert_eq!(run(&srv, &alice, &a).await.status, JobStatus::Done);
    let dir = srv.state.config.exports_dir().join(&set.id);
    assert!(dir.join("whatsapp").join("three-whatsapp.zip").is_file());
    srv.delete(&alice, &routes::path(routes::SET, &[&set.id]))
        .await
        .expect(204);
    stickpile_server::cleanup_once(&srv.state).await.unwrap();
    assert!(!dir.exists());
}

// ---------------------------------------------------------------------------
// Publishing
// ---------------------------------------------------------------------------

struct Tg {
    srv: TestServer,
    mock: Shared,
    _token_dir: tempfile::TempDir,
}

async fn telegram_server() -> Tg {
    let (url, mock) = mock_bot::start().await;
    let token_dir = tempfile::tempdir().unwrap();
    let token = token_dir.path().join("token");
    std::fs::write(&token, format!("{}\n", mock_bot::TOKEN)).unwrap();
    let srv = TestServer::with_config(|c| {
        c.telegram.enabled = true;
        c.telegram.bot_token_file = Some(token);
        c.telegram.api_url = url;
    })
    .await;
    Tg {
        srv,
        mock,
        _token_dir: token_dir,
    }
}

async fn publications(srv: &TestServer, u: &TestUser, set_id: &str) -> Vec<Publication> {
    srv.get(u, &routes::path(routes::SET_PUBLISH, &[set_id]))
        .await
        .expect(200)
        .json()
}

#[tokio::test]
async fn publish_queues_one_job_per_platform_through_the_registry() {
    let tg = telegram_server().await;
    let srv = &tg.srv;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    srv.patch(&alice, routes::ME, json!({"telegram_user_id": 555}))
        .await
        .expect(200);
    let set = srv.create_set(&alice, "pub-cats", Visibility::Public).await;
    for i in 1..=3 {
        srv.upload_one(&alice, &set.id, &format!("{i}.png"), png(i)).await;
    }
    let url = routes::path(routes::SET_PUBLISH, &[&set.id]);

    // Not published yet. Every platform the server knows is listed, the
    // ones it is not set up for as such.
    let list = publications(srv, &alice, &set.id).await;
    let known: Vec<(&str, bool)> = list.iter().map(|p| (p.platform.as_str(), p.configured)).collect();
    assert_eq!(known, [("telegram", true), ("discord", false), ("signal", false)]);
    assert_eq!(list[0].platform, "telegram");
    assert_eq!(list[0].name, "Telegram");
    assert!(list[0].configured);
    assert!(list[0].target.is_none() && list[0].published_at.is_none() && list[0].job.is_none());

    // Bad requests queue nothing.
    for (body, needle) in [
        (json!({"platforms": []}), "at least one platform"),
        (json!({"platforms": ["telegram", "myspace"]}), "unknown platform"),
        (
            json!({"platforms": ["telegram"], "options": {"telegram": {"pack_name": "bad name!"}}}),
            "pack_name",
        ),
        (
            json!({"platforms": ["telegram"], "options": {"telegram": 5}}),
            "options.telegram",
        ),
    ] {
        let r = srv.post(&alice, &url, body.clone()).await.expect(400);
        assert!(r.text().contains(needle), "{body}: {}", r.text());
    }
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'telegram_export'")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(queued, 0);

    // Readers see neither the publish state nor may they publish.
    srv.get(&bob, &url).await.expect(403);
    srv.post(&bob, &url, json!({"platforms": ["telegram"]}))
        .await
        .expect(403);

    // One job per platform (a platform listed twice runs once), the same
    // job the Telegram route queues.
    let jobs: PublishJobs = srv
        .post(
            &alice,
            &url,
            json!({"platforms": ["telegram", "telegram"], "options": {"telegram": {"title": "Published cats"}}}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(jobs.jobs.len(), 1);
    assert_eq!(jobs.jobs[0].kind, JobKind::TelegramExport);
    assert_eq!(jobs.jobs[0].subject.as_deref(), Some(set.id.as_str()));
    let list = publications(srv, &alice, &set.id).await;
    assert_eq!(
        list[0].job.as_ref().map(|j| j.id.as_str()),
        Some(jobs.jobs[0].id.as_str())
    );

    let done = run(srv, &alice, &jobs.jobs[0]).await;
    assert_eq!(done.status, JobStatus::Done, "{done:?}");
    let name = format!("pub_cats_by_{BOT}");
    {
        let m = tg.mock.lock().unwrap();
        assert_eq!(m.packs[&name].title, "Published cats");
        assert_eq!(m.packs[&name].stickers.len(), 3);
    }
    let list = publications(srv, &alice, &set.id).await;
    let p = &list[0];
    assert_eq!(p.target.as_deref(), Some(name.as_str()));
    assert_eq!(
        p.url.as_deref(),
        Some(format!("https://t.me/addstickers/{name}").as_str())
    );
    assert_eq!(p.last_status, Some(JobStatus::Done));
    assert!(p.published_at.is_some() && p.job.is_none());
    assert_eq!(p.last_result.as_ref().unwrap()["exported"], 3);

    // A failed publish is recorded next to the last good one.
    let other: PublishJobs = srv
        .post(
            &alice,
            &url,
            json!({"platforms": ["telegram"], "options": {"telegram": {"pack_name": "x_by_other_bot"}}}),
        )
        .await
        .expect(200)
        .json();
    let failed = run(srv, &alice, &other.jobs[0]).await;
    assert_eq!(failed.status, JobStatus::Failed);
    let p = &publications(srv, &alice, &set.id).await[0];
    assert_eq!(p.last_status, Some(JobStatus::Failed));
    assert!(p.last_error.as_deref().unwrap().contains("x_by_other_bot"), "{p:?}");
    assert_eq!(
        p.last_result.as_ref().unwrap()["exported"],
        3,
        "the last good result stays"
    );
    assert_eq!(p.target.as_deref(), Some(name.as_str()));

    // The Telegram route itself is unchanged and is noted the same way.
    let j: Job = srv
        .post(
            &alice,
            &routes::path(routes::SET_TELEGRAM_EXPORT, &[&set.id]),
            json!({}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(run(srv, &alice, &j).await.status, JobStatus::Done);
    let p = &publications(srv, &alice, &set.id).await[0];
    assert_eq!(p.last_status, Some(JobStatus::Done));
    assert_eq!(p.last_error, None);
}

#[tokio::test]
async fn publishing_to_an_unconfigured_platform_is_refused() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let set = srv.create_set(&alice, "cats", Visibility::Private).await;
    srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
    let r = srv
        .post(
            &alice,
            &routes::path(routes::SET_PUBLISH, &[&set.id]),
            json!({"platforms": ["telegram"]}),
        )
        .await
        .expect(400);
    assert!(r.text().contains("not configured"), "{}", r.text());
    let list = publications(&srv, &alice, &set.id).await;
    assert_eq!(list.len(), 3);
    assert!(list.iter().all(|p| !p.configured));
    // Someone else's private set: 404 as everywhere.
    let bob = srv.user("bob").await;
    srv.get(&bob, &routes::path(routes::SET_PUBLISH, &[&set.id]))
        .await
        .expect(404);
}

#[tokio::test]
async fn migration_seeds_the_publish_state_of_telegram_packs() {
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::ConnectOptions;
    let dir = tempfile::tempdir().unwrap();
    let mut conn = SqliteConnectOptions::new()
        .filename(dir.path().join("db.sqlite"))
        .create_if_missing(true)
        .connect()
        .await
        .unwrap();
    for sql in [
        include_str!("../migrations/0001_init.sql"),
        include_str!("../migrations/0002_gitpack_sync.sql"),
        include_str!("../migrations/0003_telegram_pack_owners.sql"),
        include_str!("../migrations/0004_set_cover.sql"),
        include_str!("../migrations/0005_user_theme.sql"),
        include_str!("../migrations/0006_set_cover_image.sql"),
        include_str!("../migrations/0007_problem_dismissals.sql"),
        include_str!("../migrations/0008_variants_version.sql"),
        include_str!("../migrations/0009_collaboration.sql"),
        include_str!("../migrations/0010_user_language.sql"),
        include_str!("../migrations/0011_pairing_codes.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&mut conn).await.unwrap();
    }
    sqlx::raw_sql(
        "INSERT INTO users (id, username, display_name, role, created_at) VALUES ('u1', 'alice', 'Alice', 'user', 1);
         INSERT INTO sets (id, owner_id, slug, name, description, visibility, version, source, created_at, updated_at, telegram_pack)
           VALUES ('s1', 'u1', 'cats', 'Cats', '', 'private', 1, 'local', 1, 1, 'cats_by_bot'),
                  ('s2', 'u1', 'dogs', 'Dogs', '', 'private', 1, 'local', 1, 1, NULL),
                  ('s3', 'u1', 'owls', 'Owls', '', 'private', 1, 'local', 1, 1, 'owls_by_bot');
         INSERT INTO jobs (id, kind, payload, subject, status, attempts, result, owner_id, run_after, created_at, finished_at)
           VALUES ('j1', 'telegram_export', '{}', 's1', 'done', 1, '{\"exported\": 4}', 'u1', 0, 10, 20),
                  ('j2', 'telegram_export', '{}', 's1', 'done', 1, '{\"exported\": 5}', 'u1', 0, 30, 40),
                  ('j3', 'telegram_export', '{}', 's1', 'failed', 3, NULL, 'u1', 0, 50, 60);",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0012_set_publications.sql"))
        .execute(&mut conn)
        .await
        .unwrap();
    type Row = (String, String, Option<i64>, Option<String>, Option<String>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT set_id, platform, published_at, last_result, last_status FROM set_publications ORDER BY set_id",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            (
                "s1".into(),
                "telegram".into(),
                Some(40),
                Some("{\"exported\": 5}".into()),
                Some("done".into())
            ),
            ("s3".into(), "telegram".into(), None, None, None),
        ]
    );
}

// ---------------------------------------------------------------------------
// The export page
// ---------------------------------------------------------------------------

#[tokio::test]
async fn export_page_downloads_for_readers_publishing_for_the_owner() {
    use reqwest::StatusCode;
    use web_common::{assert_fragment, assert_page, location, Browser};

    let tg = telegram_server().await;
    let srv = &tg.srv;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let set = srv.create_set(&alice, "web-cats", Visibility::Public).await;
    for i in 1..=3 {
        srv.upload_one(&alice, &set.id, &format!("{i}.png"), png(i)).await;
    }
    let page = format!("/sets/{}/export", set.id);

    // A reader: downloads and WhatsApp, no publishing.
    let bb = Browser::login(srv, &bob).await;
    let r = bb.get(&page).await;
    assert_page(&r);
    let html = r.text();
    for route in [routes::SET_EXPORT_ZIP, routes::SET_EXPORT, routes::SET_EXPORT_TGZ] {
        assert!(html.contains(&routes::path(route, &[&set.id])), "{route}");
    }
    assert!(html.contains("RAR is not offered"));
    assert!(html.contains("Build WhatsApp packs"));
    assert!(!html.contains("id=\"publish\""), "readers do not publish");
    let r = bb
        .post_form(&format!("{page}/publish"), &[("platform.telegram", "on")])
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // The reader builds the WhatsApp packs; htmx gets the job fragment.
    let r = bb.post_htmx(&format!("{page}/whatsapp"), &[]).await;
    assert_fragment(&r);
    assert!(r.text().contains("WhatsApp packs"), "{}", r.text());
    run_jobs_now(srv).await;
    let html = bb.get(&page).await.text();
    assert!(html.contains("web-cats.wastickers"), "{html}");
    assert!(html.contains("Download all packs"));

    // The owner: one row per configured platform, its options and hint.
    let ab = Browser::login(srv, &alice).await;
    let r = ab.get(&page).await;
    assert_page(&r);
    let html = r.text();
    assert!(html.contains("id=\"publish\""));
    assert!(html.contains("name=\"platform.telegram\""));
    assert!(html.contains("name=\"telegram.pack_name\""));
    assert!(html.contains("Set your Telegram user id in Settings"));
    assert!(html.contains("not yet"));
    // Errors keep the form; nothing is queued.
    let r = ab.post_form(&format!("{page}/publish"), &[]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("choose at least one platform"));
    let r = ab
        .post_form(
            &format!("{page}/publish"),
            &[("platform.telegram", "on"), ("telegram.pack_name", "bad name!")],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(
        r.text().contains("value=\"bad name!\""),
        "the form keeps what was typed"
    );
    // Publish with the row's own button: back to the page with its job.
    srv.patch(&alice, routes::ME, json!({"telegram_user_id": 42}))
        .await
        .expect(200);
    let r = ab
        .post_form(
            &format!("{page}/publish"),
            &[("only", "telegram"), ("telegram.title", "Web cats")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let back = location(&r);
    assert!(back.starts_with(&format!("{page}?jobs=")), "{back}");
    let html = ab.get(&back).await.text();
    assert!(html.contains("Telegram export"));
    assert!(html.contains("hx-trigger=\"every 2s\""));
    run_jobs_now(srv).await;
    let html = ab.get(&page).await.text();
    assert!(
        html.contains(&format!("https://t.me/addstickers/web_cats_by_{BOT}")),
        "{html}"
    );
    assert!(html.contains("3 stickers: 3 added, 0 removed, 0 left out"), "{html}");
    assert_eq!(
        tg.mock.lock().unwrap().packs[&format!("web_cats_by_{BOT}")].title,
        "Web cats"
    );
    // The set page links here, Telegram's button to the publish section.
    let html = ab.get(&format!("/sets/{}", set.id)).await.text();
    assert!(html.contains(&format!("href=\"{page}\"")));
    assert!(html.contains(&format!("href=\"{page}#publish\"")));
}

// ---------------------------------------------------------------------------
// Several platforms at once
// ---------------------------------------------------------------------------

#[tokio::test]
async fn publish_to_telegram_discord_and_signal_at_once() {
    use integrations_support::{mock_discord, mock_signal};
    use reqwest::StatusCode;
    use web_common::{assert_page, location, Browser};

    let (tg_url, tg) = mock_bot::start().await;
    let (dc_api, dc_cdn, dc_media, dc) = mock_discord::start().await;
    let (sg_service, sg_cdn, sg) = mock_signal::start().await;
    let dir = tempfile::tempdir().unwrap();
    let tg_token = dir.path().join("tg");
    std::fs::write(&tg_token, mock_bot::TOKEN).unwrap();
    let dc_token = dir.path().join("dc");
    std::fs::write(&dc_token, mock_discord::TOKEN).unwrap();
    let sg_creds = dir.path().join("sg.json");
    std::fs::write(
        &sg_creds,
        json!({"aci": mock_signal::ACI, "device_id": mock_signal::DEVICE, "password": mock_signal::PASSWORD})
            .to_string(),
    )
    .unwrap();
    let srv = TestServer::with_config(|c| {
        c.telegram.enabled = true;
        c.telegram.bot_token_file = Some(tg_token);
        c.telegram.api_url = tg_url;
        c.discord.enabled = true;
        c.discord.bot_token_file = Some(dc_token);
        c.discord.api_url = dc_api;
        c.discord.cdn_url = dc_cdn;
        c.discord.media_url = dc_media;
        c.signal.enabled = true;
        c.signal.credentials_file = Some(sg_creds);
        c.signal.service_url = sg_service;
        c.signal.cdn_url = sg_cdn;
    })
    .await;
    let alice = srv.user("alice").await;
    srv.patch(&alice, routes::ME, json!({"telegram_user_id": 555}))
        .await
        .expect(200);
    let set = srv.create_set(&alice, "all-cats", Visibility::Private).await;
    for i in 1..=3 {
        srv.upload_one(&alice, &set.id, &format!("cat {i}.png"), png(i)).await;
    }
    let url = routes::path(routes::SET_PUBLISH, &[&set.id]);

    // One bad option fails the whole request: nothing is queued.
    let r = srv
        .post(&alice, &url, json!({"platforms": ["telegram", "discord", "signal"]}))
        .await
        .expect(400);
    assert!(r.text().contains("guild_id is required"), "{}", r.text());
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind LIKE '%_export'")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(queued, 0);

    let jobs: PublishJobs = srv
        .post(
            &alice,
            &url,
            json!({
                "platforms": ["telegram", "discord", "signal"],
                "options": {"discord": {"guild_id": mock_discord::GUILD, "kind": "emojis"}},
            }),
        )
        .await
        .expect(200)
        .json();
    let kinds: Vec<JobKind> = jobs.jobs.iter().map(|j| j.kind).collect();
    assert_eq!(
        kinds,
        [JobKind::TelegramExport, JobKind::DiscordExport, JobKind::SignalExport]
    );
    assert!(jobs.jobs.iter().all(|j| j.subject.as_deref() == Some(set.id.as_str())));
    sqlx::query("DELETE FROM jobs WHERE kind = 'variants'")
        .execute(&srv.state.db)
        .await
        .unwrap();
    run_jobs_now(&srv).await;
    for j in &jobs.jobs {
        let done = job(&srv, &alice, &j.id).await;
        assert_eq!(done.status, JobStatus::Done, "{done:?}");
    }
    assert_eq!(
        tg.lock().unwrap().packs[&format!("all_cats_by_{BOT}")].stickers.len(),
        3
    );
    assert_eq!(dc.lock().unwrap().emojis.len(), 3);
    assert_eq!(sg.lock().unwrap().forms.len(), 1);
    let list = publications(&srv, &alice, &set.id).await;
    assert!(
        list.iter().all(|p| p.configured && p.published_at.is_some()),
        "{list:?}"
    );
    assert!(list[2]
        .url
        .as_deref()
        .unwrap()
        .starts_with("https://signal.art/addstickers/#"));

    // The export page: a row each, Discord's choices and last server
    // prefilled, Signal's note about new links, results per platform.
    let b = Browser::login(&srv, &alice).await;
    let page = format!("/sets/{}/export", set.id);
    let r = b.get(&page).await;
    assert_page(&r);
    let html = r.text();
    for platform in ["telegram", "discord", "signal"] {
        assert!(html.contains(&format!("name=\"platform.{platform}\"")), "{platform}");
    }
    assert!(html.contains(&format!("value=\"{}\"", mock_discord::GUILD)), "{html}");
    assert!(html.contains("<option value=\"emojis\" selected>"), "{html}");
    assert!(html.contains("every publish uploads new packs with new links"));
    assert!(html.contains("3 emoji: 3 added, 0 removed, 0 left out"), "{html}");
    assert!(html.contains("3 stickers in a new pack, 0 left out"), "{html}");
    assert!(html.contains("Cat Lounge"));
    // The form publishes the ticked rows with their options, in one go.
    let r = b
        .post_form(
            &format!("{page}/publish"),
            &[
                ("platform.discord", "on"),
                ("discord.guild_id", mock_discord::GUILD),
                ("discord.kind", "stickers"),
                ("platform.signal", "on"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let back = location(&r);
    let ids: Vec<String> = back
        .split("jobs=")
        .nth(1)
        .unwrap()
        .split('#')
        .next()
        .unwrap()
        .split(',')
        .map(str::to_string)
        .collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(job(&srv, &alice, &ids[0]).await.kind, JobKind::DiscordExport);
    run_jobs_now(&srv).await;
    assert_eq!(dc.lock().unwrap().stickers.len(), 3, "stickers this time");
    assert_eq!(sg.lock().unwrap().forms.len(), 2, "a second Signal pack");
}
