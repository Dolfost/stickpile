//! Signal import, publishing and the Signal-ready zip against the
//! in-process mock of Signal's service and CDN
//! (`integrations_support::mock_signal`, a copy of the stickpile-signal
//! crate's). Nothing here talks to Signal.

mod common;
mod integrations_support;
mod web_common;

use std::collections::BTreeMap;

use common::{png, TestServer, TestUser};
use integrations_support::mock_signal::{self, Shared, ACI, DEVICE, PASSWORD};
use integrations_support::run_jobs_now;
use serde_json::{json, Value};
use stickpile_api::{routes, Job, JobKind, JobStatus, Publication, PublishJobs, SetDetail, Visibility};
use stickpile_signal::crypto::{derive_keys, encrypt};
use stickpile_signal::{
    check_sticker, import_pack, ImageFormat, Manifest, ManifestSticker, PackKey, PackLink, SignalClient, SignalConfig,
};

struct Sg {
    srv: TestServer,
    mock: Shared,
    service: String,
    cdn: String,
    dir: tempfile::TempDir,
}

/// A server with Signal on; `credentials` false leaves out the account.
async fn setup(credentials: bool) -> Sg {
    let (service, cdn, mock) = mock_signal::start().await;
    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join("signal.json");
    std::fs::write(
        &creds,
        json!({"aci": ACI, "device_id": DEVICE, "password": PASSWORD}).to_string(),
    )
    .unwrap();
    let (s, c) = (service.clone(), cdn.clone());
    let srv = TestServer::with_config(move |cfg| {
        cfg.signal.enabled = true;
        cfg.signal.credentials_file = credentials.then_some(creds);
        cfg.signal.service_url = s;
        cfg.signal.cdn_url = c;
    })
    .await;
    Sg {
        srv,
        mock,
        service,
        cdn,
        dir,
    }
}

fn client(sg: &Sg) -> SignalClient {
    SignalClient::new(&SignalConfig {
        service_url: sg.service.clone(),
        cdn_url: sg.cdn.clone(),
        trust_public_roots: false,
    })
    .unwrap()
}

async fn job(srv: &TestServer, u: &TestUser, id: &str) -> Job {
    srv.get(u, &routes::path(routes::JOB, &[id])).await.expect(200).json()
}

async fn drop_variant_jobs(srv: &TestServer) {
    sqlx::query("DELETE FROM jobs WHERE kind IN ('variants', 'enrich')")
        .execute(&srv.state.db)
        .await
        .unwrap();
}

async fn publish(srv: &TestServer, u: &TestUser, set_id: &str) -> Job {
    let jobs: PublishJobs = srv
        .post(
            u,
            &routes::path(routes::SET_PUBLISH, &[set_id]),
            json!({"platforms": ["signal"]}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(jobs.jobs[0].kind, JobKind::SignalExport);
    drop_variant_jobs(srv).await;
    run_jobs_now(srv).await;
    job(srv, u, &jobs.jobs[0].id).await
}

fn zip_entries(zip: &[u8]) -> BTreeMap<String, Vec<u8>> {
    stickpile_server::archive::entries(zip).unwrap().into_iter().collect()
}

/// Every sticker passes Signal's rules and is a 512x512 square (smaller
/// squares only for animations that had to shrink).
fn check(bytes: &[u8]) -> stickpile_signal::ImageInfo {
    let info = check_sticker(bytes).unwrap();
    assert_eq!(info.width, info.height);
    if !info.animated {
        assert_eq!(info.width, 512);
    }
    info
}

#[tokio::test]
async fn publish_uploads_a_new_pack_every_time() {
    let sg = setup(true).await;
    let srv = &sg.srv;
    let alice = srv.user("alice").await;
    srv.patch(&alice, routes::ME, json!({"display_name": "Alice A."}))
        .await
        .expect(200);
    let set = srv.create_set(&alice, "cats", Visibility::Private).await;
    let a = srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
    let wide = common::encode(&common::image(300, 100, 2, 0), image::ImageFormat::Png);
    srv.upload_one(&alice, &set.id, "wide.png", wide).await;
    srv.upload_one(&alice, &set.id, "moving.gif", common::gif_anim(3)).await;
    srv.patch(
        &alice,
        &routes::path(routes::ITEM, &[&a.id]),
        json!({"emojis": ["cat", "😼"]}),
    )
    .await
    .expect(200);
    srv.put_cover(&alice, &set.id, "cover.png", png(9)).await.expect(200);

    let j = publish(srv, &alice, &set.id).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    assert_eq!(r["exported"], 3, "{r}");
    let url = r["url"].as_str().unwrap().to_string();
    assert!(url.starts_with("https://signal.art/addstickers/#pack_id="), "{url}");
    {
        let m = sg.mock.lock().unwrap();
        // Three stickers and the cover image, with the account's Basic auth.
        assert_eq!(m.forms, vec![(format!("{ACI}.{DEVICE}"), 4)]);
        assert!(m.refused.is_empty(), "{:?}", m.refused);
        assert!(!m.cdn_saw_auth);
    }
    // The pack reads back: the stickers in order, their emoji, the cover.
    let pack = import_pack(&client(&sg), &PackLink::parse(&url).unwrap(), 1 << 20)
        .await
        .unwrap();
    assert_eq!((pack.title.as_str(), pack.author.as_str()), ("cats", "Alice A."));
    let emojis: Vec<Option<&str>> = pack.stickers.iter().map(|s| s.emoji.as_deref()).collect();
    assert_eq!(emojis, [Some("😼"), Some("🙂"), Some("🙂")]);
    for s in &pack.stickers {
        check(&s.bytes);
    }
    assert_eq!(pack.stickers[0].format, ImageFormat::Webp);
    assert_eq!(pack.stickers[2].format, ImageFormat::Apng);
    assert!(pack.stickers[2].animated);
    let wide = image::load_from_memory(&pack.stickers[1].bytes).unwrap().to_rgba8();
    assert_eq!(wide.get_pixel(256, 10)[3], 0, "padded with transparency");
    assert_eq!(pack.cover_id, Some(3));
    check(&pack.cover.unwrap().bytes);

    let list: Vec<Publication> = srv
        .get(&alice, &routes::path(routes::SET_PUBLISH, &[&set.id]))
        .await
        .expect(200)
        .json();
    let p = list.iter().find(|p| p.platform == "signal").unwrap();
    assert!(p.configured);
    assert_eq!(p.url.as_deref(), Some(url.as_str()));
    assert_eq!(p.target.as_deref(), Some("cats"));

    // Again: a new pack with a new link, the newest is the set's.
    let again = publish(srv, &alice, &set.id).await;
    let url2 = again.result.unwrap()["url"].as_str().unwrap().to_string();
    assert_ne!(url, url2);
    assert_eq!(sg.mock.lock().unwrap().forms.len(), 2);
    let list: Vec<Publication> = srv
        .get(&alice, &routes::path(routes::SET_PUBLISH, &[&set.id]))
        .await
        .expect(200)
        .json();
    let p = list.iter().find(|p| p.platform == "signal").unwrap();
    assert_eq!(p.url.as_deref(), Some(url2.as_str()));
    let summary = stickpile_server::service::publish::platform("signal")
        .unwrap()
        .summary(p.last_result.as_ref().unwrap());
    assert_eq!(summary, "3 stickers in a new pack, 0 left out");

    // Signal takes no options; a reader may not publish.
    let r = srv
        .post(
            &alice,
            &routes::path(routes::SET_PUBLISH, &[&set.id]),
            json!({"platforms": ["signal"], "options": {"signal": {"title": "x"}}}),
        )
        .await
        .expect(400);
    assert!(r.text().contains("Signal takes no options"), "{}", r.text());
}

#[tokio::test]
async fn upload_errors_are_clear_and_keep_the_password_out() {
    let sg = setup(true).await;
    let srv = &sg.srv;
    let alice = srv.user("alice").await;
    let set = srv.create_set(&alice, "limited", Visibility::Private).await;
    srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
    sg.mock.lock().unwrap().rate_limited = 1;
    let j = publish(srv, &alice, &set.id).await;
    assert_eq!(j.status, JobStatus::Failed);
    assert_eq!(j.attempts, 1, "rate limits are not retried");
    assert!(j.error.as_deref().unwrap().contains("rate limit"), "{j:?}");

    // Wrong credentials: refused, and nothing about the password shown.
    let creds = sg.dir.path().join("signal.json");
    std::fs::write(
        &creds,
        json!({"aci": ACI, "device_id": DEVICE, "password": "SECRET-wrong-password"}).to_string(),
    )
    .unwrap();
    let j = publish(srv, &alice, &set.id).await;
    assert_eq!(j.status, JobStatus::Failed);
    assert!(j.error.as_deref().unwrap().contains("refused the account"), "{j:?}");
    let shown = serde_json::to_string(&j).unwrap();
    assert!(!shown.contains("SECRET") && !shown.contains(PASSWORD), "{shown}");

    // A credentials file that does not parse: Signal is not configured.
    std::fs::write(&creds, format!("{{\"password\": \"{PASSWORD}\"")).unwrap();
    let r = srv
        .post(
            &alice,
            &routes::path(routes::SET_PUBLISH, &[&set.id]),
            json!({"platforms": ["signal"]}),
        )
        .await
        .expect(400);
    assert!(
        r.text().contains("not configured") && !r.text().contains(PASSWORD),
        "{}",
        r.text()
    );
}

/// Puts an encrypted pack on the mock CDN: `files` by sticker id.
fn seed_pack(state: &Shared, pack_id: &str, key: &PackKey, manifest: &Manifest, files: &[(u32, Vec<u8>)]) {
    let keys = derive_keys(key.as_bytes());
    let mut st = state.lock().unwrap();
    st.objects.insert(
        format!("stickers/{pack_id}/manifest.proto"),
        encrypt(&keys, &manifest.encode()),
    );
    for (id, bytes) in files {
        st.objects
            .insert(format!("stickers/{pack_id}/full/{id}"), encrypt(&keys, bytes));
    }
}

fn ms(id: u32, emoji: &str) -> ManifestSticker {
    ManifestSticker {
        id,
        emoji: Some(emoji.into()),
        content_type: None,
    }
}

#[tokio::test]
async fn import_reads_a_pack_and_forgets_its_link() {
    // Import needs no account.
    let sg = setup(false).await;
    let srv = &sg.srv;
    let alice = srv.user("alice").await;
    let pack_id = "0123456789abcdef0123456789abcdef";
    let key = PackKey::random();
    let manifest = Manifest {
        title: Some("Signal Cats".into()),
        author: Some("Someone".into()),
        cover: Some(ms(1, "")),
        stickers: vec![ms(0, "😼"), ms(1, "😾"), ms(2, "🙀"), ms(3, "😿")],
    };
    let webp = common::encode(&common::image(512, 512, 1, 0), image::ImageFormat::WebP);
    seed_pack(
        &sg.mock,
        pack_id,
        &key,
        &manifest,
        &[(0, webp), (1, png(2)), (2, png(3))], // 3 is missing on the CDN
    );
    let link = PackLink::new(pack_id, key).unwrap();
    let key_hex = link.url().rsplit('=').next().unwrap().to_string();

    // Bad links are refused before anything is queued.
    for bad in [
        "https://signal.art/addstickers/",
        "https://example.org/#pack_id=1&pack_key=2",
        "cats",
    ] {
        let r = srv
            .post(&alice, routes::SIGNAL_IMPORT, json!({"link": bad}))
            .await
            .expect(400);
        assert!(r.text().contains("Signal sticker pack link"), "{}", r.text());
    }
    // The app form of the link works too.
    let j: Job = srv
        .post(
            &alice,
            routes::SIGNAL_IMPORT,
            json!({"link": link.app_url(), "visibility": "public"}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(j.kind, JobKind::SignalImport);
    assert_eq!(j.subject, None, "the pack is not named in the job");
    drop_variant_jobs(srv).await;
    run_jobs_now(srv).await;
    let j = job(srv, &alice, &j.id).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.clone().unwrap();
    assert_eq!(r["imported"], 3, "{r}");
    assert_eq!(r["title"], "Signal Cats");
    assert_eq!(r["author"], "Someone");
    assert_eq!(r["skipped"][0]["sticker_id"], 3);
    assert_eq!(r["cover"], true);
    let d: SetDetail = srv
        .get(&alice, &routes::path(routes::SET, &[r["set_id"].as_str().unwrap()]))
        .await
        .expect(200)
        .json();
    assert_eq!(
        (d.set.slug.as_str(), d.set.name.as_str()),
        ("signal-cats", "Signal Cats")
    );
    assert_eq!(d.set.visibility, Visibility::Public);
    let emojis: Vec<Vec<String>> = d.items.iter().map(|i| i.emojis.clone()).collect();
    assert_eq!(emojis, [vec!["😼".to_string()], vec!["😾".into()], vec!["🙀".into()]]);
    assert_eq!(d.set.cover_item_id.as_deref(), Some(d.items[1].id.as_str()));

    // The key is in no result, job or payload once the job is done.
    let payload: String = sqlx::query_scalar("SELECT payload FROM jobs WHERE id = ?")
        .bind(&j.id)
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    let shown = format!("{payload} {}", serde_json::to_string(&j).unwrap());
    assert!(!shown.contains(&key_hex) && !shown.contains("pack_key"), "{shown}");

    // A second import of the same pack gets the next free slug.
    let j: Job = srv
        .post(&alice, routes::SIGNAL_IMPORT, json!({"link": link.url()}))
        .await
        .expect(200)
        .json();
    drop_variant_jobs(srv).await;
    run_jobs_now(srv).await;
    let r = job(srv, &alice, &j.id).await.result.unwrap();
    assert_eq!(r["slug"], "signal-cats-2");

    // A pack that does not exist fails the job, without the key.
    let missing = PackLink::new("fedcba9876543210fedcba9876543210", PackKey::random()).unwrap();
    let j: Job = srv
        .post(&alice, routes::SIGNAL_IMPORT, json!({"link": missing.url()}))
        .await
        .expect(200)
        .json();
    run_jobs_now(srv).await;
    let j = job(srv, &alice, &j.id).await;
    assert_eq!(j.status, JobStatus::Failed);
    assert_eq!(j.error.as_deref(), Some("sticker pack not found"));
}

#[tokio::test]
async fn the_signal_ready_zip_works_with_and_without_signal() {
    let plain = TestServer::start().await;
    zip_round(&plain, false).await;
    let sg = setup(true).await;
    zip_round(&sg.srv, true).await;
    // Nothing went to Signal for it.
    assert!(sg.mock.lock().unwrap().forms.is_empty());
}

async fn zip_round(srv: &TestServer, configured: bool) {
    {
        let alice = srv.user("alice").await;
        let bob = srv.user("bob").await;
        let set = srv.create_set(&alice, "zip-cats", Visibility::Public).await;
        let a = srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
        srv.upload_one(&alice, &set.id, "b.gif", common::gif_anim(2)).await;
        srv.patch(&alice, &routes::path(routes::ITEM, &[&a.id]), json!({"emojis": ["😺"]}))
            .await
            .expect(200);
        drop_variant_jobs(srv).await;
        let url = routes::path(routes::SET_SIGNAL_ZIP, &[&set.id]);
        // A reader downloads it.
        let r = srv.get(&bob, &url).await.expect(200);
        assert_eq!(r.header("content-type").as_deref(), Some("application/zip"));
        assert!(r.header("content-disposition").unwrap().contains("zip-cats-signal.zip"));
        let files = zip_entries(&r.body);
        let names: Vec<&str> = files.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            ["001.webp", "002.png", "README.txt", "emoji.txt", "manifest.json"],
            "configured: {configured}"
        );
        check(&files["001.webp"]);
        assert!(check(&files["002.png"]).animated);
        assert_eq!(
            String::from_utf8_lossy(&files["emoji.txt"]),
            "001.webp 😺\n002.png 🙂\n"
        );
        let manifest: Value = serde_json::from_slice(&files["manifest.json"]).unwrap();
        assert_eq!(manifest["title"], "zip-cats");
        assert_eq!(manifest["stickers"][1]["contentType"], "image/png");
        // Unchanged: the same file again; changed: made anew.
        let again = srv.get(&alice, &url).await.expect(200);
        assert_eq!(again.body, r.body);
        srv.patch(&alice, &routes::path(routes::ITEM, &[&a.id]), json!({"emojis": ["🎉"]}))
            .await
            .expect(200);
        let changed = zip_entries(&srv.get(&alice, &url).await.expect(200).body);
        assert!(String::from_utf8_lossy(&changed["emoji.txt"]).starts_with("001.webp 🎉"));
        // Private sets: 404 for others; an empty set: nothing to make.
        let private = srv.create_set(&alice, "secret", Visibility::Private).await;
        let purl = routes::path(routes::SET_SIGNAL_ZIP, &[&private.id]);
        srv.get(&bob, &purl).await.expect(404);
        srv.get(&alice, &purl).await.expect(400);
    }
}

#[tokio::test]
async fn unconfigured_platforms_are_not_offered_and_their_routes_are_404() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let set = srv.create_set(&alice, "plain", Visibility::Private).await;
    srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
    for (route, body) in [
        (routes::DISCORD_IMPORT, json!({"guild_id": "100000000000000001"})),
        (
            routes::SIGNAL_IMPORT,
            json!({"link": "https://signal.art/addstickers/#pack_id=0123456789abcdef0123456789abcdef&pack_key=00"}),
        ),
        (routes::TELEGRAM_IMPORT, json!({"pack_name": "Cats"})),
    ] {
        let r = srv.post(&alice, route, body).await.expect(404);
        assert!(r.text().contains("disabled"), "{route}: {}", r.text());
    }
    let list: Vec<Publication> = srv
        .get(&alice, &routes::path(routes::SET_PUBLISH, &[&set.id]))
        .await
        .expect(200)
        .json();
    let ids: Vec<(&str, bool)> = list.iter().map(|p| (p.platform.as_str(), p.configured)).collect();
    assert_eq!(ids, [("telegram", false), ("discord", false), ("signal", false)]);
    for platform in ["discord", "signal"] {
        let r = srv
            .post(
                &alice,
                &routes::path(routes::SET_PUBLISH, &[&set.id]),
                json!({"platforms": [platform]}),
            )
            .await
            .expect(400);
        assert!(r.text().contains("not configured"), "{}", r.text());
    }
    // Signal on but without an account: import yes, publish no.
    let sg = setup(false).await;
    let bob = sg.srv.user("bob").await;
    let bobs = sg.srv.create_set(&bob, "b", Visibility::Private).await;
    let list: Vec<Publication> = sg
        .srv
        .get(&bob, &routes::path(routes::SET_PUBLISH, &[&bobs.id]))
        .await
        .expect(200)
        .json();
    assert!(!list.iter().find(|p| p.platform == "signal").unwrap().configured);

    // The web pages: no import pages, no publish rows, the zip offered.
    use web_common::{assert_page, Browser};
    let b = Browser::login(&srv, &alice).await;
    assert_eq!(b.get("/discord/import").await.status.as_u16(), 404);
    assert_eq!(b.get("/signal/import").await.status.as_u16(), 404);
    let r = b.get(&format!("/sets/{}/export", set.id)).await;
    assert_page(&r);
    let html = r.text();
    assert!(html.contains("No platform to publish to is set up on this server."));
    assert!(!html.contains("name=\"platform.discord\""));
    assert!(html.contains(&routes::path(routes::SET_SIGNAL_ZIP, &[&set.id])));
    assert!(html.contains("does not publish to Signal itself"));
    let html = b.get("/sets").await.text();
    assert!(!html.contains("/discord/import") && !html.contains("/signal/import"));
}

#[tokio::test]
async fn the_import_page_queues_an_import_and_forgets_a_bad_link() {
    use reqwest::StatusCode;
    use web_common::{assert_page, location, Browser};
    let sg = setup(false).await;
    let srv = &sg.srv;
    let alice = srv.user("alice").await;
    let b = Browser::login(srv, &alice).await;
    assert!(b.get("/sets").await.text().contains("href=\"/signal/import\""));
    assert!(b.get("/upload").await.text().contains("href=\"/signal/import\""));
    let r = b.get("/signal/import").await;
    assert_page(&r);
    assert!(r.text().contains("name=\"link\""));
    // A bad link is not shown back: it could hold a key.
    let bad = "https://signal.art/addstickers/#pack_id=0123&pack_key=SECRETKEY";
    let r = b.post_form("/signal/import", &[("link", bad), ("slug", "mine")]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let html = r.text();
    assert!(html.contains("Signal sticker pack link"), "{html}");
    assert!(!html.contains("SECRETKEY") && html.contains("value=\"mine\""), "{html}");
    let link = PackLink::new("0123456789abcdef0123456789abcdef", PackKey::random()).unwrap();
    let r = b.post_form("/signal/import", &[("link", &link.url())]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    assert!(location(&r).starts_with("/jobs/"));
}
