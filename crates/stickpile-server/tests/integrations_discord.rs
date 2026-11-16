//! Discord import and publishing against the in-process mock of the
//! Discord API and CDN (`integrations_support::mock_discord`, a copy of the
//! stickpile-discord crate's). Nothing here talks to Discord.

mod common;
mod integrations_support;
mod web_common;

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

use common::{png, TestServer, TestUser};
use integrations_support::mock_discord::{self, Shared, BOT_ID, GUILD, HUMAN_ID, TOKEN};
use integrations_support::run_jobs_now;
use serde_json::{json, Value};
use stickpile_api::{routes, Job, JobKind, JobStatus, Publication, PublishJobs, SetDetail, Visibility};
use stickpile_discord::{check_file, ExpressionKind, ImageFormat};

struct Dc {
    srv: TestServer,
    mock: Shared,
    _dir: tempfile::TempDir,
}

async fn setup() -> Dc {
    let (api, cdn, media, mock) = mock_discord::start().await;
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("discord-token");
    std::fs::write(&token, format!("{TOKEN}\n")).unwrap();
    let srv = TestServer::with_config(|c| {
        c.discord.enabled = true;
        c.discord.bot_token_file = Some(token);
        c.discord.api_url = api;
        c.discord.cdn_url = cdn;
        c.discord.media_url = media;
    })
    .await;
    Dc { srv, mock, _dir: dir }
}

async fn job(srv: &TestServer, u: &TestUser, id: &str) -> Job {
    srv.get(u, &routes::path(routes::JOB, &[id])).await.expect(200).json()
}

/// The renditions are made in the job from the originals: the variant jobs
/// are not needed (and would only take time).
async fn drop_variant_jobs(srv: &TestServer) {
    sqlx::query("DELETE FROM jobs WHERE kind IN ('variants', 'enrich')")
        .execute(&srv.state.db)
        .await
        .unwrap();
}

async fn publish(srv: &TestServer, u: &TestUser, set_id: &str, options: Value) -> Job {
    let jobs: PublishJobs = srv
        .post(
            u,
            &routes::path(routes::SET_PUBLISH, &[set_id]),
            json!({"platforms": ["discord"], "options": {"discord": options}}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(jobs.jobs.len(), 1);
    assert_eq!(jobs.jobs[0].kind, JobKind::DiscordExport);
    drop_variant_jobs(srv).await;
    run_jobs_now(srv).await;
    job(srv, u, &jobs.jobs[0].id).await
}

async fn publication(srv: &TestServer, u: &TestUser, set_id: &str) -> Publication {
    let list: Vec<Publication> = srv
        .get(u, &routes::path(routes::SET_PUBLISH, &[set_id]))
        .await
        .expect(200)
        .json();
    list.into_iter().find(|p| p.platform == "discord").unwrap()
}

/// The stored mapping of (set, guild, kind): item id -> Discord id.
async fn mapping(srv: &TestServer, set_id: &str, kind: &str) -> serde_json::Map<String, Value> {
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM set_publications WHERE set_id = ? AND platform = 'discord'")
            .bind(set_id)
            .fetch_optional(&srv.state.db)
            .await
            .unwrap()
            .flatten();
    let v: Value = serde_json::from_str(&state.unwrap_or_else(|| "{}".into())).unwrap();
    v["mappings"][format!("{GUILD}:{kind}")]
        .as_object()
        .cloned()
        .unwrap_or_default()
}

fn calls(mock: &Shared, route: &str) -> usize {
    mock.lock().unwrap().count(route)
}

/// A 3-frame GIF of 32x32: an animation.
fn gif() -> Vec<u8> {
    common::gif_anim(7)
}

#[tokio::test]
async fn publish_creates_keeps_updates_and_removes_stickers() {
    let dc = setup().await;
    let srv = &dc.srv;
    let alice = srv.user("alice").await;
    let set = srv.create_set(&alice, "cats", Visibility::Private).await;
    let a = srv.upload_one(&alice, &set.id, "Grumpy cat.png", png(1)).await;
    let wide = common::encode(&common::image(300, 100, 2, 0), image::ImageFormat::Png);
    let b = srv.upload_one(&alice, &set.id, "Wide cat.png", wide).await;
    let c = srv.upload_one(&alice, &set.id, "Moving cat.gif", gif()).await;
    srv.patch(
        &alice,
        &routes::path(routes::ITEM, &[&a.id]),
        json!({"emojis": ["😾", "cat"]}),
    )
    .await
    .expect(200);

    // The first publish needs the server id.
    let r = srv
        .post(
            &alice,
            &routes::path(routes::SET_PUBLISH, &[&set.id]),
            json!({"platforms": ["discord"]}),
        )
        .await
        .expect(400);
    assert!(r.text().contains("guild_id is required"), "{}", r.text());

    let j = publish(
        srv,
        &alice,
        &set.id,
        json!({"guild_id": format!("https://discord.com/channels/{GUILD}/1")}),
    )
    .await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    assert_eq!((r["added"].as_u64(), r["exported"].as_u64()), (Some(3), Some(3)), "{r}");
    assert_eq!(r["kind"], "sticker");
    assert_eq!(r["guild_name"], "Cat Lounge");
    assert_eq!(r["url"], format!("https://discord.com/channels/{GUILD}"));
    {
        let m = dc.mock.lock().unwrap();
        assert_eq!(m.stickers.len(), 3);
        let names: Vec<&str> = m.stickers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Grumpy cat", "Wide cat", "Moving cat"]);
        assert_eq!(m.stickers[0].tags, "😾", "the first real emoji");
        for s in &m.stickers {
            assert_eq!(s.user_id, BOT_ID);
            let info = check_file(ExpressionKind::Sticker, &s.bytes).unwrap();
            assert_eq!((info.width, info.height), (320, 320), "{}", s.name);
        }
        // Stills are PNG, the animation an APNG (format type 2).
        assert_eq!(m.stickers[1].format_type, 1);
        assert_eq!(m.stickers[2].format_type, 2);
        let wide = image::load_from_memory(&m.stickers[1].bytes).unwrap().to_rgba8();
        assert_eq!(wide.get_pixel(160, 5)[3], 0, "padded with transparency");
        assert_eq!(wide.get_pixel(160, 160)[3], 255);
        assert_eq!(m.missing_audit_reason, 0);
        assert!(!m.cdn_saw_auth);
    }
    let p = publication(srv, &alice, &set.id).await;
    assert!(p.configured);
    assert_eq!(p.target.as_deref(), Some("Cat Lounge"));
    assert_eq!(
        p.url.as_deref(),
        Some(format!("https://discord.com/channels/{GUILD}").as_str())
    );
    assert_eq!(p.last_status, Some(JobStatus::Done));
    assert_eq!(mapping(srv, &set.id, "sticker").await.len(), 3);

    // Again without options: the same server, nothing to do.
    let posts = calls(&dc.mock, "POST /stickers");
    let j = publish(srv, &alice, &set.id, json!({})).await;
    let r = j.result.unwrap();
    assert_eq!(
        (r["added"].as_u64(), r["unchanged"].as_u64()),
        (Some(0), Some(3)),
        "{r}"
    );
    assert_eq!(calls(&dc.mock, "POST /stickers"), posts);
    assert_eq!(calls(&dc.mock, "DELETE /stickers"), 0);

    // A rename is an update, a removed item a delete, a new one an upload.
    srv.patch(
        &alice,
        &routes::path(routes::ITEM, &[&b.id]),
        json!({"name": "Long cat"}),
    )
    .await
    .expect(200);
    srv.delete(&alice, &routes::path(routes::ITEM, &[&c.id]))
        .await
        .expect(204);
    srv.upload_one(&alice, &set.id, "New cat.png", png(4)).await;
    let j = publish(srv, &alice, &set.id, json!({})).await;
    let r = j.result.unwrap();
    assert_eq!(
        (r["added"].as_u64(), r["removed"].as_u64(), r["updated"].as_u64()),
        (Some(1), Some(1), Some(1)),
        "{r}"
    );
    {
        let m = dc.mock.lock().unwrap();
        let names: Vec<&str> = m.stickers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Grumpy cat", "Long cat", "New cat"]);
    }
    let map = mapping(srv, &set.id, "sticker").await;
    assert_eq!(map.len(), 3);
    assert!(map.contains_key(&a.id) && !map.contains_key(&c.id));

    // The summary on the export page.
    let p = publication(srv, &alice, &set.id).await;
    assert_eq!(p.last_result.unwrap()["exported"], 3);
}

#[tokio::test]
async fn slot_limits_skip_items_and_emoji_are_their_own_kind() {
    let dc = setup().await;
    let srv = &dc.srv;
    let alice = srv.user("alice").await;
    {
        let mut m = dc.mock.lock().unwrap();
        // Boost level 0: 5 sticker slots, 3 taken by people.
        for i in 0..3u8 {
            m.seed_sticker(&format!("human {i}"), "🙂", 1, mock_discord::png(64, 64, i), HUMAN_ID);
        }
    }
    let set = srv.create_set(&alice, "many", Visibility::Private).await;
    for i in 1..=4 {
        srv.upload_one(&alice, &set.id, &format!("cat {i}.png"), png(i)).await;
    }
    srv.upload_one(&alice, &set.id, "moving.gif", gif()).await;
    let j = publish(srv, &alice, &set.id, json!({"guild_id": GUILD})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    assert_eq!((r["added"].as_u64(), r["exported"].as_u64()), (Some(2), Some(2)), "{r}");
    let skipped = r["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 3, "{r}");
    assert!(skipped
        .iter()
        .all(|s| s["reason"].as_str().unwrap().contains("no free sticker slot")));
    assert_eq!(skipped[0]["name"], "cat 3");
    assert_eq!(r["slots"], json!({"used": 5, "limit": 5}));
    {
        let m = dc.mock.lock().unwrap();
        assert_eq!(m.stickers.len(), 5);
        assert!(
            m.stickers[..3].iter().all(|s| s.user_id == HUMAN_ID),
            "people's stickers stay"
        );
    }
    let p = publication(srv, &alice, &set.id).await;
    let summary = stickpile_server::service::publish::platform("discord")
        .unwrap()
        .summary(p.last_result.as_ref().unwrap());
    assert_eq!(summary, "2 stickers: 2 added, 0 removed, 3 left out");

    // The same set as emoji: PNG stills and a GIF animation, 128x128.
    let j = publish(srv, &alice, &set.id, json!({"kind": "emojis"})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    assert_eq!(r["kind"], "emoji");
    assert_eq!(r["added"], 5, "{r}");
    assert_eq!(r["animated_slots"], json!({"used": 1, "limit": 50}));
    {
        let m = dc.mock.lock().unwrap();
        assert_eq!(m.emojis.len(), 5);
        assert_eq!(m.emojis.iter().filter(|e| e.animated).count(), 1);
        for e in &m.emojis {
            let info = check_file(ExpressionKind::Emoji, &e.bytes).unwrap();
            assert_eq!((info.width, info.height), (128, 128));
            assert_eq!(
                info.format,
                if e.animated { ImageFormat::Gif } else { ImageFormat::Png }
            );
            assert!(
                e.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "{}",
                e.name
            );
        }
        assert_eq!(m.stickers.len(), 5, "stickers untouched by an emoji publish");
    }
    // Both mappings are kept; the page remembers the last server and kind.
    assert_eq!(mapping(srv, &set.id, "sticker").await.len(), 2);
    assert_eq!(mapping(srv, &set.id, "emoji").await.len(), 5);
    let row = stickpile_server::service::publish::stored(&srv.state.db, &set.id, "discord")
        .await
        .unwrap();
    let values = stickpile_server::service::publish::platform("discord")
        .unwrap()
        .option_values(row.as_ref());
    assert_eq!(
        values,
        vec![("guild_id", GUILD.to_string()), ("kind", "emojis".to_string())]
    );
}

#[tokio::test]
async fn only_the_owner_publishes_and_servers_belong_to_one_user() {
    let dc = setup().await;
    let srv = &dc.srv;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let set = srv.create_set(&alice, "shared", Visibility::Public).await;
    srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
    let url = routes::path(routes::SET_PUBLISH, &[&set.id]);
    let body = json!({"platforms": ["discord"], "options": {"discord": {"guild_id": GUILD}}});
    // A reader may not publish (as for Telegram).
    srv.post(&bob, &url, body.clone()).await.expect(403);
    srv.get(&bob, &url).await.expect(403);
    // Bad options queue nothing.
    for (options, needle) in [
        (json!({"guild_id": "cats"}), "guild_id must be"),
        (json!({"guild_id": GUILD, "kind": "gifs"}), "kind must be"),
        (json!({"guild_id": 5}), "options.discord"),
    ] {
        let r = srv
            .post(
                &alice,
                &url,
                json!({"platforms": ["discord"], "options": {"discord": options}}),
            )
            .await
            .expect(400);
        assert!(r.text().contains(needle), "{}", r.text());
    }
    let j = publish(srv, &alice, &set.id, json!({"guild_id": GUILD})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");

    // Bob's own set may not go to the server Alice publishes stickers to,
    // but emoji are free.
    let bobs = srv.create_set(&bob, "bobs", Visibility::Private).await;
    srv.upload_one(&bob, &bobs.id, "b.png", png(2)).await;
    let r = srv
        .post(
            &bob,
            &routes::path(routes::SET_PUBLISH, &[&bobs.id]),
            json!({"platforms": ["discord"], "options": {"discord": {"guild_id": GUILD}}}),
        )
        .await
        .expect(400);
    assert!(r.text().contains("another stickpile user"), "{}", r.text());
    let j = publish(srv, &bob, &bobs.id, json!({"guild_id": GUILD, "kind": "emojis"})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");

    // A server the bot is not in fails the job with a clear reason.
    let j = publish(srv, &alice, &set.id, json!({"guild_id": "100000000000000999"})).await;
    assert_eq!(j.status, JobStatus::Failed);
    assert_eq!(j.attempts, 1);
    assert!(j.error.as_deref().unwrap().contains("Unknown Guild"), "{j:?}");
    // The binding stays with the server that worked.
    let p = publication(srv, &alice, &set.id).await;
    assert_eq!(p.target.as_deref(), Some("Cat Lounge"));
    assert_eq!(p.last_status, Some(JobStatus::Failed));
}

#[tokio::test]
async fn a_failed_publish_keeps_what_it_made() {
    let dc = setup().await;
    let srv = &dc.srv;
    let alice = srv.user("alice").await;
    let set = srv.create_set(&alice, "flaky", Visibility::Private).await;
    for i in 1..=3 {
        srv.upload_one(&alice, &set.id, &format!("cat {i}.png"), png(i)).await;
    }
    // The second upload is refused (403: missing permission, not retried).
    dc.mock
        .lock()
        .unwrap()
        .fail_after
        .insert("POST /stickers".into(), (403, 1));
    let j = publish(srv, &alice, &set.id, json!({"guild_id": GUILD})).await;
    assert_eq!(j.status, JobStatus::Failed, "{j:?}");
    assert_eq!(dc.mock.lock().unwrap().stickers.len(), 1);
    assert_eq!(
        mapping(srv, &set.id, "sticker").await.len(),
        1,
        "the partial mapping is stored"
    );
    // The next publish keeps that one and adds the rest: no copies.
    let j = publish(srv, &alice, &set.id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    assert_eq!(
        (r["added"].as_u64(), r["unchanged"].as_u64()),
        (Some(2), Some(1)),
        "{r}"
    );
    assert_eq!(dc.mock.lock().unwrap().stickers.len(), 3);
}

#[tokio::test]
async fn import_makes_a_set_and_links_what_the_bot_made() {
    let dc = setup().await;
    let srv = &dc.srv;
    let alice = srv.user("alice").await;
    let admin = srv.admin("root").await;
    let (human, bot_made) = {
        let mut m = dc.mock.lock().unwrap();
        // Boost level 1: 15 sticker slots.
        m.premium_tier = 1;
        let human = m.seed_sticker("Grumpy", "😾, grumpy, cat", 1, png(1), HUMAN_ID);
        let bot_made = m.seed_sticker("Happy", "😺", 1, png(2), BOT_ID);
        m.seed_sticker("Lottie", "🙂", 3, b"{}".to_vec(), HUMAN_ID);
        m.seed_sticker("Moving", "🐈", 4, gif(), HUMAN_ID);
        m.seed_emoji("party_cat", false, png(3), HUMAN_ID);
        m.seed_emoji("dance", true, common::gif_anim(9), BOT_ID);
        (human, bot_made)
    };

    // Stickers only, by a user who does not publish there: a plain copy.
    let j: Job = srv
        .post(
            &alice,
            routes::DISCORD_IMPORT,
            json!({"guild_id": GUILD, "visibility": "unlisted"}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(j.kind, JobKind::DiscordImport);
    assert_eq!(j.subject.as_deref(), Some(GUILD));
    drop_variant_jobs(srv).await;
    run_jobs_now(srv).await;
    let j = job(srv, &alice, &j.id).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    assert_eq!(r["imported"], 3, "{r}");
    assert_eq!(r["bound"], 0);
    assert_eq!(r["guild_name"], "Cat Lounge");
    assert!(r["skipped"][0]["reason"].as_str().unwrap().contains("Lottie"), "{r}");
    let d: SetDetail = srv
        .get(&alice, &routes::path(routes::SET, &[r["set_id"].as_str().unwrap()]))
        .await
        .expect(200)
        .json();
    assert_eq!((d.set.slug.as_str(), d.set.name.as_str()), ("cat-lounge", "Cat Lounge"));
    assert_eq!(d.set.visibility, Visibility::Unlisted);
    let names: Vec<&str> = d.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["Grumpy", "Happy", "Moving"]);
    assert_eq!(d.items[0].emojis, ["😾"]);
    assert_eq!(d.items[0].keywords, ["grumpy", "cat"]);
    assert_eq!(d.items[2].media.kind, stickpile_api::MediaKind::Animation);
    assert!(mapping(srv, &d.set.id, "sticker").await.is_empty());

    // An admin adopts what the bot made there: stickers and emoji, and a
    // publish back keeps them instead of uploading copies.
    let j: Job = srv
        .post(
            &admin,
            routes::DISCORD_IMPORT,
            json!({"guild_id": GUILD, "emojis": true, "slug": "lounge"}),
        )
        .await
        .expect(200)
        .json();
    drop_variant_jobs(srv).await;
    run_jobs_now(srv).await;
    let r = job(srv, &admin, &j.id).await.result.unwrap();
    assert_eq!((r["imported"].as_u64(), r["bound"].as_u64()), (Some(5), Some(2)), "{r}");
    let set_id = r["set_id"].as_str().unwrap().to_string();
    let d: SetDetail = srv
        .get(&admin, &routes::path(routes::SET, &[&set_id]))
        .await
        .expect(200)
        .json();
    assert_eq!(d.set.slug, "lounge");
    assert_eq!(d.items[3].name, "party cat", "emoji names get their spaces back");
    let stickers = mapping(srv, &set_id, "sticker").await;
    assert_eq!(stickers.len(), 1);
    assert_eq!(stickers.values().next().unwrap()[1], bot_made.as_str());
    assert_eq!(mapping(srv, &set_id, "emoji").await.len(), 1);

    let before = calls(&dc.mock, "POST /stickers");
    let j = publish(srv, &admin, &set_id, json!({})).await;
    assert_eq!(j.status, JobStatus::Done, "{j:?}");
    let r = j.result.unwrap();
    // Four copies (people's stickers and the emoji become the bot's
    // stickers), and the bot's own sticker is kept.
    assert_eq!(
        (r["added"].as_u64(), r["unchanged"].as_u64()),
        (Some(4), Some(1)),
        "{r}"
    );
    assert_eq!(calls(&dc.mock, "POST /stickers") - before, 4);
    let m = dc.mock.lock().unwrap();
    assert!(m.sticker(&bot_made).is_some() && m.sticker(&human).is_some());
}

#[tokio::test]
async fn import_needs_a_server_id_and_something_to_import() {
    let dc = setup().await;
    let srv = &dc.srv;
    let alice = srv.user("alice").await;
    srv.post(&alice, routes::DISCORD_IMPORT, json!({"guild_id": "nope"}))
        .await
        .expect(400);
    let r = srv
        .post(
            &alice,
            routes::DISCORD_IMPORT,
            json!({"guild_id": GUILD, "stickers": false, "emojis": false}),
        )
        .await
        .expect(400);
    assert!(r.text().contains("choose stickers"), "{}", r.text());
    srv.create_set(&alice, "taken", Visibility::Private).await;
    srv.post(
        &alice,
        routes::DISCORD_IMPORT,
        json!({"guild_id": GUILD, "slug": "taken"}),
    )
    .await
    .expect(409);
}

// ---------------------------------------------------------------------------
// The token stays secret
// ---------------------------------------------------------------------------

/// Everything any test of this binary logged.
fn logs() -> Arc<Mutex<Vec<u8>>> {
    static LOGS: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    LOGS.get_or_init(|| {
        let buf = Arc::new(Mutex::new(Vec::new()));
        struct W(Arc<Mutex<Vec<u8>>>);
        impl Write for W {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let sink = buf.clone();
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || W(sink.clone()))
            .try_init();
        buf
    })
    .clone()
}

#[tokio::test]
async fn the_token_is_never_in_errors_results_or_logs() {
    let logs = logs();
    let dc = setup().await;
    let srv = &dc.srv;
    let alice = srv.admin("alice").await;
    let set = srv.create_set(&alice, "secret", Visibility::Private).await;
    srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
    // A publish that works, one that fails, and an import that fails.
    let ok = publish(srv, &alice, &set.id, json!({"guild_id": GUILD})).await;
    assert_eq!(ok.status, JobStatus::Done, "{ok:?}");
    let failed = publish(srv, &alice, &set.id, json!({"guild_id": "100000000000000998"})).await;
    assert_eq!(failed.status, JobStatus::Failed);
    let import: Job = srv
        .post(
            &alice,
            routes::DISCORD_IMPORT,
            json!({"guild_id": "100000000000000997"}),
        )
        .await
        .expect(200)
        .json();
    run_jobs_now(srv).await;
    let import = job(srv, &alice, &import.id).await;
    assert_eq!(import.status, JobStatus::Failed);

    // A token file the server cannot use: the error names the file only.
    std::fs::write(dc._dir.path().join("discord-token"), "bad token\twith a tab\n").unwrap();
    let r = srv
        .post(
            &alice,
            &routes::path(routes::SET_PUBLISH, &[&set.id]),
            json!({"platforms": ["discord"]}),
        )
        .await
        .expect(400);
    assert!(r.text().contains("not configured"), "{}", r.text());

    let secret = TOKEN.split('.').nth(1).unwrap();
    let mut seen = Vec::new();
    for j in [&ok, &failed, &import] {
        seen.push(serde_json::to_string(j).unwrap());
    }
    seen.push(srv.get(&alice, "/api/v1/admin/jobs").await.text());
    seen.push(String::from_utf8_lossy(&logs.lock().unwrap()).into_owned());
    for text in &seen {
        assert!(
            !text.contains(TOKEN) && !text.contains(secret),
            "the token leaked: {text}"
        );
    }
    assert!(seen.last().unwrap().contains("job failed"), "the logs were captured");
    let m = dc.mock.lock().unwrap();
    assert!(m.auth_headers.iter().all(|h| h == &format!("Bot {TOKEN}")));
    assert!(!m.cdn_saw_auth);
}

#[tokio::test]
async fn the_import_page_queues_an_import() {
    use reqwest::StatusCode;
    use web_common::{assert_page, location, Browser};
    let dc = setup().await;
    let srv = &dc.srv;
    let alice = srv.user("alice").await;
    let b = Browser::login(srv, &alice).await;
    // Linked from the sets list and the upload page.
    assert!(b.get("/sets").await.text().contains("href=\"/discord/import\""));
    let r = b.get("/discord/import").await;
    assert_page(&r);
    let html = r.text();
    assert!(html.contains("name=\"guild_id\""));
    assert!(html.contains("name=\"stickers\" value=\"on\" checked"), "{html}");
    assert!(!html.contains("name=\"emojis\" value=\"on\" checked"), "{html}");
    // Errors keep what was typed.
    let r = b
        .post_form(
            "/discord/import",
            &[("sent", "1"), ("guild_id", "nope"), ("emojis", "on")],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let html = r.text();
    assert!(
        html.contains("guild_id must be") && html.contains("value=\"nope\""),
        "{html}"
    );
    assert!(html.contains("name=\"emojis\" value=\"on\" checked"), "{html}");
    assert!(!html.contains("name=\"stickers\" value=\"on\" checked"), "{html}");
    let r = b
        .post_form("/discord/import", &[("sent", "1"), ("guild_id", GUILD)])
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("choose stickers"), "{}", r.text());
    // A good one goes to the job's page.
    let r = b
        .post_form(
            "/discord/import",
            &[
                ("sent", "1"),
                ("guild_id", GUILD),
                ("emojis", "on"),
                ("visibility", "public"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let to = location(&r);
    let id = to.strip_prefix("/jobs/").unwrap();
    let payload: String = sqlx::query_scalar("SELECT payload FROM jobs WHERE id = ?")
        .bind(id)
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    let p: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(
        (p["stickers"].as_bool(), p["emojis"].as_bool()),
        (Some(false), Some(true))
    );
    assert_eq!(p["visibility"], "public");
    let r = b.get(&to).await;
    assert_page(&r);
    assert!(r.text().contains("Discord import"));
}
