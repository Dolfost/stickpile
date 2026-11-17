//! Rechecks: batches that recompute keywords, emojis, vibe, text,
//! embeddings, quality and renditions of many items (MockEnricher,
//! MockTextReader). The batch's life (counters, dedup, cancel, restart),
//! `missing` against `refresh` per part, the rule that a person's value is
//! never replaced (each write path), the scopes (user, set, server), the
//! queue's fairness, the audit log and rate limit, and the server's own
//! batches at startup.

mod common;

use std::sync::Arc;

use common::{png, solid_png, Opts, Resp, TestServer, TestUser};
use serde_json::{json, Value};
use stickpile_ai::MockTextReader;
use stickpile_api::{
    routes, AuditEntry, FieldSource, Item, ItemDetail, Recheck, RecheckFailure, RecheckMode, RecheckPart, RecheckScope,
    RecheckState, StartRecheck, Visibility,
};
use stickpile_server::jobs::{self, Lane};
use stickpile_server::{AppState, EnricherSource, StartOptions, TextReaderSource};

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

/// What the mock reader reads in every frame, as the server stores it.
const READ: &str = "NO THOUGHTS\nhead empty";

fn reader() -> Arc<MockTextReader> {
    Arc::new(MockTextReader::new(&["NO THOUGHTS", "head empty"]))
}

/// A server with the mock CLIP enricher, the mock text reader and a small
/// vocabulary; jobs run when the test says so.
async fn server_with(f: impl FnOnce(&mut stickpile_server::Config)) -> (TestServer, Arc<MockTextReader>) {
    let dir = tempfile::tempdir().unwrap();
    let vocab = dir.path().join("vocab.toml");
    std::fs::write(&vocab, VOCAB).unwrap();
    let mock = reader();
    let srv = TestServer::start_with(
        Opts {
            ai: true,
            text_reader: Some(mock.clone()),
            ..Default::default()
        },
        move |c| {
            c.ai.vocabulary_file = Some(vocab);
            f(c);
        },
    )
    .await;
    // The vocabulary is read again by the restarts below.
    std::mem::forget(dir);
    (srv, mock)
}

async fn server() -> (TestServer, Arc<MockTextReader>) {
    server_with(|_| {}).await
}

const RED: [u8; 3] = [230, 20, 20];
const BLUE: [u8; 3] = [30, 70, 220];

async fn start(srv: &TestServer, u: &TestUser, body: Value) -> Resp {
    srv.post(u, routes::RECHECKS, body).await
}

async fn started(srv: &TestServer, u: &TestUser, body: Value) -> Recheck {
    start(srv, u, body).await.expect(201).json()
}

async fn recheck(srv: &TestServer, u: &TestUser, id: &str) -> Recheck {
    srv.get(u, &routes::path(routes::RECHECK, &[id]))
        .await
        .expect(200)
        .json()
}

async fn item(srv: &TestServer, u: &TestUser, id: &str) -> Item {
    srv.get(u, &routes::path(routes::ITEM, &[id]))
        .await
        .expect(200)
        .json::<ItemDetail>()
        .item
}

async fn upload(srv: &TestServer, u: &TestUser, set_id: &str, name: &str, bytes: Vec<u8>, meta: Option<Value>) -> Item {
    let res: Vec<stickpile_api::UploadResult> = srv
        .upload(u, set_id, vec![(name, bytes)], meta.map(|m| json!([m])))
        .await
        .expect(200)
        .json();
    res.into_iter().next().unwrap().item.expect("item")
}

async fn sql(srv: &TestServer, q: &str, binds: &[&str]) {
    let mut query = sqlx::query(q);
    for b in binds {
        query = query.bind(*b);
    }
    query.execute(&srv.state.db).await.unwrap();
}

async fn scalar(db: &sqlx::SqlitePool, q: &str) -> i64 {
    sqlx::query_scalar(q).fetch_one(db).await.unwrap()
}

/// (keywords, emojis, vibe, text) with their sources.
type Fields = (
    (Vec<String>, FieldSource),
    (Vec<String>, FieldSource),
    (Option<String>, FieldSource),
    (String, FieldSource),
);

fn fields(i: &Item) -> Fields {
    (
        (i.keywords.clone(), i.keywords_source),
        (i.emojis.clone(), i.emojis_source),
        (i.vibe.clone(), i.vibe_source),
        (i.text.clone(), i.text_source),
    )
}

/// The same data opened again, as a restart does (the HTTP server keeps
/// the first state; both share the database).
async fn restart(srv: &TestServer, enricher: EnricherSource, text_reader: TextReaderSource) -> AppState {
    stickpile_server::build_state(srv.state.config.clone(), StartOptions { enricher, text_reader })
        .await
        .unwrap()
}

async fn viewer(state: &AppState, username: &str) -> stickpile_server::auth::CurrentUser {
    let row = stickpile_server::db::user_by_username(&state.db, username)
        .await
        .unwrap()
        .unwrap();
    stickpile_server::auth::CurrentUser::background(row)
}

// ---------------------------------------------------------------------------
// The batch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_batch_counts_its_pictures_and_ends() {
    let (srv, _) = server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let mut items = Vec::new();
    for seed in 1..=3 {
        items.push(upload(&srv, &u, &set.id, "a.png", png(seed), None).await);
    }
    srv.run_jobs().await;
    // One picture lost its quality report.
    sql(
        &srv,
        "UPDATE media SET quality = NULL, quality_report = NULL WHERE id = ?",
        &[&items[0].media.id],
    )
    .await;

    let r = started(&srv, &u, json!({"parts": ["quality"], "mode": "refresh"})).await;
    assert_eq!(
        (r.scope, r.state, r.mode, r.parts.as_slice()),
        (
            RecheckScope::User,
            RecheckState::Running,
            RecheckMode::Refresh,
            &[RecheckPart::Quality][..]
        )
    );
    assert_eq!((r.total, r.done, r.failed, r.changed), (3, 0, 0, 0));
    assert_eq!(r.requested_by.as_deref(), Some("alice"));
    assert_eq!(r.finished_at, None);
    // One job per picture, below everything else.
    let jobs: Vec<(String, i64, Option<String>)> =
        sqlx::query_as("SELECT kind, priority, owner_id FROM jobs WHERE kind = 'recheck'")
            .fetch_all(&srv.state.db)
            .await
            .unwrap();
    assert_eq!(jobs.len(), 3);
    assert!(jobs
        .iter()
        .all(|(_, p, o)| *p < 0 && o.as_deref() == Some(u.id.as_str())));

    let ran = jobs::run_next(&srv.state, Lane::All).await.unwrap().unwrap();
    assert_eq!(ran.kind, "recheck");
    let r = recheck(&srv, &u, &r.id).await;
    assert_eq!((r.state, r.done, r.failed), (RecheckState::Running, 1, 0));
    srv.run_jobs().await;
    let r = recheck(&srv, &u, &r.id).await;
    assert_eq!((r.state, r.total, r.done, r.failed), (RecheckState::Done, 3, 3, 0));
    assert_eq!(r.changed, 1, "only the picture without a report got a new one");
    assert!(r.finished_at.is_some());
    let item = item(&srv, &u, &items[0].id).await;
    assert!(item.media.quality.is_some());

    let list: Vec<Recheck> = srv.get(&u, routes::RECHECKS).await.expect(200).json();
    assert_eq!(list.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), [r.id.as_str()]);
    let failures: Vec<RecheckFailure> = srv
        .get(&u, &routes::path(routes::RECHECK_FAILURES, &[&r.id]))
        .await
        .expect(200)
        .json();
    assert!(failures.is_empty());
    // Nobody else sees it.
    let bob = srv.user("bob").await;
    srv.get(&bob, &routes::path(routes::RECHECK, &[&r.id]))
        .await
        .expect(404);
    let none: Vec<Recheck> = srv.get(&bob, routes::RECHECKS).await.expect(200).json();
    assert!(none.is_empty());

    // Nothing missing: a batch of nothing, over at once.
    let r = started(&srv, &u, json!({"parts": ["quality"]})).await;
    assert_eq!((r.state, r.total), (RecheckState::Done, 0));
}

#[tokio::test]
async fn a_failed_picture_is_listed_and_a_batch_of_failures_fails() {
    let (srv, _) = server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let it = upload(&srv, &u, &set.id, "a.png", png(1), None).await;
    srv.run_jobs().await;
    sql(
        &srv,
        "DELETE FROM media_variants WHERE media_id = ? AND variant = 'original'",
        &[&it.media.id],
    )
    .await;
    let r = started(&srv, &u, json!({"parts": ["quality"], "mode": "refresh"})).await;
    srv.run_jobs().await;
    let r = recheck(&srv, &u, &r.id).await;
    assert_eq!((r.state, r.done, r.failed), (RecheckState::Failed, 0, 1));
    let failures: Vec<RecheckFailure> = srv
        .get(&u, &routes::path(routes::RECHECK_FAILURES, &[&r.id]))
        .await
        .expect(200)
        .json();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].media_id, it.media.id);
    assert_eq!(failures[0].item_id.as_deref(), Some(it.id.as_str()));
    assert!(
        failures[0].error.contains("original file missing"),
        "{}",
        failures[0].error
    );
}

#[tokio::test]
async fn asking_again_returns_the_running_batch() {
    let (srv, _) = server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    upload(&srv, &u, &set.id, "a.png", png(1), None).await;
    srv.run_jobs().await;
    let body = json!({"parts": ["renditions", "quality"], "mode": "refresh"});
    let first = started(&srv, &u, body.clone()).await;
    assert_eq!(first.parts, [RecheckPart::Quality, RecheckPart::Renditions], "sorted");
    // The same scope, the parts in another order: the running one, 200.
    let again: Recheck = start(
        &srv,
        &u,
        json!({"parts": ["quality", "renditions", "quality"], "mode": "refresh"}),
    )
    .await
    .expect(200)
    .json();
    assert_eq!(again.id, first.id);
    // Another mode, other parts or a set: new batches.
    let other = started(&srv, &u, json!({"parts": ["quality"], "mode": "refresh"})).await;
    assert_ne!(other.id, first.id);
    let in_set = started(
        &srv,
        &u,
        json!({"parts": ["quality"], "mode": "refresh", "set_id": set.id}),
    )
    .await;
    assert_ne!(in_set.id, other.id);
    // Another user's identical request is theirs.
    let bob = srv.user("bob").await;
    let bobs = started(&srv, &bob, body.clone()).await;
    assert_ne!(bobs.id, first.id);
    assert_eq!(bobs.total, 0);
    // Once it is over, the same request starts a new one.
    srv.run_jobs().await;
    let fresh = started(&srv, &u, body).await;
    assert_ne!(fresh.id, first.id);
}

#[tokio::test]
async fn cancelling_drops_the_queued_pictures_and_ends_the_batch() {
    let (srv, _) = server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    for seed in 1..=4 {
        upload(&srv, &u, &set.id, "a.png", png(seed), None).await;
    }
    srv.run_jobs().await;
    let r = started(&srv, &u, json!({"parts": ["quality"], "mode": "refresh"})).await;
    assert_eq!(r.total, 4);
    jobs::run_next(&srv.state, Lane::All).await.unwrap().unwrap();
    // One picture is being rechecked when the batch is cancelled.
    sql(
        &srv,
        "UPDATE jobs SET status = 'running' WHERE id = (SELECT id FROM jobs WHERE kind = 'recheck' AND status = 'queued' LIMIT 1)",
        &[],
    )
    .await;
    // Not someone else's to cancel.
    let bob = srv.user("bob").await;
    srv.post(&bob, &routes::path(routes::RECHECK_CANCEL, &[&r.id]), json!({}))
        .await
        .expect(404);
    let c: Recheck = srv
        .post(&u, &routes::path(routes::RECHECK_CANCEL, &[&r.id]), json!({}))
        .await
        .expect(200)
        .json();
    assert_eq!((c.state, c.done, c.failed), (RecheckState::Cancelled, 1, 0));
    assert!(c.finished_at.is_some());
    let db = &srv.state.db;
    assert_eq!(
        scalar(
            db,
            "SELECT COUNT(*) FROM jobs WHERE kind = 'recheck' AND status = 'queued'"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            db,
            "SELECT COUNT(*) FROM jobs WHERE kind = 'recheck' AND status = 'running'"
        )
        .await,
        1
    );
    // The running one finishes (here after a restart) and is counted; the
    // batch stays cancelled.
    jobs::requeue_interrupted(db).await.unwrap();
    assert_eq!(srv.run_jobs().await, 1);
    let c = recheck(&srv, &u, &r.id).await;
    assert_eq!((c.state, c.total, c.done, c.failed), (RecheckState::Cancelled, 4, 2, 0));
    // Cancelling again changes nothing; asking for the same starts anew.
    let again: Recheck = srv
        .post(&u, &routes::path(routes::RECHECK_CANCEL, &[&r.id]), json!({}))
        .await
        .expect(200)
        .json();
    assert_eq!(again, c);
    let new = started(&srv, &u, json!({"parts": ["quality"], "mode": "refresh"})).await;
    assert_ne!(new.id, r.id);
    // An admin may cancel a user's batch.
    let admin = srv.admin("root").await;
    let c: Recheck = srv
        .post(&admin, &routes::path(routes::RECHECK_CANCEL, &[&new.id]), json!({}))
        .await
        .expect(200)
        .json();
    assert_eq!(c.state, RecheckState::Cancelled);
}

#[tokio::test]
async fn a_restart_continues_the_batch() {
    let (srv, _) = server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    for seed in 1..=3 {
        upload(&srv, &u, &set.id, "a.png", png(seed), None).await;
    }
    srv.run_jobs().await;
    let r = started(&srv, &u, json!({"parts": ["quality", "renditions"], "mode": "refresh"})).await;
    jobs::run_next(&srv.state, Lane::All).await.unwrap().unwrap();
    // The server stops while a picture is being rechecked.
    sql(
        &srv,
        "UPDATE jobs SET status = 'running' WHERE id = (SELECT id FROM jobs WHERE kind = 'recheck' AND status = 'queued' LIMIT 1)",
        &[],
    )
    .await;
    let state = restart(&srv, EnricherSource::Disabled, TextReaderSource::Disabled).await;
    assert_eq!(
        scalar(
            &state.db,
            "SELECT COUNT(*) FROM jobs WHERE kind = 'recheck' AND status = 'queued'"
        )
        .await,
        2,
        "the interrupted picture is queued again"
    );
    assert_eq!(jobs::run_pending(&state).await.unwrap(), 2);
    let r = recheck(&srv, &u, &r.id).await;
    assert_eq!((r.state, r.total, r.done, r.failed), (RecheckState::Done, 3, 3, 0));
}

// ---------------------------------------------------------------------------
// Fairness
// ---------------------------------------------------------------------------

/// An upload made during a large recheck gets its renditions and
/// suggestions first: the queue takes higher priorities first, and a
/// recheck is below everything else.
#[tokio::test]
async fn uploads_go_before_a_large_recheck() {
    let (srv, _) = server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    for seed in 1..=6 {
        upload(&srv, &u, &set.id, "a.png", png(seed), None).await;
    }
    srv.run_jobs().await;
    let r = started(&srv, &u, json!({"mode": "refresh"})).await;
    assert_eq!(r.total, 6);
    jobs::run_next(&srv.state, Lane::All).await.unwrap().unwrap();

    let new = upload(&srv, &u, &set.id, "new.png", png(99), None).await;
    // The upload's two jobs, in either order (queued in the same second,
    // they are ordered by their random ids).
    let mut claimed = Vec::new();
    for _ in 0..2 {
        let job = jobs::run_next(&srv.state, Lane::All).await.unwrap().unwrap();
        claimed.push((job.kind, job.subject));
    }
    claimed.sort();
    assert_eq!(
        claimed,
        [
            ("enrich".to_string(), Some(new.media.id.clone())),
            ("variants".to_string(), Some(new.media.id.clone()))
        ]
    );
    // The worker kept for interactive work takes no recheck.
    assert!(jobs::run_next(&srv.state, Lane::Interactive).await.unwrap().is_none());
    let third = jobs::run_next(&srv.state, Lane::All).await.unwrap().unwrap();
    assert_eq!(third.kind, "recheck");
    let new = item(&srv, &u, &new.id).await;
    assert!(
        new.media.variants.len() > 2,
        "renditions made: {:?}",
        new.media.variants
    );
    assert_eq!(new.text, READ, "suggestions made");

    // Which worker takes what.
    assert_eq!(Lane::for_worker(0, 2), Lane::Interactive);
    assert_eq!(Lane::for_worker(1, 2), Lane::All);
    assert_eq!(Lane::for_worker(0, 1), Lane::All, "a single worker takes everything");
}

// ---------------------------------------------------------------------------
// Parts: missing against refresh
// ---------------------------------------------------------------------------

#[tokio::test]
async fn embedding_missing_and_refresh() {
    let (srv, _) = server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let a = upload(&srv, &u, &set.id, "a.png", solid_png(RED, 1), None).await;
    upload(&srv, &u, &set.id, "b.png", solid_png(BLUE, 2), None).await;
    srv.run_jobs().await;
    // One picture was embedded by another model.
    sql(
        &srv,
        "UPDATE media SET embedding_model = 'old-model' WHERE id = ?",
        &[&a.media.id],
    )
    .await;
    let r = started(&srv, &u, json!({"parts": ["embedding"]})).await;
    assert_eq!(r.total, 1);
    srv.run_jobs().await;
    let r = recheck(&srv, &u, &r.id).await;
    assert_eq!((r.state, r.done, r.changed), (RecheckState::Done, 1, 1));
    assert_eq!(
        scalar(
            &srv.state.db,
            "SELECT COUNT(*) FROM media WHERE embedding_model = 'mock-64'"
        )
        .await,
        2
    );
    let r = started(&srv, &u, json!({"parts": ["embedding"]})).await;
    assert_eq!(r.total, 0, "nothing missing");
    // Refresh embeds every picture again; the same vectors change nothing.
    let r = started(&srv, &u, json!({"parts": ["embedding"], "mode": "refresh"})).await;
    assert_eq!(r.total, 2);
    srv.run_jobs().await;
    let r = recheck(&srv, &u, &r.id).await;
    assert_eq!((r.done, r.changed), (2, 0));
    // Search still finds the picture by its look.
    let hits: stickpile_api::SearchResults = common::read(
        srv.client
            .get(srv.url(routes::SEARCH))
            .query(&[("q", "red")])
            .bearer_auth(&u.token)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200)
    .json();
    assert_eq!(hits.hits[0].item.id, a.id);
}

#[tokio::test]
async fn keywords_emojis_and_vibe_missing_and_refresh() {
    let (srv, _) = server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let a = upload(&srv, &u, &set.id, "a.png", solid_png(RED, 1), None).await;
    let b = upload(&srv, &u, &set.id, "b.png", solid_png(BLUE, 2), None).await;
    srv.run_jobs().await;
    let a0 = item(&srv, &u, &a.id).await;
    assert!(a0.keywords.contains(&"red".to_string()), "{:?}", a0.keywords);
    assert_eq!((a0.vibe.as_deref(), a0.vibe_source), (Some("angry"), FieldSource::Ai));

    // a: emptied by a person; b: an AI value from elsewhere (an import).
    srv.patch(
        &u,
        &routes::path(routes::ITEM, &[&a.id]),
        json!({"keywords": [], "emojis": []}),
    )
    .await
    .expect(200);
    sql(
        &srv,
        "UPDATE items SET keywords = '[\"stale\"]', keywords_source = 'ai', vibe = NULL WHERE id = ?",
        &[&b.id],
    )
    .await;
    // Missing: the empty fields; the AI value from this model stays.
    let r = started(&srv, &u, json!({"parts": ["keywords", "emojis", "vibe"]})).await;
    assert_eq!(r.total, 2, "a has empty keywords, b no vibe");
    srv.run_jobs().await;
    let a1 = item(&srv, &u, &a.id).await;
    assert_eq!((&a1.keywords, a1.keywords_source), (&a0.keywords, FieldSource::Ai));
    assert_eq!((&a1.emojis, a1.emojis_source), (&a0.emojis, FieldSource::Ai));
    let b1 = item(&srv, &u, &b.id).await;
    assert_eq!(b1.keywords, ["stale"]);
    assert_eq!(b1.vibe.as_deref(), Some("sad"));
    // Only the vibe asked for: keywords stay.
    sql(&srv, "UPDATE items SET vibe = NULL WHERE id = ?", &[&b.id]).await;
    let r = started(&srv, &u, json!({"parts": ["vibe"], "mode": "refresh"})).await;
    srv.run_jobs().await;
    assert_eq!(recheck(&srv, &u, &r.id).await.done, 2);
    let b2 = item(&srv, &u, &b.id).await;
    assert_eq!(
        (b2.keywords.as_slice(), b2.vibe.as_deref()),
        (&["stale".to_string()][..], Some("sad"))
    );
    // Refresh: the computer's values are replaced.
    let r = started(&srv, &u, json!({"parts": ["keywords"], "mode": "refresh"})).await;
    srv.run_jobs().await;
    let r = recheck(&srv, &u, &r.id).await;
    assert_eq!((r.done, r.changed), (2, 1));
    let b3 = item(&srv, &u, &b.id).await;
    assert!(b3.keywords.contains(&"blue".to_string()), "{:?}", b3.keywords);
    assert_eq!(b3.keywords_source, FieldSource::Ai);
}

#[tokio::test]
async fn text_missing_and_refresh() {
    let (srv, mock) = server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let a = upload(&srv, &u, &set.id, "a.png", png(1), None).await;
    let b = upload(&srv, &u, &set.id, "b.png", png(2), None).await;
    srv.run_jobs().await;
    assert_eq!(item(&srv, &u, &a.id).await.text, READ);
    let reads = mock.reads();
    // a was never read by this reader; b's text was cleared by the
    // computer (an empty AI text is "not set").
    sql(
        &srv,
        "UPDATE media SET ocr_text = NULL, ocr_model = NULL WHERE id = ?",
        &[&a.media.id],
    )
    .await;
    sql(&srv, "UPDATE items SET text = '' WHERE id = ?", &[&b.id]).await;
    let r = started(&srv, &u, json!({"parts": ["text"]})).await;
    assert_eq!(r.total, 2);
    srv.run_jobs().await;
    assert_eq!(mock.reads(), reads + 1, "only the unread picture is read");
    assert_eq!(item(&srv, &u, &a.id).await.text, READ);
    assert_eq!(item(&srv, &u, &b.id).await.text, READ, "the stored reading fills it");
    let r = started(&srv, &u, json!({"parts": ["text"]})).await;
    assert_eq!(r.total, 0);
    // Refresh reads every picture again.
    let r = started(&srv, &u, json!({"parts": ["text"], "mode": "refresh"})).await;
    srv.run_jobs().await;
    assert_eq!(mock.reads(), reads + 3);
    let r = recheck(&srv, &u, &r.id).await;
    assert_eq!((r.done, r.changed), (2, 0), "the same words");
}

#[tokio::test]
async fn quality_and_renditions_missing_and_refresh() {
    let (srv, _) = server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let a = upload(&srv, &u, &set.id, "a.png", png(1), None).await;
    let b = upload(&srv, &u, &set.id, "b.png", png(2), None).await;
    srv.run_jobs().await;
    let before = item(&srv, &u, &a.id).await;
    let (score, report): (i64, String) = sqlx::query_as("SELECT quality, quality_report FROM media WHERE id = ?")
        .bind(&a.media.id)
        .fetch_one(&srv.state.db)
        .await
        .unwrap();

    // Quality: only the picture without a report; recomputed as uploaded.
    sql(
        &srv,
        "UPDATE media SET quality = NULL, quality_report = NULL WHERE id = ?",
        &[&a.media.id],
    )
    .await;
    let r = started(&srv, &u, json!({"parts": ["quality"]})).await;
    assert_eq!(r.total, 1);
    srv.run_jobs().await;
    let (score2, report2): (i64, String) = sqlx::query_as("SELECT quality, quality_report FROM media WHERE id = ?")
        .bind(&a.media.id)
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!((score2, &report2), (score, &report));
    let after = item(&srv, &u, &a.id).await;
    assert!(after.updated_at >= before.updated_at);
    assert_eq!(after.media.quality, before.media.quality);
    // A changed score reaches the item.
    sql(&srv, "UPDATE media SET quality = 1 WHERE id = ?", &[&a.media.id]).await;
    let r = started(&srv, &u, json!({"parts": ["quality"], "mode": "refresh"})).await;
    assert_eq!(r.total, 2);
    srv.run_jobs().await;
    assert_eq!(recheck(&srv, &u, &r.id).await.changed, 1);
    assert_eq!(item(&srv, &u, &a.id).await.media.quality, before.media.quality);

    // Renditions: made by an older encoder, or never finished.
    sql(
        &srv,
        "UPDATE media SET variants_version = 0 WHERE id = ?",
        &[&a.media.id],
    )
    .await;
    sql(
        &srv,
        "DELETE FROM media_variants WHERE media_id = ? AND variant NOT IN ('original', 'thumb')",
        &[&b.media.id],
    )
    .await;
    sql(&srv, "UPDATE media SET variants_done = 0 WHERE id = ?", &[&b.media.id]).await;
    let r = started(&srv, &u, json!({"parts": ["renditions"]})).await;
    assert_eq!(r.total, 2);
    srv.run_jobs().await;
    assert_eq!(recheck(&srv, &u, &r.id).await.changed, 2);
    let current = stickpile_media::variants_version(stickpile_api::MediaKind::Still);
    let versions: Vec<(i64, bool)> = sqlx::query_as("SELECT variants_version, variants_done FROM media")
        .fetch_all(&srv.state.db)
        .await
        .unwrap();
    assert!(versions.iter().all(|v| *v == (current, true)), "{versions:?}");
    assert_eq!(
        item(&srv, &u, &b.id).await.media.variants.len(),
        before.media.variants.len()
    );
    let r = started(&srv, &u, json!({"parts": ["renditions"]})).await;
    assert_eq!(r.total, 0);
    let r = started(&srv, &u, json!({"parts": ["renditions"], "mode": "refresh"})).await;
    assert_eq!(r.total, 2);
}

// ---------------------------------------------------------------------------
// A person's values (rule 6)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_persons_values_survive_every_recheck() {
    let (srv, _) = server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let mine = upload(
        &srv,
        &u,
        &set.id,
        "mine.png",
        solid_png(RED, 1),
        Some(json!({"keywords": ["mine"], "emojis": ["🐱"], "vibe": "smug", "text": "typed by me"})),
    )
    .await;
    let cleared = upload(
        &srv,
        &u,
        &set.id,
        "cleared.png",
        solid_png(BLUE, 2),
        Some(json!({"text": ""})),
    )
    .await;
    let ai = upload(&srv, &u, &set.id, "ai.png", solid_png(BLUE, 3), None).await;
    srv.run_jobs().await;
    let mine0 = fields(&item(&srv, &u, &mine.id).await);
    assert_eq!(mine0.3, ("typed by me".to_string(), FieldSource::User));
    let cleared0 = item(&srv, &u, &cleared.id).await;
    assert_eq!((cleared0.text.as_str(), cleared0.text_source), ("", FieldSource::User));
    assert_eq!(item(&srv, &u, &ai.id).await.text, READ);

    // Every part, refreshed, and every part missing (after a model change).
    let r = started(&srv, &u, json!({"mode": "refresh"})).await;
    assert_eq!(
        r.parts,
        [
            RecheckPart::Keywords,
            RecheckPart::Emojis,
            RecheckPart::Vibe,
            RecheckPart::Text,
            RecheckPart::Embedding,
            RecheckPart::Quality,
            RecheckPart::Renditions
        ],
        "no parts: all this server can do"
    );
    srv.run_jobs().await;
    assert_eq!(recheck(&srv, &u, &r.id).await.state, RecheckState::Done);
    sql(&srv, "UPDATE media SET embedding_model = 'old', ocr_model = 'old'", &[]).await;
    let r = started(&srv, &u, json!({})).await;
    assert_eq!(r.total, 3);
    srv.run_jobs().await;
    assert_eq!(fields(&item(&srv, &u, &mine.id).await), mine0);
    let cleared1 = item(&srv, &u, &cleared.id).await;
    assert_eq!((cleared1.text.as_str(), cleared1.text_source), ("", FieldSource::User));
    assert_eq!(
        cleared1.keywords_source,
        FieldSource::Ai,
        "its empty keywords were the computer's to fill"
    );

    // A reader that no longer sees text: a refresh clears the computer's
    // text and nothing else; missing never clears.
    let blind = Arc::new(MockTextReader::with_lines(MockTextReader::MODEL_ID, Vec::new()));
    let state = restart(&srv, EnricherSource::Disabled, TextReaderSource::Provided(blind)).await;
    let alice = viewer(&state, "alice").await;
    let req = |mode| StartRecheck {
        parts: vec![RecheckPart::Text],
        mode,
        set_id: None,
    };
    // Read by another reader before: missing reads again, finds nothing.
    sql(&srv, "UPDATE media SET ocr_model = 'other'", &[]).await;
    let (r, _) = stickpile_server::service::recheck::start_own(&state, &alice, req(RecheckMode::Missing))
        .await
        .unwrap();
    assert_eq!(r.total, 3);
    jobs::run_pending(&state).await.unwrap();
    assert_eq!(
        scalar(
            &state.db,
            "SELECT COUNT(*) FROM media WHERE ocr_text = '' AND ocr_model = 'mock-ocr'"
        )
        .await,
        3
    );
    assert_eq!(item(&srv, &u, &ai.id).await.text, READ, "missing never clears");
    let (r, created) = stickpile_server::service::recheck::start_own(&state, &alice, req(RecheckMode::Refresh))
        .await
        .unwrap();
    assert!(created);
    assert_eq!(r.total, 3);
    jobs::run_pending(&state).await.unwrap();
    let ai1 = item(&srv, &u, &ai.id).await;
    assert_eq!((ai1.text.as_str(), ai1.text_source), ("", FieldSource::Ai));
    assert_eq!(fields(&item(&srv, &u, &mine.id).await), mine0);
    let cleared2 = item(&srv, &u, &cleared.id).await;
    assert_eq!((cleared2.text.as_str(), cleared2.text_source), ("", FieldSource::User));
}

/// Each AI write path the recheck added refuses a person's value, called
/// directly.
#[tokio::test]
async fn the_write_paths_refuse_a_persons_value() {
    use stickpile_server::service::ai::{self, AiFill, ItemFilter, Overwrite};
    let (srv, _) = server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let mine = upload(
        &srv,
        &u,
        &set.id,
        "mine.png",
        solid_png(RED, 1),
        Some(json!({"keywords": ["mine"], "emojis": ["🐱"], "vibe": "smug", "text": "typed"})),
    )
    .await;
    let cleared = upload(&srv, &u, &set.id, "c.png", solid_png(RED, 2), Some(json!({"text": ""}))).await;
    let fill = AiFill {
        keywords: vec!["computed".into()],
        emojis: vec!["🤖".into()],
        vibe: Some("sad".into()),
    };
    let all = ItemFilter::default();
    let mut conn = srv.state.db.acquire().await.unwrap();
    for (media, overwrite) in [
        (&mine.media.id, Overwrite::AiValues),
        (&mine.media.id, Overwrite::EmptyOnly),
        (&cleared.media.id, Overwrite::AiValues),
        (&cleared.media.id, Overwrite::EmptyOnly),
    ] {
        let changed = ai::apply_text_to(&mut conn, media, "computed", all, overwrite)
            .await
            .unwrap();
        assert!(changed.is_empty(), "{overwrite:?}");
        let changed = ai::clear_ai_text(&mut conn, media, all).await.unwrap();
        assert!(changed.is_empty());
    }
    for overwrite in [Overwrite::AiValues, Overwrite::EmptyOnly] {
        let changed = ai::apply_to(&mut conn, &mine.media.id, &fill, all, overwrite)
            .await
            .unwrap();
        assert!(changed.is_empty(), "{overwrite:?}");
    }
    drop(conn);
    let m = item(&srv, &u, &mine.id).await;
    assert_eq!(
        fields(&m),
        (
            (vec!["mine".to_string()], FieldSource::User),
            (vec!["🐱".to_string()], FieldSource::User),
            (Some("smug".to_string()), FieldSource::User),
            ("typed".to_string(), FieldSource::User)
        )
    );
    let c = item(&srv, &u, &cleared.id).await;
    assert_eq!((c.text.as_str(), c.text_source), ("", FieldSource::User));
}

// ---------------------------------------------------------------------------
// Scopes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_users_recheck_changes_only_their_items() {
    let (srv, _) = server().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let a_set = srv.create_set(&alice, "a", Visibility::Private).await;
    let b_set = srv.create_set(&bob, "b", Visibility::Private).await;
    // The same picture in both libraries.
    let a = upload(&srv, &alice, &a_set.id, "x.png", solid_png(RED, 1), None).await;
    let b = upload(&srv, &bob, &b_set.id, "x.png", solid_png(RED, 1), None).await;
    assert_eq!(a.media.id, b.media.id);
    srv.run_jobs().await;
    sql(
        &srv,
        "UPDATE items SET keywords = '[\"stale\"]', text = 'stale' WHERE media_id = ?",
        &[&a.media.id],
    )
    .await;
    let b_version: i64 = sqlx::query_scalar("SELECT version FROM sets WHERE id = ?")
        .bind(&b_set.id)
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    let r = started(&srv, &alice, json!({"parts": ["keywords", "text"], "mode": "refresh"})).await;
    assert_eq!(r.total, 1);
    srv.run_jobs().await;
    let a1 = item(&srv, &alice, &a.id).await;
    assert!(a1.keywords.contains(&"red".to_string()));
    assert_eq!(a1.text, READ);
    let b1 = item(&srv, &bob, &b.id).await;
    assert_eq!(
        (b1.keywords.as_slice(), b1.text.as_str()),
        (&["stale".to_string()][..], "stale")
    );
    let b_version1: i64 = sqlx::query_scalar("SELECT version FROM sets WHERE id = ?")
        .bind(&b_set.id)
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(b_version1, b_version, "bob's set did not change");
}

#[tokio::test]
async fn a_recheck_of_one_set() {
    let (srv, _) = server().await;
    let alice = srv.user("alice").await;
    let one = srv.create_set(&alice, "one", Visibility::Private).await;
    let two = srv.create_set(&alice, "two", Visibility::Private).await;
    let a = upload(&srv, &alice, &one.id, "a.png", solid_png(RED, 1), None).await;
    let b = upload(&srv, &alice, &two.id, "b.png", solid_png(RED, 2), None).await;
    // The same picture in both sets.
    let c1 = upload(&srv, &alice, &one.id, "c.png", solid_png(BLUE, 3), None).await;
    let c2 = upload(&srv, &alice, &two.id, "c.png", solid_png(BLUE, 3), None).await;
    srv.run_jobs().await;
    sql(&srv, "UPDATE items SET keywords = '[\"stale\"]'", &[]).await;
    let r = started(
        &srv,
        &alice,
        json!({"parts": ["keywords"], "mode": "refresh", "set_id": one.id}),
    )
    .await;
    assert_eq!((r.total, r.set_id.as_deref()), (2, Some(one.id.as_str())));
    srv.run_jobs().await;
    for (id, rechecked) in [(&a.id, true), (&c1.id, true), (&b.id, false), (&c2.id, false)] {
        let i = item(&srv, &alice, id).await;
        assert_eq!(i.keywords != ["stale"], rechecked, "{id}");
    }
    // Only one's own sets.
    let bob = srv.user("bob").await;
    let private = srv.create_set(&bob, "p", Visibility::Private).await;
    let public = srv.create_set(&bob, "q", Visibility::Public).await;
    start(&srv, &alice, json!({"set_id": private.id, "parts": ["quality"]}))
        .await
        .expect(404);
    let r = start(&srv, &alice, json!({"set_id": public.id, "parts": ["quality"]}))
        .await
        .expect(403);
    assert_eq!(r.code(), "forbidden");
}

#[tokio::test]
async fn a_server_wide_recheck_is_for_admins_and_audited() {
    let (srv, _) = server().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let admin = srv.admin("root").await;
    for (u, seed) in [(&alice, 1), (&bob, 2)] {
        let set = srv.create_set(u, "s", Visibility::Private).await;
        upload(&srv, u, &set.id, "a.png", png(seed), None).await;
    }
    srv.run_jobs().await;
    let body = json!({"parts": ["quality"], "mode": "refresh"});
    srv.post(&alice, routes::ADMIN_RECHECKS, body.clone()).await.expect(403);
    let r: Recheck = srv
        .post(&admin, routes::ADMIN_RECHECKS, body.clone())
        .await
        .expect(201)
        .json();
    assert_eq!((r.scope, r.total), (RecheckScope::Server, 2));
    assert_eq!(r.requested_by.as_deref(), Some("root"));
    // Asked again: the running one, not audited twice.
    let again: Recheck = srv.post(&admin, routes::ADMIN_RECHECKS, body).await.expect(200).json();
    assert_eq!(again.id, r.id);
    let audit: Vec<AuditEntry> = srv.get(&admin, routes::ADMIN_AUDIT).await.expect(200).json();
    let starts: Vec<&AuditEntry> = audit.iter().filter(|e| e.action == "recheck.start").collect();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].target.as_deref(), Some(r.id.as_str()));
    assert_eq!(starts[0].actor_username.as_deref(), Some("root"));
    assert_eq!(starts[0].details.as_ref().unwrap()["total"], 2);
    // An admin sees it; users do not.
    let list: Vec<Recheck> = srv.get(&admin, routes::RECHECKS).await.expect(200).json();
    assert_eq!(list[0].id, r.id);
    let list: Vec<Recheck> = srv.get(&alice, routes::RECHECKS).await.expect(200).json();
    assert!(list.is_empty());
    srv.get(&alice, &routes::path(routes::RECHECK, &[&r.id]))
        .await
        .expect(404);
    srv.post(&alice, &routes::path(routes::RECHECK_CANCEL, &[&r.id]), json!({}))
        .await
        .expect(404);
    // The job list shows the pictures as recheck jobs.
    let jobs: Vec<Value> = srv.get(&admin, routes::ADMIN_JOBS).await.expect(200).json();
    assert!(jobs.iter().any(|j| j["kind"] == "recheck"));
    let c: Recheck = srv
        .post(&admin, &routes::path(routes::RECHECK_CANCEL, &[&r.id]), json!({}))
        .await
        .expect(200)
        .json();
    assert_eq!(c.state, RecheckState::Cancelled);
    let audit: Vec<AuditEntry> = srv.get(&admin, routes::ADMIN_AUDIT).await.expect(200).json();
    assert!(audit
        .iter()
        .any(|e| e.action == "recheck.cancel" && e.target.as_deref() == Some(r.id.as_str())));
    // A set of anyone's.
    let sets: Vec<String> = sqlx::query_scalar("SELECT id FROM sets")
        .fetch_all(&srv.state.db)
        .await
        .unwrap();
    let r: Recheck = srv
        .post(
            &admin,
            routes::ADMIN_RECHECKS,
            json!({"parts": ["quality"], "mode": "refresh", "set_id": sets[0]}),
        )
        .await
        .expect(201)
        .json();
    assert_eq!(r.total, 1);
}

#[tokio::test]
async fn a_users_rechecks_are_rate_limited() {
    let (srv, _) = server_with(|c| c.limits.recheck_per_hour = 2).await;
    let u = srv.user("alice").await;
    started(&srv, &u, json!({"parts": ["quality"], "mode": "refresh"})).await;
    started(&srv, &u, json!({"parts": ["renditions"], "mode": "refresh"})).await;
    let r = start(&srv, &u, json!({"parts": ["embedding"], "mode": "refresh"}))
        .await
        .expect(429);
    assert_eq!(r.code(), "rate_limited");
    assert!(r.header("retry-after").is_some());
    // Per user.
    let bob = srv.user("bob").await;
    started(&srv, &bob, json!({"parts": ["quality"]})).await;
    // Admins' server-wide rechecks are not limited.
    let admin = srv.admin("root").await;
    for parts in [["quality"], ["renditions"], ["embedding"]] {
        srv.post(
            &admin,
            routes::ADMIN_RECHECKS,
            json!({"parts": parts, "mode": "refresh"}),
        )
        .await
        .expect(201);
    }
}

#[tokio::test]
async fn the_same_running_recheck_costs_nothing() {
    let (srv, _) = server_with(|c| c.limits.recheck_per_hour = 1).await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    upload(&srv, &u, &set.id, "a.png", png(1), None).await;
    srv.run_jobs().await;
    let body = json!({"parts": ["quality"], "mode": "refresh"});
    let r = started(&srv, &u, body.clone()).await;
    for _ in 0..3 {
        let again: Recheck = start(&srv, &u, body.clone()).await.expect(200).json();
        assert_eq!(again.id, r.id);
    }
    start(&srv, &u, json!({"parts": ["renditions"], "mode": "refresh"}))
        .await
        .expect(429);
}

#[tokio::test]
async fn parts_and_modes_are_checked() {
    let (srv, _) = server().await;
    let u = srv.user("alice").await;
    let r = start(&srv, &u, json!({"parts": ["colour"]})).await.expect(400);
    assert_eq!(r.code(), "bad_request");
    start(&srv, &u, json!({"mode": "deep"})).await.expect(400);
    // In Ukrainian like the other messages.
    let r = common::read(
        srv.client
            .post(srv.url(routes::RECHECKS))
            .bearer_auth(&u.token)
            .header("accept-language", "uk")
            .json(&json!({"parts": ["colour"]}))
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(400);
    assert_eq!(r.value()["message"], "невідома частина повторної перевірки");

    // Without AI and a reader: the CLIP parts and the text are refused, and
    // no parts means the others.
    let plain = TestServer::start().await;
    let u = plain.user("alice").await;
    let r = plain
        .post(&u, routes::RECHECKS, json!({"parts": ["keywords"]}))
        .await
        .expect(503);
    assert_eq!(r.code(), "unavailable");
    let r = plain
        .post(&u, routes::RECHECKS, json!({"parts": ["text"]}))
        .await
        .expect(503);
    assert_eq!(r.code(), "unavailable");
    let r: Recheck = plain.post(&u, routes::RECHECKS, json!({})).await.expect(201).json();
    assert_eq!(r.parts, [RecheckPart::Quality, RecheckPart::Renditions]);
}

// ---------------------------------------------------------------------------
// The server's own rechecks (startup)
// ---------------------------------------------------------------------------

/// After a model change the server rechecks the embedding and the AI
/// fields of every picture embedded by another model, as one server-wide
/// batch it starts itself; the computer's values are replaced, a person's
/// are not, and no `enrich` job is queued any more. Pictures its text
/// reader never read get their text the same way.
#[tokio::test]
async fn the_server_rechecks_after_a_model_change() {
    let (srv, _) = server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let ai = upload(&srv, &u, &set.id, "a.png", solid_png(RED, 1), None).await;
    let mine = upload(
        &srv,
        &u,
        &set.id,
        "b.png",
        solid_png(BLUE, 2),
        Some(json!({"keywords": ["mine"]})),
    )
    .await;
    srv.run_jobs().await;
    sql(
        &srv,
        "UPDATE items SET keywords = '[\"stale\"]' WHERE id = ?",
        &[&ai.id],
    )
    .await;
    let enrich_before = scalar(&srv.state.db, "SELECT COUNT(*) FROM jobs WHERE kind = 'enrich'").await;

    let other_model: Arc<dyn stickpile_ai::Enricher> = Arc::new(stickpile_ai::MockEnricher { dim: 32 });
    let state = restart(
        &srv,
        EnricherSource::Provided(other_model.clone()),
        TextReaderSource::Disabled,
    )
    .await;
    assert_eq!(
        scalar(&state.db, "SELECT COUNT(*) FROM jobs WHERE kind = 'enrich'").await,
        enrich_before,
        "no enrich job"
    );
    let rows: Vec<(String, Option<String>, String, String, i64)> =
        sqlx::query_as("SELECT scope, requested_by, parts, mode, total FROM rechecks")
            .fetch_all(&state.db)
            .await
            .unwrap();
    assert_eq!(
        rows,
        [(
            "server".to_string(),
            None,
            r#"["keywords","emojis","vibe","embedding"]"#.to_string(),
            "missing".to_string(),
            2
        )]
    );
    // A restart before it ran adds nothing.
    let again = restart(
        &srv,
        EnricherSource::Provided(other_model.clone()),
        TextReaderSource::Disabled,
    )
    .await;
    assert_eq!(scalar(&again.db, "SELECT COUNT(*) FROM rechecks").await, 1);
    jobs::run_pending(&state).await.unwrap();
    assert_eq!(
        scalar(
            &state.db,
            "SELECT COUNT(*) FROM media WHERE embedding_model = 'mock-32'"
        )
        .await,
        2
    );
    let ai1 = item(&srv, &u, &ai.id).await;
    assert!(!ai1.keywords.contains(&"stale".to_string()), "{:?}", ai1.keywords);
    assert_eq!(ai1.keywords_source, FieldSource::Ai);
    let mine1 = item(&srv, &u, &mine.id).await;
    assert_eq!(
        (mine1.keywords.as_slice(), mine1.keywords_source),
        (&["mine".to_string()][..], FieldSource::User)
    );
    // The admin sees the server's batch, done, started by nobody.
    let admin = srv.admin("root").await;
    let list: Vec<Recheck> = srv.get(&admin, routes::RECHECKS).await.expect(200).json();
    assert_eq!(list.len(), 1);
    assert_eq!(
        (list[0].requested_by.as_deref(), list[0].state),
        (None, RecheckState::Done)
    );
    // The same model again: nothing to do, no batch.
    restart(&srv, EnricherSource::Provided(other_model), TextReaderSource::Disabled).await;
    assert_eq!(scalar(&state.db, "SELECT COUNT(*) FROM rechecks").await, 1);

    // A new text reader: the pictures it never read.
    let new_reader = Arc::new(MockTextReader::with_lines(
        "mock-ocr-2",
        vec![stickpile_ai::ocr::TextLine::new("new words", 0.9)],
    ));
    let state = restart(&srv, EnricherSource::Disabled, TextReaderSource::Provided(new_reader)).await;
    let text: Vec<(String, i64)> =
        sqlx::query_as("SELECT parts, total FROM rechecks WHERE parts = '[\"text\"]' AND requested_by IS NULL")
            .fetch_all(&state.db)
            .await
            .unwrap();
    assert_eq!(text, [(r#"["text"]"#.to_string(), 2)]);
    jobs::run_pending(&state).await.unwrap();
    assert_eq!(
        item(&srv, &u, &ai.id).await.text,
        "new words",
        "a new reading replaces the computer's text"
    );
}
