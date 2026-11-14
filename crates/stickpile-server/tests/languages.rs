//! Languages (docs/languages.md): the account's choice, Accept-Language,
//! the page language, translated error messages and the scripts' texts.

mod common;
mod web_common;

use common::TestServer;
use reqwest::StatusCode;
use serde_json::json;
use stickpile_api::{routes, User};
use web_common::*;

async fn get_with_lang(srv: &TestServer, path: &str, accept: &str, token: Option<&str>) -> common::Resp {
    let mut rb = srv.client.get(srv.url(path)).header("Accept-Language", accept);
    if let Some(t) = token {
        rb = rb.bearer_auth(t);
    }
    common::read(rb.send().await.unwrap()).await
}

#[tokio::test]
async fn the_account_language_round_trips() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let me: User = srv.get(&alice, routes::ME).await.expect(200).json();
    assert_eq!(me.language, None, "automatic by default");
    let me: User = srv
        .patch(&alice, routes::ME, json!({"language": "uk"}))
        .await
        .expect(200)
        .json();
    assert_eq!(me.language.as_deref(), Some("uk"));
    // Other fields leave it alone; null goes back to automatic.
    let me: User = srv
        .patch(&alice, routes::ME, json!({"theme": "dark"}))
        .await
        .expect(200)
        .json();
    assert_eq!(me.language.as_deref(), Some("uk"));
    let r = srv
        .patch(&alice, routes::ME, json!({"language": "de"}))
        .await
        .expect(400);
    assert!(r.text().contains("unknown language"), "{}", r.text());
    let me: User = srv
        .patch(&alice, routes::ME, json!({"language": null}))
        .await
        .expect(200)
        .json();
    assert_eq!(me.language, None);
}

#[tokio::test]
async fn requests_are_answered_in_their_language() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    // Anonymous: the browser's language.
    let r = get_with_lang(&srv, "/api/v1/sets/nope", "uk-UA,uk;q=0.9,en;q=0.8", None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.header("content-language").as_deref(), Some("uk"));
    assert_eq!(r.value()["message"], "потрібна автентифікація");
    let r = get_with_lang(&srv, "/api/v1/sets/nope", "de, fr", None).await;
    assert_eq!(r.header("content-language").as_deref(), Some("en"));
    assert_eq!(r.value()["message"], "authentication required");
    // Signed in: the account's choice wins over the header.
    let r = get_with_lang(
        &srv,
        "/api/v1/sets/zzzzzzzzzzzzzzzzzzzzzzzzzz",
        "uk",
        Some(&alice.token),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.value()["message"], "не знайдено", "automatic follows the browser");
    srv.patch(&alice, routes::ME, json!({"language": "en"}))
        .await
        .expect(200);
    let r = get_with_lang(
        &srv,
        "/api/v1/sets/zzzzzzzzzzzzzzzzzzzzzzzzzz",
        "uk",
        Some(&alice.token),
    )
    .await;
    assert_eq!(r.header("content-language").as_deref(), Some("en"));
    assert_eq!(r.value()["message"], "not found");
}

#[tokio::test]
async fn pages_follow_the_language() {
    let srv = TestServer::start().await;
    let alice_u = srv.user("alice").await;
    let anon = get_with_lang(&srv, "/login", "uk", None).await;
    assert!(
        anon.text().contains("<html lang=\"uk\""),
        "the login page in the browser's language"
    );
    assert!(anon.text().contains("Увійти"));
    let en = get_with_lang(&srv, "/login", "en", None).await;
    assert!(en.text().contains("<html lang=\"en\""));
    assert!(!en.text().contains("data-i18n"), "English needs no script texts");

    // The account's choice, set from the settings page.
    let alice = Browser::login(&srv, &alice_u).await;
    let r = alice.post_form("/settings/language", &[("language", "uk")]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let page = alice.get("/settings").await;
    assert_page(&page);
    let html = page.text();
    assert!(html.contains("<html lang=\"uk\""));
    assert!(html.contains("data-i18n="), "the scripts get their texts");
    assert!(html.contains("Налаштування"), "the nav is translated");
    assert!(html.contains("selected lang=\"uk\""), "the select shows the choice");
    let r = alice.post_form("/settings/language", &[("language", "")]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let me: User = srv.get(&alice_u, routes::ME).await.expect(200).json();
    assert_eq!(me.language, None);
}
