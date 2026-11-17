//! Rechecks in the web UI: "Recheck my stickers" in the settings (start,
//! the errors in the form, recent rechecks), a recheck's page (progress
//! that polls itself, the fragment htmx asks for, cancel, the result and
//! the failed pictures), "Recheck this set" on a set page, and the admin
//! tab with its access rules. The server here has no AI and no text
//! reader, so quality and renditions are the parts on offer.

mod common;
mod web_common;

use common::{png, TestServer};
use reqwest::StatusCode;
use serde_json::json;
use stickpile_api::{routes, AuditEntry, Visibility};
use web_common::*;

fn recheck_id(r: &common::Resp) -> String {
    let loc = location(r);
    loc.strip_prefix("/rechecks/")
        .unwrap_or_else(|| panic!("not a recheck: {loc}"))
        .to_string()
}

async fn sql(s: &TestServer, q: &str, binds: &[&str]) {
    let mut query = sqlx::query(q);
    for b in binds {
        query = query.bind(*b);
    }
    query.execute(&s.state.db).await.unwrap();
}

#[tokio::test]
async fn recheck_my_stickers_from_the_settings() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let set = s.create_set(&alice, "cats", Visibility::Private).await;
    for seed in 1..=2 {
        s.upload_one(&alice, &set.id, "a.png", png(seed)).await;
    }
    s.run_jobs().await;
    let b = Browser::login(&s, &alice).await;

    // The form offers what this server can do, and says why not the rest.
    let page = b.get("/settings").await;
    assert_page(&page);
    let html = page.text();
    assert!(html.contains("id=\"recheck\""), "{html}");
    assert!(html.contains("Recheck my stickers"));
    assert!(html.contains("name=\"part.quality\"") && html.contains("name=\"part.renditions\""));
    assert!(!html.contains("name=\"part.keywords\"") && !html.contains("name=\"part.text\""));
    assert!(html.contains("AI is off on this server"), "{html}");
    assert!(html.contains("does not read the words in pictures"));
    assert!(html.contains("name=\"mode\" value=\"missing\" checked"));
    assert!(html.contains(&format!("<option value=\"{}\">cats</option>", set.id)));
    assert!(!html.contains("Recent rechecks"));

    // Start: the recheck's page, running, polling itself; a meta refresh
    // without JavaScript.
    let r = b
        .post_form("/settings/recheck", &[("part.quality", "on"), ("mode", "refresh")])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let id = recheck_id(&r);
    let page = b.get(&format!("/rechecks/{id}")).await;
    assert_page(&page);
    let html = page.text();
    assert!(html.contains("The stickers of all your sets."), "{html}");
    assert!(html.contains("Quality score") && html.contains("Everything again"));
    assert!(html.contains(&format!("hx-get=\"/rechecks/{id}\"")) && html.contains("hx-trigger=\"every 2s\""));
    assert!(
        html.contains("<noscript><meta http-equiv=\"refresh\" content=\"3\">"),
        "{html}"
    );
    assert!(html.contains(&format!("action=\"/rechecks/{id}/cancel\"")));
    assert!(html.contains("<progress max=\"2\" value=\"0\""), "{html}");

    // The fragment htmx polls, until it is over.
    let frag = b.get_htmx(&format!("/rechecks/{id}")).await.expect(200);
    assert_fragment(&frag);
    assert!(
        frag.text().starts_with("<div id=\"recheck-progress\""),
        "{}",
        frag.text()
    );
    s.run_jobs().await;
    let frag = b.get_htmx(&format!("/rechecks/{id}")).await.expect(200);
    assert_fragment(&frag);
    let html = frag.text();
    assert!(!html.contains("hx-trigger"), "no more polling: {html}");
    assert!(
        html.contains("Finished.") && html.contains("<progress max=\"2\" value=\"2\""),
        "{html}"
    );
    assert!(!html.contains("/cancel"));

    // Listed in the settings.
    let html = b.get("/settings").await.text();
    assert!(html.contains("Recent rechecks") && html.contains(&format!("href=\"/rechecks/{id}\"")));

    // Nothing ticked: the error in the form.
    let r = b.post_form("/settings/recheck", &[("mode", "missing")]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_page_status(&r);
    assert!(
        r.text().contains("Choose at least one thing to recheck"),
        "{}",
        r.text()
    );
    // A part this server cannot do: 503 in the form.
    let r = b.post_form("/settings/recheck", &[("part.keywords", "on")]).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(r.text().contains("AI is turned off on this server"), "{}", r.text());
    assert!(r.text().contains("id=\"recheck\""));
    // Someone else's set: refused.
    let bob = s.user("bob").await;
    let theirs = s.create_set(&bob, "dogs", Visibility::Public).await;
    let r = b
        .post_form("/settings/recheck", &[("part.quality", "on"), ("set_id", &theirs.id)])
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.text().contains("Only the owner can do this"), "{}", r.text());
    // Not theirs to see.
    let bob_b = Browser::login(&s, &bob).await;
    bob_b.get(&format!("/rechecks/{id}")).await.expect(404);
}

#[tokio::test]
async fn the_rate_limit_shows_in_the_form() {
    let s = TestServer::with_config(|c| c.limits.recheck_per_hour = 1).await;
    let alice = s.user("alice").await;
    let b = Browser::login(&s, &alice).await;
    let r = b.post_form("/settings/recheck", &[("part.quality", "on")]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let r = b
        .post_form("/settings/recheck", &[("part.renditions", "on"), ("mode", "refresh")])
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(r.header("retry-after").is_some());
    let html = r.text();
    assert!(html.contains("Too many requests, slow down"), "{html}");
    // The form keeps what was sent.
    assert!(html.contains("name=\"part.renditions\" value=\"on\" checked"), "{html}");
    assert!(html.contains("name=\"part.quality\" value=\"on\">"), "{html}");
    assert!(html.contains("name=\"mode\" value=\"refresh\" checked"), "{html}");
    // In Ukrainian too.
    s.patch(&alice, routes::ME, json!({"language": "uk"})).await.expect(200);
    let r = b.post_form("/settings/recheck", &[("part.renditions", "on")]).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    let html = r.text();
    assert!(html.contains("Повторно перевірити мої стікери"), "{html}");
    assert!(html.contains("Оцінка якості") && html.contains("Лише те, чого бракує"));
}

#[tokio::test]
async fn cancel_and_the_failed_pictures() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let set = s.create_set(&alice, "cats", Visibility::Private).await;
    let mut items = Vec::new();
    for seed in 1..=3 {
        items.push(
            s.upload_one(&alice, &set.id, &format!("cat{seed}.png"), png(seed))
                .await,
        );
    }
    s.run_jobs().await;
    let b = Browser::login(&s, &alice).await;

    // Cancel without JavaScript: back to the page, cancelled.
    let r = b
        .post_form("/settings/recheck", &[("part.quality", "on"), ("mode", "refresh")])
        .await;
    let id = recheck_id(&r);
    let r = b.post_form(&format!("/rechecks/{id}/cancel"), &[]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(location(&r), format!("/rechecks/{id}"));
    let html = b.get(&format!("/rechecks/{id}")).await.text();
    assert!(
        html.contains("cancelled") && html.contains("Cancelled. Pictures"),
        "{html}"
    );
    assert!(!html.contains("every 2s"));
    // With htmx: the progress box comes back.
    let r = b
        .post_form("/settings/recheck", &[("part.renditions", "on"), ("mode", "refresh")])
        .await;
    let id = recheck_id(&r);
    let r = b.post_htmx(&format!("/rechecks/{id}/cancel"), &[]).await.expect(200);
    assert_fragment(&r);
    assert!(r.text().contains("Cancelled. Pictures"));

    // A picture whose original is gone fails; the list links to the item.
    sql(
        &s,
        "DELETE FROM media_variants WHERE media_id = ? AND variant = 'original'",
        &[&items[1].media.id],
    )
    .await;
    let r = b
        .post_form("/settings/recheck", &[("part.quality", "on"), ("mode", "refresh")])
        .await;
    let id = recheck_id(&r);
    s.run_jobs().await;
    let page = b.get(&format!("/rechecks/{id}")).await;
    assert_page(&page);
    let html = page.text();
    assert!(html.contains("Failed pictures"), "{html}");
    assert!(
        html.contains(&format!("<a href=\"/items/{}\">cat2</a>", items[1].id)),
        "{html}"
    );
    assert!(html.contains("original file missing"));
    assert!(html.contains("Finished."));
}

#[tokio::test]
async fn recheck_this_set_from_the_set_page() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let bob = s.user("bob").await;
    let mine = s.create_set(&alice, "cats", Visibility::Public).await;
    let other = s.create_set(&alice, "dogs", Visibility::Private).await;
    s.upload_one(&alice, &mine.id, "a.png", png(1)).await;
    s.upload_one(&alice, &other.id, "b.png", png(2)).await;
    s.run_jobs().await;
    let b = Browser::login(&s, &alice).await;
    let link = format!("/settings?recheck_set={}#recheck", mine.id);
    let html = b.get(&format!("/sets/{}", mine.id)).await.text();
    assert!(
        html.contains(&format!("href=\"{}\"", link.replace('&', "&amp;"))),
        "{html}"
    );
    // Others do not get the button.
    let bob_b = Browser::login(&s, &bob).await;
    assert!(!bob_b
        .get(&format!("/sets/{}", mine.id))
        .await
        .text()
        .contains("recheck_set"));

    let html = b.get(&link).await.text();
    assert!(
        html.contains(&format!("<option value=\"{}\" selected>cats</option>", mine.id)),
        "{html}"
    );
    let r = b
        .post_form(
            "/settings/recheck",
            &[("part.quality", "on"), ("mode", "refresh"), ("set_id", &mine.id)],
        )
        .await;
    let id = recheck_id(&r);
    let api: serde_json::Value = s
        .get(&alice, &routes::path(routes::RECHECK, &[&id]))
        .await
        .expect(200)
        .value();
    assert_eq!(
        (api["total"].as_u64(), api["set_id"].as_str()),
        (Some(1), Some(mine.id.as_str()))
    );
    let html = b.get(&format!("/rechecks/{id}")).await.text();
    assert!(html.contains("The stickers of your set &quot;cats&quot;."), "{html}");
}

#[tokio::test]
async fn the_admin_tab_starts_and_lists_server_wide_rechecks() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let root = s.admin("root").await;
    let set = s.create_set(&alice, "cats", Visibility::Private).await;
    let item = s.upload_one(&alice, &set.id, "a.png", png(1)).await;
    s.run_jobs().await;

    // Not for users.
    let b = Browser::login(&s, &alice).await;
    b.get("/admin/rechecks").await.expect(403);
    b.post_form("/admin/rechecks", &[("part.quality", "on")])
        .await
        .expect(403);

    let a = Browser::login(&s, &root).await;
    let page = a.get("/admin/rechecks").await;
    assert_page(&page);
    let html = page.text();
    assert!(html.contains("aria-current=\"page\">Rechecks"), "{html}");
    assert!(html.contains("Recheck every sticker") && html.contains("None yet."));
    // A batch the server started by itself, after a model change.
    sql(
        &s,
        "INSERT INTO rechecks (id, scope, parts, mode, state, total, done, created_at, finished_at) \
         VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaa', 'server', '[\"embedding\"]', 'missing', 'done', 1, 1, 1, 2)",
        &[],
    )
    .await;

    let r = a
        .post_form("/admin/rechecks", &[("part.quality", "on"), ("mode", "refresh")])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let id = recheck_id(&r);
    let html = a.get(&format!("/rechecks/{id}")).await.text();
    assert!(html.contains("Every sticker on the server."), "{html}");
    assert!(html.contains("href=\"/admin/rechecks\""), "back to the admin tab");
    let audit: Vec<AuditEntry> = s.get(&root, routes::ADMIN_AUDIT).await.expect(200).json();
    assert!(audit
        .iter()
        .any(|e| e.action == "recheck.start" && e.target.as_deref() == Some(id.as_str())));
    let html = a.get("/admin/rechecks").await.text();
    assert!(
        html.contains(&format!("href=\"/rechecks/{id}\"")) && html.contains(">root<"),
        "{html}"
    );
    assert!(
        html.contains("href=\"/rechecks/aaaaaaaaaaaaaaaaaaaaaaaaaa\"") && html.contains(">the server<"),
        "{html}"
    );
    // A user cannot open it.
    b.get(&format!("/rechecks/{id}")).await.expect(404);

    // Failures of a server-wide recheck name the picture, not an item.
    sql(
        &s,
        "DELETE FROM media_variants WHERE media_id = ? AND variant = 'original'",
        &[&item.media.id],
    )
    .await;
    let r = a
        .post_form("/admin/rechecks", &[("part.quality", "on"), ("mode", "refresh")])
        .await;
    let id = recheck_id(&r);
    s.run_jobs().await;
    let html = a.get(&format!("/rechecks/{id}")).await.text();
    assert!(html.contains("Every picture failed"), "{html}");
    assert!(html.contains(&format!("<code>{}</code>", item.media.id)), "{html}");
    assert!(!html.contains(&format!("/items/{}", item.id)));
    // Errors come back in the form.
    let r = a.post_form("/admin/rechecks", &[("mode", "refresh")]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("Choose at least one thing to recheck"));
    let r = a
        .post_form("/admin/rechecks", &[("part.quality", "on"), ("set_id", "nosuchset")])
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

/// With AI the form offers the AI's parts, and the result says that new
/// values are AI suggestions to confirm.
#[tokio::test]
async fn new_values_are_called_ai_suggestions() {
    // A vocabulary the mock enricher matches a red picture against.
    let dir = tempfile::tempdir().unwrap();
    let vocab = dir.path().join("vocab.toml");
    std::fs::write(
        &vocab,
        "keywords = [\"red\", \"blue\"]\n\n[[emojis]]\nemoji = \"\u{1f534}\"\ndescription = \"red\"\n\n\
         [[vibes]]\nlabel = \"angry\"\nsynonyms = [\"red\"]\n\n[[vibes]]\nlabel = \"sad\"\nsynonyms = [\"blue\"]\n",
    )
    .unwrap();
    let s = TestServer::start_with(
        common::Opts {
            ai: true,
            ..Default::default()
        },
        move |c| c.ai.vocabulary_file = Some(vocab),
    )
    .await;
    let alice = s.user("alice").await;
    let set = s.create_set(&alice, "cats", Visibility::Private).await;
    let item = s
        .upload_one(&alice, &set.id, "red.png", common::solid_png([230, 20, 20], 1))
        .await;
    s.run_jobs().await;
    s.patch(
        &alice,
        &routes::path(routes::ITEM, &[&item.id]),
        json!({"keywords": []}),
    )
    .await
    .expect(200);
    let b = Browser::login(&s, &alice).await;
    let html = b.get("/settings").await.text();
    for part in ["keywords", "emojis", "vibe", "embedding", "quality", "renditions"] {
        assert!(html.contains(&format!("name=\"part.{part}\"")), "{part}: {html}");
    }
    assert!(!html.contains("AI is off on this server"));
    let r = b.post_form("/settings/recheck", &[("part.keywords", "on")]).await;
    let id = recheck_id(&r);
    s.run_jobs().await;
    let html = b.get(&format!("/rechecks/{id}")).await.text();
    assert!(
        html.contains("Keywords") && html.contains("Only what is missing"),
        "{html}"
    );
    assert!(
        html.contains("New keywords, emojis, vibes and texts are AI suggestions"),
        "{html}"
    );
    let item: serde_json::Value = s
        .get(&alice, &routes::path(routes::ITEM, &[&item.id]))
        .await
        .expect(200)
        .value();
    assert_eq!(item["item"]["keywords_source"], "ai");
}

/// A form page with an error status: the HTML shell, CSP and clean markup.
#[track_caller]
fn assert_page_status(r: &common::Resp) {
    let html = r.text();
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(r.header("content-security-policy").is_some());
    assert_clean_html(&html);
}
