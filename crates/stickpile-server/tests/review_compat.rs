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
    assert_eq!(text_of(&item(&srv, &u, &it.id).await), (READ.to_string(), FieldSource::Ai));
    // The person renames the sticker and saves the form as it was shown.
    let r = b
        .post_form(
            &page,
            &[
                ("name", "Grumpy"),
                ("keywords", ""),
                ("emojis", ""),
                ("vibe", ""),
                ("tags", ""),
                ("text", ""),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let after = item(&srv, &u, &it.id).await;
    assert_eq!(after.name, "Grumpy");
    assert_eq!(
        text_of(&after),
        (READ.to_string(), FieldSource::Ai),
        "the person did not touch the text"
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
