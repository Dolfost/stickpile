//! Web UI pages: every page renders for its user, access rules (404 for
//! content you cannot see, 403 for admin pages), CSRF on forms, htmx
//! fragments, escaping, CSP hygiene, static assets and error pages.

mod common;
mod web_common;

use common::{gif_anim, png, TestServer};
use reqwest::StatusCode;
use serde_json::json;
use stickpile_api::{routes, Item, Visibility};
use web_common::*;

async fn job_for(s: &TestServer, media_id: &str) -> String {
    sqlx::query_scalar("SELECT id FROM jobs WHERE subject = ? ORDER BY created_at LIMIT 1")
        .bind(media_id)
        .fetch_one(&s.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn every_page_renders_for_its_user() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let admin = s.admin("root").await;
    let set = s.create_set(&alice, "cats", Visibility::Public).await;
    let cat = s.upload_one(&alice, &set.id, "cat.png", png(1)).await;
    let dance = s.upload_one(&alice, &set.id, "dance.gif", gif_anim(5)).await;
    let job = job_for(&s, &cat.media.id).await;
    let b = Browser::login(&s, &alice).await;
    let pages = [
        "/".to_string(),
        "/sets".into(),
        "/sets?tab=public".into(),
        "/sets/new".into(),
        format!("/sets/{}", set.id),
        format!("/sets/{}?sort=likes&min_likes=0", set.id),
        format!("/sets/{}/settings", set.id),
        format!("/sets/{}/delete", set.id),
        format!("/items/{}", cat.id),
        format!("/items/{}?edit=1", cat.id),
        format!("/items/{}", dance.id),
        format!("/items/{}/delete", cat.id),
        format!("/items/{}/better", cat.id),
        "/upload".into(),
        format!("/upload?set={}", set.id),
        "/search".into(),
        "/search?q=cat".into(),
        "/search?q=type%3Agif+sort%3Alikes".into(),
        "/popular".into(),
        "/popular?scope=mine&window=all".into(),
        format!("/popular?scope=set&set={}", set.id),
        "/stats".into(),
        "/settings".into(),
        format!("/jobs/{job}"),
    ];
    for p in &pages {
        let r = b.get(p).await;
        assert_eq!(r.status, StatusCode::OK, "{p}: {}", r.text());
        assert_page(&r);
    }
    let ab = Browser::login(&s, &admin).await;
    for p in ["/admin", "/admin/users", "/admin/invitations", "/admin/audit"] {
        assert_page(&ab.get(p).await);
    }

    // Item page: large preview, copy controls per media type, files.
    let html = b.get(&format!("/items/{}", cat.id)).await.text();
    assert!(html.contains(&format!("src=\"/api/v1/items/{}/file/original\" alt=\"cat\"", cat.id)));
    assert!(html.contains("data-copy=\"image\""), "stills copy as image");
    assert!(html.contains("Quality"));
    assert!(html.contains("Find a better version"));
    assert!(html.contains(&format!("/api/v1/items/{}/file/thumb", cat.id)));
    let html = b.get(&format!("/items/{}", dance.id)).await.text();
    assert!(html.contains("data-copy=\"link\""), "animations copy as a link");
    assert!(html.contains("Browsers cannot put GIFs or videos on the clipboard"));
    assert!(html.contains(">GIF<"), "kind badge");
    // Set page: lazy thumbs with alt text, inline editing, reorder, upload.
    let html = b.get(&format!("/sets/{}", set.id)).await.text();
    assert!(html.contains("loading=\"lazy\""));
    assert!(html.contains("alt=\"dance\""));
    assert!(html.contains(&format!("/items/{}/field/keywords/edit", cat.id)));
    assert!(html.contains("Move down"));
    assert!(html.contains("enctype=\"multipart/form-data\""));
    assert!(html.contains("webkitdirectory"));
    assert!(html.contains(&format!("/api/v1/sets/{}/export.tar", set.id)));
    // Compact view checkbox (unchecked by default) and the context menu.
    assert!(html.contains("type=\"checkbox\" name=\"view\" value=\"compact\""));
    assert!(html.contains("class=\"items-section\""));
    assert!(html.contains("data-menu-button"));
    // Search finds by name; the job page polls while the job is queued.
    assert!(b
        .get("/search?q=cat")
        .await
        .text()
        .contains(&format!("/items/{}", cat.id)));
    let r = b.get_htmx(&format!("/jobs/{job}")).await;
    assert_fragment(&r);
    assert!(r.text().contains("hx-trigger=\"every 2s\""));
    s.run_jobs().await;
    let r = b.get_htmx(&format!("/jobs/{job}")).await;
    assert!(
        !r.text().contains("hx-trigger"),
        "polling stops when the job is finished: {}",
        r.text()
    );
}

/// The opening `<article>` tag of an item card on the set page.
fn card_tag(html: &str, item_id: &str) -> String {
    let marker = format!("id=\"item-{item_id}\"");
    let at = html.find(&marker).unwrap_or_else(|| panic!("card {item_id} in {html}"));
    let start = html[..at].rfind("<article").unwrap();
    let end = html[at..].find('>').unwrap() + at;
    html[start..=end].to_string()
}

/// The `<form class="filters">…</form>` of the set page.
fn filters_form(html: &str) -> String {
    let start = html.find("<form class=\"filters\"").expect("filters form");
    let end = html[start..].find("</form>").unwrap() + start;
    html[start..end].to_string()
}

#[tokio::test]
async fn set_page_compact_view_and_context_menu() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let bob = s.user("bob").await;
    let set = s.create_set(&alice, "cats", Visibility::Public).await;
    let a = s.upload_one(&alice, &set.id, "a.png", png(1)).await;
    let b_item = s.upload_one(&alice, &set.id, "b.png", png(2)).await;
    let c = s.upload_one(&alice, &set.id, "c.png", png(3)).await;
    let owner = Browser::login(&s, &alice).await;
    let page = format!("/sets/{}", set.id);

    // Default view: the checkbox sits in the GET form with the filter and
    // the sort (so Apply keeps all three), unchecked, no compact class.
    let r = owner.get(&page).await;
    assert_page(&r);
    let html = r.text();
    let form = filters_form(&html);
    assert!(
        form.contains(&format!("method=\"get\" action=\"/sets/{}\"", set.id)),
        "{form}"
    );
    for field in ["name=\"min_likes\"", "name=\"sort\"", "name=\"view\" value=\"compact\""] {
        assert!(form.contains(field), "{field} in {form}");
    }
    assert!(
        form.contains("<label class=\"check\" for=\"f-view\">"),
        "labelled checkbox"
    );
    assert!(form.contains("data-view-toggle=\"items-section\""));
    assert!(!form.contains("checked"), "unchecked by default");
    assert!(html.contains("<section class=\"items-section\" id=\"items-section\""));

    // ?view=compact: compact class and a checked box; sort and filter stay.
    let html = owner
        .get(&format!("{page}?sort=likes&min_likes=0&view=compact"))
        .await
        .text();
    assert!(html.contains("class=\"items-section compact\""), "compact class");
    let form = filters_form(&html);
    assert!(form.contains("value=\"compact\" checked"), "{form}");
    assert!(form.contains("<option value=\"likes\" selected>"), "sort kept: {form}");
    assert!(
        form.contains("name=\"min_likes\" min=\"0\" value=\"0\""),
        "filter kept: {form}"
    );
    // ...and the page's own links back (next fields) keep the view.
    assert!(html.contains("name=\"next\" value=\"/sets/"));
    assert!(html.contains("view=compact\""), "next fields carry the query");
    let html = owner.get(&format!("{page}?view=grid")).await.text();
    assert!(!html.contains("items-section compact"), "only view=compact is compact");

    // Context menu data for the owner in set order: everything, with the
    // ends marked for the move entries.
    let html = owner.get(&page).await.text();
    let first = card_tag(&html, &a.id);
    let middle = card_tag(&html, &b_item.id);
    let last = card_tag(&html, &c.id);
    for tag in [&first, &middle, &last] {
        assert!(tag.contains("data-menu=\"\""), "{tag}");
        assert!(tag.contains("data-item-url=\"/items/"), "{tag}");
        assert!(tag.contains("/file/original\""), "download url: {tag}");
        assert!(tag.contains("?edit=1#edit\""), "edit url: {tag}");
        assert!(tag.contains("data-delete-url=\"/items/"), "{tag}");
        assert!(
            tag.contains(&format!("data-move-url=\"/sets/{}/items/", set.id)),
            "{tag}"
        );
    }
    assert!(
        first.contains(" data-first") && !first.contains(" data-last"),
        "{first}"
    );
    assert!(
        !middle.contains(" data-first") && !middle.contains(" data-last"),
        "{middle}"
    );
    assert!(last.contains(" data-last") && !last.contains(" data-first"), "{last}");
    // The "⋯" button: hidden without JS, a menu button with a name.
    assert!(html.contains(
        "<button class=\"icon-btn card-menu-btn\" type=\"button\" hidden data-menu-button=\"\" aria-haspopup=\"menu\" aria-expanded=\"false\" title=\"More actions\">"
    ));
    assert!(html.contains("<span class=\"sr-only\">More actions for a</span>"));
    // Compact buttons keep their names for screen readers.
    assert!(html.contains("<span class=\"sr-only\">Delete a</span>"));
    assert!(html.contains("<span class=\"sr-only\">Download</span>"));
    assert!(html.contains("<span class=\"edit-text\">Edit</span><span class=\"sr-only\"> keywords</span>"));
    assert!(html.contains("class=\"field field-name\""), "per-field classes");
    assert!(html.contains("class=\"field field-emojis empty\""));

    // Sorted or filtered: no reordering, so no move entries.
    for q in ["?sort=likes", "?min_likes=1", "?sort=position&min_likes=0&view=compact"] {
        let html = owner.get(&format!("{page}{q}")).await.text();
        let movable = q.ends_with("compact");
        assert_eq!(html.contains("data-move-url"), movable, "{q}");
        assert_eq!(html.contains("Move down"), movable, "{q}");
        if !q.contains("min_likes=1") {
            assert!(html.contains("data-delete-url"), "{q}: still deletable");
            assert!(html.contains("data-edit-url"), "{q}");
        }
    }

    // Another user: open, download, like and copy only.
    let viewer = Browser::login(&s, &bob).await;
    let html = viewer.get(&page).await.text();
    let tag = card_tag(&html, &a.id);
    assert!(tag.contains("data-menu") && tag.contains("data-item-url") && tag.contains("data-download-url"));
    for attr in [
        "data-edit-url",
        "data-delete-url",
        "data-move-url",
        "data-first",
        "data-last",
    ] {
        assert!(!tag.contains(attr), "{attr} for a non-editor: {tag}");
    }
    assert!(
        html.contains("More actions for a"),
        "the menu button is there for everyone"
    );
    assert!(!html.contains("Delete a"));
    assert!(!html.contains("/field/name/edit"));

    // The htmx grid after a move carries the menu data but not the wrapper
    // (the view class lives outside the swapped grid).
    let r = owner
        .post_htmx(&format!("/sets/{}/items/{}/move", set.id, c.id), &[("dir", "top")])
        .await;
    assert_fragment(&r);
    let html = r.text();
    assert!(html.contains("id=\"set-items\""));
    assert!(!html.contains("items-section"));
    assert!(card_tag(&html, &c.id).contains(" data-first"));
    assert!(card_tag(&html, &b_item.id).contains(" data-last"));
    // Move to bottom works with the plain form too (the menu's no-htmx path).
    let r = owner
        .post_form(&format!("/sets/{}/items/{}/move", set.id, c.id), &[("dir", "bottom")])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let d = s.get(&alice, &routes::path(routes::SET, &[&set.id])).await.value();
    assert_eq!(d["items"][2]["id"], c.id.as_str());
}

#[tokio::test]
async fn content_you_cannot_see_is_a_404_page() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let bob = s.user("bob").await;
    let private = s.create_set(&bob, "secret", Visibility::Private).await;
    let unlisted = s.create_set(&bob, "link-only", Visibility::Unlisted).await;
    let public = s.create_set(&bob, "shared", Visibility::Public).await;
    let p_item = s.upload_one(&bob, &private.id, "p.png", png(1)).await;
    let u_item = s.upload_one(&bob, &unlisted.id, "u.png", png(2)).await;
    let job = job_for(&s, &p_item.media.id).await;
    let b = Browser::login(&s, &alice).await;

    for p in [
        format!("/sets/{}", private.id),
        format!("/sets/{}/settings", private.id),
        format!("/items/{}", p_item.id),
        format!("/items/{}/better", p_item.id),
        format!("/jobs/{job}"),
        "/sets/not-an-id".into(),
        "/items/aaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
    ] {
        let r = b.get(&p).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{p}");
        assert!(r.header("content-type").unwrap().starts_with("text/html"));
        let html = r.text();
        assert!(html.contains("Not found"), "{p}");
        assert!(!html.contains("secret"), "nothing about the set leaks");
        assert_clean_html(&html);
    }
    let r = b
        .post_form(&format!("/items/{}/like", p_item.id), &[("liked", "1")])
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // Unlisted: readable by link, but no edit controls and no edits.
    let r = b.get(&format!("/sets/{}", unlisted.id)).await;
    assert_page(&r);
    let html = r.text();
    assert!(!html.contains("/field/name/edit"));
    assert!(!html.contains(&format!("/sets/{}/settings", unlisted.id)));
    assert!(!html.contains("enctype=\"multipart/form-data\""));
    assert_page(&b.get(&format!("/items/{}", u_item.id)).await);
    let r = b.get(&format!("/sets/{}/settings", unlisted.id)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = b
        .post_form(&format!("/items/{}/field/name", u_item.id), &[("value", "mine now")])
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = b
        .post_multipart(
            &format!("/sets/{}/upload", unlisted.id),
            vec![("file", "x.png", png(9))],
            true,
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Likes work on unlisted items.
    let r = b
        .post_form(&format!("/items/{}/like", u_item.id), &[("liked", "1")])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);

    // Listing: public yes, unlisted and private no.
    let html = b.get("/sets?tab=public").await.text();
    assert!(html.contains("shared"));
    assert!(!html.contains("link-only"));
    assert!(!html.contains(&private.id));
    assert!(b.get(&format!("/sets/{}", public.id)).await.status.is_success());
}

#[tokio::test]
async fn admin_pages_are_for_admins_only() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let b = Browser::login(&s, &alice).await;
    for p in ["/admin", "/admin/users", "/admin/invitations", "/admin/audit"] {
        let r = b.get(p).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{p}");
        assert!(r.text().contains("Not allowed"));
        assert_clean_html(&r.text());
        let r = b.get_htmx(p).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        assert_eq!(r.header("hx-retarget").as_deref(), Some("#flash"));
        assert_fragment(&r);
    }
    let r = b
        .post_form("/admin/users", &[("username", "mallory"), ("role", "admin")])
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = b.post_form("/admin/invitations", &[("role", "admin")]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(
        !b.get("/").await.text().contains("href=\"/admin\""),
        "no admin link for users"
    );
}

#[tokio::test]
async fn forms_without_a_csrf_token_are_rejected() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let set = s.create_set(&alice, "cats", Visibility::Private).await;
    let b = Browser::login(&s, &alice).await;

    // urlencoded form
    let r = b.post_raw("/sets", &[("name", "No token")]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.text().contains("Request blocked"));
    let r = b
        .post_raw("/sets", &[("csrf_token", "wrong"), ("name", "Wrong token")])
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // htmx without the header
    let r = common::read(
        b.client
            .post(b.url("/sets"))
            .header("HX-Request", "true")
            .form(&[("name", "x")])
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // A valid token from another site is still rejected.
    let r = common::read(
        b.client
            .post(b.url("/sets"))
            .header("Origin", "http://evil.example")
            .form(&[("csrf_token", b.csrf.as_deref().unwrap()), ("name", "x")])
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // With the token it works.
    let r = b.post_form("/sets", &[("name", "With token")]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());

    // Multipart: the token must be the first field.
    let path = format!("/sets/{}/upload", set.id);
    let r = b.post_multipart(&path, vec![("file", "a.png", png(1))], false).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let mut form =
        reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(png(1)).file_name("a.png"));
    form = form.text("csrf_token", b.csrf.clone().unwrap());
    let r = common::read(b.client.post(b.url(&path)).multipart(form).send().await.unwrap()).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "token after the file");
    let form = reqwest::multipart::Form::new()
        .text("csrf_token", "wrong")
        .part("file", reqwest::multipart::Part::bytes(png(1)).file_name("a.png"));
    let r = common::read(b.client.post(b.url(&path)).multipart(form).send().await.unwrap()).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "wrong token");
    let r = b.post_multipart(&path, vec![("file", "a.png", png(1))], true).await;
    assert_page(&r);
    assert!(r.text().contains("1 file added"), "{}", r.text());
    // htmx multipart: header instead of the field.
    let form = reqwest::multipart::Form::new().part("file", reqwest::multipart::Part::bytes(png(2)).file_name("b.png"));
    let r = common::read(
        b.client
            .post(b.url(&path))
            .header("HX-Request", "true")
            .header("X-CSRF-Token", b.csrf.clone().unwrap())
            .multipart(form)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_fragment(&r);
    assert!(r.text().contains("1 file added"));
    // The set page's grid is refreshed out of band.
    assert!(r.text().contains("id=\"set-items\" hx-swap-oob=\"true\""));
}

#[tokio::test]
async fn htmx_requests_get_fragments() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let set = s.create_set(&alice, "cats", Visibility::Public).await;
    let a = s.upload_one(&alice, &set.id, "a.png", png(1)).await;
    let b_item = s.upload_one(&alice, &set.id, "b.png", png(2)).await;
    let b = Browser::login(&s, &alice).await;

    // Like toggle.
    let r = b
        .post_htmx(&format!("/items/{}/like", a.id), &[("liked", "1"), ("next", "/")])
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_fragment(&r);
    assert!(r.text().contains("aria-pressed=\"true\""));
    assert!(r.text().contains("<span class=\"count\">1</span>"));
    let r = b.post_htmx(&format!("/items/{}/like", a.id), &[("liked", "0")]).await;
    assert!(r.text().contains("aria-pressed=\"false\""));

    // Inline edit: form, save, view; edited fields become user-sourced.
    let r = b.get_htmx(&format!("/items/{}/field/keywords/edit", a.id)).await;
    assert_fragment(&r);
    assert!(r.text().contains("<form") && r.text().contains("name=\"value\""));
    let r = b
        .post_htmx(
            &format!("/items/{}/field/keywords", a.id),
            &[("value", "cat, cute cat ,")],
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_fragment(&r);
    assert!(r.text().contains("<li>cute cat</li>"));
    let v = s.get(&alice, &routes::path(routes::ITEM, &[&a.id])).await.value();
    let item: Item = serde_json::from_value(v["item"].clone()).unwrap();
    assert_eq!(item.keywords, vec!["cat", "cute cat"]);
    // Invalid input re-renders the form with the error (422).
    let long = "x".repeat(300);
    let r = b
        .post_htmx(&format!("/items/{}/field/name", a.id), &[("value", &long)])
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_fragment(&r);
    assert!(r.text().contains("error-text"));
    // Cancel returns the view.
    let r = b.get_htmx(&format!("/items/{}/field/name", a.id)).await;
    assert_fragment(&r);

    // AI suggestions: marked, confirmable, confirming keeps the value.
    sqlx::query("UPDATE items SET vibe = 'tired', vibe_source = 'ai' WHERE id = ?")
        .bind(&a.id)
        .execute(&s.state.db)
        .await
        .unwrap();
    let html = b.get(&format!("/items/{}", a.id)).await.text();
    assert!(html.contains("abbr class=\"ai\""));
    assert!(html.contains(&format!("/items/{}/confirm/vibe", a.id)));
    let r = b.post_htmx(&format!("/items/{}/confirm/vibe", a.id), &[]).await;
    assert_fragment(&r);
    assert!(!r.text().contains("ai-field"));
    assert!(r.text().contains("tired"));
    let v = s.get(&alice, &routes::path(routes::ITEM, &[&a.id])).await.value();
    assert_eq!(v["item"]["vibe_source"], "user");
    assert_eq!(v["item"]["vibe"], "tired");

    // Reorder returns the grid.
    let r = b
        .post_htmx(&format!("/sets/{}/items/{}/move", set.id, a.id), &[("dir", "down")])
        .await;
    assert_fragment(&r);
    assert!(r.text().contains("id=\"set-items\""));
    let d = s.get(&alice, &routes::path(routes::SET, &[&set.id])).await.value();
    assert_eq!(d["items"][0]["id"], b_item.id.as_str());
    // Fragments point `next` fields at the page the user is on (HX-Current-URL).
    let r = common::read(
        b.client
            .post(b.url(&format!("/sets/{}/items/{}/move", set.id, a.id)))
            .header("HX-Request", "true")
            .header("X-CSRF-Token", b.csrf.clone().unwrap())
            .header("HX-Current-URL", format!("{}/sets/{}#top", s.base, set.id))
            .form(&[("dir", "up")])
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_fragment(&r);
    assert!(r.text().contains(&format!("name=\"next\" value=\"/sets/{}\"", set.id)));
    assert!(!r.text().contains("value=\"/sets/{}/items/"), "not the fragment URL");

    // History restoration (back button after hx-push-url) gets full pages.
    let r = common::read(
        b.client
            .get(b.url("/popular"))
            .header("HX-Request", "true")
            .header("HX-History-Restore-Request", "true")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_page(&r);

    // Search "more" and popular filters.
    let r = b.get_htmx("/search?q=png").await;
    assert_fragment(&r);
    assert!(r.text().contains("id=\"results\""));
    let r = b.get_htmx("/popular?scope=mine&window=day").await;
    assert_fragment(&r);
    assert!(r.text().contains("id=\"popular-cols\""));

    // Errors are retargeted into #flash.
    let r = b.get_htmx("/items/aaaaaaaaaaaaaaaaaaaaaaaaaa").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.header("hx-retarget").as_deref(), Some("#flash"));
    assert_eq!(r.header("hx-reswap").as_deref(), Some("innerHTML"));
    assert_fragment(&r);

    // Delete answers with nothing (the card is removed client-side).
    let r = b.post_htmx(&format!("/items/{}/delete", a.id), &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.text().is_empty());
    assert_eq!(
        s.get(&alice, &routes::path(routes::ITEM, &[&a.id])).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn user_content_is_escaped() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let evil = "<script>alert(1)</script>\"q'";
    let set: stickpile_api::Set = s
        .post(
            &alice,
            routes::SETS,
            json!({"slug": "evil", "name": evil, "description": evil, "visibility": "public"}),
        )
        .await
        .expect(201)
        .json();
    let item = s.upload_one(&alice, &set.id, "x.png", png(1)).await;
    s.patch(
        &alice,
        &routes::path(routes::ITEM, &[&item.id]),
        json!({"name": evil, "keywords": [evil], "tags": [evil]}),
    )
    .await
    .expect(200);
    let b = Browser::login(&s, &alice).await;
    for p in [
        "/".to_string(),
        "/sets".into(),
        format!("/sets/{}", set.id),
        format!("/sets/{}/settings", set.id),
        format!("/items/{}", item.id),
        format!("/items/{}?edit=1", item.id),
        "/search?q=alert".into(),
        "/search?q=%3Cscript%3E".into(),
    ] {
        let r = b.get(&p).await;
        assert_page(&r);
        let html = r.text();
        assert!(!html.contains("<script>alert"), "{p}");
        assert!(!html.contains("\"q'"), "{p}: raw quote");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;&quot;q'"), "{p}");
    }
    // Attributes too (alt text and the delete confirmation).
    let html = b.get(&format!("/sets/{}", set.id)).await.text();
    assert!(html.contains("alt=\"&lt;script&gt;alert(1)&lt;/script&gt;&quot;q'\""));
    let r = b.get_htmx(&format!("/items/{}/field/name/edit", item.id)).await;
    assert!(r
        .text()
        .contains("value=\"&lt;script&gt;alert(1)&lt;/script&gt;&quot;q'\""));
}

#[tokio::test]
async fn csp_and_static_assets() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let b = Browser::login(&s, &alice).await;
    let r = b.get("/").await;
    assert_eq!(
        r.header("content-security-policy").as_deref(),
        Some(stickpile_server::security::CSP)
    );
    assert_eq!(r.header("x-frame-options").as_deref(), Some("DENY"));
    let html = r.text();
    assert!(html.contains("name=\"htmx-config\""));
    assert!(html.contains("&quot;allowEval&quot;:false"));
    assert!(html.contains("&quot;includeIndicatorStyles&quot;:false"));
    for (name, ct) in [
        ("app.css", "text/css"),
        ("app.js", "text/javascript"),
        ("upload.js", "text/javascript"),
        ("htmx.min.js", "text/javascript"),
        ("favicon.svg", "image/svg+xml"),
    ] {
        let marker = format!("/static/{name}?v=");
        let start = html
            .find(&marker)
            .unwrap_or_else(|| panic!("{name} linked with a version"));
        let end = html[start..].find('"').unwrap() + start;
        let r = Browser::anonymous(&s).get(&html[start..end]).await;
        assert_eq!(r.status, StatusCode::OK, "{name}");
        assert!(r.header("content-type").unwrap().starts_with(ct), "{name}");
        assert!(r.header("cache-control").unwrap().contains("immutable"));
        assert!(!r.body.is_empty());
    }
    let css = Browser::anonymous(&s).get("/static/app.css").await.text();
    assert!(css.contains("prefers-color-scheme: dark"));
    assert!(css.contains(":focus-visible"));
    assert!(
        css.contains("[hidden] { display: none !important; }"),
        "JS-only buttons stay hidden"
    );
    assert!(css.contains(".items-section.compact .editor-grid"));
    assert!(css.contains(".ctx-menu"));
    let js = Browser::anonymous(&s).get("/static/app.js").await.text();
    for needle in [
        "\"contextmenu\"",
        "role\", \"menu\"",
        "\"menuitem\"",
        "data-view-toggle",
        "localStorage",
        "F10",
    ] {
        assert!(js.contains(needle), "app.js: {needle}");
    }
}

#[tokio::test]
async fn errors_render_as_html_pages() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let b = Browser::login(&s, &alice).await;
    let r = b.get("/no/such/page").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(r.header("content-type").unwrap().starts_with("text/html"));
    let html = r.text();
    assert!(html.contains("<!DOCTYPE html>") && html.contains("Not found"));
    assert!(html.contains("alice"), "the layout knows who is signed in");
    assert_clean_html(&html);
    let r = b.get("/logout").await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(r.text().contains("<!DOCTYPE html>"));
    let r = Browser::anonymous(&s).get("/nope").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(r.text().contains("sign in"));
    // Bad form input is a 4xx HTML page too, never JSON.
    let r = b.get(&format!("/search?q={}", "x".repeat(1100))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.header("content-type").unwrap().starts_with("text/html"));
    // The API keeps answering JSON.
    let r = b.get("/api/v1/nope").await;
    assert!(r.header("content-type").unwrap().starts_with("application/json"));
}
