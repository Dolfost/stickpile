//! Telegram import and export against the in-process mock Bot API
//! (`integrations_support::mock_bot`).

mod common;
mod integrations_support;

use common::{png, TestServer, TestUser};
use integrations_support::mock_bot::{self, unique_id, Shared, BOT};
use integrations_support::run_jobs_now;
use serde_json::{json, Value};
use stickpile_api::{routes, Job, JobKind, JobStatus, Set, SetDetail, SetSource, User, Visibility};

struct Tg {
    srv: TestServer,
    mock: Shared,
    _token_dir: tempfile::TempDir,
}

async fn setup(f: impl FnOnce(&mut stickpile_server::Config)) -> Tg {
    let (url, mock) = mock_bot::start().await;
    let token_dir = tempfile::tempdir().unwrap();
    let token = token_dir.path().join("token");
    std::fs::write(&token, format!("{}\n", mock_bot::TOKEN)).unwrap();
    let srv = TestServer::with_config(|c| {
        c.telegram.enabled = true;
        c.telegram.bot_token_file = Some(token);
        c.telegram.api_url = url;
        f(c);
    })
    .await;
    Tg {
        srv,
        mock,
        _token_dir: token_dir,
    }
}

fn webp(seed: u64) -> Vec<u8> {
    common::encode(&common::image(512, 512, seed, 0), image::ImageFormat::WebP)
}

async fn job(srv: &TestServer, u: &TestUser, id: &str) -> Job {
    srv.get(u, &routes::path(routes::JOB, &[id])).await.expect(200).json()
}

async fn run(srv: &TestServer, u: &TestUser, j: &Job) -> Job {
    run_jobs_now(srv).await;
    job(srv, u, &j.id).await
}

async fn export(srv: &TestServer, u: &TestUser, set_id: &str, body: Value) -> Job {
    let j: Job = srv
        .post(u, &routes::path(routes::SET_TELEGRAM_EXPORT, &[set_id]), body)
        .await
        .expect(200)
        .json();
    assert_eq!(j.kind, JobKind::TelegramExport);
    run(srv, u, &j).await
}

async fn detail(srv: &TestServer, u: &TestUser, id: &str) -> SetDetail {
    srv.get(u, &routes::path(routes::SET, &[id])).await.expect(200).json()
}

#[tokio::test]
async fn import_pack_skips_tgs_and_duplicates() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let alice = srv.user("alice").await;
    tg.mock.lock().unwrap().seed(
        "Animals",
        "Animal friends",
        1,
        vec![
            ("static", webp(1), "🐱"),
            ("static", webp(2), "🐶"),
            ("animated", b"{\"tgs\": true}".to_vec(), "🦊"),
            ("static", webp(1), "😺"),
        ],
    );
    let j: Job = srv
        .post(
            &alice,
            routes::TELEGRAM_IMPORT,
            json!({"pack_name": "https://t.me/addstickers/Animals", "visibility": "unlisted"}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(j.kind, JobKind::TelegramImport);
    let j = run(srv, &alice, &j).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    assert_eq!(r["imported"], 2, "{r}");
    assert_eq!(r["bound"], false, "someone else's pack");
    assert_eq!(r["url"], "https://t.me/addstickers/Animals");
    let reasons: Vec<&str> = r["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["reason"].as_str().unwrap())
        .collect();
    assert!(reasons.contains(&stickpile_telegram::TGS_SKIP_REASON), "{reasons:?}");
    assert!(reasons.iter().any(|r| r.contains("same file")), "{reasons:?}");

    let d = detail(srv, &alice, r["set_id"].as_str().unwrap()).await;
    assert_eq!(d.set.slug, "animals");
    assert_eq!(d.set.name, "Animal friends");
    assert_eq!(d.set.source, SetSource::Telegram);
    assert_eq!(d.set.visibility, Visibility::Unlisted);
    assert_eq!(d.set.telegram_pack, None);
    assert_eq!(d.items.len(), 2);
    assert_eq!(d.items[0].emojis, ["🐱"]);
    assert_eq!(d.items[1].emojis, ["🐶"]);
    assert_eq!(d.items[0].media.format, stickpile_api::MediaFormat::Webp);

    // Same slug again -> 409 right away; an unknown pack fails the job.
    srv.post(&alice, routes::TELEGRAM_IMPORT, json!({"pack_name": "Animals"}))
        .await
        .expect(409);
    srv.post(&alice, routes::TELEGRAM_IMPORT, json!({"pack_name": "not a name"}))
        .await
        .expect(400);
    let j: Job = srv
        .post(
            &alice,
            routes::TELEGRAM_IMPORT,
            json!({"pack_name": "Nope", "slug": "nope"}),
        )
        .await
        .expect(200)
        .json();
    let j = run(srv, &alice, &j).await;
    assert_eq!(j.status, JobStatus::Failed);
    assert_eq!(j.attempts, 1);
    assert_eq!(j.error.as_deref(), Some("sticker pack not found"));
    // The token never leaks into job results or errors.
    let all = serde_json::to_string(&srv.get(&alice, &routes::path(routes::JOB, &[&j.id])).await.value()).unwrap();
    assert!(!all.contains(mock_bot::TOKEN));
}

#[tokio::test]
async fn export_creates_then_updates_the_pack() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let alice = srv.user("alice").await;
    let set = srv.create_set(&alice, "my-cats", Visibility::Private).await;
    let a = srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
    let b = srv.upload_one(&alice, &set.id, "b.png", png(2)).await;
    let c = srv.upload_one(&alice, &set.id, "c.jpg", common::jpeg(3)).await;
    srv.patch(
        &alice,
        &routes::path(routes::ITEM, &[&a.id]),
        json!({"emojis": ["😼", "🐈"], "keywords": ["cat", "smug"]}),
    )
    .await
    .expect(200);
    let gif = srv.upload_one(&alice, &set.id, "d.gif", common::gif_anim(4)).await;
    // The Variants jobs have not run: the export renders what it needs.
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'variants' AND status = 'queued'")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(pending, 4);
    // Make sure the in-job rendering is exercised, not the queue.
    sqlx::query("DELETE FROM jobs WHERE kind = 'variants'")
        .execute(&srv.state.db)
        .await
        .unwrap();

    // No Telegram user id anywhere -> a clear 400.
    let r = srv
        .post(
            &alice,
            &routes::path(routes::SET_TELEGRAM_EXPORT, &[&set.id]),
            json!({}),
        )
        .await
        .expect(400);
    assert!(r.text().contains("telegram_user_id"), "{}", r.text());
    let me: User = srv
        .patch(&alice, routes::ME, json!({"telegram_user_id": 555}))
        .await
        .expect(200)
        .json();
    assert_eq!(me.telegram_user_id, Some(555));

    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    let name = format!("my_cats_by_{BOT}");
    assert_eq!(r["pack_name"], name.as_str());
    assert_eq!(r["url"], format!("https://t.me/addstickers/{name}"));
    assert_eq!(r["created_pack"], true);
    let video = common::ffmpeg_available() && r["exported"] == 4;
    if !video {
        assert_eq!(r["exported"], 3, "{r}");
        assert_eq!(r["skipped"][0]["item_id"], gif.id.as_str(), "{r}");
    }
    {
        let m = tg.mock.lock().unwrap();
        let pack = &m.packs[&name];
        assert_eq!(pack.owner, 555);
        assert_eq!(pack.title, "my-cats");
        assert_eq!(pack.stickers.len(), if video { 4 } else { 3 });
        assert_eq!(pack.stickers[0].emoji_list, ["😼", "🐈"]);
        assert_eq!(pack.stickers[0].keywords, ["cat", "smug"]);
        assert_eq!(pack.stickers[1].emoji_list, [stickpile_telegram::DEFAULT_EMOJI]);
        assert!(pack.stickers[..3].iter().all(|s| s.format == "static"));
        if video {
            assert_eq!(pack.stickers[3].format, "video");
        }
    }
    // The stickers are the server's tg_sticker renditions.
    let sticker = srv
        .get(&alice, &routes::path(routes::ITEM_FILE, &[&a.id, "tg_sticker"]))
        .await
        .expect(200);
    assert_eq!(
        tg.mock.lock().unwrap().packs[&name].stickers[0].fuid,
        unique_id(&sticker.body)
    );
    let s: SetDetail = detail(srv, &alice, &set.id).await;
    assert_eq!(s.set.telegram_pack.as_deref(), Some(name.as_str()));

    // Update: remove b, add e, put e first, change a's emoji, new title.
    srv.delete(&alice, &routes::path(routes::ITEM, &[&b.id]))
        .await
        .expect(204);
    if !video {
        srv.delete(&alice, &routes::path(routes::ITEM, &[&gif.id]))
            .await
            .expect(204);
    }
    let e = srv.upload_one(&alice, &set.id, "e.png", png(5)).await;
    srv.patch(&alice, &routes::path(routes::ITEM, &[&a.id]), json!({"emojis": ["🙀"]}))
        .await
        .expect(200);
    let order: Vec<String> = detail(srv, &alice, &set.id)
        .await
        .items
        .iter()
        .map(|i| i.id.clone())
        .collect();
    let mut new_order = vec![e.id.clone()];
    new_order.extend(order.into_iter().filter(|i| *i != e.id));
    srv.put(
        &alice,
        &routes::path(routes::SET_ORDER, &[&set.id]),
        json!({"item_ids": new_order}),
    )
    .await
    .expect(200);
    let created_before = tg.mock.lock().unwrap().count("createNewStickerSet");
    let j = export(srv, &alice, &set.id, json!({"title": "Cats!"})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    assert_eq!(r["created_pack"], false);
    assert_eq!(r["added"], 1, "{r}");
    assert_eq!(r["removed"], 1, "{r}");
    let mut want = Vec::new();
    for id in &new_order {
        let variant = if *id == gif.id {
            "tg_video_sticker"
        } else {
            "tg_sticker"
        };
        let f = srv
            .get(&alice, &routes::path(routes::ITEM_FILE, &[id, variant]))
            .await
            .expect(200);
        want.push(unique_id(&f.body));
    }
    let m = tg.mock.lock().unwrap();
    assert_eq!(m.count("createNewStickerSet"), created_before);
    let pack = &m.packs[&name];
    assert_eq!(pack.title, "Cats!");
    let got: Vec<String> = pack.stickers.iter().map(|s| s.fuid.clone()).collect();
    assert_eq!(got, want, "pack order follows the set order");
    // e first, then a with its new emoji, then c.
    assert_eq!(new_order[1], a.id);
    assert_eq!(new_order[2], c.id);
    assert_eq!(pack.stickers[0].emoji_list, [stickpile_telegram::DEFAULT_EMOJI]);
    assert_eq!(pack.stickers[1].emoji_list, ["🙀"]);
}

#[tokio::test]
async fn export_validates_pack_names_and_uses_the_default_owner() {
    let tg = setup(|c| c.telegram.default_owner_user_id = Some(777)).await;
    let srv = &tg.srv;
    let alice = srv.user("alice").await;
    let set = srv.create_set(&alice, "x", Visibility::Private).await;
    srv.upload_one(&alice, &set.id, "a.png", png(1)).await;

    let j = export(srv, &alice, &set.id, json!({"pack_name": "other_by_someone_bot"})).await;
    assert_eq!(j.status, JobStatus::Failed);
    assert!(j.error.as_deref().unwrap().contains(&format!("_by_{BOT}")), "{j:?}");
    srv.post(
        &alice,
        &routes::path(routes::SET_TELEGRAM_EXPORT, &[&set.id]),
        json!({"pack_name": "bad name!"}),
    )
    .await
    .expect(400);

    let custom = format!("Mine_by_{BOT}");
    let j = export(srv, &alice, &set.id, json!({"pack_name": custom})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(tg.mock.lock().unwrap().packs[&custom].owner, 777);
    // Others cannot export someone else's set; an empty set has nothing to export.
    let bob = srv.user("bob").await;
    srv.post(&bob, &routes::path(routes::SET_TELEGRAM_EXPORT, &[&set.id]), json!({}))
        .await
        .expect(404);
    let empty = srv.create_set(&alice, "empty", Visibility::Private).await;
    let j = export(srv, &alice, &empty.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Failed);
    assert!(j.error.unwrap().contains("nothing to export"));
}

#[tokio::test]
async fn importing_an_own_pack_binds_it_so_export_keeps_stickers() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let alice = srv.user("alice").await;
    srv.patch(&alice, routes::ME, json!({"telegram_user_id": 555}))
        .await
        .expect(200);
    let name = format!("Faves_by_{BOT}");
    tg.mock.lock().unwrap().seed(
        &name,
        "Faves",
        555,
        vec![("static", webp(7), "⭐"), ("static", webp(8), "🔥")],
    );
    let j: Job = srv
        .post(
            &alice,
            routes::TELEGRAM_IMPORT,
            json!({"pack_name": name, "slug": "faves"}),
        )
        .await
        .expect(200)
        .json();
    let j = run(srv, &alice, &j).await;
    assert_eq!(j.result.as_ref().unwrap()["bound"], true, "{j:?}");
    let set_id = j.result.unwrap()["set_id"].as_str().unwrap().to_string();
    let s: Set = detail(srv, &alice, &set_id).await.set;
    assert_eq!(s.telegram_pack.as_deref(), Some(name.as_str()));

    let before = tg.mock.lock().unwrap().calls.len();
    let j = export(srv, &alice, &set_id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    assert_eq!((r["added"].as_u64(), r["removed"].as_u64()), (Some(0), Some(0)), "{r}");
    let m = tg.mock.lock().unwrap();
    let calls = &m.calls[before..];
    for upload in [
        "createNewStickerSet",
        "addStickerToSet",
        "replaceStickerInSet",
        "deleteStickerFromSet",
    ] {
        assert!(!calls.iter().any(|c| c == upload), "{upload} in {calls:?}");
    }
    assert_eq!(m.packs[&name].stickers.len(), 2);
}

#[tokio::test]
async fn disabled_or_unconfigured_telegram_is_not_found() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let set = srv.create_set(&alice, "s", Visibility::Private).await;
    srv.post(&alice, routes::TELEGRAM_IMPORT, json!({"pack_name": "Animals"}))
        .await
        .expect(404);
    srv.post(
        &alice,
        &routes::path(routes::SET_TELEGRAM_EXPORT, &[&set.id]),
        json!({}),
    )
    .await
    .expect(404);
    assert!(!stickpile_server::service::telegram::enabled(&srv.state));

    // Enabled, but the token file is missing.
    let srv = TestServer::with_config(|c| {
        c.telegram.enabled = true;
        c.telegram.bot_token_file = Some("/nonexistent/stickpile-token".into());
    })
    .await;
    assert!(!stickpile_server::service::telegram::enabled(&srv.state));
    let alice = srv.user("alice").await;
    let r = srv
        .post(&alice, routes::TELEGRAM_IMPORT, json!({"pack_name": "Animals"}))
        .await
        .expect(404);
    assert!(r.text().contains("Telegram"));
}
