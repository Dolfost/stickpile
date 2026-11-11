//! Web UI flows end to end, the way a browser without JavaScript does them
//! (forms + redirects), plus the htmx variants where they differ: sets,
//! uploads with near-duplicate warnings, directory import, item editing,
//! stats and popular, settings, the admin panel, git packs and Telegram.

mod common;
mod web_common;

use common::{png, png_near, TestServer};
use reqwest::StatusCode;
use serde_json::{json, Value};
use stickpile_api::{routes, Visibility};
use web_common::*;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn id_from_location(loc: &str, prefix: &str) -> String {
    loc.strip_prefix(prefix)
        .unwrap()
        .split(['?', '#', '/'])
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn set_lifecycle_without_javascript() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let b = Browser::login(&s, &alice).await;

    // Create: the slug comes from the name.
    let r = b
        .post_form(
            "/sets",
            &[("name", "My Cats"), ("visibility", "private"), ("aspect", "1:1")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let set_id = id_from_location(&location(&r), "/sets/");
    let set = s.get(&alice, &routes::path(routes::SET, &[&set_id])).await.value()["set"].clone();
    assert_eq!(set["slug"], "my-cats");
    assert_eq!(set["aspect_ratio"], json!([1, 1]));
    // Same slug again: the form comes back with the error.
    let r = b.post_form("/sets", &[("name", "My Cats")]).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.text().contains("already have a set"));
    assert!(r.text().contains("value=\"My Cats\""), "input kept");

    // Upload (plain form): results with links.
    let upload = format!("/sets/{set_id}/upload");
    let r = b
        .post_multipart(
            &upload,
            vec![("file", "cat.png", png(1)), ("file", "dog.png", png(2))],
            true,
        )
        .await;
    assert_page(&r);
    assert!(r.text().contains("2 files added"), "{}", r.text());
    // Same file again: already in the set. A near-duplicate: the warning.
    let r = b
        .post_multipart(
            &upload,
            vec![("file", "cat.png", png(1)), ("file", "cat-v2.png", png_near(1))],
            true,
        )
        .await;
    let html = r.text();
    assert!(html.contains("already in this set"), "{html}");
    assert!(html.contains("You already have this in set"), "{html}");
    assert!(html.contains(&format!("href=\"/sets/{set_id}\">My Cats</a>")));
    // A broken file is reported, not a crash.
    let r = b
        .post_multipart(&upload, vec![("file", "junk.png", b"not an image".to_vec())], true)
        .await;
    assert_page(&r);
    assert!(r.text().contains("0 files added"));
    assert!(r.text().contains("1 failed"));
    // Empty file input.
    let r = b.post_multipart(&upload, vec![], true).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("choose at least one file"));

    // /upload with the set selector.
    let r = b
        .post_multipart_fields("/upload", &[("set_id", &set_id)], vec![("file", "fox.png", png(4))])
        .await;
    assert_page(&r);
    assert!(r.text().contains("1 file added"));

    let d = s.get(&alice, &routes::path(routes::SET, &[&set_id])).await.value();
    let items = d["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 4);
    let first = items[0]["id"].as_str().unwrap().to_string();

    // Reorder with the plain form.
    let r = b
        .post_form(&format!("/sets/{set_id}/items/{first}/move"), &[("dir", "bottom")])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let d = s.get(&alice, &routes::path(routes::SET, &[&set_id])).await.value();
    assert_eq!(d["items"][3]["id"], first.as_str());

    // Edit everything (no-JS form): only changed fields become user-sourced.
    sqlx::query("UPDATE items SET keywords = '[\"meow\"]', keywords_source = 'ai' WHERE id = ?")
        .bind(&first)
        .execute(&s.state.db)
        .await
        .unwrap();
    let r = b
        .post_form(
            &format!("/items/{first}"),
            &[
                ("name", "Grumpy"),
                ("keywords", "meow"),
                ("emojis", "😾 🐱"),
                ("vibe", "smug"),
                ("tags", "reaction"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let v = s.get(&alice, &routes::path(routes::ITEM, &[&first])).await.value()["item"].clone();
    assert_eq!(v["name"], "Grumpy");
    assert_eq!(v["emojis"], json!(["😾", "🐱"]));
    assert_eq!(v["keywords_source"], "ai", "unchanged AI field stays AI");
    assert_eq!(v["vibe_source"], "user");
    // Inline edit without JS falls back to the full form.
    let r = b.get(&format!("/items/{first}/field/name/edit")).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(location(&r).contains("?edit=1"));

    // Likes and the "hide below N likes" filter.
    let r = b
        .post_form(
            &format!("/items/{first}/like"),
            &[("liked", "1"), ("next", &format!("/sets/{set_id}"))],
        )
        .await;
    assert_eq!(location(&r), format!("/sets/{set_id}"));
    let html = b.get(&format!("/sets/{set_id}?min_likes=1&sort=likes")).await.text();
    assert!(html.contains("Grumpy"));
    assert!(!html.contains("alt=\"dog\""));
    assert!(html.contains("3 items are"), "hidden count shown");
    assert!(!html.contains("Move down"), "no reordering while filtered");
    assert!(
        !html.contains("data-move-url"),
        "no move entries in the context menu either"
    );
    // The compact view survives the like round trip without JavaScript.
    let compact = format!("/sets/{set_id}?min_likes=0&sort=position&view=compact");
    let r = b
        .post_form(&format!("/items/{first}/like"), &[("liked", "0"), ("next", &compact)])
        .await;
    assert_eq!(location(&r), compact);
    let html = b.get(&compact).await.text();
    assert!(html.contains("items-section compact"));
    let r = b
        .post_form(&format!("/items/{first}/like"), &[("liked", "1"), ("next", &compact)])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);

    // Settings.
    let r = b
        .post_form(
            &format!("/sets/{set_id}/settings"),
            &[
                ("name", "Cats!"),
                ("slug", "cats"),
                ("description", "the best"),
                ("visibility", "public"),
                ("aspect", "16:9"),
                ("recommended_px", "512"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let set = s.get(&alice, &routes::path(routes::SET, &[&set_id])).await.value()["set"].clone();
    assert_eq!(set["name"], "Cats!");
    assert_eq!(set["visibility"], "public");
    assert_eq!(set["recommended_px"], 512);
    let r = b
        .post_form(
            &format!("/sets/{set_id}/settings"),
            &[("name", "x"), ("slug", "x"), ("aspect", "wide")],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("aspect ratio"));
    assert!(b
        .get(&format!("/sets/{set_id}/settings?saved=1"))
        .await
        .text()
        .contains("Saved."));

    // Move an item to another set, delete one, delete the set.
    let r = b.post_form("/sets", &[("name", "Other")]).await;
    let other = id_from_location(&location(&r), "/sets/");
    let r = b
        .post_form(&format!("/items/{first}/move"), &[("set_id", &other)])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let v = s.get(&alice, &routes::path(routes::ITEM, &[&first])).await.value();
    assert_eq!(v["item"]["set_id"], other.as_str());
    let r = b.post_form(&format!("/items/{first}/delete"), &[]).await;
    assert_eq!(location(&r), format!("/sets/{other}"));
    let r = b.post_form(&format!("/sets/{set_id}/delete"), &[]).await;
    assert_eq!(location(&r), "/sets");
    assert_eq!(b.get(&format!("/sets/{set_id}")).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn directory_import_creates_and_replaces_sets() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let b = Browser::login(&s, &alice).await;
    let manifest =
        "name = \"Imported\"\nslug = \"imported\"\n\n[items.\"001-a.png\"]\nname = \"alpha\"\nkeywords = [\"first\"]\n";
    let r = b
        .post_multipart(
            "/import",
            vec![
                ("files", "imported/set.toml", manifest.as_bytes().to_vec()),
                ("files", "imported/001-a.png", png(1)),
                ("files", "imported/002-b.png", png(2)),
            ],
            true,
        )
        .await;
    assert_page(&r);
    let html = r.text();
    assert!(html.contains("Imported into “Imported”"), "{html}");
    let set_id = attr_after(&html, "class=\"button primary\" href=\"/sets/").unwrap();
    let d = s.get(&alice, &routes::path(routes::SET, &[&set_id])).await.value();
    assert_eq!(d["items"].as_array().unwrap().len(), 2);
    assert_eq!(d["items"][0]["name"], "alpha");

    // Into the set: the directory wins (b removed, c added).
    let r = b
        .post_multipart(
            &format!("/sets/{set_id}/import"),
            vec![
                ("files", "imported/set.toml", manifest.as_bytes().to_vec()),
                ("files", "imported/001-a.png", png(1)),
                ("files", "imported/003-c.png", png(3)),
            ],
            true,
        )
        .await;
    assert_page(&r);
    let d = s.get(&alice, &routes::path(routes::SET, &[&set_id])).await.value();
    let names: Vec<&str> = d["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["alpha", "c"]);

    // A tar export imports as a new set (htmx: fragment).
    let tar = s
        .get(&alice, &routes::path(routes::SET_EXPORT, &[&set_id]))
        .await
        .expect(200)
        .body;
    let form = reqwest::multipart::Form::new().part(
        "archive",
        reqwest::multipart::Part::bytes(tar).file_name("imported.tar"),
    );
    // Slug "imported" is taken: a 409 for the import, shown in #flash.
    let r = common::read(
        b.client
            .post(b.url("/import"))
            .header("HX-Request", "true")
            .header("X-CSRF-Token", b.csrf.clone().unwrap())
            .multipart(form)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.header("hx-retarget").as_deref(), Some("#flash"));
    assert_fragment(&r);
}

#[tokio::test]
async fn usage_shows_up_in_stats_popular_and_home() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let set = s.create_set(&alice, "cats", Visibility::Public).await;
    let cat = s.upload_one(&alice, &set.id, "cat.png", png(1)).await;
    let t = now() as i64;
    s.post(
        &alice,
        routes::USAGE,
        json!({"events": [
            {"id": "e1", "item_id": cat.id, "copied_at": t, "target": "telegram", "client": "test"},
            {"id": "e2", "item_id": cat.id, "copied_at": t - 86_400, "target": "telegram", "client": "test"},
        ]}),
    )
    .await
    .expect(200);
    let b = Browser::login(&s, &alice).await;

    // The web client's own report (what app.js sends after a copy).
    let r = common::read(
        b.client
            .post(b.url(routes::USAGE))
            .header("X-CSRF-Token", b.csrf.clone().unwrap())
            .json(&json!({"events": [{"id": "web-1", "item_id": cat.id, "copied_at": t, "client": "web", "variant": "original"}]}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.value()["accepted"], 1);

    let html = b.get("/stats").await.text();
    assert!(html.contains("class=\"bar-chart\""));
    assert!(html.contains("<table class=\"sr-only\">"), "text alternative");
    assert!(html.contains("3 copies in the last 30 days"), "{html}");
    assert!(html.contains("Telegram"));
    assert!(html.contains(&format!("/items/{}", cat.id)));
    let html = b.get("/popular?window=all").await.text();
    assert!(html.contains("Most copied") && html.contains("Most liked") && html.contains("Trending"));
    assert!(html.contains("3 copies"), "{html}");
    let html = b.get("/").await.text();
    assert!(html.contains("Recently copied"));
    assert!(html.contains(&format!("/items/{}", cat.id)));
}

#[tokio::test]
async fn account_settings() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let mut b = Browser::login(&s, &alice).await;

    // Profile.
    let r = b
        .post_form(
            "/settings",
            &[("display_name", "Alice A."), ("telegram_user_id", "12345")],
        )
        .await;
    assert_eq!(location(&r), "/settings?done=profile");
    let me = s.get(&alice, routes::ME).await.value();
    assert_eq!(me["display_name"], "Alice A.");
    assert_eq!(me["telegram_user_id"], 12345);
    let r = b
        .post_form("/settings", &[("display_name", "A"), ("telegram_user_id", "abc")])
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("must be a number"));
    let r = b
        .post_form("/settings", &[("display_name", "Alice A."), ("telegram_user_id", "")])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(s.get(&alice, routes::ME).await.value()["telegram_user_id"], Value::Null);

    // Password.
    let r = b
        .post_form(
            "/settings/password",
            &[
                ("current_password", "wrong-one"),
                ("new_password", "new-password-1"),
                ("new_password2", "new-password-1"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.text().contains("Current password is wrong"));
    let r = b
        .post_form(
            "/settings/password",
            &[
                ("current_password", &alice.password),
                ("new_password", "new-password-1"),
                ("new_password2", "nope"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = b
        .post_form(
            "/settings/password",
            &[
                ("current_password", &alice.password),
                ("new_password", "new-password-1"),
                ("new_password2", "new-password-1"),
            ],
        )
        .await;
    assert_eq!(location(&r), "/settings?done=password");
    assert!(b
        .get("/settings?done=password")
        .await
        .text()
        .contains("Password changed"));
    let r = Browser::anonymous(&s)
        .post_raw("/login", &[("username", "alice"), ("password", "new-password-1")])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);

    // API tokens: the secret is shown once, revocation works.
    let r = b
        .post_form("/settings/tokens", &[("name", "laptop"), ("expires_days", "30")])
        .await;
    assert_page(&r);
    let html = r.text();
    let start = html.find("spk_").expect("token shown");
    let token: String = html[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let api = |tok: &str| s.client.get(s.url(routes::ME)).bearer_auth(tok.to_string()).send();
    assert_eq!(api(&token).await.unwrap().status(), StatusCode::OK);
    let page = b.get("/settings").await.text();
    assert!(page.contains("laptop"));
    assert!(!page.contains(&token), "never shown again");
    let tokens = s.get(&alice, routes::TOKENS).await.value();
    let laptop = tokens
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "laptop")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = b.post_form(&format!("/settings/tokens/{laptop}/revoke"), &[]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(api(&token).await.unwrap().status(), StatusCode::UNAUTHORIZED);

    // TOTP: QR code + key, confirm, then disable.
    let r = b.post_form("/settings/totp", &[]).await;
    assert_page(&r);
    let html = r.text();
    assert!(html.contains("<svg class=\"qr\""));
    let secret: String = sqlx::query_scalar("SELECT totp_pending_secret FROM users WHERE id = ?")
        .bind(&alice.id)
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    let grouped = secret
        .chars()
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(html.contains(&grouped), "key shown for manual entry");
    let r = b.post_form("/settings/totp/confirm", &[("code", "000000")]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("<svg class=\"qr\""), "same key shown again");
    let code = stickpile_server::auth::totp::code_at(&secret, now()).unwrap();
    let r = b.post_form("/settings/totp/confirm", &[("code", &code)]).await;
    assert_eq!(location(&r), "/settings?done=totp-on");
    assert_eq!(s.get(&alice, routes::ME).await.value()["totp_enabled"], true);
    let next = stickpile_server::auth::totp::code_at(&secret, now() + 30).unwrap();
    let r = b.post_form("/settings/totp/disable", &[("code", &next)]).await;
    assert_eq!(location(&r), "/settings?done=totp-off");
    assert_eq!(s.get(&alice, routes::ME).await.value()["totp_enabled"], false);
    b.refresh_csrf().await;
}

#[tokio::test]
async fn admin_panel() {
    let s = TestServer::start().await;
    let root = s.admin("root").await;
    let alice = s.user("alice").await;
    let b = Browser::login(&s, &root).await;

    let r = b
        .post_form(
            "/admin/users",
            &[
                ("username", "carol"),
                ("password", "carol-password"),
                ("role", "user"),
                ("display_name", "Carol"),
            ],
        )
        .await;
    assert_eq!(location(&r), "/admin/users?done=created");
    let r = b
        .post_form("/admin/users", &[("username", "carol"), ("role", "user")])
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.text().contains("alert error"), "the users page with the error");
    assert_clean_html(&r.text());

    let users = s.get(&root, routes::ADMIN_USERS).await.value();
    let carol = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["user"]["username"] == "carol")
        .unwrap()["user"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    for (action, extra) in [
        ("role", ("role", "admin")),
        ("disable", ("x", "")),
        ("enable", ("x", "")),
        ("password", ("password", "brand-new-pass")),
        ("reset_totp", ("x", "")),
        ("tailscale", ("tailscale_login", "carol@github")),
    ] {
        let r = b
            .post_form(&format!("/admin/users/{carol}"), &[("action", action), extra])
            .await;
        assert_eq!(location(&r), "/admin/users?done=updated", "{action}: {}", r.text());
    }
    let u = s.get(&root, routes::ADMIN_USERS).await.value();
    let c = u
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["user"]["id"] == carol.as_str())
        .unwrap()
        .clone();
    assert_eq!(c["user"]["role"], "admin");
    assert_eq!(c["disabled"], false);
    assert_eq!(c["user"]["tailscale_login"], "carol@github");
    // The last admin cannot demote themselves... but two admins exist now.
    let me = root.id.clone();
    let r = b
        .post_form(&format!("/admin/users/{me}"), &[("action", "role"), ("role", "user")])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let r = b
        .post_form(
            &format!("/admin/users/{carol}"),
            &[("action", "role"), ("role", "user")],
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "root is no admin any more");

    // Fresh admin session for the rest.
    let b = Browser::login(&s, &s.admin("boss").await).await;
    assert_page(&b.get(&format!("/admin/users/{}/delete", alice.id)).await);
    let r = b.post_form(&format!("/admin/users/{}/delete", alice.id), &[]).await;
    assert_eq!(location(&r), "/admin/users?done=deleted");

    // Invitations: link shown once, revocable.
    let r = b
        .post_form("/admin/invitations", &[("role", "user"), ("expires_days", "7")])
        .await;
    assert_page(&r);
    assert!(r.text().contains("/register?code="));
    let list = b.get("/admin/invitations").await.text();
    assert!(!list.contains("/register?code="), "shown only once");
    let invs = s.get(&root_admin(&s).await, routes::ADMIN_INVITATIONS).await.value();
    let inv = invs[0]["id"].as_str().unwrap().to_string();
    let r = b.post_form(&format!("/admin/invitations/{inv}/delete"), &[]).await;
    assert_eq!(location(&r), "/admin/invitations?done=revoked");

    // Audit log and overview.
    let html = b.get("/admin/audit").await.text();
    assert!(html.contains("user.create") && html.contains("invitation.create") && html.contains("user.delete"));
    let html = b.get("/admin").await.text();
    assert!(html.contains("Job queue") && html.contains("Storage per user"));
}

async fn root_admin(s: &TestServer) -> common::TestUser {
    s.admin("auditor").await
}

#[tokio::test]
async fn telegram_and_git_pack_ui() {
    // Disabled integrations: no links, 404 pages.
    let s = TestServer::with_config(|c| c.git.enabled = false).await;
    let alice = s.user("alice").await;
    let set = s.create_set(&alice, "cats", Visibility::Public).await;
    s.upload_one(&alice, &set.id, "cat.png", png(1)).await;
    let b = Browser::login(&s, &alice).await;
    let html = b.get(&format!("/sets/{}", set.id)).await.text();
    assert!(!html.contains("Publish to Telegram") && !html.contains("Pull now"));
    for p in ["/gitpacks/new", "/telegram/import"] {
        assert_eq!(b.get(p).await.status, StatusCode::NOT_FOUND, "{p}");
    }
    assert_eq!(
        b.get(&format!("/sets/{}/telegram", set.id)).await.status,
        StatusCode::NOT_FOUND
    );

    // Enabled.
    let token = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(token.path(), "123456:test-token").unwrap();
    let path = token.path().to_path_buf();
    let s = TestServer::with_config(move |c| {
        c.telegram.enabled = true;
        c.telegram.bot_token_file = Some(path);
        c.telegram.api_url = "http://127.0.0.1:9".into();
    })
    .await;
    let alice = s.user("alice").await;
    let bob = s.user("bob").await;
    let set = s.create_set(&alice, "cats", Visibility::Public).await;
    s.upload_one(&alice, &set.id, "cat.png", png(1)).await;
    let b = Browser::login(&s, &alice).await;
    let html = b.get("/sets").await.text();
    assert!(html.contains("Add a git pack") && html.contains("Import a Telegram pack"));
    let html = b.get(&format!("/sets/{}", set.id)).await.text();
    assert!(html.contains("Publish to Telegram"));
    assert_page(&b.get(&format!("/sets/{}/telegram", set.id)).await);
    // No Telegram user id yet: explained on the form.
    let r = b.post_form(&format!("/sets/{}/telegram", set.id), &[]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("Telegram user id"));
    b.post_form("/settings", &[("display_name", "alice"), ("telegram_user_id", "42")])
        .await;
    let r = b.post_form(&format!("/sets/{}/telegram", set.id), &[]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let job_page = location(&r);
    assert!(job_page.starts_with("/jobs/"));
    let html = b.get(&job_page).await.text();
    assert!(html.contains("Telegram export"));
    assert!(html.contains("hx-trigger=\"every 2s\""));
    assert!(html.contains("http-equiv=\"refresh\""), "no-JS refresh while running");
    // Jobs are private to their owner.
    let bb = Browser::login(&s, &bob).await;
    assert_eq!(bb.get(&job_page).await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        bb.get(&format!("/sets/{}/telegram", set.id)).await.status,
        StatusCode::FORBIDDEN
    );
    // Telegram import form.
    assert_page(&b.get("/telegram/import").await);
    let r = b.post_form("/telegram/import", &[("pack_name", "not a pack!")]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = b
        .post_form(
            "/telegram/import",
            &[("pack_name", "https://t.me/addstickers/SomePack")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());

    // Git pack: webhook secret shown once, read-only set without edit controls.
    assert_page(&b.get("/gitpacks/new").await);
    let r = b
        .post_form("/gitpacks/new", &[("url", "ftp://example.invalid/x.git")])
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = b
        .post_form(
            "/gitpacks/new",
            &[
                ("url", "https://example.invalid/someone/dogs.git"),
                ("mode", "read_only"),
            ],
        )
        .await;
    assert_page(&r);
    let html = r.text();
    assert!(html.contains("/api/v1/hooks/git/"), "webhook URL");
    assert!(html.contains("shown only now"));
    let set_id = attr_after(&html, "class=\"button primary\" href=\"/sets/").unwrap();
    let html = b.get(&format!("/sets/{set_id}")).await.text();
    assert!(html.contains("Git pack") && html.contains("Pull now"));
    assert!(html.contains("Edit upstream"));
    assert!(html.contains("https://example.invalid/someone/dogs"));
    assert!(
        !html.contains("enctype=\"multipart/form-data\""),
        "read-only: no upload"
    );
    let r = b.post_htmx(&format!("/sets/{set_id}/pull"), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_fragment(&r);
    assert!(r.text().contains("Git pull"));
}
