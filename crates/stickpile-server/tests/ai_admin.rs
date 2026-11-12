//! AI enrichment (MockEnricher), AI suggestions before an upload, semantic
//! search, AI-less startup, admin storage and job listings.

mod common;
mod web_common;

use common::{png, read, solid_png, Opts, Resp, TestServer, TestUser};
use serde_json::json;
use stickpile_api::{
    routes, AiSuggestions, FieldSource, Item, ItemDetail, Job, JobKind, JobStatus, SetDetail, StorageUsage,
    UploadResult, Visibility,
};
use web_common::Browser;

const VOCAB: &str = r#"
keywords = ["red", "blue", "green"]

[[emojis]]
emoji = "🔴"
description = "red"

[[emojis]]
emoji = "🔵"
description = "blue"

[[vibes]]
label = "angry"
synonyms = ["red", "furious", "grr"]

[[vibes]]
label = "sad"
synonyms = ["blue", "sob"]
"#;

async fn ai_server() -> TestServer {
    ai_server_with(|_| {}).await
}

async fn ai_server_with(f: impl FnOnce(&mut stickpile_server::Config)) -> TestServer {
    let dir = tempfile::tempdir().unwrap();
    let vocab = dir.path().join("vocab.toml");
    std::fs::write(&vocab, VOCAB).unwrap();
    let v = vocab.clone();
    let srv = TestServer::start_with(
        Opts {
            ai: true,
            ..Default::default()
        },
        move |c| {
            c.ai.vocabulary_file = Some(v);
            f(c);
        },
    )
    .await;
    // Keep the vocabulary file alive for the server's lifetime (it is read at start only).
    std::mem::forget(dir);
    srv
}

fn image_form(bytes: Vec<u8>) -> reqwest::multipart::Form {
    reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(bytes).file_name("suggest.png"))
}

/// `POST /api/v1/ai/suggest` with a bearer token.
async fn suggest(srv: &TestServer, u: &TestUser, bytes: Vec<u8>) -> Resp {
    read(
        srv.client
            .post(srv.url(routes::AI_SUGGEST))
            .bearer_auth(&u.token)
            .multipart(image_form(bytes))
            .send()
            .await
            .unwrap(),
    )
    .await
}

async fn count(srv: &TestServer, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(&srv.state.db).await.unwrap()
}

#[tokio::test]
async fn suggestions_before_upload() {
    let srv = ai_server().await;
    let u = srv.user("alice").await;
    let red = solid_png([230, 20, 20], 1);

    // Chosen like enrichment chooses them, and nothing is stored.
    let s: AiSuggestions = suggest(&srv, &u, red.clone()).await.expect(200).json();
    assert!(s.keywords.contains(&"red".to_string()), "{s:?}");
    assert!(s.emojis.contains(&"🔴".to_string()), "{s:?}");
    assert_eq!(s.vibe.as_deref(), Some("angry"));
    let blue: AiSuggestions = suggest(&srv, &u, solid_png([30, 70, 220], 2)).await.expect(200).json();
    assert_eq!(blue.vibe.as_deref(), Some("sad"));
    assert_eq!(count(&srv, "SELECT COUNT(*) FROM media").await, 0);
    assert_eq!(count(&srv, "SELECT COUNT(*) FROM jobs").await, 0);
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let item = srv.upload_one(&u, &set.id, "red.png", red).await;
    srv.run_jobs().await;
    let item: Item = srv
        .get(&u, &routes::path(routes::ITEM, &[&item.id]))
        .await
        .expect(200)
        .json::<ItemDetail>()
        .item;
    assert_eq!(
        (item.keywords, item.emojis, item.vibe),
        (s.keywords, s.emojis, s.vibe),
        "the enrichment of the same picture"
    );

    // Animations give their first frame.
    suggest(&srv, &u, common::gif_anim(3)).await.expect(200);

    // Bad input.
    let r = suggest(&srv, &u, b"not an image".to_vec()).await.expect(415);
    assert_eq!(r.code(), "unsupported_media");
    let r = suggest(&srv, &u, vec![0; 2 * 1024 * 1024 + 1]).await.expect(413);
    assert_eq!(r.code(), "too_large");
    let r = suggest(&srv, &u, vec![0; 3 * 1024 * 1024]).await.expect(413);
    assert_eq!(r.code(), "too_large");
    let no_file = reqwest::multipart::Form::new().text("note", "no file");
    let r = read(
        srv.client
            .post(srv.url(routes::AI_SUGGEST))
            .bearer_auth(&u.token)
            .multipart(no_file)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(400);
    assert_eq!(r.code(), "bad_request");
}

#[tokio::test]
async fn suggestions_need_auth_and_csrf() {
    let srv = ai_server().await;
    let u = srv.user("alice").await;
    let post = |client: &reqwest::Client| {
        client
            .post(srv.url(routes::AI_SUGGEST))
            .multipart(image_form(solid_png([230, 20, 20], 1)))
    };
    let anon = read(post(&srv.client).send().await.unwrap()).await.expect(401);
    assert_eq!(anon.code(), "unauthorized");

    // A web session (the upload tray): the CSRF header and the same origin.
    let b = Browser::login(&srv, &u).await;
    let token = b.csrf.clone().unwrap();
    let r = read(post(&b.client).send().await.unwrap()).await.expect(403);
    assert_eq!(r.code(), "csrf");
    let r = read(post(&b.client).header("X-CSRF-Token", "wrong").send().await.unwrap())
        .await
        .expect(403);
    assert_eq!(r.code(), "csrf");
    let r = read(
        post(&b.client)
            .header("X-CSRF-Token", &token)
            .header("Sec-Fetch-Site", "cross-site")
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(403);
    assert_eq!(r.code(), "csrf");
    let s: AiSuggestions = read(
        post(&b.client)
            .header("X-CSRF-Token", &token)
            .header("Origin", srv.base.clone())
            .header("Sec-Fetch-Site", "same-origin")
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200)
    .json();
    assert!(s.keywords.contains(&"red".to_string()));

    // The upload forms tell the tray where to ask.
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    for page in ["/upload".to_string(), format!("/sets/{}", set.id)] {
        let html = b.get(&page).await.expect(200).text();
        assert!(
            html.contains(r#"data-ai="ready" data-ai-suggest="/api/v1/ai/suggest""#),
            "{page}"
        );
    }
}

#[tokio::test]
async fn suggestions_are_rate_limited() {
    let srv = ai_server_with(|c| c.limits.ai_suggest_per_minute = 2).await;
    let u = srv.user("alice").await;
    for seed in 0..2 {
        suggest(&srv, &u, solid_png([230, 20, 20], seed)).await.expect(200);
    }
    let r = suggest(&srv, &u, solid_png([230, 20, 20], 9)).await.expect(429);
    assert_eq!(r.code(), "rate_limited");
    assert!(r.header("retry-after").is_some());
    // Per user, and apart from uploads.
    let bob = srv.user("bob").await;
    suggest(&srv, &bob, solid_png([230, 20, 20], 9)).await.expect(200);
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    srv.upload_one(&u, &set.id, "a.png", png(1)).await;
}

#[tokio::test]
async fn suggestions_without_ai() {
    use stickpile_server::state::AiStatus;
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let r = suggest(&srv, &u, solid_png([230, 20, 20], 1)).await.expect(503);
    assert_eq!(r.code(), "unavailable");
    assert!(
        r.value()["message"].as_str().unwrap().contains("turned off"),
        "{}",
        r.text()
    );
    assert!(r.header("retry-after").is_none(), "off is not \"later\"");
    // No URL, so the tray does not ask; it says AI is off (data-ai).
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let item = srv.upload_one(&u, &set.id, "a.png", png(1)).await;
    let b = Browser::login(&srv, &u).await;
    for page in ["/upload".to_string(), format!("/sets/{}", set.id)] {
        let html = b.get(&page).await.expect(200).text();
        assert!(html.contains("data-dropzone"), "{page}");
        assert!(html.contains("data-ai=\"off\""), "{page}");
        assert!(!html.contains("data-ai-suggest"), "{page}");
    }
    // While the model loads: 503 `ai_loading` with a short Retry-After (the
    // tray asks again then), for suggestions and re-running enrichment.
    srv.state.set_ai_status(AiStatus::Loading);
    let r = suggest(&srv, &u, solid_png([230, 20, 20], 1)).await.expect(503);
    assert_eq!(r.code(), "ai_loading");
    assert_eq!(r.header("retry-after").as_deref(), Some("2"));
    let r = srv
        .post(&u, &routes::path(routes::ITEM_ENRICH, &[&item.id]), json!({}))
        .await
        .expect(503);
    assert_eq!(r.code(), "ai_loading");
    assert_eq!(r.header("retry-after").as_deref(), Some("2"));
    let html = b.get("/upload").await.expect(200).text();
    assert!(
        html.contains("data-ai=\"loading\" data-ai-suggest=\"/api/v1/ai/suggest\""),
        "{html}"
    );
    // The model failed to load: "not here", like off, with its own message.
    srv.state.set_ai_status(AiStatus::Failed);
    let r = suggest(&srv, &u, solid_png([230, 20, 20], 1)).await.expect(503);
    assert_eq!(r.code(), "unavailable");
    assert!(r.value()["message"].as_str().unwrap().contains("could not be loaded"));
    assert!(r.header("retry-after").is_none());
    let html = b.get("/upload").await.expect(200).text();
    assert!(html.contains("data-ai=\"failed\"") && !html.contains("data-ai-suggest"));
}

#[tokio::test]
async fn upload_keeps_unapproved_suggestions_marked_ai() {
    let srv = ai_server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let res: Vec<UploadResult> = srv
        .upload(
            &u,
            &set.id,
            vec![
                ("pending.png", solid_png([230, 20, 20], 1)),
                ("approved.png", solid_png([225, 25, 25], 2)),
                ("old.png", solid_png([220, 30, 30], 3)),
            ],
            Some(json!([
                // Keywords and vibe left unapproved, emojis approved.
                {"keywords": ["tray kw"], "emojis": ["🐱"], "vibe": "smug", "ai_fields": ["keywords", "vibe"]},
                // Everything approved or typed.
                {"keywords": ["mine"], "emojis": ["🐶"], "vibe": "happy", "tags": ["t"]},
                // A field without a value has nothing to mark.
                {"keywords": ["x"], "ai_fields": ["emojis"]},
            ])),
        )
        .await
        .expect(200)
        .json();
    let pending = res[0].item.clone().unwrap();
    let approved = res[1].item.clone().unwrap();
    let old = res[2].item.clone().unwrap();
    // (keywords, emojis, vibe)
    let sources = |i: &Item| (i.keywords_source, i.emojis_source, i.vibe_source);
    const ALL_USER: (FieldSource, FieldSource, FieldSource) = (FieldSource::User, FieldSource::User, FieldSource::User);
    const AI_USER_AI: (FieldSource, FieldSource, FieldSource) = (FieldSource::Ai, FieldSource::User, FieldSource::Ai);
    assert_eq!(sources(&pending), AI_USER_AI);
    assert_eq!(pending.keywords, vec!["tray kw"]);
    assert_eq!(pending.vibe.as_deref(), Some("smug"));
    assert_eq!(sources(&approved), ALL_USER);
    assert_eq!(sources(&old), ALL_USER);
    // The set shows the same.
    let d: SetDetail = srv
        .get(&u, &routes::path(routes::SET, &[&set.id]))
        .await
        .expect(200)
        .json();
    assert_eq!(sources(&d.items[0]), AI_USER_AI);
    assert_eq!(sources(&d.items[1]), ALL_USER);

    // Names and tags cannot be AI suggestions; unknown fields are refused.
    for bad in ["tags", "name", "bogus"] {
        let r = srv
            .upload(
                &u,
                &set.id,
                vec![("bad.png", png(4))],
                Some(json!([{"tags": ["t"], "name": "n", "ai_fields": [bad]}])),
            )
            .await
            .expect(400);
        assert_eq!(r.code(), "bad_request", "{bad}");
    }

    // Enrichment afterwards: unapproved values are AI values like any other
    // and get the stored picture's suggestions; approved ones stay.
    srv.run_jobs().await;
    let get = |id: String| {
        let srv = &srv;
        let u = &u;
        async move {
            srv.get(u, &routes::path(routes::ITEM, &[&id]))
                .await
                .expect(200)
                .json::<ItemDetail>()
                .item
        }
    };
    let p = get(pending.id.clone()).await;
    assert!(p.keywords.contains(&"red".to_string()), "{:?}", p.keywords);
    assert_eq!(p.emojis, vec!["🐱"]);
    assert_eq!(p.vibe.as_deref(), Some("angry"));
    assert_eq!(sources(&p), AI_USER_AI);
    let a = get(approved.id.clone()).await;
    assert_eq!(
        (a.keywords, a.emojis, a.vibe, a.tags),
        (
            vec!["mine".to_string()],
            vec!["🐶".to_string()],
            Some("happy".to_string()),
            vec!["t".to_string()]
        )
    );
    assert_eq!(sources(&get(approved.id.clone()).await), ALL_USER);
    let o = get(old.id.clone()).await;
    assert_eq!(o.emojis_source, FieldSource::Ai, "empty fields are filled as before");
    // The job for the approved picture stored its embedding and changed no item.
    let admin = srv.admin("root").await;
    let jobs: Vec<Job> = srv.get(&admin, routes::ADMIN_JOBS).await.expect(200).json();
    let job = jobs
        .iter()
        .find(|j| j.kind == JobKind::Enrich && j.subject.as_deref() == Some(approved.media.id.as_str()))
        .expect("enrich job of the approved picture");
    assert_eq!(job.status, JobStatus::Done, "{:?}", job.error);
    assert_eq!(job.result.as_ref().unwrap()["items_updated"], 0);
}

#[tokio::test]
async fn enrichment_fills_only_empty_or_ai_fields() {
    let srv = ai_server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let res: Vec<UploadResult> = srv
        .upload(
            &u,
            &set.id,
            vec![
                ("plain.png", solid_png([230, 20, 20], 1)),
                ("mine.png", solid_png([225, 25, 25], 2)),
            ],
            Some(json!([{}, {"keywords": ["my own"]}])),
        )
        .await
        .expect(200)
        .json();
    let plain = res[0].item.clone().unwrap();
    let mine = res[1].item.clone().unwrap();
    assert!(plain.keywords.is_empty());

    let jobs: Vec<Job> = {
        let admin = srv.admin("root").await;
        srv.get(&admin, &format!("{}?status=queued", routes::ADMIN_JOBS))
            .await
            .expect(200)
            .json()
    };
    assert!(jobs.iter().any(|j| j.kind == JobKind::Enrich));
    assert!(jobs.iter().any(|j| j.kind == JobKind::Variants));
    srv.run_jobs().await;

    let get = |id: String| {
        let srv = &srv;
        let u = &u;
        async move {
            srv.get(u, &routes::path(routes::ITEM, &[&id]))
                .await
                .expect(200)
                .json::<ItemDetail>()
                .item
        }
    };
    let p: Item = get(plain.id.clone()).await;
    assert!(p.keywords.contains(&"red".to_string()), "{:?}", p.keywords);
    assert_eq!(p.keywords_source, FieldSource::Ai);
    assert!(p.emojis.contains(&"🔴".to_string()), "{:?}", p.emojis);
    assert_eq!(p.emojis_source, FieldSource::Ai);
    assert_eq!(p.vibe.as_deref(), Some("angry"));
    assert_eq!(p.vibe_source, FieldSource::Ai);
    assert_eq!(p.name, "plain", "names are never AI-filled");
    let m: Item = get(mine.id.clone()).await;
    assert_eq!(m.keywords, vec!["my own"], "user keywords are not overwritten");
    assert_eq!(m.keywords_source, FieldSource::User);
    assert_eq!(m.emojis_source, FieldSource::Ai, "empty fields are filled");

    // A confirmed (user-sourced) value survives re-enrichment.
    srv.patch(
        &u,
        &routes::path(routes::ITEM, &[&plain.id]),
        json!({"confirm": ["vibe"], "keywords": ["edited"]}),
    )
    .await
    .expect(200);
    let job: Job = srv
        .post(&u, &routes::path(routes::ITEM_ENRICH, &[&plain.id]), json!({}))
        .await
        .expect(200)
        .json();
    assert_eq!(job.kind, JobKind::Enrich);
    srv.run_jobs().await;
    let job: Job = srv
        .get(&u, &routes::path(routes::JOB, &[&job.id]))
        .await
        .expect(200)
        .json();
    assert_eq!(job.status, JobStatus::Done, "{:?}", job.error);
    let p: Item = get(plain.id.clone()).await;
    assert_eq!(p.keywords, vec!["edited"]);
    assert_eq!(p.vibe_source, FieldSource::User);
    assert_eq!(p.vibe.as_deref(), Some("angry"));

    // The same bytes uploaded again reuse the stored suggestions at once.
    let set2 = srv.create_set(&u, "s2", Visibility::Private).await;
    let again = srv
        .upload_one(&u, &set2.id, "again.png", solid_png([230, 20, 20], 1))
        .await;
    assert!(again.keywords.contains(&"red".to_string()));
    assert_eq!(again.keywords_source, FieldSource::Ai);

    // Export keeps the AI markers.
    let d: SetDetail = srv
        .get(&u, &routes::path(routes::SET, &[&set2.id]))
        .await
        .expect(200)
        .json();
    assert_eq!(d.items[0].emojis_source, FieldSource::Ai);
}

#[tokio::test]
async fn semantic_search_and_query_expansion() {
    let srv = ai_server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let red = srv
        .upload_one(&u, &set.id, "thing one.png", solid_png([230, 20, 20], 1))
        .await;
    let blue = srv
        .upload_one(&u, &set.id, "thing two.png", solid_png([30, 70, 220], 2))
        .await;
    srv.run_jobs().await;

    // "red" matches the red picture through the image embedding / AI keywords.
    let r = srv
        .get(&u, &format!("{}?q=red", routes::SEARCH))
        .await
        .expect(200)
        .value();
    assert_eq!(r["semantic"], true);
    assert_eq!(r["hits"][0]["item"]["id"], red.id.as_str(), "{r}");
    // Vibe synonyms expand queries without the model: "grr" -> angry.
    let r = srv
        .get(&u, &format!("{}?q=grr", routes::SEARCH))
        .await
        .expect(200)
        .value();
    let ids: Vec<&str> = r["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["item"]["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&red.id.as_str()), "{r}");
    assert!(!ids.contains(&blue.id.as_str()) || ids[0] == red.id, "{r}");
    // Filters still apply.
    let r = srv
        .get(&u, &format!("{}?q=red vibe:sad", routes::SEARCH))
        .await
        .expect(200)
        .value();
    let ids: Vec<&str> = r["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["item"]["id"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&red.id.as_str()));
    assert_eq!(r["query"], "red vibe:sad");
    // Limits are capped.
    let r = srv
        .get(&u, &format!("{}?q=&limit=100000", routes::SEARCH))
        .await
        .expect(200)
        .value();
    assert!(r["hits"].as_array().unwrap().len() <= 200);
}

#[tokio::test]
async fn startup_without_loadable_model_disables_ai() {
    let srv = TestServer::start_with(
        Opts {
            ai_from_config: true,
            ..Default::default()
        },
        |c| {
            c.ai.enabled = true;
            c.ai.model_dir = "/nonexistent/models".into();
        },
    )
    .await;
    // The model loads in the background; the server serves meanwhile.
    for _ in 0..200 {
        if srv.state.ai_status() != stickpile_server::state::AiStatus::Loading {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(srv.state.ai_status(), stickpile_server::state::AiStatus::Failed);
    assert!(srv.state.ai().is_none());
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let item = srv.upload_one(&u, &set.id, "a.png", png(1)).await;
    let r = srv
        .get(&u, &format!("{}?q=anything", routes::SEARCH))
        .await
        .expect(200)
        .value();
    assert_eq!(r["semantic"], false);
    let r = srv
        .post(&u, &routes::path(routes::ITEM_ENRICH, &[&item.id]), json!({}))
        .await
        .expect(503);
    assert_eq!(r.code(), "unavailable");
    // No Enrich job was queued for the upload.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'enrich'")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn storage_and_job_listing() {
    let srv = TestServer::start().await;
    let admin = srv.admin("root").await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    srv.upload_one(&u, &set.id, "a.png", png(1)).await;
    srv.upload_one(&u, &set.id, "b.png", png(2)).await;
    let s: StorageUsage = srv.get(&admin, routes::ADMIN_STORAGE).await.expect(200).json();
    assert_eq!(s.media_count, 2);
    assert_eq!(s.item_count, 2);
    assert!(s.media_bytes > 0);
    assert!(s.database_bytes > 0);
    let alice = s.per_user.iter().find(|(n, _)| n == "alice").unwrap();
    assert!(alice.1 > 0);
    assert!(alice.1 <= s.media_bytes);
    let users: Vec<stickpile_api::AdminUser> = srv.get(&admin, routes::ADMIN_USERS).await.expect(200).json();
    let a = users.iter().find(|x| x.user.username == "alice").unwrap();
    assert_eq!((a.sets, a.items), (1, 2));

    let all: Vec<Job> = srv.get(&admin, routes::ADMIN_JOBS).await.expect(200).json();
    assert_eq!(all.len(), 2);
    srv.run_jobs().await;
    let done: Vec<Job> = srv
        .get(&admin, &format!("{}?status=done", routes::ADMIN_JOBS))
        .await
        .expect(200)
        .json();
    assert_eq!(done.len(), 2);
    let queued: Vec<Job> = srv
        .get(&admin, &format!("{}?status=queued", routes::ADMIN_JOBS))
        .await
        .expect(200)
        .json();
    assert!(queued.is_empty());
    srv.get(&admin, &format!("{}?status=bogus", routes::ADMIN_JOBS))
        .await
        .expect(400);
    // Admins can read any job, owners theirs.
    srv.get(&admin, &routes::path(routes::JOB, &[&done[0].id]))
        .await
        .expect(200);
    srv.get(&u, &routes::path(routes::JOB, &[&done[0].id]))
        .await
        .expect(200);
}

#[tokio::test]
async fn integration_jobs_without_a_subject() {
    let srv = TestServer::start().await;
    // The set is gone (deleted meanwhile): nothing to do.
    let gone = stickpile_server::jobs::enqueue(&srv.state, JobKind::GitPull, json!({"set_id": "x"}), None, None)
        .await
        .unwrap();
    // A malformed payload fails permanently.
    let bad = stickpile_server::jobs::enqueue(&srv.state, JobKind::GitPush, json!({}), None, None)
        .await
        .unwrap();
    srv.run_jobs().await;
    let get = |id: String| {
        let db = srv.state.db.clone();
        async move { stickpile_server::jobs::get(&db, &id).await.unwrap().unwrap().to_api() }
    };
    let row = get(gone.id).await;
    assert_eq!(row.status, JobStatus::Done);
    assert!(row.result.unwrap()["skipped"]
        .as_str()
        .unwrap()
        .contains("no longer exists"));
    let row = get(bad.id).await;
    assert_eq!(row.status, JobStatus::Failed);
    assert_eq!(row.attempts, 1);
    assert!(row.error.unwrap().contains("set_id"));
}

#[tokio::test]
async fn transient_job_failures_retry_with_backoff() {
    let srv = TestServer::start().await;
    // A Variants job whose original file vanished: retried, then failed.
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let item = srv.upload_one(&u, &set.id, "a.png", png(1)).await;
    let dir = srv
        .state
        .config
        .media_dir()
        .join(&item.media.sha256[..2])
        .join(&item.media.sha256);
    std::fs::remove_file(dir.join("original.png")).unwrap();
    srv.run_jobs().await;
    let (status, attempts, run_after): (String, i64, i64) =
        sqlx::query_as("SELECT status, attempts, run_after FROM jobs WHERE kind = 'variants'")
            .fetch_one(&srv.state.db)
            .await
            .unwrap();
    assert_eq!((status.as_str(), attempts), ("queued", 1));
    assert!(run_after > stickpile_server::ids::now());
    for _ in 0..2 {
        sqlx::query("UPDATE jobs SET run_after = 0")
            .execute(&srv.state.db)
            .await
            .unwrap();
        srv.run_jobs().await;
    }
    let (status, attempts): (String, i64) = sqlx::query_as("SELECT status, attempts FROM jobs WHERE kind = 'variants'")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!((status.as_str(), attempts), ("failed", 3));
}

#[tokio::test]
async fn ai_becoming_available_backfills_enrichment() {
    // Upload while AI is off ...
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let item = srv.upload_one(&u, &set.id, "a.png", solid_png([30, 70, 220], 1)).await;
    let enrich: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'enrich'")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(enrich, 0);
    // ... then start with AI on the same data: missing embeddings are queued.
    let mock: std::sync::Arc<dyn stickpile_ai::Enricher> = std::sync::Arc::new(stickpile_ai::MockEnricher::default());
    let state = stickpile_server::build_state(
        srv.state.config.clone(),
        stickpile_server::StartOptions {
            enricher: stickpile_server::EnricherSource::Provided(mock),
        },
    )
    .await
    .unwrap();
    assert_eq!(state.ai_status(), stickpile_server::state::AiStatus::Ready);
    let enrich: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'enrich' AND status = 'queued'")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(enrich, 1);
    stickpile_server::jobs::run_pending(&state).await.unwrap();
    let model: Option<String> = sqlx::query_scalar("SELECT embedding_model FROM media WHERE id = ?")
        .bind(&item.media.id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(model.as_deref(), Some("mock-64"));
    // Nothing left to backfill.
    assert_eq!(
        stickpile_server::jobs::media::enqueue_missing_enrichment(&state)
            .await
            .unwrap(),
        0
    );
}

fn bin() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_stickpile-server"))
}

#[tokio::test]
async fn cli_check_config_and_create_user() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, format!("[server]\ndata_dir = \"{}\"\n", data.display())).unwrap();
    let pw = dir.path().join("pw");
    std::fs::write(&pw, "password-cli\n").unwrap();

    let out = bin().arg("--config").arg(&cfg).arg("check-config").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("configuration OK"));
    let bad = dir.path().join("bad.toml");
    std::fs::write(&bad, "[server]\nlisten_addr = 1\n").unwrap();
    assert!(!bin()
        .arg("--config")
        .arg(&bad)
        .arg("check-config")
        .output()
        .unwrap()
        .status
        .success());

    let create = |args: &[&str]| {
        bin()
            .arg("--config")
            .arg(&cfg)
            .arg("create-user")
            .args(args)
            .output()
            .unwrap()
    };
    let out = create(&[
        "carl",
        "--admin",
        "--password-file",
        pw.to_str().unwrap(),
        "--tailscale-login",
        "carl@gh",
    ]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        !create(&["carl", "--password-file", pw.to_str().unwrap()])
            .status
            .success(),
        "duplicate"
    );
    assert!(!create(&["nologin"]).status.success(), "needs a way to log in");

    let config = stickpile_server::Config::load(&cfg).unwrap();
    let state = stickpile_server::build_state(config, Default::default()).await.unwrap();
    let u = stickpile_server::db::user_by_username(&state.db, "carl")
        .await
        .unwrap()
        .unwrap();
    assert!(u.is_admin());
    assert_eq!(u.tailscale_login.as_deref(), Some("carl@gh"));
    let logged_in = stickpile_server::auth::login::password_login(
        &state,
        "carl",
        "password-cli",
        None,
        None,
        stickpile_server::auth::login::LoginPurpose::WebSession,
    )
    .await
    .unwrap();
    assert_eq!(logged_in.id, u.id);
}

#[tokio::test]
async fn bootstrap_admin_from_config() {
    let dir = tempfile::tempdir().unwrap();
    let pw = dir.path().join("admin-pw");
    std::fs::write(&pw, "bootstrap-pass\n").unwrap();
    let p = pw.clone();
    let srv = TestServer::with_config(move |c| {
        c.auth.initial_admin_username = Some("root".into());
        c.auth.initial_admin_password_file = Some(p);
    })
    .await;
    let tok: stickpile_api::TokenCreated = srv
        .send(
            None,
            reqwest::Method::POST,
            routes::AUTH_TOKEN,
            Some(json!({"username": "root", "password": "bootstrap-pass", "token_name": "x"})),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(tok.user.role, stickpile_api::Role::Admin);
    // A restart does not create it again (or reset its password).
    let again = stickpile_server::build_state(srv.state.config.clone(), Default::default())
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&again.db)
        .await
        .unwrap();
    assert_eq!(n, 1);
    // A missing password file is a startup error.
    let mut cfg = srv.state.config.clone();
    cfg.server.data_dir = dir.path().join("other");
    cfg.auth.initial_admin_password_file = Some(dir.path().join("missing"));
    assert!(stickpile_server::build_state(cfg, Default::default()).await.is_err());
}
