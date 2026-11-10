//! Web UI authentication: login (+TOTP step), logout, registration per
//! policy, anonymous redirects, tailnet identity, same-origin protection.

mod common;
mod web_common;

use common::{TestServer, TestUser};
use reqwest::StatusCode;
use stickpile_api::Role;
use stickpile_server::config::Registration;
use web_common::*;

const PAGES: &[&str] = &[
    "/sets",
    "/sets?tab=public",
    "/sets/new",
    "/upload",
    "/search?q=cat",
    "/popular",
    "/stats",
    "/settings",
    "/admin",
    "/admin/users",
    "/admin/invitations",
    "/admin/audit",
    "/sets/aaaaaaaaaaaaaaaaaaaaaaaaaa",
    "/items/aaaaaaaaaaaaaaaaaaaaaaaaaa",
    "/jobs/aaaaaaaaaaaaaaaaaaaaaaaaaa",
];

#[tokio::test]
async fn anonymous_visitors_are_sent_to_login() {
    let s = TestServer::start().await;
    let b = Browser::anonymous(&s);
    for path in PAGES {
        let r = b.get(path).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{path}");
        let loc = location(&r);
        assert!(loc.starts_with("/login?next=%2F"), "{path} -> {loc}");
        // htmx requests get 401 plus HX-Redirect instead of a redirect.
        let r = b.get_htmx(path).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(r.header("hx-redirect").as_deref(), Some("/login"));
        assert_fragment(&r);
    }
    // Anonymous form posts are redirected too.
    let r = b.post_raw("/sets", &[("name", "x")]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(location(&r), "/login");
    // The start page shows the sign-in form to anonymous visitors.
    let r = b.get("/").await;
    assert_page(&r);
    assert!(r.text().contains("name=\"password\""));
    assert!(!r.text().contains("Sign out"));
    // The login page itself renders.
    let r = b.get("/login?next=%2Fstats").await;
    assert_page(&r);
    let html = r.text();
    assert!(html.contains("name=\"password\""));
    assert_eq!(hidden_value(&html, "next").as_deref(), Some("/stats"));
}

#[tokio::test]
async fn login_sets_session_cookie_and_logout_clears_it() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let b = Browser::anonymous(&s);

    // Wrong password: the form again, 401, no cookie.
    let r = b
        .post_raw("/login", &[("username", "alice"), ("password", "nope-nope")])
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.text().contains("Wrong username or password"));
    assert!(set_cookies(&r.headers).is_empty());
    assert_clean_html(&r.text());

    // Right password: 303 to `next`, session cookie with the right flags.
    let r = b
        .post_raw(
            "/login",
            &[("username", "alice"), ("password", &alice.password), ("next", "/stats")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(location(&r), "/stats");
    let cookies = set_cookies(&r.headers);
    let c = cookies
        .iter()
        .find(|c| c.starts_with("stickpile_session="))
        .expect("session cookie");
    assert!(c.contains("HttpOnly"), "{c}");
    assert!(c.contains("SameSite=Lax"), "{c}");
    assert!(c.contains("Path=/"), "{c}");
    assert!(!c.contains("Secure"), "secure_cookies = false in tests: {c}");
    let secret = c
        .trim_start_matches("stickpile_session=")
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let home = b.get("/").await;
    assert_page(&home);
    let html = home.text();
    assert!(html.contains("alice"));
    assert!(html.contains("Sign out"));
    let csrf = csrf_from(&html).expect("csrf");
    assert!(html.contains(&format!(
        "hx-headers=\"{{&quot;X-CSRF-Token&quot;:&quot;{csrf}&quot;}}\""
    )));

    // Signed-in users skip the login page.
    let r = b.get("/login").await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(location(&r), "/");

    // Logout needs the CSRF token.
    let r = b.post_raw("/logout", &[]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.text().contains("Request blocked"));
    let r = b.post_raw("/logout", &[("csrf_token", &csrf)]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(location(&r), "/login");
    let cleared = set_cookies(&r.headers);
    assert!(
        cleared
            .iter()
            .any(|c| c.starts_with("stickpile_session=;") && c.contains("Max-Age=0")),
        "{cleared:?}"
    );
    // The browser dropped the cookie, and the session is gone server-side.
    assert_eq!(b.get("/stats").await.status, StatusCode::SEE_OTHER);
    let r = common::read(
        s.client
            .get(s.url("/stats"))
            .header("cookie", format!("stickpile_session={secret}"))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn secure_cookie_flag_follows_config() {
    let s = TestServer::with_config(|c| c.server.secure_cookies = true).await;
    let alice = s.user("alice").await;
    let r = Browser::anonymous(&s)
        .post_raw("/login", &[("username", "alice"), ("password", &alice.password)])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let cookies = set_cookies(&r.headers);
    assert!(cookies
        .iter()
        .any(|c| c.starts_with("stickpile_session=") && c.contains("; Secure")));
}

#[tokio::test]
async fn login_redirects_only_to_local_paths() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    for next in [
        "//evil.example/x",
        "https://evil.example/",
        "/\\evil.example",
        "/logout",
    ] {
        let r = Browser::anonymous(&s)
            .post_raw(
                "/login",
                &[("username", "alice"), ("password", &alice.password), ("next", next)],
            )
            .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        assert_eq!(location(&r), "/", "next = {next}");
    }
}

#[tokio::test]
async fn login_and_register_reject_cross_site_posts() {
    let s = TestServer::with_config(|c| c.auth.registration = Registration::Open).await;
    let alice = s.user("alice").await;
    let b = Browser::anonymous(&s);
    let r = common::read(
        b.client
            .post(b.url("/login"))
            .header("Origin", "http://evil.example")
            .form(&[("username", "alice"), ("password", alice.password.as_str())])
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.header("content-type").unwrap().starts_with("text/html"));
    assert!(set_cookies(&r.headers).is_empty());
    let r = common::read(
        b.client
            .post(b.url("/register"))
            .header("Sec-Fetch-Site", "cross-site")
            .form(&[
                ("username", "mallory"),
                ("password", "longenough"),
                ("password2", "longenough"),
            ])
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Same-origin browsers pass.
    let r = common::read(
        b.client
            .post(b.url("/login"))
            .header("Origin", s.base.clone())
            .header("Sec-Fetch-Site", "same-origin")
            .form(&[("username", "alice"), ("password", alice.password.as_str())])
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn totp_accounts_get_a_second_step() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let secret = "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP";
    sqlx::query("UPDATE users SET totp_secret = ? WHERE id = ?")
        .bind(secret)
        .bind(&alice.id)
        .execute(&s.state.db)
        .await
        .unwrap();
    let b = Browser::anonymous(&s);
    let r = b
        .post_raw(
            "/login",
            &[
                ("username", "alice"),
                ("password", &alice.password),
                ("next", "/settings"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(set_cookies(&r.headers).is_empty(), "no session before the code");
    let html = r.text();
    assert!(html.contains("Two-step verification"));
    assert!(
        !html.contains(&alice.password),
        "the password never goes back to the browser"
    );
    assert_clean_html(&html);
    let token = hidden_value(&html, "token").expect("pending token");

    // A wrong code keeps the step.
    let r = b
        .post_raw("/login/totp", &[("token", &token), ("code", "000000")])
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.text().contains("not valid"));
    // A forged token is useless.
    let r = b
        .post_raw("/login/totp", &[("token", "forged"), ("code", "123456")])
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.text().contains("expired"));

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let code = stickpile_server::auth::totp::code_at(secret, now).unwrap();
    let r = b.post_raw("/login/totp", &[("token", &token), ("code", &code)]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    assert_eq!(location(&r), "/settings");
    assert!(set_cookies(&r.headers)
        .iter()
        .any(|c| c.starts_with("stickpile_session=")));
    assert_page(&b.get("/settings").await);
    // The pending login is single-use.
    let r = b.post_raw("/login/totp", &[("token", &token), ("code", &code)]).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn registration_follows_the_policy() {
    // closed
    let s = TestServer::start().await;
    let b = Browser::anonymous(&s);
    let r = b.get("/register").await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.text().contains("Registration is closed"));
    let r = b
        .post_raw(
            "/register",
            &[
                ("username", "eve"),
                ("password", "longenough"),
                ("password2", "longenough"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(set_cookies(&r.headers).is_empty());

    // open
    let s = TestServer::with_config(|c| c.auth.registration = Registration::Open).await;
    let b = Browser::anonymous(&s);
    assert_page(&b.get("/register").await);
    let r = b
        .post_raw(
            "/register",
            &[
                ("username", "eve"),
                ("password", "longenough"),
                ("password2", "different"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("do not match"));
    let r = b
        .post_raw(
            "/register",
            &[
                ("username", "eve"),
                ("display_name", "Eve"),
                ("password", "longenough"),
                ("password2", "longenough"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    assert_eq!(location(&r), "/");
    assert!(b.get("/").await.text().contains("Hi, Eve"));
    // Taken usernames re-render the form.
    let r = Browser::anonymous(&s)
        .post_raw(
            "/register",
            &[
                ("username", "eve"),
                ("password", "longenough"),
                ("password2", "longenough"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.text().contains("taken"));

    // invite
    let s = TestServer::with_config(|c| c.auth.registration = Registration::Invite).await;
    let admin = s.admin("root").await;
    let inv: serde_json::Value = s
        .post(&admin, "/api/v1/admin/invitations", serde_json::json!({"role": "user"}))
        .await
        .expect(201)
        .value();
    let url = inv["url"].as_str().unwrap();
    let code = url.split("code=").nth(1).unwrap().to_string();
    let b = Browser::anonymous(&s);
    let page = b.get(&format!("/register?code={code}")).await;
    assert_page(&page);
    assert!(page.text().contains(&format!("value=\"{code}\"")), "code prefilled");
    let r = b
        .post_raw(
            "/register",
            &[
                ("username", "ivy"),
                ("password", "longenough"),
                ("password2", "longenough"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.text().contains("invitation code is required"));
    let r = b
        .post_raw(
            "/register",
            &[
                ("username", "ivy"),
                ("password", "longenough"),
                ("password2", "longenough"),
                ("invite_code", &code),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    // Single use.
    let r = Browser::anonymous(&s)
        .post_raw(
            "/register",
            &[
                ("username", "ivy2"),
                ("password", "longenough"),
                ("password2", "longenough"),
                ("invite_code", &code),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn tailnet_users_are_signed_in_without_a_form() {
    let s = TestServer::start().await;
    let bob: TestUser = s.user_with_role("bob", Role::User).await;
    sqlx::query("UPDATE users SET tailscale_login = 'bob@github' WHERE id = ?")
        .bind(&bob.id)
        .execute(&s.state.db)
        .await
        .unwrap();
    let r = s
        .unix("GET", "/", &[("Tailscale-User-Login", "bob@github")], None)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let html = r.text();
    assert!(html.contains("bob"));
    assert!(html.contains("tailnet"));
    assert!(!html.contains("name=\"password\""));
    assert!(!html.contains("Sign out"), "nothing to sign out of");
    assert!(csrf_from(&html).is_none(), "tailnet callers need no CSRF token");
    assert_clean_html(&html);
    let r = s
        .unix("GET", "/login", &[("Tailscale-User-Login", "bob@github")], None)
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    // Without the identity the socket is anonymous.
    let r = s.unix("GET", "/stats", &[], None).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    // The header is ignored on the public TCP listener.
    let r = common::read(
        s.client
            .get(s.url("/stats"))
            .header("Tailscale-User-Login", "bob@github")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    // A tailnet form post from another site is blocked (same-origin check).
    let r = s
        .unix(
            "POST",
            "/sets",
            &[
                ("Tailscale-User-Login", "bob@github"),
                ("Origin", "http://evil.example"),
            ],
            Some("{}"),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}
