//! Telegram import and export against the in-process mock Bot API
//! (`integrations_support::mock_bot`).

mod common;
mod integrations_support;
mod web_common;

use std::collections::{BTreeMap, BTreeSet};

use common::{png, TestServer, TestUser};
use integrations_support::mock_bot::{self, unique_id, Shared, BOT};
use integrations_support::run_jobs_now;
use serde_json::{json, Value};
use stickpile_api::{
    routes, Job, JobKind, JobStatus, Publication, PublishJobs, Set, SetDetail, SetSource, TelegramPlan, User,
    Visibility,
};

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

// ---------------------------------------------------------------------------
// Sets larger than one pack: parts (docs/telegram.md)
// ---------------------------------------------------------------------------

/// Upload `n` distinct stills (seeds `from..from + n`, in that order) in
/// batches and render them; their item ids in set order.
async fn add_items(srv: &TestServer, u: &TestUser, set_id: &str, from: u64, n: u64) -> Vec<String> {
    let seeds: Vec<u64> = (from..from + n).collect();
    let mut ids = Vec::new();
    for chunk in seeds.chunks(50) {
        let files: Vec<(String, Vec<u8>)> = chunk.iter().map(|s| (format!("s{s:05}.png"), png(*s))).collect();
        let res: Vec<stickpile_api::UploadResult> = srv
            .upload(
                u,
                set_id,
                files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect(),
                None,
            )
            .await
            .expect(200)
            .json();
        ids.extend(res.into_iter().map(|r| r.item.expect("item").id));
    }
    run_jobs_now(srv).await;
    ids
}

/// A user with a Telegram user id and a set `slug` of `n` stills.
async fn big_set(tg: &Tg, username: &str, slug: &str, n: u64) -> (TestUser, Set, Vec<String>) {
    let srv = &tg.srv;
    let u = srv.user(username).await;
    srv.patch(&u, routes::ME, json!({"telegram_user_id": 555}))
        .await
        .expect(200);
    let set = srv.create_set(&u, slug, Visibility::Private).await;
    let ids = add_items(srv, &u, &set.id, 1, n).await;
    (u, set, ids)
}

fn part_name(base: &str, k: u32) -> String {
    stickpile_telegram::part_pack_name(&format!("{base}_by_{BOT}"), k, BOT).unwrap()
}

/// pack name -> file_unique_ids of its stickers, in pack order.
fn packs(tg: &Tg) -> BTreeMap<String, Vec<String>> {
    let m = tg.mock.lock().unwrap();
    m.packs
        .iter()
        .map(|(n, p)| (n.clone(), p.stickers.iter().map(|s| s.fuid.clone()).collect()))
        .collect()
}

/// The `telegram_parts` rows of a set: (part, pack name, stickers, has a
/// stored state).
async fn part_rows(srv: &TestServer, set_id: &str) -> Vec<(i64, String, i64, bool)> {
    sqlx::query_as(
        "SELECT part, pack_name, stickers, mapping IS NOT NULL FROM telegram_parts WHERE set_id = ? ORDER BY part",
    )
    .bind(set_id)
    .fetch_all(&srv.state.db)
    .await
    .unwrap()
}

async fn telegram_base(srv: &TestServer, set_id: &str) -> Option<String> {
    sqlx::query_scalar("SELECT telegram_base FROM sets WHERE id = ?")
        .bind(set_id)
        .fetch_one(&srv.state.db)
        .await
        .unwrap()
}

/// The pack every published item's sticker is in (item id -> pack name),
/// from the mappings stored with the parts.
async fn item_packs(srv: &TestServer, set_id: &str) -> BTreeMap<String, String> {
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT pack_name, mapping FROM telegram_parts WHERE set_id = ? ORDER BY part")
            .bind(set_id)
            .fetch_all(&srv.state.db)
            .await
            .unwrap();
    let mut out = BTreeMap::new();
    for (name, mapping) in rows {
        let m: stickpile_telegram::PackMapping = serde_json::from_str(mapping.as_deref().unwrap_or("{}")).unwrap();
        for key in m.keys() {
            assert!(out.insert(key.clone(), name.clone()).is_none(), "{key} in two parts");
        }
    }
    out
}

/// Calls that change a pack on Telegram.
const WRITES: [&str; 10] = [
    "createNewStickerSet",
    "addStickerToSet",
    "replaceStickerInSet",
    "deleteStickerFromSet",
    "setStickerPositionInSet",
    "setStickerEmojiList",
    "setStickerKeywords",
    "setStickerSetTitle",
    "setStickerSetThumbnail",
    "deleteStickerSet",
];

fn writes_since(tg: &Tg, from: usize) -> Vec<String> {
    let m = tg.mock.lock().unwrap();
    m.calls[from..]
        .iter()
        .filter(|c| WRITES.contains(&c.as_str()))
        .cloned()
        .collect()
}

/// Writes other than the emoji and keywords set again on every export.
fn uploads_since(tg: &Tg, from: usize) -> Vec<String> {
    writes_since(tg, from)
        .into_iter()
        .filter(|c| c != "setStickerEmojiList" && c != "setStickerKeywords")
        .collect()
}

async fn publication(srv: &TestServer, u: &TestUser, set_id: &str) -> Publication {
    let list: Vec<Publication> = srv
        .get(u, &routes::path(routes::SET_PUBLISH, &[set_id]))
        .await
        .expect(200)
        .json();
    list.into_iter().find(|p| p.platform == "telegram").unwrap()
}

async fn delete_items(srv: &TestServer, u: &TestUser, ids: &[String]) {
    for id in ids {
        srv.delete(u, &routes::path(routes::ITEM, &[id])).await.expect(204);
    }
}

async fn plan(srv: &TestServer, u: &TestUser, set_id: &str, repack: bool) -> TelegramPlan {
    srv.get(
        u,
        &format!("{}?repack={repack}", routes::path(routes::SET_TELEGRAM_PLAN, &[set_id])),
    )
    .await
    .expect(200)
    .json()
}

async fn sticker_id(srv: &TestServer, u: &TestUser, item_id: &str) -> String {
    let f = srv
        .get(u, &routes::path(routes::ITEM_FILE, &[item_id, "tg_sticker"]))
        .await
        .expect(200);
    unique_id(&f.body)
}

#[tokio::test]
async fn migration_keeps_existing_binding_as_part_1() {
    use sqlx::sqlite::SqliteConnectOptions;
    let dir = tempfile::tempdir().unwrap();
    let opts = SqliteConnectOptions::new()
        .filename(dir.path().join("t.db"))
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = sqlx::SqlitePool::connect_with(opts).await.unwrap();
    // The schema before parts, then that migration on top.
    for m in stickpile_server::db::MIGRATOR.iter().filter(|m| m.version < 17) {
        sqlx::raw_sql(&m.sql).execute(&pool).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO users (id, username, display_name, role, created_at) VALUES ('u1', 'alice', 'alice', 'user', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mapping = r#"{"i1":["sha1","fu1"],"i2":["sha2","fu2"]}"#;
    for (id, pack, mapping, thumb) in [
        ("s1", Some("cats_by_b_bot"), Some(mapping), Some("image:abc")),
        ("s2", Some("Dogs_by_b_bot"), None, None),
        ("s3", None, None, None),
    ] {
        sqlx::query(
            "INSERT INTO sets (id, owner_id, slug, name, telegram_pack, telegram_mapping, telegram_thumbnail, \
             created_at, updated_at) VALUES (?, 'u1', ?, ?, ?, ?, ?, 10, 20)",
        )
        .bind(id)
        .bind(id)
        .bind(id)
        .bind(pack)
        .bind(mapping)
        .bind(thumb)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO telegram_packs (name, user_id, created_at) VALUES ('cats_by_b_bot', 'u1', 5)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0017_telegram_parts.sql"))
        .execute(&pool)
        .await
        .unwrap();

    type PartRow = (String, i64, String, Option<String>, Option<String>, i64);
    let parts: Vec<PartRow> = sqlx::query_as(
        "SELECT set_id, part, pack_name, mapping, thumbnail, stickers FROM telegram_parts ORDER BY set_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        parts,
        vec![
            (
                "s1".into(),
                1,
                "cats_by_b_bot".into(),
                Some(mapping.into()),
                Some("image:abc".into()),
                2
            ),
            ("s2".into(), 1, "Dogs_by_b_bot".into(), None, None, 0),
        ],
        "every binding is part 1, unchanged"
    );
    type SetRow = (String, Option<String>, Option<String>, Option<String>, Option<String>);
    let sets: Vec<SetRow> = sqlx::query_as(
        "SELECT id, telegram_pack, telegram_base, telegram_mapping, telegram_thumbnail FROM sets ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let bound = |id: &str, pack: &str| {
        (
            id.to_string(),
            Some(pack.to_string()),
            Some(pack.to_string()),
            None,
            None,
        )
    };
    assert_eq!(
        sets,
        vec![
            bound("s1", "cats_by_b_bot"),
            bound("s2", "Dogs_by_b_bot"),
            ("s3".into(), None, None, None, None),
        ],
        "the binding stays; the mapping lives with the part now"
    );
    let deleted: Option<i64> = sqlx::query_scalar("SELECT deleted_at FROM telegram_packs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(deleted, None);
}

#[tokio::test]
async fn existing_single_pack_exports_as_part_1_without_change() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, _) = big_set(&tg, "alice", "cats", 3).await;
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let name = format!("cats_by_{BOT}");
    let stored: (String, Option<String>) =
        sqlx::query_as("SELECT mapping, thumbnail FROM telegram_parts WHERE set_id = ? AND part = 1")
            .bind(&set.id)
            .fetch_one(&srv.state.db)
            .await
            .unwrap();
    // The binding as a server before parts kept it, then the 0017
    // migration's own statements on it.
    sqlx::query("DELETE FROM telegram_parts")
        .execute(&srv.state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE sets SET telegram_base = NULL, telegram_mapping = ?, telegram_thumbnail = ? WHERE id = ?")
        .bind(&stored.0)
        .bind(&stored.1)
        .bind(&set.id)
        .execute(&srv.state.db)
        .await
        .unwrap();
    let migration = include_str!("../migrations/0017_telegram_parts.sql");
    let at = migration.find("-- Every existing binding").unwrap();
    sqlx::raw_sql(&migration[at..]).execute(&srv.state.db).await.unwrap();
    assert_eq!(part_rows(srv, &set.id).await, vec![(1, name.clone(), 3, true)]);

    let before = packs(&tg);
    let calls = tg.mock.lock().unwrap().calls.len();
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    for (k, v) in [
        ("pack_name", json!(name)),
        ("url", json!(format!("https://t.me/addstickers/{name}"))),
        ("title", json!("cats")),
        ("created_pack", json!(false)),
        ("added", json!(0)),
        ("removed", json!(0)),
        ("reordered", json!(false)),
        ("exported", json!(3)),
        ("moved", json!(0)),
    ] {
        assert_eq!(r[k], v, "{k}: {r}");
    }
    assert_eq!(r["parts"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["parts"][0]["number"], 1);
    let uploads = uploads_since(&tg, calls);
    assert!(uploads.is_empty(), "{uploads:?}");
    assert_eq!(packs(&tg), before, "the pack is as it was");
    let s = detail(srv, &alice, &set.id).await.set;
    assert_eq!(s.telegram_pack.as_deref(), Some(name.as_str()));
    assert!(s.telegram_parts.is_empty(), "one pack");
    assert!(publication(srv, &alice, &set.id).await.parts.is_empty());
    assert_eq!(telegram_base(srv, &set.id).await.as_deref(), Some(name.as_str()));
}

#[tokio::test]
async fn export_250_items_makes_three_parts_with_links() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, ids) = big_set(&tg, "alice", "cats", 250).await;
    srv.patch(
        &alice,
        &routes::path(routes::SET, &[&set.id]),
        json!({"name": "Cats", "cover_item_id": ids[200]}),
    )
    .await
    .expect(200);
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    let names: Vec<String> = (1..=3).map(|k| part_name("cats", k)).collect();
    assert_eq!(names[1], format!("cats_2_by_{BOT}"));
    let urls: Vec<String> = names.iter().map(|n| format!("https://t.me/addstickers/{n}")).collect();
    // Part 1's fields as a single pack's, then every part.
    assert_eq!(r["pack_name"], names[0].as_str());
    assert_eq!(r["url"], urls[0].as_str());
    assert_eq!(r["created_pack"], true);
    assert_eq!(r["exported"], 250);
    let got: Vec<(u64, String, u64, String)> = r["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["number"].as_u64().unwrap(),
                p["name"].as_str().unwrap().to_string(),
                p["stickers"].as_u64().unwrap(),
                p["url"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (1, names[0].clone(), 120, urls[0].clone()),
            (2, names[1].clone(), 120, urls[1].clone()),
            (3, names[2].clone(), 10, urls[2].clone()),
        ]
    );
    {
        let m = tg.mock.lock().unwrap();
        let titles: Vec<&str> = names.iter().map(|n| m.packs[n].title.as_str()).collect();
        assert_eq!(titles, ["Cats", "Cats 2", "Cats 3"]);
        for n in &names {
            assert_eq!(m.packs[n].owner, 555);
            assert!(m.packs[n].thumbnail.is_some(), "the cover on every part: {n}");
        }
        assert_eq!(m.count("createNewStickerSet"), 3);
    }
    // The packs hold the set in order, 120 at a time.
    let mut want = Vec::new();
    for id in &ids {
        want.push(sticker_id(srv, &alice, id).await);
    }
    let now = packs(&tg);
    assert_eq!(now[&names[0]], want[..120]);
    assert_eq!(now[&names[1]], want[120..240]);
    assert_eq!(now[&names[2]], want[240..]);
    // Every name is the owner's.
    for n in &names {
        let owner: String = sqlx::query_scalar("SELECT user_id FROM telegram_packs WHERE name = ?")
            .bind(n)
            .fetch_one(&srv.state.db)
            .await
            .unwrap();
        assert_eq!(owner, alice.id);
    }
    // The set and its publication list every part; the link is part 1.
    let want_parts = vec![
        (1, names[0].clone(), Some(120)),
        (2, names[1].clone(), Some(120)),
        (3, names[2].clone(), Some(10)),
    ];
    let s = detail(srv, &alice, &set.id).await.set;
    assert_eq!(s.telegram_pack.as_deref(), Some(names[0].as_str()));
    let listed: Vec<(u32, String, Option<u32>)> = s
        .telegram_parts
        .iter()
        .map(|p| (p.number, p.name.clone(), p.stickers))
        .collect();
    assert_eq!(listed, want_parts);
    let p = publication(srv, &alice, &set.id).await;
    assert_eq!(p.target.as_deref(), Some(names[0].as_str()));
    let listed: Vec<(u32, String, Option<u32>)> =
        p.parts.iter().map(|p| (p.number, p.name.clone(), p.stickers)).collect();
    assert_eq!(listed, want_parts);
    assert_eq!(p.parts[2].url.as_deref(), Some(urls[2].as_str()));
    // The same again changes nothing.
    let calls = tg.mock.lock().unwrap().calls.len();
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let uploads = uploads_since(&tg, calls);
    assert!(uploads.is_empty(), "{uploads:?}");
    assert_eq!(packs(&tg), now);
}

#[tokio::test]
async fn add_items_fill_first_part_with_room() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, ids) = big_set(&tg, "alice", "cats", 130).await;
    assert_eq!(export(srv, &alice, &set.id, json!({})).await.status, JobStatus::Done);
    let before = item_packs(srv, &set.id).await;
    let (p1, p2) = (part_name("cats", 1), part_name("cats", 2));
    // Two stickers of part 1 go; five new items come at the end.
    let gone = [ids[3].clone(), ids[50].clone()];
    delete_items(srv, &alice, &gone).await;
    let new = add_items(srv, &alice, &set.id, 1000, 5).await;
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(j.result.unwrap()["moved"], 0);
    let after = item_packs(srv, &set.id).await;
    // Published stickers stay where they were.
    for (item, pack) in before.iter().filter(|(i, _)| !gone.contains(i)) {
        assert_eq!(after.get(item), Some(pack), "{item} moved");
    }
    // The new ones: the first two fill part 1, the rest go to part 2.
    let placed: Vec<&str> = new.iter().map(|i| after[i].as_str()).collect();
    assert_eq!(
        placed,
        [p1.as_str(), p1.as_str(), p2.as_str(), p2.as_str(), p2.as_str()]
    );
    let now = packs(&tg);
    assert_eq!((now[&p1].len(), now[&p2].len()), (120, 13));
}

#[tokio::test]
async fn delete_items_frees_room_nothing_moves() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, ids) = big_set(&tg, "alice", "cats", 125).await;
    assert_eq!(export(srv, &alice, &set.id, json!({})).await.status, JobStatus::Done);
    let (p1, p2) = (part_name("cats", 1), part_name("cats", 2));
    let part2 = packs(&tg)[&p2].clone();
    delete_items(srv, &alice, &ids[10..13]).await;
    let calls = tg.mock.lock().unwrap().calls.len();
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    assert_eq!((r["removed"].clone(), r["moved"].clone()), (json!(3), json!(0)), "{r}");
    let now = packs(&tg);
    assert_eq!(now[&p1].len(), 117, "the room stays free");
    assert_eq!(now[&p2], part2, "part 2 is as it was");
    let uploads = uploads_since(&tg, calls);
    assert!(!uploads.iter().any(|c| c == "addStickerToSet"), "{uploads:?}");
}

#[tokio::test]
async fn reorder_across_boundary_moves_nothing() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, ids) = big_set(&tg, "alice", "cats", 125).await;
    assert_eq!(export(srv, &alice, &set.id, json!({})).await.status, JobStatus::Done);
    let before = item_packs(srv, &set.id).await;
    let (p1, p2) = (part_name("cats", 1), part_name("cats", 2));
    let contents =
        |p: &BTreeMap<String, Vec<String>>| [&p1, &p2].map(|n| p[n].iter().cloned().collect::<BTreeSet<String>>());
    let stickers_before = contents(&packs(&tg));
    // Part 2's first item to the start of the set, part 1's first to the end.
    let mut order: Vec<String> = vec![ids[120].clone()];
    order.extend(ids[1..120].iter().cloned());
    order.extend(ids[121..].iter().cloned());
    order.push(ids[0].clone());
    srv.put(
        &alice,
        &routes::path(routes::SET_ORDER, &[&set.id]),
        json!({"item_ids": order}),
    )
    .await
    .expect(200);
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(j.result.unwrap()["moved"], 0);
    assert_eq!(item_packs(srv, &set.id).await, before, "no sticker changes pack");
    let now = packs(&tg);
    assert_eq!(contents(&now), stickers_before);
    // The order inside a pack follows the set: in part 1, ids[0] is last now.
    assert_eq!(now[&p1].last(), Some(&sticker_id(srv, &alice, &ids[0]).await));
    assert_eq!(now[&p2].first(), Some(&sticker_id(srv, &alice, &ids[120]).await));
}

#[tokio::test]
async fn repack_count_matches_what_moves() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, ids) = big_set(&tg, "alice", "cats", 125).await;
    assert_eq!(export(srv, &alice, &set.id, json!({})).await.status, JobStatus::Done);
    let (p1, p2) = (part_name("cats", 1), part_name("cats", 2));
    // Five of part 1 go: an ordinary export leaves part 1 with 115.
    delete_items(srv, &alice, &ids[..5]).await;
    let stay = plan(srv, &alice, &set.id, false).await;
    assert_eq!(stay.moved, 0);
    let sizes: Vec<(u32, Option<u32>)> = stay.parts.iter().map(|p| (p.number, p.stickers)).collect();
    assert_eq!(sizes, [(1, Some(115)), (2, Some(5))]);
    // A repack moves part 2's five into part 1 and deletes part 2.
    let calls = tg.mock.lock().unwrap().calls.len();
    let repack = plan(srv, &alice, &set.id, true).await;
    assert_eq!(repack.moved, 5);
    assert_eq!(repack.parts.len(), 1);
    assert_eq!(repack.parts[0].stickers, Some(120));
    assert_eq!(repack.delete.len(), 1);
    assert_eq!(repack.delete[0].name, p2);
    assert!(writes_since(&tg, calls).is_empty(), "the plan changed nothing");
    let before = item_packs(srv, &set.id).await;
    let j = export(srv, &alice, &set.id, json!({"repack": true})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(j.result.unwrap()["moved"], repack.moved);
    let after = item_packs(srv, &set.id).await;
    let moved = after
        .iter()
        .filter(|(item, pack)| before.get(*item).is_some_and(|b| b != *pack))
        .count();
    assert_eq!(moved as u32, repack.moved, "the confirmation's count is what moved");
    let now = packs(&tg);
    assert_eq!(now[&p1].len(), 120);
    assert!(!now.contains_key(&p2), "the empty part was deleted");
    each_sticker_once(&tg, &alice, &set.id).await;
    // Through the publish route too, with the option as text.
    let jobs: PublishJobs = srv
        .post(
            &alice,
            &routes::path(routes::SET_PUBLISH, &[&set.id]),
            json!({"platforms": ["telegram"], "options": {"telegram": {"repack": "true"}}}),
        )
        .await
        .expect(200)
        .json();
    let j = run(srv, &alice, &jobs.jobs[0]).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(j.result.unwrap()["moved"], 0);
}

#[tokio::test]
async fn shrink_deletes_empty_parts_on_telegram() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, ids) = big_set(&tg, "alice", "cats", 125).await;
    assert_eq!(export(srv, &alice, &set.id, json!({})).await.status, JobStatus::Done);
    let (p1, p2) = (part_name("cats", 1), part_name("cats", 2));
    delete_items(srv, &alice, &ids[120..]).await;
    let calls = tg.mock.lock().unwrap().calls.len();
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    let deleted: Vec<bool> = r["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["deleted"].as_bool().unwrap())
        .collect();
    assert_eq!(deleted, [false, true], "{r}");
    // Deleted after the other parts were updated.
    let writes = writes_since(&tg, calls);
    assert_eq!(
        writes.last().map(String::as_str),
        Some("deleteStickerSet"),
        "{writes:?}"
    );
    assert_eq!(tg.mock.lock().unwrap().deleted, std::slice::from_ref(&p2));
    assert!(!packs(&tg).contains_key(&p2));
    assert_eq!(part_rows(srv, &set.id).await, vec![(1, p1.clone(), 120, true)]);
    let gone: Option<i64> = sqlx::query_scalar("SELECT deleted_at FROM telegram_packs WHERE name = ?")
        .bind(&p2)
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert!(gone.is_some(), "the name is marked as deleted");
    let s = detail(srv, &alice, &set.id).await.set;
    assert_eq!(s.telegram_pack.as_deref(), Some(p1.as_str()));
    assert!(s.telegram_parts.is_empty(), "one pack left");
}

#[tokio::test]
async fn part1_emptied_repoints_telegram_pack() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, ids) = big_set(&tg, "alice", "cats", 125).await;
    assert_eq!(export(srv, &alice, &set.id, json!({})).await.status, JobStatus::Done);
    let (p1, p2) = (part_name("cats", 1), part_name("cats", 2));
    delete_items(srv, &alice, &ids[..120]).await;
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    // The link of a set whose part 1 is gone: the lowest part left.
    assert_eq!(r["pack_name"], p2.as_str(), "{r}");
    assert!(!packs(&tg).contains_key(&p1), "part 1 was deleted on Telegram");
    let s = detail(srv, &alice, &set.id).await.set;
    assert_eq!(s.telegram_pack.as_deref(), Some(p2.as_str()));
    assert!(s.telegram_parts.is_empty(), "one pack left");
    let p = publication(srv, &alice, &set.id).await;
    assert_eq!(p.target.as_deref(), Some(p2.as_str()));
    assert_eq!(p.url, Some(format!("https://t.me/addstickers/{p2}")));
    // The names still derive from part 1's.
    assert_eq!(telegram_base(srv, &set.id).await.as_deref(), Some(p1.as_str()));
    // New stickers go to the part left, not to a new part 1; asking for the
    // pack the set links to by name keeps the parts as they are.
    let new = add_items(srv, &alice, &set.id, 2000, 3).await;
    let j = export(srv, &alice, &set.id, json!({"pack_name": p2})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let placed = item_packs(srv, &set.id).await;
    assert!(new.iter().all(|i| placed[i] == p2), "{placed:?}");
    let now = packs(&tg);
    assert_eq!(now[&p2].len(), 8);
    assert!(!now.contains_key(&p1));
    assert_eq!(telegram_base(srv, &set.id).await.as_deref(), Some(p1.as_str()));
    // The old form offers part 1's name, the name the parts derive from.
    let b = web_common::Browser::login(srv, &alice).await;
    let html = b.get(&format!("/sets/{}/telegram", set.id)).await.expect(200).text();
    assert!(html.contains(&format!("value=\"{p1}\"")), "{html}");
}

#[tokio::test]
async fn deleted_part_number_not_reused() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, ids) = big_set(&tg, "alice", "cats", 125).await;
    assert_eq!(export(srv, &alice, &set.id, json!({})).await.status, JobStatus::Done);
    let (p2, p3) = (part_name("cats", 2), part_name("cats", 3));
    delete_items(srv, &alice, &ids[120..]).await;
    assert_eq!(export(srv, &alice, &set.id, json!({})).await.status, JobStatus::Done);
    assert!(!packs(&tg).contains_key(&p2));
    // More than part 1 has room for: a new part, numbered 3.
    add_items(srv, &alice, &set.id, 3000, 10).await;
    let numbers = |plan: TelegramPlan| plan.parts.iter().map(|p| p.number).collect::<Vec<u32>>();
    assert_eq!(numbers(plan(srv, &alice, &set.id, false).await), [1, 3]);
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let now = packs(&tg);
    assert_eq!(now[&p3].len(), 10);
    assert!(!now.contains_key(&p2), "the deleted name is not created again");
    assert_eq!(
        tg.mock.lock().unwrap().count("createNewStickerSet"),
        3,
        "parts 1, 2 and 3, once each"
    );
    // A repack does not use it either.
    assert_eq!(numbers(plan(srv, &alice, &set.id, true).await), [1, 3]);
}

#[tokio::test]
async fn part_name_owned_by_another_user_fails_before_any_change() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    // Bob publishes under the name Alice's part 2 would get.
    let (bob, bobs, _) = big_set(&tg, "bob", "bobs", 1).await;
    let taken = part_name("cats", 2);
    let j = export(srv, &bob, &bobs.id, json!({"pack_name": "cats_2"})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let (alice, set, _) = big_set(&tg, "alice", "cats", 125).await;
    let calls = tg.mock.lock().unwrap().calls.len();
    let before = packs(&tg);
    let j = export(srv, &alice, &set.id, json!({})).await;
    failed_with(
        &j,
        &format!("the pack {taken} (part 2 of this set) belongs to another stickpile user"),
    );
    assert_eq!(j.attempts, 1, "not retried");
    assert!(writes_since(&tg, calls).is_empty(), "nothing on Telegram changed");
    assert_eq!(packs(&tg), before);
    // Nothing was claimed or bound either.
    let claimed: Option<String> = sqlx::query_scalar("SELECT user_id FROM telegram_packs WHERE name = ?")
        .bind(part_name("cats", 1))
        .fetch_optional(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(claimed, None);
    assert!(part_rows(srv, &set.id).await.is_empty());
    assert_eq!(detail(srv, &alice, &set.id).await.set.telegram_pack, None);
}

#[tokio::test]
async fn part_name_bound_to_another_set_fails_before_any_change() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, _) = big_set(&tg, "alice", "cats", 125).await;
    // Alice's other set is published under the name part 2 of "cats" gets.
    let dogs = srv.create_set(&alice, "dogs", Visibility::Private).await;
    add_items(srv, &alice, &dogs.id, 5000, 2).await;
    let j = export(srv, &alice, &dogs.id, json!({"pack_name": "cats_2"})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let taken = part_name("cats", 2);
    assert_eq!(j.result.unwrap()["pack_name"], taken.as_str());
    let calls = tg.mock.lock().unwrap().calls.len();
    let before = packs(&tg);
    let j = export(srv, &alice, &set.id, json!({})).await;
    failed_with(
        &j,
        &format!("the pack {taken} (part 2 of this set) is part 1 of your set dogs"),
    );
    assert!(writes_since(&tg, calls).is_empty(), "nothing on Telegram changed");
    assert_eq!(packs(&tg), before);
    assert!(part_rows(srv, &set.id).await.is_empty());
    assert_eq!(
        detail(srv, &alice, &dogs.id).await.set.telegram_pack.as_deref(),
        Some(taken.as_str()),
        "dogs keeps its pack"
    );
    // Under another name it works.
    let j = export(srv, &alice, &set.id, json!({"pack_name": "kittens"})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(packs(&tg)[&part_name("kittens", 2)].len(), 5);
}

/// Every sticker of the set is in exactly one pack: the stickers of the
/// set's items, each once over all packs.
async fn each_sticker_once(tg: &Tg, u: &TestUser, set_id: &str) {
    let srv = &tg.srv;
    let mut want = BTreeSet::new();
    for it in &detail(srv, u, set_id).await.items {
        want.insert(sticker_id(srv, u, &it.id).await);
    }
    let all: Vec<String> = packs(tg).into_values().flatten().collect();
    let distinct: BTreeSet<String> = all.iter().cloned().collect();
    assert_eq!(all.len(), distinct.len(), "a sticker in two packs");
    assert_eq!(distinct, want);
}

#[tokio::test]
async fn failed_export_between_parts_continues_without_duplicates() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, _) = big_set(&tg, "alice", "cats", 250).await;
    let names: Vec<String> = (1..=3).map(|k| part_name("cats", k)).collect();
    // Parts 1 and 2 are created, part 3's creation fails.
    tg.mock
        .lock()
        .unwrap()
        .fail_after
        .insert("createNewStickerSet".into(), 2);
    let j: Job = srv
        .post(
            &alice,
            &routes::path(routes::SET_TELEGRAM_EXPORT, &[&set.id]),
            json!({}),
        )
        .await
        .expect(200)
        .json();
    srv.run_jobs().await;
    let failed = job(srv, &alice, &j.id).await;
    assert_eq!(failed.status, JobStatus::Queued, "retried later: {failed:?}");
    assert!(
        failed
            .error
            .as_deref()
            .unwrap_or("")
            .contains(&format!("part 3 ({})", names[2])),
        "{failed:?}"
    );
    // What was done is kept: parts 1 and 2 with their stickers, part 3
    // known without stickers.
    let rows = part_rows(srv, &set.id).await;
    assert_eq!(
        rows,
        vec![
            (1, names[0].clone(), 120, true),
            (2, names[1].clone(), 120, true),
            (3, names[2].clone(), 0, rows[2].3),
        ]
    );
    assert_eq!(
        detail(srv, &alice, &set.id).await.set.telegram_pack.as_deref(),
        Some(names[0].as_str())
    );
    // The retried job continues: no second pack for a part, each sticker once.
    tg.mock.lock().unwrap().fail_after.clear();
    run_jobs_now(srv).await;
    let done = job(srv, &alice, &j.id).await;
    assert_eq!(done.status, JobStatus::Done, "{done:?}");
    let r = done.result.unwrap();
    let created: Vec<bool> = r["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["created_pack"].as_bool().unwrap())
        .collect();
    assert_eq!(created, [false, false, true], "{r}");
    let m = packs(&tg);
    assert_eq!(m.len(), 3);
    let sizes: Vec<usize> = names.iter().map(|n| m[n].len()).collect();
    assert_eq!(sizes, [120, 120, 10]);
    each_sticker_once(&tg, &alice, &set.id).await;
}

#[tokio::test]
async fn failed_export_mid_part_continues_without_duplicates() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, _) = big_set(&tg, "alice", "cats", 130).await;
    let (p1, p2) = (part_name("cats", 1), part_name("cats", 2));
    // Part 1 is created with 50 stickers, 30 more are added, then adding
    // fails.
    tg.mock.lock().unwrap().fail_after.insert("addStickerToSet".into(), 30);
    let j: Job = srv
        .post(
            &alice,
            &routes::path(routes::SET_TELEGRAM_EXPORT, &[&set.id]),
            json!({}),
        )
        .await
        .expect(200)
        .json();
    srv.run_jobs().await;
    let failed = job(srv, &alice, &j.id).await;
    assert_eq!(failed.status, JobStatus::Queued, "retried later: {failed:?}");
    assert_eq!(packs(&tg)[&p1].len(), 80);
    assert!(!packs(&tg).contains_key(&p2));
    // Part 1's state as far as it got.
    let rows = part_rows(srv, &set.id).await;
    assert_eq!(rows[0], (1, p1.clone(), 80, true));
    assert_eq!(rows[1].0, 2);
    tg.mock.lock().unwrap().fail_after.clear();
    run_jobs_now(srv).await;
    let done = job(srv, &alice, &j.id).await;
    assert_eq!(done.status, JobStatus::Done, "{done:?}");
    let m = packs(&tg);
    assert_eq!((m[&p1].len(), m[&p2].len()), (120, 10));
    assert_eq!(tg.mock.lock().unwrap().count("createNewStickerSet"), 2);
    each_sticker_once(&tg, &alice, &set.id).await;
    // And again: nothing to upload.
    let calls = tg.mock.lock().unwrap().calls.len();
    assert_eq!(export(srv, &alice, &set.id, json!({})).await.status, JobStatus::Done);
    let uploads = uploads_since(&tg, calls);
    assert!(uploads.is_empty(), "{uploads:?}");
}

/// The shapes a 0.8.8 client reads (`stickpile-api` of v0.8.8): no
/// `Set::telegram_parts`, no `Publication::parts`.
mod v088 {
    use serde::Deserialize;
    use stickpile_api::{CoverImage, GitBinding, Job, JobStatus, Permissions, SetRole, SetSource, Visibility};

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    pub struct Set {
        pub id: String,
        pub owner_id: String,
        pub owner_username: String,
        pub slug: String,
        pub name: String,
        pub description: String,
        pub visibility: Visibility,
        pub aspect_ratio: Option<(u32, u32)>,
        pub recommended_px: Option<u32>,
        pub version: u64,
        pub source: SetSource,
        pub git: Option<GitBinding>,
        pub telegram_pack: Option<String>,
        #[serde(default)]
        pub cover_item_id: Option<String>,
        #[serde(default)]
        pub cover_image: Option<CoverImage>,
        #[serde(default)]
        pub cover_url: Option<String>,
        pub item_count: u64,
        pub editable: bool,
        #[serde(default)]
        pub role: SetRole,
        #[serde(default)]
        pub permissions: Permissions,
        pub created_at: i64,
        pub updated_at: i64,
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    pub struct Publication {
        pub platform: String,
        pub name: String,
        pub configured: bool,
        pub target: Option<String>,
        pub url: Option<String>,
        pub published_at: Option<i64>,
        pub last_status: Option<JobStatus>,
        pub last_finished_at: Option<i64>,
        pub last_error: Option<String>,
        pub last_result: Option<serde_json::Value>,
        pub job: Option<Job>,
    }
}

#[tokio::test]
async fn old_client_reads_set_and_publication() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, _) = big_set(&tg, "alice", "cats", 125).await;
    let j = export(srv, &alice, &set.id, json!({"pack_name": "cats", "title": "Cats"})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let p1 = part_name("cats", 1);
    // A 0.8.8 client reads the set: telegram_pack is a working link, part 1.
    let raw = srv
        .get(&alice, &routes::path(routes::SET, &[&set.id]))
        .await
        .expect(200)
        .value();
    assert_eq!(raw["set"]["telegram_parts"].as_array().map(Vec::len), Some(2));
    let old: v088::Set = serde_json::from_value(raw["set"].clone()).unwrap();
    assert_eq!(old.telegram_pack.as_deref(), Some(p1.as_str()));
    let raw = srv
        .get(&alice, &routes::path(routes::SET_PUBLISH, &[&set.id]))
        .await
        .expect(200)
        .value();
    let old: Vec<v088::Publication> = serde_json::from_value(raw).unwrap();
    let tgp = old.iter().find(|p| p.platform == "telegram").unwrap();
    assert_eq!(tgp.target.as_deref(), Some(p1.as_str()));
    assert_eq!(tgp.url, Some(format!("https://t.me/addstickers/{p1}")));
    // The job result keeps every field a 0.8.8 client reads, for part 1.
    let r = tgp.last_result.clone().unwrap();
    for k in [
        "pack_name",
        "url",
        "title",
        "created_pack",
        "added",
        "removed",
        "updated",
        "reordered",
        "thumbnail",
        "thumbnail_changed",
        "exported",
        "skipped",
    ] {
        assert!(r.get(k).is_some(), "{k} missing: {r}");
    }
    assert_eq!(r["pack_name"], p1.as_str());
    assert_eq!(r["added"], 120, "part 1's");
    assert_eq!(r["title"], "Cats");
    // A 0.8.8 client's export request (no repack) is read as before.
    let j = export(srv, &alice, &set.id, json!({"pack_name": null, "title": null})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    assert_eq!(j.result.unwrap()["moved"], 0);
}

#[tokio::test]
async fn web_export_page_lists_parts() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, ids) = big_set(&tg, "alice", "cats", 125).await;
    assert_eq!(export(srv, &alice, &set.id, json!({})).await.status, JobStatus::Done);
    let (p1, p2) = (part_name("cats", 1), part_name("cats", 2));
    let b = web_common::Browser::login(srv, &alice).await;
    let html = b.get(&format!("/sets/{}/export", set.id)).await.expect(200).text();
    web_common::assert_clean_html(&html);
    for (name, count) in [(&p1, "120 stickers"), (&p2, "5 stickers")] {
        assert!(
            html.contains(&format!("href=\"https://t.me/addstickers/{name}\"")),
            "{name}: {html}"
        );
        assert!(html.contains(count), "{count}");
    }
    assert!(html.contains("Published as 2 packs."), "{html}");
    assert!(html.contains("A Telegram pack holds at most 120 stickers."));
    let confirm = format!("/sets/{}/export/telegram/repack", set.id);
    assert!(html.contains(&format!("href=\"{confirm}\"")), "{html}");
    // The set page links every pack; its button is unchanged.
    let page = b.get(&format!("/sets/{}", set.id)).await.expect(200).text();
    assert!(page.contains(&format!("https://t.me/addstickers/{p2}")));
    assert!(page.contains("Update Telegram pack"));

    // Repack: the confirmation says how many stickers move.
    delete_items(srv, &alice, &ids[..2]).await;
    let calls = tg.mock.lock().unwrap().calls.len();
    let frag = b.get_htmx(&confirm).await.expect(200);
    web_common::assert_fragment(&frag);
    let text = frag.text();
    assert!(text.contains("2 published stickers move to other packs."), "{text}");
    assert!(text.contains(&format!("hx-post=\"{confirm}\"")), "{text}");
    let page = b.get(&confirm).await.expect(200);
    web_common::assert_page(&page);
    assert!(page.text().contains("2 published stickers move to other packs."));
    assert!(writes_since(&tg, calls).is_empty(), "asking changes nothing");
    // Without JavaScript the form posts and the page shows the job.
    let r = b.post_form(&confirm, &[]).await;
    assert_eq!(r.status, reqwest::StatusCode::SEE_OTHER, "{}", r.text());
    let to = web_common::location(&r);
    assert!(to.starts_with(&format!("/sets/{}/export?jobs=", set.id)), "{to}");
    run_jobs_now(srv).await;
    let m = packs(&tg);
    assert_eq!((m[&p1].len(), m[&p2].len()), (120, 3));
    // In Ukrainian.
    sqlx::query("UPDATE users SET language = 'uk' WHERE id = ?")
        .bind(&alice.id)
        .execute(&srv.state.db)
        .await
        .unwrap();
    let html = b.get(&format!("/sets/{}/export", set.id)).await.expect(200).text();
    assert!(html.contains("Опубліковано як 2 набори."), "{html}");
    assert!(html.contains("Перепакувати частини"));
}

// ---------------------------------------------------------------------------
// Review of the parts round: findings
// ---------------------------------------------------------------------------

/// Packs on the mock that are no part of any set (`telegram_parts`).
async fn unbound_packs(tg: &Tg) -> Vec<String> {
    let bound: BTreeSet<String> = sqlx::query_scalar::<_, String>("SELECT lower(pack_name) FROM telegram_parts")
        .fetch_all(&tg.srv.state.db)
        .await
        .unwrap()
        .into_iter()
        .collect();
    packs(tg)
        .into_keys()
        .filter(|n| !bound.contains(&n.to_ascii_lowercase()))
        .collect()
}

/// Stickers that are in more than one pack.
fn doubled(tg: &Tg) -> Vec<String> {
    let mut seen = BTreeMap::<String, usize>::new();
    for f in packs(tg).into_values().flatten() {
        *seen.entry(f).or_default() += 1;
    }
    seen.into_iter().filter(|(_, n)| *n > 1).map(|(f, _)| f).collect()
}

/// Finding: an export under a new name that fails before it changes
/// anything on Telegram still drops the set's parts (rows, mappings) and its
/// `telegram_pack`. Going back to the name the set is still published under
/// on Telegram is then "a new name" again: it starts from nothing, moves the
/// stickers of part 2 into part 1 without a repack, and leaves part 2 on
/// Telegram, bound to nothing, with copies of stickers that are now in
/// part 1 (rules 5 and 6).
#[tokio::test]
async fn review_failed_rename_loses_the_parts() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, ids) = big_set(&tg, "alice", "cats", 125).await;
    let (p1, p2) = (part_name("cats", 1), part_name("cats", 2));
    assert_eq!(export(srv, &alice, &set.id, json!({})).await.status, JobStatus::Done);
    // 20 stickers of part 1 go: 100 and 5, nothing moves.
    delete_items(srv, &alice, &ids[..20]).await;
    assert_eq!(export(srv, &alice, &set.id, json!({})).await.status, JobStatus::Done);
    let before = packs(&tg);
    assert_eq!((before[&p1].len(), before[&p2].len()), (100, 5));
    let placed = item_packs(srv, &set.id).await;

    // A rename whose first call on Telegram keeps failing (an outage, or a
    // name Telegram refuses): the job fails, Telegram is unchanged.
    tg.mock
        .lock()
        .unwrap()
        .fail_after
        .insert("createNewStickerSet".into(), 0);
    let j = export(srv, &alice, &set.id, json!({"pack_name": "dogs"})).await;
    assert_eq!(j.status, JobStatus::Failed, "{j:?}");
    tg.mock.lock().unwrap().fail_after.clear();
    assert_eq!(packs(&tg), before, "nothing on Telegram changed");
    let link_after_failure = detail(srv, &alice, &set.id).await.set.telegram_pack;
    let rows_after_failure = part_rows(srv, &set.id).await;

    // The owner goes back to the name the set is published under.
    let j = export(srv, &alice, &set.id, json!({"pack_name": "cats"})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let after = item_packs(srv, &set.id).await;
    let moved: Vec<&String> = after
        .iter()
        .filter(|(item, pack)| placed.get(*item) != Some(*pack))
        .map(|(item, _)| item)
        .collect();
    let unbound = unbound_packs(&tg).await;
    let twice = doubled(&tg);
    assert!(
        link_after_failure.as_deref() == Some(p1.as_str())
            && moved.is_empty()
            && unbound.is_empty()
            && twice.is_empty(),
        "after the failed rename: telegram_pack {link_after_failure:?}, parts {rows_after_failure:?}; after going \
         back to \"cats\": {} published stickers moved without a repack, packs bound to no set {unbound:?}, {} \
         stickers in two packs",
        moved.len(),
        twice.len()
    );
}

/// Finding (minor): a part registered for an export that stopped before its
/// pack was created is "deleted" when the set shrinks (`deleteStickerSet`
/// answers that the pack does not exist), and its never-used name is
/// retired: the next part the set needs skips the number.
#[tokio::test]
async fn review_a_part_never_created_is_not_retired() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, set, ids) = big_set(&tg, "alice", "cats", 250).await;
    // Parts 1 and 2 are created, part 3's creation keeps failing.
    tg.mock
        .lock()
        .unwrap()
        .fail_after
        .insert("createNewStickerSet".into(), 2);
    let j = export(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Failed, "{j:?}");
    tg.mock.lock().unwrap().fail_after.clear();
    assert!(
        !packs(&tg).contains_key(&part_name("cats", 3)),
        "part 3 was never created"
    );
    // The last ten items go before the next export: part 3 is not needed.
    delete_items(srv, &alice, &ids[240..]).await;
    assert_eq!(export(srv, &alice, &set.id, json!({})).await.status, JobStatus::Done);
    // Later the set grows past two packs again.
    add_items(srv, &alice, &set.id, 8000, 5).await;
    let numbers: Vec<u32> = plan(srv, &alice, &set.id, false)
        .await
        .parts
        .iter()
        .map(|p| p.number)
        .collect();
    assert_eq!(numbers, [1, 2, 3], "a name no pack ever had is not retired");
}

/// Finding: publishing another of the user's sets under a set's name moves
/// every part at the start of the export; when that export fails before it
/// changes anything on Telegram, the first set has lost its packs (its
/// `telegram_pack` is gone at once) and its mappings. Its next export takes
/// the packs back without a mapping: every sticker is uploaded again and
/// the stickers of part 2 move into part 1 without a repack (rule 5), part 2
/// is deleted.
#[tokio::test]
async fn review_failed_takeover_loses_the_first_sets_parts() {
    let tg = setup(|_| {}).await;
    let srv = &tg.srv;
    let (alice, cats, ids) = big_set(&tg, "alice", "cats", 125).await;
    let (p1, p2) = (part_name("cats", 1), part_name("cats", 2));
    assert_eq!(export(srv, &alice, &cats.id, json!({})).await.status, JobStatus::Done);
    delete_items(srv, &alice, &ids[..20]).await;
    assert_eq!(export(srv, &alice, &cats.id, json!({})).await.status, JobStatus::Done);
    let before = packs(&tg);
    assert_eq!((before[&p1].len(), before[&p2].len()), (100, 5));
    let placed = item_packs(srv, &cats.id).await;

    // Alice's other set takes the name over; its first write on Telegram
    // keeps failing.
    let dogs = srv.create_set(&alice, "dogs", Visibility::Private).await;
    add_items(srv, &alice, &dogs.id, 7000, 3).await;
    tg.mock
        .lock()
        .unwrap()
        .fail_after
        .insert("deleteStickerFromSet".into(), 0);
    let j = export(srv, &alice, &dogs.id, json!({"pack_name": "cats"})).await;
    assert_eq!(j.status, JobStatus::Failed, "{j:?}");
    tg.mock.lock().unwrap().fail_after.clear();
    assert_eq!(packs(&tg), before, "nothing on Telegram changed");
    let link_after_failure = detail(srv, &alice, &cats.id).await.set.telegram_pack;

    // cats is exported again, as it was published before.
    let calls = tg.mock.lock().unwrap().calls.len();
    let j = export(srv, &alice, &cats.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let uploads = uploads_since(&tg, calls)
        .into_iter()
        .filter(|c| c == "addStickerToSet" || c == "replaceStickerInSet" || c == "createNewStickerSet")
        .count();
    let after = item_packs(srv, &cats.id).await;
    let moved = after
        .iter()
        .filter(|(item, pack)| placed.get(*item) != Some(*pack))
        .count();
    assert!(
        link_after_failure.as_deref() == Some(p1.as_str()) && moved == 0 && uploads == 0,
        "after dogs' failed takeover: cats' telegram_pack {link_after_failure:?}; cats' next export: {moved} \
         published stickers moved to another pack without a repack, {uploads} stickers uploaded again, packs now \
         {:?}",
        packs(&tg).iter().map(|(n, s)| (n.clone(), s.len())).collect::<Vec<_>>()
    );
}
