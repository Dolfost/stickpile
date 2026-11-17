//! The item text, the words in the picture: OCR through the reader seam
//! (`MockTextReader`), the rule that the computer never writes a text a
//! person wrote or cleared (each AI write path), search by text, and the
//! text in uploads, edits, suggestions, imports, exports, proposals and
//! the problems page.

mod common;
mod web_common;

use std::sync::{Arc, Mutex};

use common::{gif_anim, png, read, Opts, TestServer, TestUser};
use reqwest::StatusCode;
use serde_json::{json, Value};
use stickpile_ai::MockTextReader;
use stickpile_api::{
    routes, AiSuggestions, FieldSource, ImportReport, Item, ItemDetail, Job, JobKind, JobStatus, ProblemKind, Proposal,
    SearchResults, SetDetail, SetProblems, UploadResult, Visibility,
};
use web_common::Browser;

/// What the mock reader reads in every frame, as the server stores it.
const READ: &str = "NO THOUGHTS\nhead empty";

/// A server with a mock reader (and the mock CLIP enricher when `clip`).
async fn ocr_server_with(
    clip: bool,
    f: impl FnOnce(&mut stickpile_server::Config),
) -> (TestServer, Arc<MockTextReader>) {
    let mock = Arc::new(MockTextReader::new(&["NO THOUGHTS", "head empty"]));
    let srv = TestServer::start_with(
        Opts {
            ai: clip,
            text_reader: Some(mock.clone()),
            ..Default::default()
        },
        f,
    )
    .await;
    (srv, mock)
}

async fn ocr_server() -> (TestServer, Arc<MockTextReader>) {
    ocr_server_with(true, |_| {}).await
}

async fn item(srv: &TestServer, u: &TestUser, id: &str) -> Item {
    srv.get(u, &routes::path(routes::ITEM, &[id]))
        .await
        .expect(200)
        .json::<ItemDetail>()
        .item
}

/// Upload one file with optional metadata; the new (or existing) item.
async fn upload(srv: &TestServer, u: &TestUser, set_id: &str, name: &str, bytes: Vec<u8>, meta: Option<Value>) -> Item {
    let res: Vec<UploadResult> = srv
        .upload(u, set_id, vec![(name, bytes)], meta.map(|m| json!([m])))
        .await
        .expect(200)
        .json();
    res.into_iter().next().unwrap().item.expect("item")
}

fn text_of(i: &Item) -> (&str, FieldSource) {
    (i.text.as_str(), i.text_source)
}

/// (ocr_text, ocr_model) of a media row.
async fn media_ocr(srv: &TestServer, media_id: &str) -> (Option<String>, Option<String>) {
    sqlx::query_as("SELECT ocr_text, ocr_model FROM media WHERE id = ?")
        .bind(media_id)
        .fetch_one(&srv.state.db)
        .await
        .unwrap()
}

/// Whether CLIP embedded the media.
async fn embedded(srv: &TestServer, media_id: &str) -> bool {
    sqlx::query_scalar::<_, Option<String>>("SELECT embedding_model FROM media WHERE id = ?")
        .bind(media_id)
        .fetch_one(&srv.state.db)
        .await
        .unwrap()
        .is_some()
}

async fn patch_item(srv: &TestServer, u: &TestUser, id: &str, body: Value) -> Item {
    srv.patch(u, &routes::path(routes::ITEM, &[id]), body)
        .await
        .expect(200)
        .json()
}

/// Run a manual re-enrichment of the item's media to the end.
async fn enrich_again(srv: &TestServer, u: &TestUser, id: &str) -> Job {
    let job: Job = srv
        .post(u, &routes::path(routes::ITEM_ENRICH, &[id]), json!({}))
        .await
        .expect(200)
        .json();
    srv.run_jobs().await;
    let job: Job = srv
        .get(u, &routes::path(routes::JOB, &[&job.id]))
        .await
        .expect(200)
        .json();
    assert_eq!(job.status, JobStatus::Done, "{:?}", job.error);
    job
}

async fn search(srv: &TestServer, u: &TestUser, q: &str) -> SearchResults {
    read(
        srv.client
            .get(srv.url(routes::SEARCH))
            .query(&[("q", q)])
            .bearer_auth(&u.token)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200)
    .json()
}

fn hit_ids(r: &SearchResults) -> Vec<String> {
    let mut v: Vec<String> = r.hits.iter().map(|h| h.item.id.clone()).collect();
    v.sort();
    v
}

/// An animated GIF with `n` frames.
fn gif_frames(n: u64) -> Vec<u8> {
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame};
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut out);
        enc.set_repeat(Repeat::Infinite).unwrap();
        for i in 0..n {
            let frame = Frame::from_parts(
                common::image(40, 30, 7 + i, 0),
                0,
                0,
                Delay::from_numer_denom_ms(100, 1),
            );
            enc.encode_frame(frame).unwrap();
        }
    }
    out
}

// ---------------------------------------------------------------------------
// OCR fills the text, once per file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upload_reads_the_text_once_per_file() {
    let (srv, mock) = ocr_server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let up = upload(&srv, &u, &set.id, "cat.png", png(1), None).await;
    assert_eq!(text_of(&up), ("", FieldSource::Ai), "not set until read");
    assert_eq!(mock.reads(), 0, "read by the job, not the upload");
    srv.run_jobs().await;
    assert_eq!(mock.reads(), 1);
    let first = item(&srv, &u, &up.id).await;
    assert_eq!(text_of(&first), (READ, FieldSource::Ai));
    assert_eq!(
        media_ocr(&srv, &up.media.id).await,
        (Some(READ.into()), Some(MockTextReader::MODEL_ID.into()))
    );
    // The job reports what it read.
    let admin = srv.admin("root").await;
    let jobs: Vec<Job> = srv.get(&admin, routes::ADMIN_JOBS).await.expect(200).json();
    let job = jobs.iter().find(|j| j.kind == JobKind::Enrich).unwrap();
    let result = job.result.as_ref().unwrap();
    assert_eq!(result["text"], READ);
    assert_eq!(result["ocr_model"], MockTextReader::MODEL_ID);
    assert_eq!(result["text_items_updated"], 1);

    // The same file in another set: the text comes from the media at once,
    // and nothing is read (or queued) again.
    let set2 = srv.create_set(&u, "s2", Visibility::Private).await;
    let again = upload(&srv, &u, &set2.id, "again.png", png(1), None).await;
    assert_eq!(text_of(&again), (READ, FieldSource::Ai));
    srv.run_jobs().await;
    assert_eq!(mock.reads(), 1, "a file is read once");
    let enrich_jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'enrich'")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(enrich_jobs, 1);

    // Search finds both by the words, reporting `text`.
    let r = search(&srv, &u, "thoughts").await;
    assert_eq!(hit_ids(&r), {
        let mut v = vec![up.id.clone(), again.id.clone()];
        v.sort();
        v
    });
    assert!(
        r.hits.iter().all(|h| h.matched.contains(&"text".to_string())),
        "{:?}",
        r.hits
    );
    assert_eq!(search(&srv, &u, "text:\"no thoughts\"").await.hits.len(), 2);
    assert_eq!(search(&srv, &u, "x:\"thoughts no\"").await.hits.len(), 0);
    assert_eq!(search(&srv, &u, "-text:empty").await.hits.len(), 0);
    assert_eq!(search(&srv, &u, "cat -x:empty").await.hits.len(), 0);
    assert_eq!(
        search(&srv, &u, "text:\"no thoughts\"").await.query,
        "text:\"no thoughts\""
    );
}

#[tokio::test]
async fn an_animation_is_read_at_three_frames_without_clip() {
    // OCR alone: the Enrich job reads without a CLIP model.
    let (srv, mock) = ocr_server_with(false, |_| {}).await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let anim = upload(&srv, &u, &set.id, "anim.gif", gif_frames(5), None).await;
    srv.run_jobs().await;
    assert_eq!(mock.frames(), vec![(40, 30); 3], "first, middle and last of 5 frames");
    let anim = item(&srv, &u, &anim.id).await;
    assert_eq!(text_of(&anim), (READ, FieldSource::Ai), "lines merged without repeats");
    assert!(!embedded(&srv, &anim.media.id).await, "no CLIP here");
    // A 3-frame animation is read at every frame, a still once.
    upload(&srv, &u, &set.id, "three.gif", gif_anim(9), None).await;
    upload(&srv, &u, &set.id, "still.png", png(9), None).await;
    srv.run_jobs().await;
    assert_eq!(mock.reads(), 3 + 3 + 1);
}

// ---------------------------------------------------------------------------
// A text a person wrote is never overwritten
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_text_a_person_wrote_or_cleared_stays() {
    let (srv, mock) = ocr_server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    // Typed at upload, and cleared at upload.
    let typed = upload(
        &srv,
        &u,
        &set.id,
        "typed.png",
        png(1),
        Some(json!({"text": " my words\r\n"})),
    )
    .await;
    assert_eq!(text_of(&typed), ("my words", FieldSource::User));
    let cleared = upload(&srv, &u, &set.id, "cleared.png", png(2), Some(json!({"text": ""}))).await;
    assert_eq!(text_of(&cleared), ("", FieldSource::User));
    let plain = upload(&srv, &u, &set.id, "plain.png", png(3), None).await;
    srv.run_jobs().await;
    assert_eq!(mock.reads(), 3, "every file is read: the media keeps the result");
    assert_eq!(
        text_of(&item(&srv, &u, &typed.id).await),
        ("my words", FieldSource::User)
    );
    assert_eq!(text_of(&item(&srv, &u, &cleared.id).await), ("", FieldSource::User));
    assert_eq!(text_of(&item(&srv, &u, &plain.id).await), (READ, FieldSource::Ai));

    // Cleared by an edit: the user's, and a later Enrich does not refill it.
    let edited = patch_item(&srv, &u, &plain.id, json!({"text": ""})).await;
    assert_eq!(text_of(&edited), ("", FieldSource::User));
    let job = enrich_again(&srv, &u, &plain.id).await;
    assert_eq!(job.result.as_ref().unwrap()["text_items_updated"], 0);
    assert_eq!(text_of(&item(&srv, &u, &plain.id).await), ("", FieldSource::User));
    // A user text survives the Enrich job too.
    enrich_again(&srv, &u, &typed.id).await;
    assert_eq!(
        text_of(&item(&srv, &u, &typed.id).await),
        ("my words", FieldSource::User)
    );

    // The same file as a user's item gets nothing from the media's result,
    // while a new item of it does.
    let set2 = srv.create_set(&u, "s2", Visibility::Private).await;
    let mine = upload(
        &srv,
        &u,
        &set2.id,
        "typed.png",
        png(1),
        Some(json!({"text": "other words"})),
    )
    .await;
    assert_eq!(text_of(&mine), ("other words", FieldSource::User));
    let fresh = upload(&srv, &u, &set2.id, "plain.png", png(3), None).await;
    assert_eq!(text_of(&fresh), (READ, FieldSource::Ai));

    // Edits: cleaned, at most 2000 characters, any value is the user's.
    let e = patch_item(&srv, &u, &fresh.id, json!({"text": "a\r\nb\tc\u{7}  "})).await;
    assert_eq!(text_of(&e), ("a\nb c", FieldSource::User));
    let long = "w".repeat(2001);
    srv.patch(&u, &routes::path(routes::ITEM, &[&fresh.id]), json!({"text": long}))
        .await
        .expect(400);
    srv.upload(
        &u,
        &set.id,
        vec![("long.png", png(4))],
        Some(json!([{"text": "w".repeat(2001)}])),
    )
    .await
    .expect(400);
    let max = patch_item(&srv, &u, &fresh.id, json!({"text": "ї".repeat(2000)})).await;
    assert_eq!(max.text.chars().count(), 2000);
    // Edits of other fields leave the text alone.
    let other = patch_item(&srv, &u, &typed.id, json!({"keywords": ["k"], "name": "n"})).await;
    assert_eq!(text_of(&other), ("my words", FieldSource::User));
}

#[tokio::test]
async fn unapproved_and_confirmed_texts() {
    let (srv, _mock) = ocr_server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    // The tray's unapproved suggestion is an AI text: OCR may replace it.
    let guess = upload(
        &srv,
        &u,
        &set.id,
        "guess.png",
        png(1),
        Some(json!({"text": "tray guess", "ai_fields": ["text"]})),
    )
    .await;
    assert_eq!(text_of(&guess), ("tray guess", FieldSource::Ai));
    // An empty suggestion is no text: OCR fills it.
    let empty = upload(
        &srv,
        &u,
        &set.id,
        "empty.png",
        png(2),
        Some(json!({"text": "", "ai_fields": ["text"]})),
    )
    .await;
    assert_eq!(text_of(&empty), ("", FieldSource::Ai));
    srv.run_jobs().await;
    assert_eq!(text_of(&item(&srv, &u, &guess.id).await), (READ, FieldSource::Ai));
    assert_eq!(text_of(&item(&srv, &u, &empty.id).await), (READ, FieldSource::Ai));

    // The problems page lists the unconfirmed AI text.
    let problems: SetProblems = srv
        .get(&u, &routes::path(routes::SET_PROBLEMS, &[&set.id]))
        .await
        .expect(200)
        .json();
    let p = problems
        .problems
        .iter()
        .find(|p| p.kind == ProblemKind::AiUnreviewed)
        .expect("AI values to review");
    let it = p.items.iter().find(|i| i.item_id == guess.id).unwrap();
    assert!(it.detail.as_deref().unwrap().contains("text"), "{:?}", it.detail);

    // Confirming keeps the value and makes it the user's.
    let c = patch_item(&srv, &u, &guess.id, json!({"confirm": ["text"]})).await;
    assert_eq!(text_of(&c), (READ, FieldSource::User));
    enrich_again(&srv, &u, &guess.id).await;
    assert_eq!(text_of(&item(&srv, &u, &guess.id).await), (READ, FieldSource::User));
}

/// The web UI's item form has no text field yet: saving it keeps the text.
#[tokio::test]
async fn the_web_item_form_keeps_the_text() {
    let (srv, _mock) = ocr_server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let it = upload(&srv, &u, &set.id, "cat.png", png(1), None).await;
    srv.run_jobs().await;
    let b = Browser::login(&srv, &u).await;
    let r = b
        .post_form(
            &format!("/items/{}", it.id),
            &[
                ("name", "Grumpy"),
                ("keywords", "meow"),
                ("emojis", ""),
                ("vibe", ""),
                ("tags", ""),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let after = item(&srv, &u, &it.id).await;
    assert_eq!(after.name, "Grumpy");
    assert_eq!(text_of(&after), (READ, FieldSource::Ai));
}

// ---------------------------------------------------------------------------
// Suggestions, and when there is nothing to read with
// ---------------------------------------------------------------------------

#[tokio::test]
async fn suggestions_carry_the_text() {
    let (srv, mock) = ocr_server().await;
    let u = srv.user("alice").await;
    let s: AiSuggestions = read(
        srv.client
            .post(srv.url(routes::AI_SUGGEST))
            .bearer_auth(&u.token)
            .multipart(
                reqwest::multipart::Form::new()
                    .part("file", reqwest::multipart::Part::bytes(png(1)).file_name("s.png")),
            )
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200)
    .json();
    assert_eq!(s.text.as_deref(), Some(READ));
    assert_eq!(mock.reads(), 1);
    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(stored, 0, "nothing stored");
}

#[tokio::test]
async fn ocr_off_reads_nothing() {
    let (srv, mock) = ocr_server_with(true, |c| c.ai.ocr = false).await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let it = upload(&srv, &u, &set.id, "cat.png", png(1), None).await;
    srv.run_jobs().await;
    assert_eq!(mock.reads(), 0);
    let it = item(&srv, &u, &it.id).await;
    assert_eq!(text_of(&it), ("", FieldSource::Ai));
    assert!(embedded(&srv, &it.media.id).await, "CLIP still ran");
    assert_eq!(media_ocr(&srv, &it.media.id).await, (None, None));
    let s: AiSuggestions = read(
        srv.client
            .post(srv.url(routes::AI_SUGGEST))
            .bearer_auth(&u.token)
            .multipart(
                reqwest::multipart::Form::new()
                    .part("file", reqwest::multipart::Part::bytes(png(1)).file_name("s.png")),
            )
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200)
    .json();
    assert_eq!(s.text, None);
}

/// Log lines written while it is the default subscriber.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Logs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = Logs;
    fn make_writer(&'a self) -> Logs {
        self.clone()
    }
}

#[tokio::test]
async fn without_a_reader_it_says_so_once_and_goes_on() {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    // `ai.ocr` is on by default; the test server's model dir has no OCR models.
    let srv = TestServer::start_with(
        Opts {
            ai: true,
            ocr_from_config: true,
            ..Default::default()
        },
        |c| {
            c.ai.enabled = true;
            c.ai.model_dir = "/nonexistent/stickpile-test-models".into();
            assert!(c.ai.ocr);
        },
    )
    .await;
    assert!(srv.state.text_reader().is_none());
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let it = upload(&srv, &u, &set.id, "cat.png", png(1), None).await;
    srv.run_jobs().await;
    let it = item(&srv, &u, &it.id).await;
    assert!(embedded(&srv, &it.media.id).await, "enrichment went on");
    assert_eq!(text_of(&it), ("", FieldSource::Ai));
    assert_eq!(media_ocr(&srv, &it.media.id).await, (None, None));
    let admin = srv.admin("root").await;
    let jobs: Vec<Job> = srv.get(&admin, routes::ADMIN_JOBS).await.expect(200).json();
    assert!(jobs.iter().all(|j| j.status == JobStatus::Done), "{jobs:?}");
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert_eq!(
        text.matches("ai.ocr is on, but the OCR models are missing").count(),
        1,
        "{text}"
    );
}

// ---------------------------------------------------------------------------
// Imports, exports and older servers' set.toml files
// ---------------------------------------------------------------------------

/// Import `manifest` plus PNG files (`name`, seed) as a new set or `into` one.
async fn import(
    srv: &TestServer,
    u: &TestUser,
    manifest: &str,
    files: &[(&str, u64)],
    into: Option<&str>,
) -> ImportReport {
    let mut form = reqwest::multipart::Form::new().text("set.toml", manifest.to_string());
    for (name, seed) in files {
        form = form.part(
            "file",
            reqwest::multipart::Part::bytes(png(*seed)).file_name(format!("texts/{name}")),
        );
    }
    let mut url = srv.url(routes::SETS_IMPORT);
    if let Some(id) = into {
        url = format!("{url}?into={id}");
    }
    read(
        srv.client
            .post(url)
            .bearer_auth(&u.token)
            .multipart(form)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200)
    .json()
}

const TEXTS: &str = r#"
name = "Texts"
slug = "texts"

[items."1-ai.png"]
name = "ai"
text = "read by a machine"
text_source = "ai"

[items."2-typed.png"]
name = "typed"
text = "Кіт у мішку"

[items."3-cleared.png"]
name = "cleared"
text_source = "user"

[items."4-unset.png"]
name = "unset"
"#;

const FILES: [(&str, u64); 4] = [
    ("1-ai.png", 1),
    ("2-typed.png", 2),
    ("3-cleared.png", 3),
    ("4-unset.png", 4),
];

async fn texts_of(srv: &TestServer, u: &TestUser, set_id: &str) -> Vec<(String, FieldSource)> {
    let d: SetDetail = srv
        .get(u, &routes::path(routes::SET, &[set_id]))
        .await
        .expect(200)
        .json();
    d.items.iter().map(|i| (i.text.clone(), i.text_source)).collect()
}

fn texts(v: &[(&str, FieldSource)]) -> Vec<(String, FieldSource)> {
    v.iter().map(|(t, s)| (t.to_string(), *s)).collect()
}

#[tokio::test]
async fn import_and_export_keep_the_text_and_its_source() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let report = import(&srv, &u, TEXTS, &FILES, None).await;
    assert_eq!(report.created, 4, "{report:?}");
    let set = report.set.unwrap();
    let want = texts(&[
        ("read by a machine", FieldSource::Ai),
        ("Кіт у мішку", FieldSource::User),
        ("", FieldSource::User),
        ("", FieldSource::Ai),
    ]);
    assert_eq!(texts_of(&srv, &u, &set.id).await, want);

    // The export writes them back, and nothing for the item without one.
    let tar = srv
        .get(&u, &routes::path(routes::SET_EXPORT, &[&set.id]))
        .await
        .expect(200);
    let bundle = stickpile_setfmt::read_tar(&tar.body, 1 << 30).unwrap();
    let mine = stickpile_setfmt::Manifest::parse(TEXTS).unwrap();
    for (got, want) in bundle.manifest.items.values().zip(mine.items.values()) {
        assert_eq!((&got.text, got.text_source), (&want.text, want.text_source));
    }
    let toml = bundle.manifest.to_toml();
    let unset = toml.split("4-unset.png").nth(1).unwrap();
    assert!(!unset.contains("text"), "{toml}");

    // Another server imports the export: the same texts.
    let bob = srv.user("bob").await;
    let rep = read(
        srv.client
            .post(srv.url(routes::SETS_IMPORT))
            .bearer_auth(&bob.token)
            .multipart(reqwest::multipart::Form::new().part(
                "archive",
                reqwest::multipart::Part::bytes(tar.body.clone()).file_name("texts.tar"),
            ))
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200)
    .json::<ImportReport>();
    assert_eq!(texts_of(&srv, &bob, &rep.set.unwrap().id).await, want);

    // Re-importing a set.toml without texts (an older server's, or written
    // by hand) changes no text.
    let plain = r#"
name = "Texts"
[items."1-ai.png"]
name = "ai"
[items."2-typed.png"]
name = "typed"
[items."3-cleared.png"]
name = "cleared"
[items."4-unset.png"]
name = "unset"
"#;
    import(&srv, &u, plain, &FILES, Some(&set.id)).await;
    assert_eq!(texts_of(&srv, &u, &set.id).await, want);

    // An AI text from a file never replaces a person's; a person's replaces
    // anything.
    let changed = r#"
name = "Texts"
[items."1-ai.png"]
name = "ai"
text = "fixed by hand"
[items."2-typed.png"]
name = "typed"
text = "a machine's guess"
text_source = "ai"
[items."3-cleared.png"]
name = "cleared"
text = "another guess"
text_source = "ai"
[items."4-unset.png"]
name = "unset"
text = "machine"
text_source = "ai"
"#;
    let rep = import(&srv, &u, changed, &FILES, Some(&set.id)).await;
    assert_eq!((rep.updated, rep.unchanged), (2, 2), "{rep:?}");
    assert_eq!(
        texts_of(&srv, &u, &set.id).await,
        texts(&[
            ("fixed by hand", FieldSource::User),
            ("Кіт у мішку", FieldSource::User),
            ("", FieldSource::User),
            ("machine", FieldSource::Ai),
        ])
    );
}

/// A 0.7.0 server keeps a newer set.toml's text keys as unknown keys
/// (`items.extra_toml`). After an upgrade they become the item's text,
/// unless a person's text is there.
#[tokio::test]
async fn texts_an_older_server_kept_as_unknown_keys_are_taken_over() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let a = srv.upload_one(&u, &set.id, "a.png", png(1)).await;
    let b = upload(&srv, &u, &set.id, "b.png", png(2), Some(json!({"text": "mine"}))).await;
    for (id, extra) in [
        (
            &a.id,
            "rating = 5\ntext = \"from an older server\"\ntext_source = \"ai\"\n",
        ),
        (&b.id, "text = \"robot\"\ntext_source = \"ai\"\n"),
    ] {
        sqlx::query("UPDATE items SET extra_toml = ?, updated_at = 1 WHERE id = ?")
            .bind(extra)
            .bind(id)
            .execute(&srv.state.db)
            .await
            .unwrap();
    }
    let n = stickpile_server::service::transfer::lift_text_from_extra(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(n, 2);
    let extra = |id: String| {
        let db = srv.state.db.clone();
        async move {
            sqlx::query_scalar::<_, Option<String>>("SELECT extra_toml FROM items WHERE id = ?")
                .bind(id)
                .fetch_one(&db)
                .await
                .unwrap()
        }
    };
    assert_eq!(extra(a.id.clone()).await.as_deref(), Some("rating = 5\n"));
    assert_eq!(extra(b.id.clone()).await, None);
    assert_eq!(
        text_of(&item(&srv, &u, &a.id).await),
        ("from an older server", FieldSource::Ai)
    );
    assert!(item(&srv, &u, &a.id).await.updated_at > 1, "client caches learn it");
    assert_eq!(text_of(&item(&srv, &u, &b.id).await), ("mine", FieldSource::User));
    assert_eq!(
        stickpile_server::service::transfer::lift_text_from_extra(&srv.state.db)
            .await
            .unwrap(),
        0,
        "once"
    );
}

#[tokio::test]
async fn migration_leaves_existing_items_without_a_text() {
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
        include_str!("../migrations/0012_set_publications.sql"),
        include_str!("../migrations/0013_discord_guilds.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&mut conn).await.unwrap();
    }
    sqlx::raw_sql(
        "INSERT INTO users (id, username, display_name, role, created_at) VALUES ('u1', 'alice', 'Alice', 'user', 1);
         INSERT INTO sets (id, owner_id, slug, name, created_at, updated_at) VALUES ('s1', 'u1', 'cats', 'Cats', 1, 1);
         INSERT INTO media (id, sha256, original_sha256, kind, format, width, height, has_alpha, size_bytes, phash,
                            dhash, created_at)
           VALUES ('m1', 'aa', 'aa', 'still', 'png', 1, 1, 0, 1, 0, 0, 1);
         INSERT INTO items (id, set_id, media_id, position, filename, created_at, updated_at)
           VALUES ('i1', 's1', 'm1', 0, 'cat', 1, 1);",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0014_item_text.sql"))
        .execute(&mut conn)
        .await
        .unwrap();
    let row: (String, String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT i.text, i.text_source, m.ocr_text, m.ocr_model FROM items i JOIN media m ON m.id = i.media_id",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert_eq!(row, (String::new(), "ai".into(), None, None), "not set, never read");
    assert!(sqlx::query("UPDATE items SET text_source = 'robot'")
        .execute(&mut conn)
        .await
        .is_err());
}

// ---------------------------------------------------------------------------
// Proposals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_accepted_text_proposal_is_the_owners_text() {
    let (srv, _mock) = ocr_server_with(false, |_| {}).await;
    let alice = srv.user("alice").await;
    let eve = srv.user("eve").await;
    let set = srv.create_set(&alice, "public", Visibility::Public).await;
    let cat = srv.upload_one(&alice, &set.id, "cat.png", png(1)).await;
    srv.run_jobs().await;
    assert_eq!(text_of(&item(&srv, &alice, &cat.id).await), (READ, FieldSource::Ai));
    let proposals = routes::path(routes::ITEM_PROPOSALS, &[&cat.id]);
    srv.post(&eve, &proposals, json!({"text": "NO THOUGHTS\r\nhead empty"}))
        .await
        .expect(400);
    let p: Proposal = srv
        .post(
            &eve,
            &proposals,
            json!({"message": "typo", "text": "no thoughts, head empty"}),
        )
        .await
        .expect(201)
        .json();
    assert_eq!(p.changes.text.as_deref(), Some("no thoughts, head empty"));
    assert_eq!(p.before.text.as_deref(), Some(READ));
    // Texts belong to stickers, not sets.
    srv.post(
        &eve,
        &routes::path(routes::SET_PROPOSALS, &[&set.id]),
        json!({"text": "x"}),
    )
    .await
    .expect(400);
    srv.post(&alice, &routes::path(routes::PROPOSAL_ACCEPT, &[&p.id]), json!({}))
        .await
        .expect(200);
    assert_eq!(
        text_of(&item(&srv, &alice, &cat.id).await),
        ("no thoughts, head empty", FieldSource::User)
    );
}
