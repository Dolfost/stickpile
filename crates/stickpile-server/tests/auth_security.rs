//! Authentication, sessions, CSRF, tailnet identity, rate limiting, security
//! headers, OpenAPI coverage, admin-only routes.

mod common;

use common::{read, Opts, TestServer};
use reqwest::Method;
use serde_json::json;
use stickpile_api::{
    routes, AdminUser, ApiToken, AuditEntry, InvitationCreated, Role, TokenCreated, TotpSetup, User, Visibility,
};
use stickpile_server::config::Registration;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

async fn audit_actions(srv: &TestServer) -> Vec<String> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT action FROM audit_log ORDER BY id")
        .fetch_all(&srv.state.db)
        .await
        .unwrap();
    rows.into_iter().map(|r| r.0).collect()
}

#[tokio::test]
async fn health_openapi_headers_and_static() {
    let srv = TestServer::start().await;
    let r = srv.anon_get(routes::HEALTH).await.expect(200);
    assert_eq!(r.value()["status"], "ok");
    for h in [
        "content-security-policy",
        "x-content-type-options",
        "referrer-policy",
        "x-frame-options",
        "permissions-policy",
    ] {
        assert!(r.header(h).is_some(), "missing {h}");
    }
    assert!(r
        .header("content-security-policy")
        .unwrap()
        .contains("default-src 'self'"));
    assert_eq!(r.header("x-frame-options").unwrap(), "DENY");
    assert!(r.header("strict-transport-security").is_none(), "no HSTS on http");

    // Error responses carry them too.
    let r = srv.anon_get(routes::ME).await.expect(401);
    assert_eq!(r.code(), "unauthorized");
    assert!(r.header("content-security-policy").is_some());

    let js = srv.anon_get("/static/htmx.min.js").await.expect(200);
    assert!(js.header("content-type").unwrap().starts_with("text/javascript"));
    assert!(js.header("cache-control").unwrap().contains("immutable"));
    assert!(js.body.len() > 10_000);

    let page = srv.anon_get("/").await.expect(200);
    assert!(page.header("content-type").unwrap().starts_with("text/html"));

    // Unknown API path / wrong method -> ApiError JSON.
    let r = srv.anon_get("/api/v1/nope").await.expect(404);
    assert_eq!(r.code(), "not_found");
    let u = srv.user("alice").await;
    let r = srv
        .send(Some(&u), Method::DELETE, routes::ME, Some(json!({})))
        .await
        .expect(405);
    assert_eq!(r.code(), "method_not_allowed");
}

#[tokio::test]
async fn hsts_when_public_url_is_https() {
    let srv = TestServer::with_config(|c| c.server.public_url = "https://stickpile.example.ts.net".into()).await;
    let r = srv.anon_get(routes::HEALTH).await.expect(200);
    assert!(r.header("strict-transport-security").unwrap().contains("max-age="));
}

#[tokio::test]
async fn openapi_lists_every_route() {
    let srv = TestServer::start().await;
    let spec = srv.anon_get(routes::OPENAPI).await.expect(200).value();
    let paths = spec["paths"].as_object().expect("paths");
    let src = include_str!("../../stickpile-api/src/routes.rs");
    let mut checked = 0;
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("pub const ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once(": &str = \"") else {
            continue;
        };
        let path = value.trim_end_matches("\";");
        if name == "PREFIX" {
            continue;
        }
        assert!(
            paths.contains_key(path),
            "route {name} = {path} missing from the OpenAPI document"
        );
        checked += 1;
    }
    assert!(checked >= 40, "only {checked} routes found");
    assert!(spec["components"]["schemas"].get("ApiError").is_some());
    assert!(spec["components"]["securitySchemes"].get("bearer").is_some());
}

#[tokio::test]
async fn password_login_issues_tokens_and_is_audited() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let r = srv
        .send(
            None,
            Method::POST,
            routes::AUTH_TOKEN,
            Some(json!({"username": "alice", "password": u.password, "token_name": "laptop"})),
        )
        .await
        .expect(200);
    let created: TokenCreated = r.json();
    assert!(created.token.starts_with("spk_"));
    assert_eq!(created.user.username, "alice");
    assert_eq!(created.info.name, "laptop");
    assert!(created.token.starts_with(&created.info.prefix));

    let me: User = read(
        srv.client
            .get(srv.url(routes::ME))
            .bearer_auth(&created.token)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200)
    .json();
    assert_eq!(me.id, u.id);

    // Case-insensitive username.
    srv.send(
        None,
        Method::POST,
        routes::AUTH_TOKEN,
        Some(json!({"username": "ALICE", "password": u.password, "token_name": "x"})),
    )
    .await
    .expect(200);
    let r = srv
        .send(
            None,
            Method::POST,
            routes::AUTH_TOKEN,
            Some(json!({"username": "alice", "password": "wrong-password", "token_name": "x"})),
        )
        .await
        .expect(401);
    assert_eq!(r.code(), "unauthorized");
    let r = srv
        .send(
            None,
            Method::POST,
            routes::AUTH_TOKEN,
            Some(json!({"username": "nobody", "password": "whatever-pw", "token_name": "x"})),
        )
        .await
        .expect(401);
    assert_eq!(r.code(), "unauthorized");

    let actions = audit_actions(&srv).await;
    assert!(actions.iter().filter(|a| *a == "login").count() >= 2, "{actions:?}");
    assert!(
        actions.iter().filter(|a| *a == "login_failed").count() >= 2,
        "{actions:?}"
    );
    assert!(actions.contains(&"token.create".to_string()));
}

#[tokio::test]
async fn token_lifecycle_revoke_and_expiry() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let other = srv.user("bob").await;
    let created: TokenCreated = srv
        .post(&u, routes::TOKENS, json!({"name": "phone"}))
        .await
        .expect(200)
        .json();
    let list: Vec<ApiToken> = srv.get(&u, routes::TOKENS).await.expect(200).json();
    assert!(list.iter().any(|t| t.id == created.info.id && !t.revoked));
    assert!(list.iter().all(|t| t.name != "test" || t.prefix.starts_with("spk_")));

    let call = |token: String| {
        let srv = &srv;
        async move {
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
    };
    call(created.token.clone()).await.expect(200);
    // last_used_at recorded.
    let list: Vec<ApiToken> = srv.get(&u, routes::TOKENS).await.expect(200).json();
    assert!(list
        .iter()
        .find(|t| t.id == created.info.id)
        .unwrap()
        .last_used_at
        .is_some());

    // Someone else cannot revoke it.
    srv.delete(&other, &routes::path(routes::TOKEN, &[&created.info.id]))
        .await
        .expect(404);
    srv.delete(&u, &routes::path(routes::TOKEN, &[&created.info.id]))
        .await
        .expect(204);
    let r = call(created.token.clone()).await.expect(401);
    assert_eq!(r.code(), "unauthorized");
    let list: Vec<ApiToken> = srv.get(&u, routes::TOKENS).await.expect(200).json();
    assert!(list.iter().find(|t| t.id == created.info.id).unwrap().revoked);

    // Expiry.
    srv.post(&u, routes::TOKENS, json!({"name": "past", "expires_at": now() - 5}))
        .await
        .expect(400);
    let short: TokenCreated = srv
        .post(&u, routes::TOKENS, json!({"name": "short", "expires_at": now() + 3600}))
        .await
        .expect(200)
        .json();
    call(short.token.clone()).await.expect(200);
    sqlx::query("UPDATE api_tokens SET expires_at = ? WHERE id = ?")
        .bind(now() - 1)
        .bind(&short.info.id)
        .execute(&srv.state.db)
        .await
        .unwrap();
    call(short.token.clone()).await.expect(401);

    // Garbage tokens and schemes.
    call("spk_nonsense".into()).await.expect(401);
    let r = read(
        srv.client
            .get(srv.url(routes::ME))
            .header("authorization", "Basic YTpi")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, 401);

    let actions = audit_actions(&srv).await;
    assert!(actions.contains(&"token.revoke".to_string()));
}

#[tokio::test]
async fn totp_enrolment_login_and_replay() {
    let srv = TestServer::with_config(|c| c.limits.login_per_minute = 100).await;
    let u = srv.user("alice").await;
    let setup: TotpSetup = srv.post(&u, routes::ME_TOTP, json!({})).await.expect(200).json();
    assert!(setup.otpauth_url.starts_with("otpauth://totp/"));
    srv.post(&u, routes::ME_TOTP_CONFIRM, json!({"code": "000000"}))
        .await
        .expect(400);
    let code = stickpile_server::auth::totp::code_at(&setup.secret, now() as u64).unwrap();
    let me: User = srv
        .post(&u, routes::ME_TOTP_CONFIRM, json!({"code": code}))
        .await
        .expect(200)
        .json();
    assert!(me.totp_enabled);
    srv.post(&u, routes::ME_TOTP, json!({})).await.expect(409);

    let login = |totp: Option<String>| {
        let srv = &srv;
        let pw = u.password.clone();
        async move {
            srv.send(
                None,
                Method::POST,
                routes::AUTH_TOKEN,
                Some(json!({"username": "alice", "password": pw, "totp": totp, "token_name": "t"})),
            )
            .await
        }
    };
    let r = login(None).await.expect(401);
    assert_eq!(r.code(), "totp_required");
    // The code used for confirmation cannot be replayed.
    login(Some(code.clone())).await.expect(401);
    let next = stickpile_server::auth::totp::code_at(&setup.secret, now() as u64 + 30).unwrap();
    login(Some(next.clone())).await.expect(200);
    login(Some(next.clone())).await.expect(401);

    // Disable needs a fresh valid code.
    srv.post(&u, routes::ME_TOTP_DISABLE, json!({"code": "123456"}))
        .await
        .expect(400);
    // Every step in the ±1 window has been used; pretend 90 s passed.
    sqlx::query("UPDATE users SET totp_last_step = NULL")
        .execute(&srv.state.db)
        .await
        .unwrap();
    let later = stickpile_server::auth::totp::code_at(&setup.secret, now() as u64).unwrap();
    let me: User = srv
        .post(&u, routes::ME_TOTP_DISABLE, json!({"code": later}))
        .await
        .expect(200)
        .json();
    assert!(!me.totp_enabled);
    login(None).await.expect(200);
}

#[tokio::test]
async fn password_change() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    srv.put(
        &u,
        routes::ME_PASSWORD,
        json!({"current_password": "nope-nope", "new_password": "new-password-1"}),
    )
    .await
    .expect(403);
    srv.put(
        &u,
        routes::ME_PASSWORD,
        json!({"current_password": u.password, "new_password": "short"}),
    )
    .await
    .expect(400);
    srv.put(
        &u,
        routes::ME_PASSWORD,
        json!({"current_password": u.password, "new_password": "new-password-1"}),
    )
    .await
    .expect(204);
    srv.send(
        None,
        Method::POST,
        routes::AUTH_TOKEN,
        Some(json!({"username": "alice", "password": "new-password-1", "token_name": "x"})),
    )
    .await
    .expect(200);
}

#[tokio::test]
async fn login_rate_limit_returns_429() {
    let srv = TestServer::with_config(|c| c.limits.login_per_minute = 3).await;
    srv.user("alice").await;
    let attempt = || {
        srv.send(
            None,
            Method::POST,
            routes::AUTH_TOKEN,
            Some(json!({"username": "alice", "password": "bad-password", "token_name": "x"})),
        )
    };
    for _ in 0..3 {
        attempt().await.expect(401);
    }
    let r = attempt().await.expect(429);
    assert_eq!(r.code(), "rate_limited");
    let retry: u64 = r.header("retry-after").expect("Retry-After").parse().unwrap();
    assert!((1..=60).contains(&retry));
    // Registration shares the limiter.
    let r = srv
        .send(
            None,
            Method::POST,
            routes::AUTH_REGISTER,
            Some(json!({"username": "zed", "password": "password-zed"})),
        )
        .await;
    assert_eq!(r.status, 429);
}

#[tokio::test]
async fn upload_rate_limit_returns_429() {
    let srv = TestServer::with_config(|c| c.limits.upload_per_minute = 1).await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    srv.upload(&u, &set.id, vec![("a.txt", b"hello".to_vec())], None)
        .await
        .expect(415);
    let r = srv
        .upload(&u, &set.id, vec![("a.txt", b"hello".to_vec())], None)
        .await
        .expect(429);
    assert_eq!(r.code(), "rate_limited");
    assert!(r.header("retry-after").is_some());
}

#[tokio::test]
async fn registration_policies() {
    // closed (default)
    let srv = TestServer::start().await;
    let r = srv
        .send(
            None,
            Method::POST,
            routes::AUTH_REGISTER,
            Some(json!({"username": "newbie", "password": "password-1"})),
        )
        .await
        .expect(403);
    assert_eq!(r.code(), "forbidden");

    // open
    let srv = TestServer::with_config(|c| c.auth.registration = Registration::Open).await;
    let user: User = srv
        .send(
            None,
            Method::POST,
            routes::AUTH_REGISTER,
            Some(json!({"username": "newbie", "password": "password-1", "display_name": "New"})),
        )
        .await
        .expect(201)
        .json();
    assert_eq!(user.role, Role::User);
    assert_eq!(user.display_name, "New");
    srv.send(
        None,
        Method::POST,
        routes::AUTH_REGISTER,
        Some(json!({"username": "NEWBIE", "password": "password-1"})),
    )
    .await
    .expect(409);
    srv.send(
        None,
        Method::POST,
        routes::AUTH_REGISTER,
        Some(json!({"username": "x", "password": "password-1"})),
    )
    .await
    .expect(400);
    srv.send(
        None,
        Method::POST,
        routes::AUTH_REGISTER,
        Some(json!({"username": "shortpw", "password": "pw"})),
    )
    .await
    .expect(400);
    srv.send(
        None,
        Method::POST,
        routes::AUTH_TOKEN,
        Some(json!({"username": "newbie", "password": "password-1", "token_name": "x"})),
    )
    .await
    .expect(200);
    assert!(audit_actions(&srv).await.contains(&"register".to_string()));

    // invite
    let srv = TestServer::with_config(|c| c.auth.registration = Registration::Invite).await;
    let admin = srv.admin("root").await;
    srv.send(
        None,
        Method::POST,
        routes::AUTH_REGISTER,
        Some(json!({"username": "guest", "password": "password-1"})),
    )
    .await
    .expect(403);
    srv.send(
        None,
        Method::POST,
        routes::AUTH_REGISTER,
        Some(json!({"username": "guest", "password": "password-1", "invite_code": "bogus"})),
    )
    .await
    .expect(403);
    let inv: InvitationCreated = srv
        .post(&admin, routes::ADMIN_INVITATIONS, json!({"role": "admin"}))
        .await
        .expect(201)
        .json();
    assert!(
        inv.url.starts_with(&format!("{}/register?code=", srv.base)),
        "{}",
        inv.url
    );
    let code = inv.url.split("code=").nth(1).unwrap().to_string();
    let user: User = srv
        .send(
            None,
            Method::POST,
            routes::AUTH_REGISTER,
            Some(json!({"username": "guest", "password": "password-1", "invite_code": code})),
        )
        .await
        .expect(201)
        .json();
    assert_eq!(user.role, Role::Admin, "role comes from the invitation");
    srv.send(
        None,
        Method::POST,
        routes::AUTH_REGISTER,
        Some(json!({"username": "guest2", "password": "password-1", "invite_code": code})),
    )
    .await
    .expect(403);
    let invs: Vec<stickpile_api::Invitation> = srv.get(&admin, routes::ADMIN_INVITATIONS).await.expect(200).json();
    assert_eq!(invs[0].used_by.as_deref(), Some(user.id.as_str()));
    // Expired invitations do not work.
    let inv: InvitationCreated = srv
        .post(&admin, routes::ADMIN_INVITATIONS, json!({}))
        .await
        .expect(201)
        .json();
    sqlx::query("UPDATE invitations SET expires_at = ? WHERE id = ?")
        .bind(now() - 1)
        .bind(&inv.invitation.id)
        .execute(&srv.state.db)
        .await
        .unwrap();
    let code = inv.url.split("code=").nth(1).unwrap();
    srv.send(
        None,
        Method::POST,
        routes::AUTH_REGISTER,
        Some(json!({"username": "late", "password": "password-1", "invite_code": code})),
    )
    .await
    .expect(403);
}

/// A web session created the way the web UI will (after password login).
async fn session_for(srv: &TestServer, username: &str) -> (String, String) {
    let user = stickpile_server::db::user_by_username(&srv.state.db, username)
        .await
        .unwrap()
        .unwrap();
    let s = stickpile_server::auth::session::create(&srv.state, &user, Some("127.0.0.1"), Some("test"))
        .await
        .unwrap();
    (
        format!("{}={}", stickpile_server::auth::session::COOKIE_NAME, s.secret),
        s.csrf_token,
    )
}

#[tokio::test]
async fn session_cookie_and_csrf() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let (cookie, csrf) = session_for(&srv, "alice").await;

    let me: User = read(
        srv.client
            .get(srv.url(routes::ME))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200)
    .json();
    assert_eq!(me.id, u.id);

    let body = json!({"slug": "s1", "name": "S1"});
    let post = |headers: Vec<(&'static str, String)>, slug: &str| {
        let mut rb = srv
            .client
            .post(srv.url(routes::SETS))
            .header("cookie", &cookie)
            .json(&json!({"slug": slug, "name": slug}));
        for (k, v) in headers {
            rb = rb.header(k, v);
        }
        async move { read(rb.send().await.unwrap()).await }
    };
    let r = post(vec![], "a").await.expect(403);
    assert_eq!(r.code(), "csrf");
    let r = post(vec![("x-csrf-token", "wrong".into())], "a").await.expect(403);
    assert_eq!(r.code(), "csrf");
    post(vec![("x-csrf-token", csrf.clone())], "a").await.expect(201);
    // Cross-origin with a valid token is still rejected.
    let r = post(
        vec![
            ("x-csrf-token", csrf.clone()),
            ("origin", "https://evil.example".into()),
        ],
        "b",
    )
    .await
    .expect(403);
    assert_eq!(r.code(), "csrf");
    let r = post(
        vec![("x-csrf-token", csrf.clone()), ("sec-fetch-site", "cross-site".into())],
        "b",
    )
    .await
    .expect(403);
    assert_eq!(r.code(), "csrf");
    post(vec![("x-csrf-token", csrf.clone()), ("origin", srv.base.clone())], "b")
        .await
        .expect(201);
    // Urlencoded forms may carry the token as a field (the JSON endpoint then
    // rejects the body type, but the CSRF check passed).
    let r = read(
        srv.client
            .post(srv.url(routes::SETS))
            .header("cookie", &cookie)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(format!("slug=c&name=c&csrf_token={csrf}"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, 415, "{}", r.text());
    let r = read(
        srv.client
            .post(srv.url(routes::SETS))
            .header("cookie", &cookie)
            .header("content-type", "application/x-www-form-urlencoded")
            .body("slug=c&name=c&csrf_token=nope")
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(403);
    assert_eq!(r.code(), "csrf");
    // Bearer requests need no CSRF token.
    srv.post(&u, routes::SETS, body).await.expect(201);
    // GETs need no token.
    read(
        srv.client
            .get(srv.url(routes::SETS))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200);

    // Logout (destroy) ends the session.
    let sid = stickpile_server::ids::sha256_hex(cookie.split('=').nth(1).unwrap().as_bytes());
    stickpile_server::auth::session::destroy(&srv.state.db, &sid)
        .await
        .unwrap();
    read(
        srv.client
            .get(srv.url(routes::ME))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(401);
}

#[tokio::test]
async fn session_cookie_attributes_and_expiry() {
    let srv = TestServer::with_config(|c| c.server.secure_cookies = true).await;
    srv.user("alice").await;
    let user = stickpile_server::db::user_by_username(&srv.state.db, "alice")
        .await
        .unwrap()
        .unwrap();
    let s = stickpile_server::auth::session::create(&srv.state, &user, None, None)
        .await
        .unwrap();
    let c = s.cookie.to_str().unwrap();
    for attr in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/", "Max-Age="] {
        assert!(c.contains(attr), "{c} lacks {attr}");
    }
    assert!(c.starts_with("stickpile_session="));
    let cookie = format!("stickpile_session={}", s.secret);
    read(
        srv.client
            .get(srv.url(routes::ME))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200);
    // Session ids are stored hashed.
    let stored: Vec<(String,)> = sqlx::query_as("SELECT id FROM sessions")
        .fetch_all(&srv.state.db)
        .await
        .unwrap();
    assert!(stored.iter().all(|(id,)| id != &s.secret));
    // Sliding expiry: an old last_seen_at is refreshed and the cookie re-sent.
    sqlx::query("UPDATE sessions SET last_seen_at = last_seen_at - 3600, expires_at = expires_at - 3600")
        .execute(&srv.state.db)
        .await
        .unwrap();
    let r = read(
        srv.client
            .get(srv.url(routes::ME))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200);
    let set_cookie = r.header("set-cookie").expect("refreshed cookie");
    assert!(
        set_cookie.starts_with(&cookie) && set_cookie.contains("Max-Age="),
        "{set_cookie}"
    );
    let (seen, exp): (i64, i64) = sqlx::query_as("SELECT last_seen_at, expires_at FROM sessions")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert!(seen >= now() - 5);
    assert!(exp >= now() + 29 * 86400);
    // Expired sessions are rejected.
    sqlx::query("UPDATE sessions SET expires_at = ?")
        .bind(now() - 1)
        .execute(&srv.state.db)
        .await
        .unwrap();
    read(
        srv.client
            .get(srv.url(routes::ME))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(401);
}

#[tokio::test]
async fn disabled_users_cannot_authenticate() {
    let srv = TestServer::start().await;
    let admin = srv.admin("root").await;
    let u = srv.user("alice").await;
    let (cookie, _) = session_for(&srv, "alice").await;
    sqlx::query("UPDATE users SET tailscale_login = 'alice@github' WHERE id = ?")
        .bind(&u.id)
        .execute(&srv.state.db)
        .await
        .unwrap();
    let _: AdminUser = srv
        .patch(
            &admin,
            &routes::path(routes::ADMIN_USER, &[&u.id]),
            json!({"disabled": true}),
        )
        .await
        .expect(200)
        .json();
    srv.get(&u, routes::ME).await.expect(401);
    read(
        srv.client
            .get(srv.url(routes::ME))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(401);
    srv.unix("GET", routes::ME, &[("Tailscale-User-Login", "alice@github")], None)
        .await
        .expect(401);
    srv.send(
        None,
        Method::POST,
        routes::AUTH_TOKEN,
        Some(json!({"username": "alice", "password": u.password, "token_name": "x"})),
    )
    .await
    .expect(401);
}

#[tokio::test]
async fn tailscale_identity_only_through_trusted_socket() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    sqlx::query("UPDATE users SET tailscale_login = 'alice@github' WHERE id = ?")
        .bind(&u.id)
        .execute(&srv.state.db)
        .await
        .unwrap();

    let me: User = srv
        .unix("GET", routes::ME, &[("Tailscale-User-Login", "alice@github")], None)
        .await
        .expect(200)
        .json();
    assert_eq!(me.id, u.id);
    // The same headers over TCP are ignored.
    let r = read(
        srv.client
            .get(srv.url(routes::ME))
            .header("Tailscale-User-Login", "alice@github")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, 401);
    // Unknown login without auto-create: anonymous.
    srv.unix("GET", routes::ME, &[("Tailscale-User-Login", "mallory@github")], None)
        .await
        .expect(401);
    // No headers on the socket: anonymous.
    srv.unix("GET", routes::ME, &[], None).await.expect(401);
    // Bearer beats tailscale.
    let bob = srv.user("bob").await;
    let me: User = srv
        .unix(
            "GET",
            routes::ME,
            &[
                ("Tailscale-User-Login", "alice@github"),
                ("Authorization", &format!("Bearer {}", bob.token)),
            ],
            None,
        )
        .await
        .expect(200)
        .json();
    assert_eq!(me.id, bob.id);

    // Unsafe requests: no CSRF token needed, but cross-site browser requests are rejected.
    let body = r#"{"slug":"ts","name":"TS"}"#;
    srv.unix(
        "POST",
        routes::SETS,
        &[("Tailscale-User-Login", "alice@github")],
        Some(body),
    )
    .await
    .expect(201);
    let r = srv
        .unix(
            "POST",
            routes::SETS,
            &[
                ("Tailscale-User-Login", "alice@github"),
                ("Origin", "https://evil.example"),
            ],
            Some(r#"{"slug":"ts2","name":"x"}"#),
        )
        .await
        .expect(403);
    assert_eq!(r.code(), "csrf");
    srv.unix(
        "POST",
        routes::SETS,
        &[
            ("Tailscale-User-Login", "alice@github"),
            ("Origin", "http://stickpile.test"),
        ],
        Some(r#"{"slug":"ts3","name":"x"}"#),
    )
    .await
    .expect(201);
}

#[tokio::test]
async fn tailscale_autocreate_and_admins() {
    let srv = TestServer::with_config(|c| {
        c.auth.tailscale_autocreate = true;
        c.auth.tailscale_admins = vec!["boss@github".into()];
    })
    .await;
    srv.user("carol").await; // occupies the derived username
    let me: User = srv
        .unix(
            "GET",
            routes::ME,
            &[
                ("Tailscale-User-Login", "carol@github"),
                ("Tailscale-User-Name", "Carol C"),
            ],
            None,
        )
        .await
        .expect(200)
        .json();
    assert_eq!(me.username, "carol-2");
    assert_eq!(me.display_name, "Carol C");
    assert_eq!(me.tailscale_login.as_deref(), Some("carol@github"));
    assert_eq!(me.role, Role::User);
    // Same login again maps to the same user.
    let again: User = srv
        .unix("GET", routes::ME, &[("Tailscale-User-Login", "carol@github")], None)
        .await
        .expect(200)
        .json();
    assert_eq!(again.id, me.id);

    let boss: User = srv
        .unix("GET", routes::ME, &[("Tailscale-User-Login", "boss@github")], None)
        .await
        .expect(200)
        .json();
    assert_eq!(boss.role, Role::Admin);
    srv.unix(
        "GET",
        routes::ADMIN_USERS,
        &[("Tailscale-User-Login", "boss@github")],
        None,
    )
    .await
    .expect(200);
    srv.unix(
        "GET",
        routes::ADMIN_USERS,
        &[("Tailscale-User-Login", "carol@github")],
        None,
    )
    .await
    .expect(403);
    // Over TCP nothing is created.
    read(
        srv.client
            .get(srv.url(routes::ME))
            .header("Tailscale-User-Login", "eve@github")
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(401);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE tailscale_login = 'eve@github'")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert!(audit_actions(&srv).await.contains(&"user.autocreate".to_string()));

    // Existing users listed in tailscale_admins are promoted on login.
    drop(srv);
    let srv2 = TestServer::with_config(|c| c.auth.tailscale_admins = vec!["boss2@github".into()]).await;
    let d2 = srv2.user("dave").await;
    sqlx::query("UPDATE users SET tailscale_login = 'boss2@github' WHERE id = ?")
        .bind(&d2.id)
        .execute(&srv2.state.db)
        .await
        .unwrap();
    let me: User = srv2
        .unix("GET", routes::ME, &[("Tailscale-User-Login", "boss2@github")], None)
        .await
        .expect(200)
        .json();
    assert_eq!(me.role, Role::Admin);
}

#[tokio::test]
async fn tailscale_over_loopback_tcp_needs_shared_secret() {
    let dir = tempfile::tempdir().unwrap();
    let secret_file = dir.path().join("proxy-secret");
    std::fs::write(&secret_file, "s3cret-value\n").unwrap();
    let sf = secret_file.clone();
    let srv = TestServer::with_config(move |c| c.trusted_proxy.secret_file = Some(sf)).await;
    let u = srv.user("alice").await;
    sqlx::query("UPDATE users SET tailscale_login = 'alice@github' WHERE id = ?")
        .bind(&u.id)
        .execute(&srv.state.db)
        .await
        .unwrap();
    let get = |secret: Option<&str>| {
        let mut rb = srv
            .client
            .get(srv.url(routes::ME))
            .header("Tailscale-User-Login", "alice@github");
        if let Some(s) = secret {
            rb = rb.header("X-Stickpile-Proxy-Secret", s);
        }
        async move { read(rb.send().await.unwrap()).await }
    };
    let me: User = get(Some("s3cret-value")).await.expect(200).json();
    assert_eq!(me.id, u.id);
    get(Some("wrong")).await.expect(401);
    get(None).await.expect(401);
}

#[tokio::test]
async fn admin_routes_require_admin() {
    let srv = TestServer::start().await;
    let admin = srv.admin("root").await;
    let u = srv.user("alice").await;
    for path in [
        routes::ADMIN_USERS,
        routes::ADMIN_INVITATIONS,
        routes::ADMIN_AUDIT,
        routes::ADMIN_STORAGE,
        routes::ADMIN_JOBS,
    ] {
        srv.anon_get(path).await.expect(401);
        let r = srv.get(&u, path).await.expect(403);
        assert_eq!(r.code(), "forbidden");
        srv.get(&admin, path).await.expect(200);
    }
    let target = routes::path(routes::ADMIN_USER, &[&admin.id]);
    srv.patch(&u, &target, json!({"role": "user"})).await.expect(403);
    srv.delete(&u, &target).await.expect(403);
    srv.post(&u, routes::ADMIN_USERS, json!({"username": "x2", "role": "admin"}))
        .await
        .expect(403);
    srv.post(&u, routes::ADMIN_INVITATIONS, json!({})).await.expect(403);
    // A regular user cannot promote themselves either.
    srv.patch(
        &u,
        &routes::path(routes::ADMIN_USER, &[&u.id]),
        json!({"role": "admin"}),
    )
    .await
    .expect(403);
}

#[tokio::test]
async fn admin_user_management() {
    let srv = TestServer::start().await;
    let admin = srv.admin("root").await;
    let created: AdminUser = srv
        .post(
            &admin,
            routes::ADMIN_USERS,
            json!({"username": "zoe", "password": "password-zoe", "role": "user", "tailscale_login": "zoe@gh"}),
        )
        .await
        .expect(201)
        .json();
    assert!(created.has_password);
    assert_eq!(created.user.tailscale_login.as_deref(), Some("zoe@gh"));
    srv.post(&admin, routes::ADMIN_USERS, json!({"username": "Zoe", "role": "user"}))
        .await
        .expect(409);
    let list: Vec<AdminUser> = srv.get(&admin, routes::ADMIN_USERS).await.expect(200).json();
    assert_eq!(list.len(), 2);

    let path = routes::path(routes::ADMIN_USER, &[&created.user.id]);
    let up: AdminUser = srv
        .patch(
            &admin,
            &path,
            json!({"role": "admin", "display_name": "Zoe Z", "tailscale_login": null, "password": "password-new"}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(up.user.role, Role::Admin);
    assert_eq!(up.user.display_name, "Zoe Z");
    assert_eq!(up.user.tailscale_login, None);
    srv.send(
        None,
        Method::POST,
        routes::AUTH_TOKEN,
        Some(json!({"username": "zoe", "password": "password-new", "token_name": "x"})),
    )
    .await
    .expect(200);
    // reset_totp clears it.
    sqlx::query("UPDATE users SET totp_secret = 'JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP' WHERE id = ?")
        .bind(&created.user.id)
        .execute(&srv.state.db)
        .await
        .unwrap();
    let up: AdminUser = srv
        .patch(&admin, &path, json!({"reset_totp": true}))
        .await
        .expect(200)
        .json();
    assert!(!up.user.totp_enabled);

    // Last admin protection.
    srv.patch(&admin, &path, json!({"role": "user"})).await.expect(200);
    let me_path = routes::path(routes::ADMIN_USER, &[&admin.id]);
    srv.patch(&admin, &me_path, json!({"role": "user"})).await.expect(409);
    srv.delete(&admin, &me_path).await.expect(409);

    // Deleting a user removes their sets.
    let zoe_tok: TokenCreated = srv
        .send(
            None,
            Method::POST,
            routes::AUTH_TOKEN,
            Some(json!({"username": "zoe", "password": "password-new", "token_name": "x"})),
        )
        .await
        .expect(200)
        .json();
    let zoe = common::TestUser {
        id: created.user.id.clone(),
        username: "zoe".into(),
        password: String::new(),
        token: zoe_tok.token,
    };
    srv.create_set(&zoe, "zset", Visibility::Public).await;
    srv.delete(&admin, &path).await.expect(204);
    srv.delete(&admin, &path).await.expect(404);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sets")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(n, 0);

    let audit: Vec<AuditEntry> = srv
        .get(&admin, &format!("{}?limit=100", routes::ADMIN_AUDIT))
        .await
        .expect(200)
        .json();
    let actions: Vec<&str> = audit.iter().map(|a| a.action.as_str()).collect();
    for a in ["user.create", "user.update", "user.delete"] {
        assert!(actions.contains(&a), "{a} missing from {actions:?}");
    }
    assert!(audit
        .iter()
        .any(|a| a.action == "user.create" && a.actor_username.as_deref() == Some("root")));
    // Paging.
    let first = audit[0].id;
    let older: Vec<AuditEntry> = srv
        .get(&admin, &format!("{}?before={first}&limit=2", routes::ADMIN_AUDIT))
        .await
        .expect(200)
        .json();
    assert!(older.len() <= 2 && older.iter().all(|e| e.id < first));

    let inv: InvitationCreated = srv
        .post(&admin, routes::ADMIN_INVITATIONS, json!({"expires_in": 3600}))
        .await
        .expect(201)
        .json();
    srv.delete(&admin, &routes::path(routes::ADMIN_INVITATION, &[&inv.invitation.id]))
        .await
        .expect(204);
    srv.delete(&admin, &routes::path(routes::ADMIN_INVITATION, &[&inv.invitation.id]))
        .await
        .expect(404);
    let audit: Vec<AuditEntry> = srv.get(&admin, routes::ADMIN_AUDIT).await.expect(200).json();
    assert!(audit.iter().any(|a| a.action == "invitation.create"));
    assert!(audit.iter().any(|a| a.action == "invitation.delete"));
}

#[tokio::test]
async fn integrations_require_auth() {
    // Behaviour is covered by tests/integrations_*.rs; here only the access rules.
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    for path in [
        routes::SETS_GITPACK.to_string(),
        routes::path(routes::SET_PULL, &[&set.id]),
        routes::path(routes::SET_TELEGRAM_EXPORT, &[&set.id]),
        routes::TELEGRAM_IMPORT.to_string(),
    ] {
        let r = srv.send(None, Method::POST, &path, Some(json!({}))).await.expect(401);
        assert_eq!(r.code(), "unauthorized");
    }
    // The webhook needs no login, but only knows git packs.
    let r = srv
        .send(
            None,
            Method::POST,
            &routes::path(routes::GIT_WEBHOOK, &[&set.id]),
            Some(json!({})),
        )
        .await
        .expect(404);
    assert_eq!(r.code(), "not_found");
    // Pulling a local set is a client error.
    srv.post(&u, &routes::path(routes::SET_PULL, &[&set.id]), json!({}))
        .await
        .expect(400);
}

#[tokio::test]
async fn anonymous_is_rejected_everywhere() {
    let srv = TestServer::start().await;
    let fake = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
    for path in [
        routes::ME.to_string(),
        routes::TOKENS.to_string(),
        routes::SETS.to_string(),
        routes::path(routes::SET, &[fake]),
        routes::path(routes::SET_EXPORT, &[fake]),
        routes::ITEMS.to_string(),
        routes::path(routes::ITEM, &[fake]),
        routes::path(routes::ITEM_FILE, &[fake, "thumb"]),
        routes::path(routes::ITEM_BETTER, &[fake]),
        routes::SEARCH.to_string(),
        routes::POPULAR.to_string(),
        routes::STATS_ME.to_string(),
        routes::PROFILES.to_string(),
        routes::path(routes::JOB, &[fake]),
    ] {
        let r = srv.anon_get(&path).await;
        assert_eq!(r.status, 401, "GET {path}: {}", r.text());
        assert_eq!(r.code(), "unauthorized");
    }
    for (m, path) in [
        (Method::POST, routes::USAGE.to_string()),
        (Method::PUT, routes::path(routes::ITEM_LIKE, &[fake])),
        (Method::POST, routes::path(routes::ITEM_ENRICH, &[fake])),
        (Method::PATCH, routes::path(routes::ITEM, &[fake])),
        (Method::DELETE, routes::path(routes::SET, &[fake])),
        (Method::PUT, routes::path(routes::SET_ORDER, &[fake])),
    ] {
        let r = srv.send(None, m.clone(), &path, Some(json!({}))).await;
        assert_eq!(r.status, 401, "{m} {path}");
    }
    let _ = Opts::default();
}

async fn search_ids(srv: &TestServer, u: &common::TestUser, q: &str) -> Vec<String> {
    srv.get(u, &format!("{}?q={q}", routes::SEARCH))
        .await
        .expect(200)
        .value()["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["item"]["id"].as_str().unwrap().to_string())
        .collect()
}

async fn change_password(srv: &TestServer, u: &common::TestUser, new: &str) -> common::Resp {
    let body = json!({"current_password": u.password, "new_password": new});
    srv.put(u, routes::ME_PASSWORD, body).await
}

#[tokio::test]
async fn username_rename() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let admin = srv.admin("root").await;
    let set = srv.create_set(&alice, "cats", Visibility::Public).await;
    let item = srv.upload_one(&alice, &set.id, "cat.png", common::png(1)).await;
    // Pretend the item was synced long ago, to see the rename touch it.
    sqlx::query("UPDATE items SET updated_at = 1000")
        .execute(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(search_ids(&srv, &bob, "by:alice").await, vec![item.id.clone()]);

    // Taken (ignoring case) and invalid names change nothing.
    srv.patch(&alice, routes::ME, json!({"username": "BOB"}))
        .await
        .expect(409);
    srv.patch(&alice, routes::ME, json!({"username": "-x"}))
        .await
        .expect(400);
    // A bad display name next to a good username: nothing is renamed.
    srv.patch(
        &alice,
        routes::ME,
        json!({"username": "kitty", "display_name": "x".repeat(101)}),
    )
    .await
    .expect(400);
    let me: User = srv.get(&alice, routes::ME).await.expect(200).json();
    assert_eq!(me.username, "alice");

    // The rename; a display name equal to the old username follows it, also
    // when a form sends that old value back.
    let me: User = srv
        .patch(
            &alice,
            routes::ME,
            json!({"username": "kitty", "display_name": "alice"}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(me.username, "kitty");
    assert_eq!(me.display_name, "kitty");
    let set_now = srv
        .get(&bob, &routes::path(routes::SET, &[&set.id]))
        .await
        .expect(200)
        .value();
    assert_eq!(set_now["set"]["owner_username"], "kitty");
    assert_eq!(search_ids(&srv, &bob, "by:kitty").await, vec![item.id.clone()]);
    assert!(search_ids(&srv, &bob, "by:alice").await.is_empty());
    // Clients' offline caches refetch the item.
    let synced = srv
        .get(&bob, &format!("{}?updated_since=2000", routes::ITEMS))
        .await
        .expect(200)
        .value();
    assert_eq!(synced["items"][0]["id"], item.id.as_str());
    assert_eq!(synced["items"][0]["owner_username"], "kitty");
    // Tokens keep working (same token as before); the password login uses the new name.
    let login = |name: &str| {
        let body = json!({"username": name, "password": alice.password, "token_name": "x"});
        srv.send(None, Method::POST, routes::AUTH_TOKEN, Some(body))
    };
    login("kitty").await.expect(200);
    login("alice").await.expect(401);
    // A change of case only is allowed; the same name again is a no-op.
    let me: User = srv
        .patch(&alice, routes::ME, json!({"username": "Kitty"}))
        .await
        .expect(200)
        .json();
    assert_eq!(me.username, "Kitty");
    srv.patch(&alice, routes::ME, json!({"username": "Kitty"}))
        .await
        .expect(200);
    // The freed name can be taken by someone else.
    let me: User = srv
        .patch(&bob, routes::ME, json!({"username": "alice"}))
        .await
        .expect(200)
        .json();
    assert_eq!(me.username, "alice");

    // Admin rename; a custom display name stays.
    srv.patch(&bob, routes::ME, json!({"display_name": "Bob B"}))
        .await
        .expect(200);
    let up: AdminUser = srv
        .patch(
            &admin,
            &routes::path(routes::ADMIN_USER, &[&bob.id]),
            json!({"username": "bobby"}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(up.user.username, "bobby");
    assert_eq!(up.user.display_name, "Bob B");
    srv.patch(
        &admin,
        &routes::path(routes::ADMIN_USER, &[&bob.id]),
        json!({"username": "kitty"}),
    )
    .await
    .expect(409);
    let audit: Vec<(String, String)> =
        sqlx::query_as("SELECT action, details FROM audit_log WHERE details LIKE '%\"username\":{%' ORDER BY id")
            .fetch_all(&srv.state.db)
            .await
            .unwrap();
    let actions: Vec<&str> = audit.iter().map(|(a, _)| a.as_str()).collect();
    assert_eq!(
        actions,
        [
            "user.update_self",
            "user.update_self",
            "user.update_self",
            "user.update"
        ],
        "{audit:?}"
    );
    assert!(audit[0].1.contains("\"from\":\"alice\"") && audit[0].1.contains("\"to\":\"kitty\""));
}

#[tokio::test]
async fn password_minimum_is_configurable() {
    // Default: 8.
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    change_password(&srv, &u, "1234").await.expect(400);
    let html = srv.get(&u, "/settings").await.text();
    assert!(html.contains("minlength=\"8\""));

    // Lowered to 4: four characters are fine, three are not; forms follow.
    let srv = TestServer::with_config(|c| c.auth.min_password_length = 4).await;
    let u = srv.user("alice").await;
    let admin = srv.admin("root").await;
    change_password(&srv, &u, "123").await.expect(400);
    change_password(&srv, &u, "1234").await.expect(204);
    srv.send(
        None,
        Method::POST,
        routes::AUTH_TOKEN,
        Some(json!({"username": "alice", "password": "1234", "token_name": "x"})),
    )
    .await
    .expect(200);
    srv.post(
        &admin,
        routes::ADMIN_USERS,
        json!({"username": "zoe", "password": "abcd", "role": "user"}),
    )
    .await
    .expect(201);
    srv.post(
        &admin,
        routes::ADMIN_USERS,
        json!({"username": "yan", "password": "abc", "role": "user"}),
    )
    .await
    .expect(400);
    assert!(srv.get(&admin, "/admin/users").await.text().contains("minlength=\"4\""));

    // Out of range is a config error.
    let mut cfg = stickpile_server::Config::default();
    cfg.auth.min_password_length = 3;
    assert!(cfg.validate().is_err());
    cfg.auth.min_password_length = 129;
    assert!(cfg.validate().is_err());
}

#[tokio::test]
async fn tailnet_sign_in_sets_a_password_without_the_current_one() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    sqlx::query("UPDATE users SET tailscale_login = 'alice@github' WHERE id = ?")
        .bind(&u.id)
        .execute(&srv.state.db)
        .await
        .unwrap();
    // A token (or a session) still needs the current password.
    srv.put(&u, routes::ME_PASSWORD, json!({"new_password": "new-password-1"}))
        .await
        .expect(403);
    // Through the tailnet it does not.
    srv.unix(
        "PUT",
        routes::ME_PASSWORD,
        &[("Tailscale-User-Login", "alice@github")],
        Some(r#"{"new_password": "new-password-2"}"#),
    )
    .await
    .expect(204);
    srv.send(
        None,
        Method::POST,
        routes::AUTH_TOKEN,
        Some(json!({"username": "alice", "password": "new-password-2", "token_name": "x"})),
    )
    .await
    .expect(200);
    // The settings page leaves out the current-password field there.
    let html = srv
        .unix("GET", "/settings", &[("Tailscale-User-Login", "alice@github")], None)
        .await
        .expect(200)
        .text();
    assert!(!html.contains("current_password"), "{html}");
    assert!(html.contains("the current password is not needed"));
}

#[tokio::test]
async fn operator_password_reset() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let db = &srv.state.db;
    let reset = |pw: &str, revoke: bool, totp: bool| {
        stickpile_server::service::users::reset_password(db, "ALICE", pw.to_string(), 8, revoke, totp)
    };
    assert!(reset("short", false, false).await.is_err());
    let err = stickpile_server::service::users::reset_password(db, "nobody", "long-enough".into(), 8, false, false)
        .await
        .err()
        .unwrap();
    assert!(err.message.contains("no user named"), "{}", err.message);

    sqlx::query("UPDATE users SET totp_secret = 'x' WHERE id = ?")
        .bind(&u.id)
        .execute(db)
        .await
        .unwrap();
    reset("brand-new-pw", false, true).await.unwrap();
    // The token survives; the new password works; TOTP is off.
    srv.get(&u, routes::ME).await.expect(200);
    srv.send(
        None,
        Method::POST,
        routes::AUTH_TOKEN,
        Some(json!({"username": "alice", "password": "brand-new-pw", "token_name": "x"})),
    )
    .await
    .expect(200);
    let me: User = srv.get(&u, routes::ME).await.expect(200).json();
    assert!(!me.totp_enabled);
    // With --revoke-tokens the old token stops working.
    reset("another-new-pw", true, false).await.unwrap();
    srv.get(&u, routes::ME).await.expect(401);
    assert!(
        audit_actions(&srv)
            .await
            .iter()
            .filter(|a| *a == "user.password_reset")
            .count()
            == 2
    );
}
