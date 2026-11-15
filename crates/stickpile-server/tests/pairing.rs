//! Pairing a new device with a one-time code: `POST /api/v1/pairings`
//! (signed in) and `POST /api/v1/auth/pair` (anonymous), and Settings ->
//! "Add a device" in the web UI.

mod common;
mod web_common;

use common::{read, Resp, TestServer, TestUser};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use stickpile_api::{pairing, routes, AuditEntry, PairingCreated, TokenCreated, User};
use web_common::*;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

async fn create_code(srv: &TestServer, user: &TestUser) -> PairingCreated {
    srv.post(user, routes::PAIRINGS, json!({})).await.expect(201).json()
}

async fn exchange(srv: &TestServer, code: &str, name: &str) -> Resp {
    srv.send(
        None,
        Method::POST,
        routes::AUTH_PAIR,
        Some(json!({"code": code, "token_name": name})),
    )
    .await
}

async fn me_with(srv: &TestServer, token: &str) -> Resp {
    read(
        srv.client
            .get(srv.url(routes::ME))
            .bearer_auth(token)
            .send()
            .await
            .unwrap(),
    )
    .await
}

#[derive(Debug, sqlx::FromRow)]
struct Entry {
    action: String,
    actor_id: Option<String>,
    target: Option<String>,
    details: Option<String>,
}

impl Entry {
    fn details(&self) -> Value {
        self.details
            .as_deref()
            .map(|d| serde_json::from_str(d).unwrap())
            .unwrap_or(Value::Null)
    }
}

/// Every audit entry, oldest first.
async fn audit(srv: &TestServer) -> Vec<Entry> {
    sqlx::query_as("SELECT action, actor_id, target, details FROM audit_log ORDER BY id")
        .fetch_all(&srv.state.db)
        .await
        .unwrap()
}

/// A code of the right shape that the server did not make (with
/// overwhelming probability).
fn made_up_code() -> String {
    pairing::display(&pairing::code_from_bits(0x0123_4567_89AB_CDEF))
}

#[tokio::test]
async fn creating_a_code_needs_a_signed_in_user() {
    let srv = TestServer::with_config(|c| c.server.public_url = "https://stickers.example.ts.net/".into()).await;
    let r = srv.send(None, Method::POST, routes::PAIRINGS, Some(json!({}))).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.text());
    assert_eq!(r.code(), "unauthorized");
    let r = read(
        srv.client
            .post(srv.url(routes::PAIRINGS))
            .bearer_auth("spk_not-a-real-token")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    let alice = srv.user("alice").await;
    let before = now();
    let created = create_code(&srv, &alice).await;
    // Display form: three groups of four, canonical characters.
    assert_eq!(created.code.len(), 14, "{}", created.code);
    assert_eq!(created.code.matches('-').count(), 2);
    let canonical = pairing::normalize(&created.code).expect("a valid code");
    assert_eq!(pairing::display(&canonical), created.code);
    assert!((before + 300..=now() + 300).contains(&created.expires_at));
    // The pairing link: the server's public URL and the code, URL-encoded.
    assert!(
        created
            .pairing_url
            .starts_with("stickpile://pair?url=https%3A%2F%2Fstickers.example.ts.net&code="),
        "{}",
        created.pairing_url
    );
    assert!(created.pairing_url.ends_with(&format!("&code={}", created.code)));
    let link = pairing::parse_link(&created.pairing_url).unwrap();
    assert_eq!(link.server_url, "https://stickers.example.ts.net");
    assert_eq!(link.code, canonical);
    // Each request makes a new code.
    assert_ne!(create_code(&srv, &alice).await.code, created.code);
}

#[tokio::test]
async fn a_code_gives_a_working_token_once() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let _bob = srv.user("bob").await;
    let created = create_code(&srv, &alice).await;

    let r = exchange(&srv, &created.code, "Pixel 8").await.expect(200);
    let token: TokenCreated = r.json();
    assert!(token.token.starts_with("spk_"));
    assert_eq!(token.user.id, alice.id, "the account that created the code");
    assert_eq!(token.info.name, "Pixel 8");
    assert_eq!(token.info.expires_at, None);
    let me: User = me_with(&srv, &token.token).await.expect(200).json();
    assert_eq!(me.username, "alice");
    let list = srv.get(&alice, routes::TOKENS).await.expect(200).value();
    assert!(list.as_array().unwrap().iter().any(|t| t["name"] == "Pixel 8"));

    // Single use.
    let again = exchange(&srv, &created.code, "Pixel 8 again").await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);
    assert_eq!(again.code(), "unauthorized");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM api_tokens WHERE name LIKE 'Pixel 8%'")
            .fetch_one(&srv.state.db)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn typed_codes_are_normalised() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let variants: [fn(&str) -> String; 6] = [
        |c: &str| c.to_lowercase(),
        |c: &str| c.replace('-', " "),
        |c: &str| c.replace('-', ""),
        |c: &str| format!("  {}  ", c.replace('-', " - ")),
        // What a person may read instead: O for 0, I or L for 1.
        |c: &str| c.replace('0', "O").replace('1', "l"),
        |c: &str| c.replace('1', "I").to_lowercase(),
    ];
    for variant in variants {
        let created = create_code(&srv, &alice).await;
        let typed = variant(&created.code);
        let r = exchange(&srv, &typed, "typed").await;
        assert_eq!(r.status, StatusCode::OK, "{typed:?} for {}: {}", created.code, r.text());
    }
}

#[tokio::test]
async fn wrong_expired_and_used_codes_look_the_same() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;

    let used = create_code(&srv, &alice).await;
    exchange(&srv, &used.code, "first").await.expect(200);
    let expired = create_code(&srv, &alice).await;
    sqlx::query("UPDATE pairing_codes SET expires_at = ? WHERE code_hash = ?")
        .bind(now() - 1)
        .bind(stickpile_server::auth::pairing::hash(
            &pairing::normalize(&expired.code).unwrap(),
        ))
        .execute(&srv.state.db)
        .await
        .unwrap();
    let disabled_user = srv.user("dora").await;
    let of_disabled = create_code(&srv, &disabled_user).await;
    sqlx::query("UPDATE users SET disabled = 1 WHERE id = ?")
        .bind(&disabled_user.id)
        .execute(&srv.state.db)
        .await
        .unwrap();

    let mut answers = Vec::new();
    for code in [
        used.code.clone(),
        expired.code.clone(),
        made_up_code(),
        of_disabled.code.clone(),
        "not a code at all".to_string(),
        String::new(),
        "x".repeat(10_000),
    ] {
        let r = exchange(&srv, &code, "phone").await;
        answers.push((r.status, r.header("content-type"), r.body.clone()));
    }
    let first = &answers[0];
    assert_eq!(first.0, StatusCode::UNAUTHORIZED);
    let body: Value = serde_json::from_slice(&first.2).unwrap();
    assert_eq!(body["code"], "unauthorized");
    assert_eq!(body["message"], "invalid or expired pairing code");
    for (i, a) in answers.iter().enumerate() {
        assert_eq!(a, first, "answer {i} differs");
    }
    // No token was issued for any of them (the disabled account's code
    // included).
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM api_tokens WHERE name = 'phone'")
            .fetch_one(&srv.state.db)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn a_bad_token_name_does_not_spend_the_code() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let created = create_code(&srv, &alice).await;
    let r = exchange(&srv, &created.code, "   ").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    exchange(&srv, &created.code, &"n".repeat(101)).await.expect(400);
    exchange(&srv, &created.code, "tablet").await.expect(200);
}

#[tokio::test]
async fn two_exchanges_at_once_get_one_token() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let created = create_code(&srv, &alice).await;
    let mut tasks = Vec::new();
    for i in 0..8 {
        let client = srv.client.clone();
        let url = srv.url(routes::AUTH_PAIR);
        let code = created.code.clone();
        tasks.push(tokio::spawn(async move {
            client
                .post(url)
                .json(&json!({"code": code, "token_name": format!("racer {i}")}))
                .send()
                .await
                .unwrap()
                .status()
        }));
    }
    let mut ok = 0;
    for t in tasks {
        match t.await.unwrap() {
            StatusCode::OK => ok += 1,
            StatusCode::UNAUTHORIZED => {}
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!(ok, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM api_tokens WHERE name LIKE 'racer %'")
            .fetch_one(&srv.state.db)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn exchanges_are_rate_limited_like_logins() {
    let srv = TestServer::with_config(|c| c.limits.login_per_minute = 3).await;
    let alice = srv.user("alice").await;
    let created = create_code(&srv, &alice).await;
    for _ in 0..3 {
        exchange(&srv, &made_up_code(), "x").await.expect(401);
    }
    let r = exchange(&srv, &created.code, "x").await.expect(429);
    assert_eq!(r.code(), "rate_limited");
    let retry: u64 = r.header("retry-after").expect("Retry-After").parse().unwrap();
    assert!((1..=60).contains(&retry));
    // The same per-IP budget as password logins.
    let r = srv
        .send(
            None,
            Method::POST,
            routes::AUTH_TOKEN,
            Some(json!({"username": "alice", "password": alice.password, "token_name": "x"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    // Refused before the code was looked at: it is still unused.
    let used: Option<i64> = sqlx::query_scalar("SELECT used_at FROM pairing_codes")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(used, None);
}

#[tokio::test]
async fn creation_exchange_and_failures_are_audited() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let created = create_code(&srv, &alice).await;
    let token: TokenCreated = exchange(&srv, &created.code, "phone").await.expect(200).json();
    exchange(&srv, &created.code, "phone").await.expect(401);
    exchange(&srv, &made_up_code(), "phone").await.expect(401);
    let expired = create_code(&srv, &alice).await;
    sqlx::query("UPDATE pairing_codes SET expires_at = ? WHERE used_at IS NULL")
        .bind(now() - 1)
        .execute(&srv.state.db)
        .await
        .unwrap();
    exchange(&srv, &expired.code, "phone").await.expect(401);

    let pairing_ids: Vec<String> = sqlx::query_scalar("SELECT id FROM pairing_codes ORDER BY created_at, rowid")
        .fetch_all(&srv.state.db)
        .await
        .unwrap();
    let entries = audit(&srv).await;
    let created_entries: Vec<_> = entries.iter().filter(|e| e.action == "pairing.create").collect();
    assert_eq!(created_entries.len(), 2);
    for (e, id) in created_entries.iter().zip(&pairing_ids) {
        assert_eq!(e.actor_id.as_deref(), Some(alice.id.as_str()));
        assert_eq!(e.target.as_deref(), Some(id.as_str()));
        assert!(e.details()["expires_at"].as_i64().unwrap() > now());
    }
    let login = entries
        .iter()
        .find(|e| e.action == "login" && e.details()["method"] == "pairing")
        .expect("login entry");
    assert_eq!(login.actor_id.as_deref(), Some(alice.id.as_str()));
    assert_eq!(login.details()["pairing"], pairing_ids[0].as_str());
    let tok = entries
        .iter()
        .find(|e| e.action == "token.create" && e.details()["via"] == "pairing")
        .expect("token.create entry");
    assert_eq!(tok.target.as_deref(), Some(token.info.id.as_str()));
    assert_eq!(tok.details()["name"], "phone");
    let failures: Vec<(String, Option<String>)> = entries
        .iter()
        .filter(|e| e.action == "login_failed")
        .map(|e| {
            let d = e.details();
            assert_eq!(d["method"], "pairing");
            (d["reason"].as_str().unwrap().to_string(), e.actor_id.clone())
        })
        .collect();
    let alice_id = Some(alice.id.clone());
    assert_eq!(
        failures,
        [
            ("used_code".to_string(), alice_id.clone()),
            ("unknown_code".to_string(), None),
            ("expired_code".to_string(), alice_id),
        ]
    );
    // Admins read them through the audit API like every other entry.
    let admin = srv.admin("root").await;
    let api: Vec<AuditEntry> = srv
        .get(&admin, &format!("{}?limit=100", routes::ADMIN_AUDIT))
        .await
        .expect(200)
        .json();
    assert!(api.iter().any(|e| e.action == "pairing.create"));
}

#[tokio::test]
async fn the_database_keeps_only_a_hash() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let created = create_code(&srv, &alice).await;
    let canonical = pairing::normalize(&created.code).unwrap();
    let rows: Vec<(String, String, String, i64, i64, Option<i64>)> =
        sqlx::query_as("SELECT id, code_hash, user_id, created_at, expires_at, used_at FROM pairing_codes")
            .fetch_all(&srv.state.db)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);
    let (id, code_hash, user_id, created_at, expires_at, used_at) = &rows[0];
    assert_eq!(code_hash, &stickpile_server::ids::sha256_hex(canonical.as_bytes()));
    assert_eq!(user_id, &alice.id);
    assert_eq!(expires_at - created_at, 300);
    assert_eq!(*expires_at, created.expires_at);
    assert_eq!(*used_at, None);
    let everything = format!("{rows:?}").to_uppercase();
    assert!(!everything.contains(&canonical), "{everything}");
    assert!(!everything.contains(&created.code));
    assert!(!id.is_empty());
    // Nor does the audit log.
    let log = format!("{:?}", audit(&srv).await).to_uppercase();
    assert!(!log.contains(&canonical) && !log.contains(&created.code));
}

#[tokio::test]
async fn old_codes_are_cleaned_up() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let first = create_code(&srv, &alice).await;
    exchange(&srv, &first.code, "phone").await.expect(200);
    let second = create_code(&srv, &alice).await;
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pairing_codes")
            .fetch_one(&srv.state.db)
            .await
            .unwrap()
    };
    // Used and expired codes stay for an hour (their failures are audited
    // as such), then the periodic cleanup removes them.
    stickpile_server::app::cleanup_once(&srv.state).await.unwrap();
    assert_eq!(count().await, 2);
    sqlx::query("UPDATE pairing_codes SET expires_at = ?")
        .bind(now() - stickpile_server::auth::pairing::KEEP_SECS - 1)
        .execute(&srv.state.db)
        .await
        .unwrap();
    stickpile_server::app::cleanup_once(&srv.state).await.unwrap();
    assert_eq!(count().await, 0);
    exchange(&srv, &second.code, "phone").await.expect(401);
    // Creating a code cleans up too.
    create_code(&srv, &alice).await;
    sqlx::query("UPDATE pairing_codes SET expires_at = ?")
        .bind(now() - stickpile_server::auth::pairing::KEEP_SECS - 1)
        .execute(&srv.state.db)
        .await
        .unwrap();
    create_code(&srv, &alice).await;
    assert_eq!(count().await, 1);
}

#[tokio::test]
async fn openapi_documents_pairing() {
    let srv = TestServer::start().await;
    let spec = srv.anon_get(routes::OPENAPI).await.expect(200).value();
    let pair = &spec["paths"][routes::AUTH_PAIR]["post"];
    assert!(pair["security"].is_null(), "anonymous: {pair}");
    assert!(pair["responses"]["401"].is_object());
    assert!(pair["responses"]["429"].is_object());
    let create = &spec["paths"][routes::PAIRINGS]["post"];
    assert!(create["responses"]["201"].is_object());
    assert!(create["security"].is_array());
    let schemas = &spec["components"]["schemas"];
    for name in ["PairRequest", "PairingCreated", "TokenCreated"] {
        assert!(schemas.get(name).is_some(), "schema {name}");
    }
}

#[tokio::test]
async fn settings_add_a_device() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let b = Browser::login(&srv, &alice).await;

    let page = b.get("/settings").await;
    assert_page(&page);
    let html = page.text();
    assert!(html.contains("<section id=\"devices\" aria-labelledby=\"s-devices\">"));
    assert!(html.contains("action=\"/settings/pairing\""));

    // Without the CSRF token: refused, and no code is made.
    let r = b.post_raw("/settings/pairing", &[]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let codes = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pairing_codes")
            .fetch_one(&srv.state.db)
            .await
            .unwrap()
    };
    assert_eq!(codes().await, 0);

    let r = b.post_form("/settings/pairing", &[]).await;
    assert_page(&r);
    assert_eq!(r.header("cache-control").as_deref(), Some("no-store"));
    let html = r.text();
    assert!(html.contains("<svg class=\"qr\""), "QR code");
    assert!(html.contains("aria-label=\"QR code to sign in a new device\""));
    assert!(html.contains(&format!("<code>{}</code>", srv.base)), "server address");
    let at = html.find("id=\"pairing-code\"").expect("code shown");
    let code = html[at..]
        .split_once('>')
        .and_then(|(_, rest)| rest.split('<').next())
        .unwrap()
        .to_string();
    assert!(pairing::normalize(&code).is_some(), "{code}");
    assert!(html.contains("<time datetime="), "expiry shown");
    assert_eq!(codes().await, 1);
    // The code on the page signs a device in.
    let token: TokenCreated = exchange(&srv, &code, "phone").await.expect(200).json();
    assert_eq!(token.user.id, alice.id);

    // Anonymous visitors are sent to the login page.
    let r = Browser::anonymous(&srv).post_raw("/settings/pairing", &[]).await;
    assert!(r.status.is_redirection(), "{}", r.status);
    assert!(location(&r).starts_with("/login"), "{}", location(&r));
    assert_eq!(codes().await, 1);
}
