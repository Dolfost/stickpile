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

/// A 1024 px PNG of white noise: its lossless 512 px sticker is over the
/// 512 KB pack limit.
fn noise_png() -> Vec<u8> {
    let mut state = 0x1234_5678u32;
    let img = image::RgbaImage::from_fn(1024, 1024, |_, _| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let b = state.to_le_bytes();
        image::Rgba([b[0], b[1], b[2], 255])
    });
    common::encode(&img, image::ImageFormat::Png)
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
async fn a_sticker_over_the_pack_limit_is_squeezed_for_the_pack_only() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let alice = srv.user("alice").await;
    srv.patch(&alice, routes::ME, json!({"telegram_user_id": 555}))
        .await
        .expect(200);
    let set = srv.create_set(&alice, "noise", Visibility::Private).await;
    let item = srv.upload_one(&alice, &set.id, "noise.png", noise_png()).await;
    run_jobs_now(srv).await;
    // The stored sticker is lossless, too big for a pack but fine for chats.
    let stored = srv
        .get(&alice, &routes::path(routes::ITEM_FILE, &[&item.id, "tg_sticker"]))
        .await
        .expect(200)
        .body;
    assert!(stored.len() > 512 * 1024, "{}", stored.len());

    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(j.result.unwrap()["exported"], 1);
    let m = tg.mock.lock().unwrap();
    let sent = &m.packs[&format!("noise_by_{BOT}")].stickers[0].bytes;
    assert!(sent.len() <= 512 * 1024, "{}", sent.len());
    let img = image::load_from_memory(sent).unwrap();
    assert_eq!((img.width(), img.height()), (512, 512));
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
async fn the_cover_is_the_pack_thumbnail() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let alice = srv.user("alice").await;
    srv.patch(&alice, routes::ME, json!({"telegram_user_id": 555}))
        .await
        .expect(200);
    let set = srv.create_set(&alice, "covered", Visibility::Private).await;
    srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
    let b = srv.upload_one(&alice, &set.id, "b.png", png(2)).await;
    let name = format!("covered_by_{BOT}");
    let thumbnail = |tg: &Tg| tg.mock.lock().unwrap().packs[&name].thumbnail.clone();

    // No cover: Telegram shows the first sticker, nothing is set.
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(j.result.unwrap()["thumbnail"], false);
    assert_eq!(thumbnail(&tg), None);

    // b as the cover: a 100×100 PNG of it on the next publish, once.
    let s: Set = srv
        .patch(
            &alice,
            &routes::path(routes::SET, &[&set.id]),
            json!({"cover_item_id": b.id}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(s.cover_item_id.as_deref(), Some(b.id.as_str()));
    let j = export(srv, &alice, &set.id, json!({})).await;
    let r = j.result.unwrap();
    assert_eq!(
        (r["thumbnail"].clone(), r["thumbnail_changed"].clone()),
        (json!(true), json!(true))
    );
    let png = thumbnail(&tg).expect("thumbnail set");
    let img = image::load_from_memory(&png).unwrap();
    assert_eq!((img.width(), img.height()), (100, 100));
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.result.unwrap()["thumbnail_changed"], false);
    assert_eq!(tg.mock.lock().unwrap().count("setStickerSetThumbnail"), 1);

    // Back to the first item: the thumbnail is dropped.
    srv.patch(
        &alice,
        &routes::path(routes::SET, &[&set.id]),
        json!({"cover_item_id": null}),
    )
    .await
    .expect(200);
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.result.unwrap()["thumbnail_changed"], true);
    assert_eq!(thumbnail(&tg), None);
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
    // A bare name gets the bot suffix appended.
    let j = export(srv, &alice, &set.id, json!({"pack_name": "test"})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let short = format!("test_by_{BOT}");
    assert_eq!(j.result.unwrap()["pack_name"], short.as_str());
    assert!(tg.mock.lock().unwrap().packs.contains_key(&short));
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
    // The pack was made with the bot outside stickpile: only an admin may
    // adopt it (see packs_made_elsewhere_are_adopted_only_by_admins).
    let alice = srv.admin("alice").await;
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

async fn import(srv: &TestServer, u: &TestUser, pack: &str, slug: &str) -> Value {
    let j: Job = srv
        .post(u, routes::TELEGRAM_IMPORT, json!({"pack_name": pack, "slug": slug}))
        .await
        .expect(200)
        .json();
    let j = run(srv, u, &j).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    j.result.unwrap()
}

fn failed_with(j: &Job, text: &str) {
    assert_eq!(j.status, JobStatus::Failed, "{j:?}");
    assert!(j.error.as_deref().unwrap_or_default().contains(text), "{j:?}");
}

#[tokio::test]
async fn packs_belong_to_the_user_who_created_them() {
    let tg = setup(|c| c.telegram.default_owner_user_id = Some(777)).await;
    let srv = &tg.srv;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let a = srv.create_set(&alice, "cats", Visibility::Private).await;
    srv.upload_one(&alice, &a.id, "a.png", png(1)).await;
    let j = export(srv, &alice, &a.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let name = format!("cats_by_{BOT}");
    let stickers = |tg: &Tg| tg.mock.lock().unwrap().packs[&name].stickers.len();
    assert_eq!(stickers(&tg), 1);

    // Bob's set with the same slug derives the same default name, and a
    // typed name that differs only in case is the same pack on Telegram.
    let b = srv.create_set(&bob, "cats", Visibility::Private).await;
    srv.upload_one(&bob, &b.id, "b.png", png(2)).await;
    srv.upload_one(&bob, &b.id, "c.png", png(3)).await;
    for body in [
        json!({}),
        json!({"pack_name": "CATS"}),
        json!({"pack_name": name.to_uppercase()}),
    ] {
        failed_with(&export(srv, &bob, &b.id, body).await, "another stickpile user");
    }

    // Importing Alice's pack gives Bob a copy without the binding, and the
    // copy cannot be exported into her pack either.
    let r = import(srv, &bob, &name, "her-cats").await;
    assert_eq!(r["bound"], false, "{r}");
    let copy = r["set_id"].as_str().unwrap().to_string();
    assert_eq!(detail(srv, &bob, &copy).await.set.telegram_pack, None);
    failed_with(
        &export(srv, &bob, &copy, json!({"pack_name": name})).await,
        "another stickpile user",
    );
    assert_eq!(stickers(&tg), 1, "Alice's pack was changed");

    // Bob publishes under his own name; Alice keeps updating hers.
    let j = export(srv, &bob, &b.id, json!({"pack_name": "bobs_cats"})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    srv.upload_one(&alice, &a.id, "d.png", png(4)).await;
    let j = export(srv, &alice, &a.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(stickers(&tg), 2);

    // A pack follows one set: publishing another of Alice's sets to it moves
    // the binding.
    let a2 = srv.create_set(&alice, "more", Visibility::Private).await;
    srv.upload_one(&alice, &a2.id, "e.png", png(5)).await;
    let j = export(srv, &alice, &a2.id, json!({"pack_name": name})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(detail(srv, &alice, &a.id).await.set.telegram_pack, None);
    assert_eq!(
        detail(srv, &alice, &a2.id).await.set.telegram_pack.as_deref(),
        Some(name.as_str())
    );
}

#[tokio::test]
async fn packs_made_elsewhere_are_adopted_only_by_admins() {
    let tg = setup(|c| c.telegram.default_owner_user_id = Some(777)).await;
    let srv = &tg.srv;
    let name = format!("Old_by_{BOT}");
    tg.mock
        .lock()
        .unwrap()
        .seed(&name, "Old", 777, vec![("static", webp(1), "🐱")]);
    let bob = srv.user("bob").await;
    let set = srv.create_set(&bob, "x", Visibility::Private).await;
    srv.upload_one(&bob, &set.id, "a.png", png(1)).await;
    failed_with(
        &export(srv, &bob, &set.id, json!({"pack_name": name})).await,
        "not published through this server",
    );
    assert_eq!(import(srv, &bob, &name, "old-copy").await["bound"], false);
    assert_eq!(tg.mock.lock().unwrap().packs[&name].stickers.len(), 1);

    // An admin's import adopts it; from then on it is theirs.
    let root = srv.admin("root").await;
    let r = import(srv, &root, &name, "old").await;
    assert_eq!(r["bound"], true, "{r}");
    let adopted = r["set_id"].as_str().unwrap().to_string();
    failed_with(
        &export(srv, &bob, &set.id, json!({"pack_name": name})).await,
        "another stickpile user",
    );
    let j = export(srv, &root, &adopted, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(j.result.unwrap()["pack_name"], name.as_str());
}

#[tokio::test]
async fn migration_gives_existing_packs_to_the_earliest_bound_set() {
    use sqlx::sqlite::SqliteConnectOptions;
    let dir = tempfile::tempdir().unwrap();
    let opts = SqliteConnectOptions::new()
        .filename(dir.path().join("t.db"))
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = sqlx::SqlitePool::connect_with(opts).await.unwrap();
    // The schema before the ownership table, then that migration on top.
    for sql in [
        include_str!("../migrations/0001_init.sql"),
        include_str!("../migrations/0002_gitpack_sync.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    for (id, name) in [("u1", "alice"), ("u2", "bob")] {
        sqlx::query("INSERT INTO users (id, username, display_name, role, created_at) VALUES (?, ?, ?, 'user', 1)")
            .bind(id)
            .bind(name)
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();
    }
    for (id, owner, pack, at) in [
        ("s1", "u1", Some("cats_by_b_bot"), 10),
        ("s2", "u2", Some("CATS_by_b_bot"), 20),
        ("s3", "u2", Some("dogs_by_b_bot"), 30),
        ("s4", "u1", None, 40),
    ] {
        sqlx::query(
            "INSERT INTO sets (id, owner_id, slug, name, telegram_pack, telegram_mapping, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, '{}', ?, ?)",
        )
        .bind(id)
        .bind(owner)
        .bind(id)
        .bind(id)
        .bind(pack)
        .bind(at)
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::raw_sql(include_str!("../migrations/0003_telegram_pack_owners.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let owners: Vec<(String, String)> = sqlx::query_as("SELECT name, user_id FROM telegram_packs ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        owners,
        vec![
            ("cats_by_b_bot".into(), "u1".into()),
            ("dogs_by_b_bot".into(), "u2".into())
        ]
    );
    let bound: Vec<(String, Option<String>)> = sqlx::query_as("SELECT id, telegram_pack FROM sets ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        bound,
        vec![
            ("s1".into(), Some("cats_by_b_bot".into())),
            ("s2".into(), None),
            ("s3".into(), Some("dogs_by_b_bot".into())),
            ("s4".into(), None),
        ]
    );
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

#[tokio::test]
async fn the_cover_image_is_the_pack_thumbnail() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let alice = srv.user("alice").await;
    srv.patch(&alice, routes::ME, json!({"telegram_user_id": 555}))
        .await
        .expect(200);
    let set = srv.create_set(&alice, "pictured", Visibility::Private).await;
    srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
    let b = srv.upload_one(&alice, &set.id, "b.png", png(2)).await;
    let set_path = routes::path(routes::SET, &[&set.id]);
    let cover_path = routes::path(routes::SET_COVER, &[&set.id]);
    let name = format!("pictured_by_{BOT}");
    let thumbnail = |tg: &Tg| tg.mock.lock().unwrap().packs[&name].thumbnail.clone();
    let calls = |tg: &Tg| tg.mock.lock().unwrap().count("setStickerSetThumbnail");
    // A wide red image: centred on the transparent square, red in the middle.
    let red = common::encode(
        &image::RgbaImage::from_pixel(300, 150, image::Rgba([220, 20, 20, 255])),
        image::ImageFormat::Png,
    );
    let blue = common::encode(
        &image::RgbaImage::from_pixel(120, 120, image::Rgba([20, 20, 220, 255])),
        image::ImageFormat::Png,
    );

    // The image is the thumbnail; it is not a sticker.
    srv.put_cover(&alice, &set.id, "red.png", red).await.expect(200);
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    assert_eq!((r["thumbnail"].clone(), r["exported"].clone()), (json!(true), json!(2)));
    let png_bytes = thumbnail(&tg).expect("thumbnail set");
    let img = image::load_from_memory(&png_bytes).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (100, 100));
    let red_at = |img: &image::RgbaImage| {
        let p = img.get_pixel(50, 50).0;
        p[0] > 200 && p[1] < 60 && p[2] < 60 && p[3] == 255
    };
    let blue_at = |img: &image::RgbaImage| {
        let p = img.get_pixel(50, 50).0;
        p[0] < 60 && p[1] < 60 && p[2] > 200 && p[3] == 255
    };
    assert!(red_at(&img), "{:?}", img.get_pixel(50, 50));
    assert_eq!(img.get_pixel(50, 2)[3], 0, "padded to a square");
    assert_eq!(tg.mock.lock().unwrap().packs[&name].stickers.len(), 2);
    // Unchanged: not uploaded again.
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.result.unwrap()["thumbnail_changed"], false);
    assert_eq!(calls(&tg), 1);

    // Another image: uploaded on the next publish.
    srv.put_cover(&alice, &set.id, "blue.png", blue.clone())
        .await
        .expect(200);
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.result.unwrap()["thumbnail_changed"], true);
    let img = image::load_from_memory(&thumbnail(&tg).unwrap()).unwrap().to_rgba8();
    assert!(blue_at(&img), "{:?}", img.get_pixel(50, 50));
    assert_eq!(calls(&tg), 2);

    // An item chosen as the cover replaces the image: its thumbnail now.
    srv.patch(&alice, &set_path, json!({"cover_item_id": b.id}))
        .await
        .expect(200);
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.result.unwrap()["thumbnail_changed"], true);
    let img = image::load_from_memory(&thumbnail(&tg).unwrap()).unwrap().to_rgba8();
    assert!(!blue_at(&img) && !red_at(&img));

    // An image again, then removed: back to the chosen item (sent again,
    // the key changed), then to the first item (dropped).
    srv.put_cover(&alice, &set.id, "blue.png", blue).await.expect(200);
    export(srv, &alice, &set.id, json!({})).await;
    srv.delete(&alice, &cover_path).await.expect(200);
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.result.unwrap()["thumbnail_changed"], true);
    assert!(thumbnail(&tg).is_some());
    srv.patch(&alice, &set_path, json!({"cover_item_id": null}))
        .await
        .expect(200);
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.result.unwrap()["thumbnail_changed"], true);
    assert_eq!(thumbnail(&tg), None);
    assert_eq!(tg.mock.lock().unwrap().packs[&name].stickers.len(), 2);
}
