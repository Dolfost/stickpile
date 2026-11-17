//! Review of the metadata round against rules 5 and 6 (branch
//! review-compat): each test here shows something the review found. They
//! fail on 5eb6528.

mod common;
mod web_common;

use std::sync::Arc;

use common::{png, Opts, TestServer, TestUser};
use reqwest::StatusCode;
use serde_json::json;
use stickpile_ai::MockTextReader;
use stickpile_api::{routes, FieldSource, Item, ItemDetail, Proposal, Visibility};
use web_common::Browser;

/// What the mock reader reads in every picture.
const READ: &str = "NO THOUGHTS\nhead empty";

/// A server that reads text (mock reader), without CLIP, so that only the
/// text changes behind a person's back.
async fn ocr_server() -> TestServer {
    let mock = Arc::new(MockTextReader::new(&["NO THOUGHTS", "head empty"]));
    TestServer::start_with(
        Opts {
            text_reader: Some(mock),
            ..Default::default()
        },
        |_| {},
    )
    .await
}

async fn item(srv: &TestServer, u: &TestUser, id: &str) -> Item {
    srv.get(u, &routes::path(routes::ITEM, &[id]))
        .await
        .expect(200)
        .json::<ItemDetail>()
        .item
}

fn text_of(i: &Item) -> (String, FieldSource) {
    (i.text.clone(), i.text_source)
}

/// Rule 6, the reverse problem: the no-JS edit form compares the posted
/// text with the item's text *now*, not with what the form showed. A
/// person who opened the form before the picture was read (the text was
/// empty then), and saves it after the reader filled it, posts the empty
/// text they were shown; the server takes that as the person clearing the
/// text, so the computer's text is gone and the item is `user` for good
/// (the reader never fills it again).
#[tokio::test]
async fn the_full_form_keeps_a_text_read_after_the_page_was_loaded() {
    let srv = ocr_server().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let it = srv.upload_one(&u, &set.id, "cat.png", png(1)).await;
    assert_eq!(text_of(&it), (String::new(), FieldSource::Ai), "not read yet");
    let b = Browser::login(&srv, &u).await;
    let page = format!("/items/{}", it.id);
    // The full form, opened before the picture is read: an empty text.
    let html = b.get(&format!("{page}?edit=1")).await.text();
    assert!(
        html.contains("<textarea id=\"all-text\" name=\"text\" rows=\"4\" maxlength=\"2000\"></textarea>"),
        "{html}"
    );
    // The reader reads the picture meanwhile.
    srv.run_jobs().await;
    assert_eq!(
        text_of(&item(&srv, &u, &it.id).await),
        (READ.to_string(), FieldSource::Ai)
    );
    // The person renames the sticker and saves the form as it was shown
    // (with the values it showed, as a browser sends them).
    let mut fields = vec![
        ("name", "Grumpy"),
        ("keywords", ""),
        ("emojis", ""),
        ("vibe", ""),
        ("tags", ""),
        ("text", ""),
    ];
    let was = shown(&html);
    fields.extend(was.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let r = b.post_form(&page, &fields).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let after = item(&srv, &u, &it.id).await;
    assert_eq!(after.name, "Grumpy");
    assert_eq!(
        text_of(&after),
        (READ.to_string(), FieldSource::Ai),
        "the person did not touch the text"
    );

    // The form opened now shows the text; cleared and saved, it is the
    // person's empty text.
    let html = b.get(&format!("{page}?edit=1")).await.text();
    let mut fields = vec![
        ("name", "Grumpy"),
        ("keywords", ""),
        ("emojis", ""),
        ("vibe", ""),
        ("tags", ""),
        ("text", ""),
    ];
    let was = shown(&html);
    assert!(was.contains(&("text_was".to_string(), READ.to_string())), "{was:?}");
    fields.extend(was.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let r = b.post_form(&page, &fields).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let after = item(&srv, &u, &it.id).await;
    assert_eq!(
        text_of(&after),
        (String::new(), FieldSource::User),
        "cleared by the person"
    );
    assert_eq!(after.name_source, FieldSource::User);
}

/// Rule 6, the same in "Suggest changes": a text the reader filled after
/// the form was shown is not proposed back as the empty text the form
/// still holds; one the proposer changes is.
#[tokio::test]
async fn the_suggest_form_proposes_only_what_was_changed_on_it() {
    let srv = ocr_server().await;
    let alice = srv.user("alice").await;
    let eve = srv.user("eve").await;
    let set = srv.create_set(&alice, "public", Visibility::Public).await;
    let cat = srv.upload_one(&alice, &set.id, "cat.png", png(1)).await;
    let b = Browser::login(&srv, &eve).await;
    let url = format!("/items/{}/suggest", cat.id);
    // Opened before the picture is read: an empty text.
    let html = b.get(&url).await.text();
    assert!(html.contains("aria-describedby=\"s-text-hint\"></textarea>"), "{html}");
    srv.run_jobs().await;
    assert_eq!(
        text_of(&item(&srv, &alice, &cat.id).await),
        (READ.to_string(), FieldSource::Ai)
    );
    let post = |html: String, name: &'static str, text: &'static str| {
        let b = &b;
        let url = &url;
        async move {
            let mut fields = vec![
                ("name", name.to_string()),
                ("keywords", String::new()),
                ("emojis", String::new()),
                ("tags", String::new()),
                ("vibe", String::new()),
                ("text", text.to_string()),
                ("message", String::new()),
            ];
            fields.extend(shown_suggest(&html));
            let fields: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
            b.post_form(url, &fields).await
        }
    };
    // Only the name changed on the form: only the name is proposed.
    let r = post(html, "Grumpy", "").await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let p = proposal_at(&srv, &eve, &r).await;
    assert_eq!(p.changes.name.as_deref(), Some("Grumpy"));
    assert_eq!(p.changes.text, None, "the text was not touched on the form");
    assert_eq!(p.changes.keywords, None);
    // Nothing changed on the form: nothing to propose.
    let html = b.get(&url).await.text();
    let r = post(html, "cat", READ).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    // Cleared on a form that showed the text: proposed.
    let html = b.get(&url).await.text();
    let r = post(html, "cat", "").await;
    let p = proposal_at(&srv, &eve, &r).await;
    assert_eq!(p.changes.text.as_deref(), Some(""));
    assert_eq!(p.changes.name, None);
}

/// The `{field}_was` values of the full item form in `html`.
fn shown(html: &str) -> Vec<(String, String)> {
    ["name", "keywords", "emojis", "vibe", "tags", "text"]
        .iter()
        .map(|k| {
            let key = format!("{k}_was");
            let v = web_common::hidden_value(html, &key).unwrap_or_else(|| panic!("{key} in {html}"));
            (key, v)
        })
        .collect()
}

/// The `{field}_was` values of the "Suggest changes" form in `html`.
fn shown_suggest(html: &str) -> Vec<(&'static str, String)> {
    [
        "name_was",
        "keywords_was",
        "emojis_was",
        "tags_was",
        "vibe_was",
        "text_was",
    ]
    .into_iter()
    .map(|k| {
        (
            k,
            web_common::hidden_value(html, k).unwrap_or_else(|| panic!("{k} in {html}")),
        )
    })
    .collect()
}

/// The proposal a form post `r` made (it redirects to its page).
async fn proposal_at(srv: &TestServer, u: &TestUser, r: &common::Resp) -> Proposal {
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let id = web_common::location(r)
        .strip_prefix("/proposals/")
        .expect("a proposal page")
        .to_string();
    srv.get(u, &routes::path(routes::PROPOSAL, &[&id]))
        .await
        .expect(200)
        .json()
}

/// Rule 6: an import into an existing set (and a git pull, which uses the
/// same `transfer::import_bundle_into`) reads the set's items once, before
/// its loop (`hydrate::fetch_by_set`), and `apply_meta` later writes the
/// text `ItemText::merged_into` worked out from that snapshot with an
/// unguarded UPDATE. A text a person types while the import runs (here:
/// while it waits for the media lock to add a new file) is replaced by the
/// file's AI text, and marked `ai`.
#[tokio::test]
async fn an_import_does_not_overwrite_a_text_typed_while_it_runs() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let x = srv.upload_one(&u, &set.id, "x.png", png(2)).await;
    assert_eq!(text_of(&x), (String::new(), FieldSource::Ai));
    let manifest = r#"
name = "s"

[items."1-new.png"]
name = "new"

[items."2-x.png"]
name = "x"
text = "a machine's guess"
text_source = "ai"
"#;
    let form = reqwest::multipart::Form::new()
        .text("set.toml", manifest.to_string())
        .part(
            "file",
            reqwest::multipart::Part::bytes(png(1)).file_name("s/1-new.png".to_string()),
        )
        .part(
            "file",
            reqwest::multipart::Part::bytes(png(2)).file_name("s/2-x.png".to_string()),
        );
    // Media work waits: the import stops at its new file, after it read
    // the set's items.
    let lock = srv.state.media_lock.write().await;
    let rb = srv
        .client
        .post(format!("{}?into={}", srv.url(routes::SETS_IMPORT), set.id))
        .bearer_auth(&u.token)
        .multipart(form);
    let import = tokio::spawn(async move { common::read(rb.send().await.unwrap()).await });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    // The person types the text meanwhile.
    let typed: Item = srv
        .patch(&u, &routes::path(routes::ITEM, &[&x.id]), json!({"text": "mine"}))
        .await
        .expect(200)
        .json();
    assert_eq!(text_of(&typed), ("mine".to_string(), FieldSource::User));
    drop(lock);
    import.await.unwrap().expect(200);
    assert_eq!(
        text_of(&item(&srv, &u, &x.id).await),
        ("mine".to_string(), FieldSource::User),
        "an AI text from a file never replaces a person's"
    );
}

/// Rule 6 (and W2: the text travels with the upload metadata): a proposal
/// of new stickers takes `meta` "as for uploads", but the text in it is
/// dropped (`proposal_items` has no text, `add_proposed` builds an
/// `ItemInput` without it). Accepted, the sticker gets the reader's text
/// instead of the words the proposer typed.
#[tokio::test]
async fn a_proposed_stickers_text_is_kept() {
    let srv = ocr_server().await;
    let alice = srv.user("alice").await;
    let eve = srv.user("eve").await;
    let set = srv.create_set(&alice, "public", Visibility::Public).await;
    let form = reqwest::multipart::Form::new()
        .text("message", "a cat")
        .part(
            "file",
            reqwest::multipart::Part::bytes(png(1)).file_name("cat.png".to_string()),
        )
        .text("meta", json!([{"name": "cat", "text": "typed by eve"}]).to_string());
    let rb = srv
        .client
        .post(srv.url(&routes::path(routes::SET_PROPOSAL_ITEMS, &[&set.id])))
        .bearer_auth(&eve.token);
    let p: Proposal = common::read(rb.multipart(form).send().await.unwrap())
        .await
        .expect(201)
        .json();
    let p: Proposal = srv
        .post(&alice, &routes::path(routes::PROPOSAL_ACCEPT, &[&p.id]), json!({}))
        .await
        .expect(200)
        .json();
    srv.run_jobs().await;
    let id = p.items[0].item_id.clone().expect("accepted into an item");
    assert_eq!(
        text_of(&item(&srv, &alice, &id).await),
        ("typed by eve".to_string(), FieldSource::User),
        "the words the proposer typed"
    );
}
