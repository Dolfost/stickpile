//! Web UI flows end to end, the way a browser without JavaScript does them
//! (forms + redirects), plus the htmx variants where they differ: sets,
//! uploads with near-duplicate warnings, directory import, item editing,
//! stats and popular, settings, the admin panel, git packs and Telegram.

mod common;
mod web_common;

use common::{gif_anim, png, png_near, TestServer};
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
    // The upload form hands the set's aspect ratio to the editing tray
    // (upload.js, a crop preset) and mentions pasting once the script runs.
    let page = b.get(&format!("/sets/{set_id}")).await.text();
    assert!(page.contains("data-aspect=\"1:1\""), "{page}");
    assert!(page.contains("<span class=\"paste-hint\" hidden>"), "{page}");
    // The tray's vibe field suggests the vocabulary.
    assert!(
        page.contains(&format!("data-vibes=\"upload-vibes-{set_id}\"")),
        "{page}"
    );
    assert!(
        page.contains(&format!("<datalist id=\"upload-vibes-{set_id}\"><option value=")),
        "{page}"
    );
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

    // /upload with the set selector, and the tray's details for the file
    // (`meta`, one entry per file like the API).
    let meta = json!([{"name": "Sly fox", "keywords": ["orange", "sly"], "emojis": ["🦊"], "vibe": "smug", "tags": ["animals"]}]);
    let r = b
        .post_multipart_fields(
            "/upload",
            &[("set_id", &set_id), ("meta", &meta.to_string())],
            vec![("file", "fox.png", png(4))],
        )
        .await;
    assert_page(&r);
    assert!(r.text().contains("1 file added"));
    // Broken details are an error, not ignored.
    let r = b
        .post_multipart_fields(
            "/upload",
            &[("set_id", &set_id), ("meta", "{not json")],
            vec![("file", "owl.png", png(5))],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("meta:"), "{}", r.text());

    let d = s.get(&alice, &routes::path(routes::SET, &[&set_id])).await.value();
    let items = d["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 4);
    let fox = items.iter().find(|i| i["name"] == "Sly fox").expect("fox item");
    assert_eq!(fox["keywords"], json!(["orange", "sly"]));
    assert_eq!(fox["emojis"], json!(["🦊"]));
    assert_eq!(fox["vibe"], "smug");
    assert_eq!(fox["tags"], json!(["animals"]));
    assert_eq!(fox["keywords_source"], "user");

    // The cover: "Use as cover" in the item menu posts here (htmx gets the
    // grid back in the page's sort), and the settings form chooses too.
    let dog = items.iter().find(|i| i["name"] == "dog").expect("dog")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = b
        .post_htmx_from(
            &format!("/sets/{set_id}/cover"),
            &[("item", &dog)],
            &format!("/sets/{set_id}?sort=likes"),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let grid = r.text();
    assert!(grid.contains("id=\"set-items\""), "{grid}");
    assert!(grid.contains(&format!("id=\"item-{dog}\"")));
    assert_eq!(
        grid.matches(" data-cover>").count() + grid.matches(" data-cover ").count(),
        1,
        "{grid}"
    );
    assert_eq!(grid.matches("class=\"badge cover-badge\"").count(), 1, "{grid}");
    assert!(grid.contains(&format!("data-cover-url=\"/sets/{set_id}/cover\"")));
    let settings = b.get(&format!("/sets/{set_id}/settings")).await.text();
    assert!(
        settings.contains(&format!(
            "<input type=\"radio\" name=\"cover\" value=\"{dog}\" checked>"
        )),
        "{settings}"
    );
    let r = b
        .post_form(
            &format!("/sets/{set_id}/settings"),
            &[
                ("name", "My Cats"),
                ("slug", "my-cats"),
                ("visibility", "private"),
                ("aspect", "1:1"),
                ("cover", ""),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let d = s.get(&alice, &routes::path(routes::SET, &[&set_id])).await.value();
    assert_eq!(d["set"]["cover_item_id"], serde_json::Value::Null);
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

/// The tag (`<…>`) that contains `marker`.
fn tag_with<'a>(html: &'a str, marker: &str) -> &'a str {
    let at = html.find(marker).unwrap_or_else(|| panic!("{marker} in {html}"));
    let start = html[..at].rfind('<').unwrap();
    let end = html[at..].find('>').unwrap() + at;
    &html[start..=end]
}

/// `POST /api/v1/usage` the way app.js sends it: session cookie, JSON, the
/// page's `Origin` and `Sec-Fetch-Site`, and `X-CSRF-Token` if given.
async fn post_usage(b: &Browser, body: &Value, token: Option<&str>, origin: &str, site: &str) -> common::Resp {
    let mut req = b
        .client
        .post(b.url(routes::USAGE))
        .header("Origin", origin)
        .header("Sec-Fetch-Site", site)
        .json(body);
    if let Some(t) = token {
        req = req.header("X-CSRF-Token", t);
    }
    common::read(req.send().await.unwrap()).await
}

/// A copy in the browser (app.js; docs/web.md, "Copy to clipboard"): the
/// markup says what to copy, and the usage event posted after a successful
/// copy (`client` "web", `target` "generic", the variant the PNG was made
/// from) is accepted once from the page, counts, and puts the item first
/// in "Recently copied", whose cards copy again (controls, quick copy and
/// menu). Without the CSRF token or from another site nothing is recorded.
#[tokio::test]
async fn web_copy_is_recorded_and_shows_in_recently_copied() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let set = s.create_set(&alice, "cats", Visibility::Private).await;
    let cat = s.upload_one(&alice, &set.id, "cat.png", png(1)).await;
    let dance = s.upload_one(&alice, &set.id, "dance.gif", gif_anim(3)).await;
    let b = Browser::login(&s, &alice).await;
    assert!(b.get("/").await.text().contains("Nothing copied yet"));

    // What app.js copies: a PNG still as it is, a GIF as a still frame of
    // the original (the thumbnail as the fallback).
    let page = b.get(&format!("/sets/{}", set.id)).await.text();
    let still = tag_with(&page, &format!("data-copy-item=\"{}\"", cat.id));
    assert!(still.contains("data-copy-kind=\"still\""), "{still}");
    assert!(still.contains(&format!("data-copy-src=\"/api/v1/items/{}/file/original\"", cat.id)));
    assert!(still.contains("data-copy-variant=\"original\""), "{still}");
    let anim = tag_with(&page, &format!("data-copy-item=\"{}\"", dance.id));
    assert!(anim.contains("data-copy-kind=\"animation\""), "{anim}");
    assert!(anim.contains(&format!("data-copy-thumb=\"/api/v1/items/{}/file/thumb\"", dance.id)));
    assert!(
        !anim.contains("data-copy-page"),
        "only the item page copies on Ctrl+C without focus"
    );
    assert!(page.contains(">Copy frame<"), "the label says it is a still");
    assert!(page.contains("class=\"quick-copy\""));

    let t = now();
    let event = |id: &str, item: &str, variant: &str| {
        json!({"events": [{"id": id, "item_id": item, "copied_at": t - 1, "target": "generic",
            "variant": variant, "client": "web"}]})
    };
    let csrf = b.csrf.clone().unwrap();
    let rows = || async {
        sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>)>(
            "SELECT item_id, client, target, variant FROM usage_events ORDER BY copied_at, id",
        )
        .fetch_all(&s.state.db)
        .await
        .unwrap()
    };

    // Refused: no token; a token from another origin; a cross-site request.
    let cat_event = event("web-cat-1", &cat.id, "original");
    for (token, origin, site) in [
        (None, s.base.as_str(), "same-origin"),
        (Some(csrf.as_str()), "https://evil.example", "cross-site"),
        (Some(csrf.as_str()), s.base.as_str(), "cross-site"),
    ] {
        let r = post_usage(&b, &cat_event, token, origin, site).await;
        assert_eq!(
            r.status,
            StatusCode::FORBIDDEN,
            "{token:?} {origin} {site}: {}",
            r.text()
        );
        assert_eq!(r.code(), "csrf");
    }
    assert!(rows().await.is_empty());

    // Accepted from the page, once: a retry with the same id is a duplicate.
    let r = post_usage(&b, &cat_event, Some(&csrf), &s.base, "same-origin").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(
        (r.value()["accepted"].clone(), r.value()["duplicates"].clone()),
        (json!(1), json!(0))
    );
    let r = post_usage(&b, &cat_event, Some(&csrf), &s.base, "same-origin").await;
    assert_eq!(
        (r.value()["accepted"].clone(), r.value()["duplicates"].clone()),
        (json!(0), json!(1))
    );
    // The GIF's frame, a moment later.
    let mut frame = event("web-dance-1", &dance.id, "original");
    frame["events"][0]["copied_at"] = json!(t);
    let r = post_usage(&b, &frame, Some(&csrf), &s.base, "same-origin").await;
    assert_eq!(r.value()["accepted"], 1, "{}", r.text());

    let web = (
        Some("web".to_string()),
        Some("generic".to_string()),
        Some("original".to_string()),
    );
    assert_eq!(
        rows().await,
        vec![
            (cat.id.clone(), web.0.clone(), web.1.clone(), web.2.clone()),
            (dance.id.clone(), web.0, web.1, web.2),
        ]
    );
    let item = s.get(&alice, &routes::path(routes::ITEM, &[&cat.id])).await.value();
    assert_eq!(item["item"]["copies"], 1);

    // "Recently copied": newest first, each card ready to copy again.
    let home = b.get("/").await.text();
    assert!(home.contains("Recently copied"));
    let (d, c) = (
        home.find(&format!("id=\"card-{}\"", dance.id))
            .expect("dance in recently copied"),
        home.find(&format!("id=\"card-{}\"", cat.id))
            .expect("cat in recently copied"),
    );
    assert!(d < c, "newest first");
    let card = tag_with(&home, &format!("id=\"card-{}\"", cat.id));
    assert!(card.contains("data-menu"), "{card}");
    assert!(card.contains(&format!("data-item-url=\"/items/{}\"", cat.id)));
    assert!(card.contains(&format!("data-download-url=\"/api/v1/items/{}/file/original\"", cat.id)));
    assert!(home.contains(&format!("data-copy-item=\"{}\"", cat.id)));
    assert!(home.contains("More actions for cat"), "the menu button");
    assert!(home.contains("class=\"quick-copy\""));

    // Stats count it; the web reports the generic profile it copies for.
    let stats = b.get("/stats").await.text();
    assert!(stats.contains("2 copies in the last 30 days"), "{stats}");
    assert!(stats.contains("Other apps"));
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

    // Username: the form sends every field back; a taken name is an error
    // shown in the profile section; a free one renames the account.
    let page = b.get("/settings").await.text();
    assert!(page.contains("name=\"username\" value=\"alice\""), "{page}");
    s.user("bob").await;
    let r = b
        .post_form(
            "/settings",
            &[
                ("username", "Bob"),
                ("display_name", "Alice A."),
                ("telegram_user_id", ""),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.text().contains("That username is already taken"));
    let r = b
        .post_form(
            "/settings",
            &[
                ("username", "ally"),
                ("display_name", "Alice A."),
                ("telegram_user_id", ""),
            ],
        )
        .await;
    assert_eq!(location(&r), "/settings?done=profile");
    let me = s.get(&alice, routes::ME).await.value();
    assert_eq!(me["username"], "ally");
    assert_eq!(me["display_name"], "Alice A.", "a custom display name stays");
    assert!(b.get("/settings").await.text().contains("value=\"ally\""));

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
        .post_raw("/login", &[("username", "ally"), ("password", "new-password-1")])
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

/// The account's theme as the API reports it.
async fn theme_of(s: &TestServer, user: &common::TestUser) -> Value {
    s.get(user, routes::ME).await.expect(200).value()["theme"].clone()
}

#[tokio::test]
async fn theme_from_settings_and_the_top_bar() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let b = Browser::login(&s, &alice).await;
    let theme = || theme_of(&s, &alice);
    assert_eq!(theme().await, "system");

    // Settings → Appearance: System, Light, Dark with the current one checked.
    let r = b.get("/settings").await;
    assert_page(&r);
    let html = r.text();
    assert!(html.contains("<section id=\"appearance\" aria-labelledby=\"s-appearance\">"));
    assert!(html.contains("<form class=\"stack narrow\" method=\"post\" action=\"/settings/theme\">"));
    for (value, checked) in [("system", true), ("light", false), ("dark", false)] {
        let radio = format!("<input type=\"radio\" name=\"theme\" value=\"{value}\"");
        assert!(html.contains(&radio), "{value}");
        assert_eq!(html.contains(&format!("{radio} checked>")), checked, "{value}");
    }

    // Saving: stored, back to the settings page with a notice, rendered.
    let r = b
        .post_form(
            "/settings/theme",
            &[("theme", "dark"), ("next", "/settings?done=theme#appearance")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(location(&r), "/settings?done=theme#appearance");
    assert_eq!(theme().await, "dark");
    let html = b.get("/settings?done=theme").await.text();
    assert!(html.contains("Theme saved."));
    assert!(html.contains("<html lang=\"en\" data-theme=\"dark\">"));
    assert!(html.contains("<input type=\"radio\" name=\"theme\" value=\"dark\" checked>"));

    // Anything but the three names is refused in the Appearance section and
    // changes nothing.
    for bad in ["purple", "Dark", " ", ""] {
        let r = b.post_form("/settings/theme", &[("theme", bad)]).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad:?}");
        let html = r.text();
        let err = html.find("The theme must be system, light or dark").expect("message");
        assert!(err > html.find("id=\"appearance\"").unwrap() && err < html.find("id=\"password\"").unwrap());
    }
    assert_eq!(theme().await, "dark");

    // The top bar's switch without JavaScript: its form sends the other
    // theme and returns to the page it was on (the Referer).
    let set = s.create_set(&alice, "cats", Visibility::Private).await;
    let from = format!("/sets/{}?view=compact", set.id);
    let r = common::read(
        b.client
            .post(b.url("/settings/theme"))
            .header("referer", b.url(&format!("{from}#items-head")))
            .form(&[("csrf_token", b.csrf.as_deref().unwrap()), ("theme", "light")])
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(location(&r), from);
    assert_eq!(theme().await, "light");
    assert!(b
        .get(&from)
        .await
        .text()
        .contains("<html lang=\"en\" data-theme=\"light\">"));
    // No Referer and no (local) next: the settings page.
    for next in [None, Some("//evil.example/x")] {
        let mut fields = vec![("theme", "system")];
        if let Some(n) = next {
            fields.push(("next", n));
        }
        let r = b.post_form("/settings/theme", &fields).await;
        assert_eq!(location(&r), "/settings?done=theme#appearance", "{next:?}");
    }
    assert_eq!(theme().await, "system");
    assert!(b.get("/").await.text().contains("<html lang=\"en\">"));
    // Without the CSRF token: refused.
    let r = b.post_raw("/settings/theme", &[("theme", "dark")]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(theme().await, "system");

    // With JavaScript the switch saves through PATCH /api/v1/me with the
    // session cookie and its CSRF header.
    let patch = |token: Option<String>, body: Value| {
        let mut rb = b.client.patch(b.url(routes::ME)).json(&body);
        if let Some(t) = token {
            rb = rb.header("X-CSRF-Token", t);
        }
        async move { common::read(rb.send().await.unwrap()).await }
    };
    let r = patch(b.csrf.clone(), json!({"theme": "dark"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.value()["theme"], "dark");
    let r = patch(None, json!({"theme": "light"})).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.code(), "csrf");
    assert_eq!(theme().await, "dark");
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
        ("username", ("username", "caroline")),
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
    assert_eq!(c["user"]["username"], "caroline");
    assert_eq!(c["user"]["display_name"], "Carol", "a custom display name stays");
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

#[tokio::test]
async fn cover_image_in_the_settings() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let bob = s.user("bob").await;
    let b = Browser::login(&s, &alice).await;
    let set = s.create_set(&alice, "covered", Visibility::Public).await;
    let cat = s.upload_one(&alice, &set.id, "cat.png", png(1)).await;
    s.upload_one(&alice, &set.id, "dog.png", png(2)).await;
    let settings = format!("/sets/{}/settings", set.id);
    let upload = format!("/sets/{}/cover/image", set.id);
    let api_set = || async { s.get(&alice, &routes::path(routes::SET, &[&set.id])).await.value()["set"].clone() };

    // The chooser points to the cover image form: a plain multipart form
    // that upload.js turns into its one-image tray.
    let r = b.get(&settings).await;
    assert_page(&r);
    let html = r.text();
    assert!(
        html.contains("<a href=\"#cover-image\">upload a cover image</a>"),
        "{html}"
    );
    assert!(html.contains(&format!(
        "<form class=\"upload-form cover-upload\" method=\"post\" enctype=\"multipart/form-data\" action=\"{upload}#cover-image\" data-tray=\"cover\">"
    )), "{html}");
    assert!(html.contains("accept=\"image/png,image/jpeg,image/webp,.png,.jpg,.jpeg,.webp\""));
    assert!(html.contains(">Upload image</button>"));
    assert!(!html.contains("Remove image"));

    // Upload without JavaScript: the token first, back to the settings.
    let r = b
        .post_multipart(&upload, vec![("file", "cover.png", png(9))], false)
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "the CSRF token is required");
    let r = b
        .post_multipart(&upload, vec![("file", "cover.png", png(9))], true)
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    assert_eq!(location(&r), format!("{settings}?saved=cover#cover-image"));
    let set_json = api_set().await;
    let thumb = set_json["cover_image"]["thumb_url"]
        .as_str()
        .expect("cover image")
        .to_string();
    let r = b.get(&format!("{settings}?saved=cover")).await;
    assert_page(&r);
    let html = r.text();
    assert!(html.contains("The new cover image is in place."), "{html}");
    assert!(
        html.contains("<input type=\"radio\" name=\"cover\" value=\"image\" checked>"),
        "{html}"
    );
    assert!(html.contains(&format!("src=\"{thumb}\"")));
    assert!(html.contains("Choosing an item instead removes the cover image"));
    assert!(html.contains(">Remove image</button>"));
    assert!(html.contains(">Replace image</button>"));
    assert!(html.contains("The cover goes back to the first item."), "{html}");

    // Lists, the start page and the set page show it; no item is the cover.
    for page in ["/sets", "/"] {
        let html = b.get(page).await.text();
        assert!(html.contains(&format!("<img src=\"{thumb}\"")), "{page}: {html}");
    }
    let html = b.get(&format!("/sets/{}", set.id)).await.text();
    assert!(
        html.contains(&format!("id=\"set-cover\"><img src=\"{thumb}\"")),
        "{html}"
    );
    assert!(html.contains("<div id=\"set-items\" data-cover-image>"), "{html}");
    assert!(!html.contains("cover-badge"));
    // Bob sees it on the public set, but cannot change it.
    let bob_b = Browser::login(&s, &bob).await;
    assert!(bob_b.get(&format!("/sets/{}", set.id)).await.text().contains(&thumb));
    let r = bob_b
        .post_multipart(&upload, vec![("file", "x.png", png(3))], true)
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Not an image for a cover: the page again, with the message there.
    let r = b
        .post_multipart(&upload, vec![("file", "anim.gif", common::gif_anim(1))], true)
        .await;
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let html = r.text();
    assert!(
        html.contains("A cover image must be a still PNG, JPEG or WebP image, not a GIF"),
        "{html}"
    );
    assert!(html.contains("id=\"cover-image\""));
    assert_clean_html(&html);

    // Saving the set form with "Cover image" keeps it.
    let r = b
        .post_form(
            &settings,
            &[
                ("name", "Covered"),
                ("slug", "covered"),
                ("visibility", "public"),
                ("cover", "image"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    assert!(api_set().await["cover_image"].is_object());

    // "Use as cover" from the item menu replaces it; htmx also gets the
    // header's (now empty) cover.
    let r = b
        .post_htmx_from(
            &format!("/sets/{}/cover", set.id),
            &[("item", &cat.id)],
            &format!("/sets/{}", set.id),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_fragment(&r);
    let html = r.text();
    assert!(html.contains("id=\"set-cover\" hx-swap-oob=\"true\"></span>"), "{html}");
    assert!(html.contains("<div id=\"set-items\">"), "{html}");
    assert_eq!(html.matches("class=\"badge cover-badge\"").count(), 1);
    let set_json = api_set().await;
    assert!(set_json["cover_image"].is_null());
    assert_eq!(set_json["cover_item_id"], cat.id.as_str());

    // Upload again, then "Remove image": the chosen item is the cover again.
    b.post_multipart(&upload, vec![("file", "cover.png", png(9))], true)
        .await;
    assert!(b.get(&settings).await.text().contains("The cover goes back to “cat”."));
    let r = b.post_form(&format!("/sets/{}/cover/image/delete", set.id), &[]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(location(&r), format!("{settings}?saved=nocover#cover-image"));
    assert!(b
        .get(&format!("{settings}?saved=nocover"))
        .await
        .text()
        .contains("The cover image is removed."));
    let set_json = api_set().await;
    assert!(set_json["cover_image"].is_null());
    assert_eq!(set_json["cover_item_id"], cat.id.as_str());
}
