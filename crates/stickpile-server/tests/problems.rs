//! Problems report: the API (per set, own sets, server scope for admins,
//! dismissals) and the web UI (the set page's section, the Problems tab,
//! actions), with the permission rules: owner, other users, admins, private
//! sets.

mod common;
mod web_common;

use common::{png, png_near, TestServer, TestUser};
use reqwest::StatusCode;
use serde_json::json;
use stickpile_api::{routes, Item, ProblemKind, ProblemReport, SetProblems, Visibility};
use web_common::*;

/// Title of a near-duplicate card (the words also appear in hints and the
/// kind filter).
const NEAR_TITLE: &str = "-title\">Near-duplicates</strong>";

fn set_problems(id: &str) -> String {
    routes::path(routes::SET_PROBLEMS, &[id])
}

fn dismissal(set: &str, problem: &str) -> String {
    routes::path(routes::SET_PROBLEM_DISMISSAL, &[set, problem])
}

async fn problems_of(s: &TestServer, u: &TestUser, set: &str, query: &str) -> SetProblems {
    s.get(u, &format!("{}{query}", set_problems(set)))
        .await
        .expect(200)
        .json()
}

fn kinds(sp: &SetProblems) -> Vec<ProblemKind> {
    sp.problems.iter().map(|p| p.kind).collect()
}

/// The same picture as PNG with other compression settings: different
/// upload bytes, the same sanitised original.
fn png_recompressed(seed: u64) -> Vec<u8> {
    use image::ImageEncoder;
    let img = common::image(64, 64, seed, 0);
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    )
    .write_image(img.as_raw(), 64, 64, image::ExtendedColorType::Rgba8)
    .unwrap();
    out
}

#[tokio::test]
async fn owners_see_duplicates_and_others_do_not() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let bob = s.user("bob").await;
    let set = s.create_set(&alice, "cats", Visibility::Public).await;
    let private = s.create_set(&alice, "secret", Visibility::Private).await;

    // An empty set is reported (as info).
    let sp = problems_of(&s, &alice, &set.id, "").await;
    assert_eq!(kinds(&sp), [ProblemKind::EmptySet]);
    assert!(sp.can_dismiss && sp.can_edit);

    let a = s.upload_one(&alice, &set.id, "a.png", png(1)).await;
    let b = s.upload_one(&alice, &set.id, "b.png", png_near(1)).await;
    let c = s.upload_one(&alice, &set.id, "c.png", png_recompressed(1)).await;
    let other = s.upload_one(&alice, &set.id, "other.png", png(2)).await;
    assert_ne!(a.media.id, c.media.id, "different uploads");
    let sp = problems_of(&s, &alice, &set.id, "").await;
    let exact: Vec<_> = sp
        .problems
        .iter()
        .filter(|p| p.kind == ProblemKind::ExactDuplicate)
        .collect();
    assert_eq!(exact.len(), 1, "{:?}", kinds(&sp));
    let ids: Vec<&str> = exact[0].items.iter().map(|i| i.item_id.as_str()).collect();
    assert_eq!(ids, [a.id.as_str(), c.id.as_str()]);
    let near: Vec<_> = sp
        .problems
        .iter()
        .filter(|p| p.kind == ProblemKind::NearDuplicate)
        .collect();
    assert_eq!(near.len(), 2, "a~b and b~c: {:?}", sp.problems);
    assert!(near.iter().all(|p| p.items.iter().any(|i| i.item_id == b.id)));
    assert!(
        !sp.problems
            .iter()
            .any(|p| p.items.iter().any(|i| i.item_id == other.id) && p.kind == ProblemKind::NearDuplicate),
        "an unrelated picture is no duplicate"
    );
    // 64 px pictures are very small.
    assert!(kinds(&sp).contains(&ProblemKind::LowResolution));
    assert!(sp.counts.warning >= 3, "{:?}", sp.counts);
    // Filters.
    let only = problems_of(&s, &alice, &set.id, "?kind=near_duplicate").await;
    assert_eq!(kinds(&only), [ProblemKind::NearDuplicate, ProblemKind::NearDuplicate]);
    assert_eq!(only.counts, sp.counts, "counts ignore the filters");
    let errors = problems_of(&s, &alice, &set.id, "?severity=error").await;
    assert!(errors.problems.is_empty());
    s.get(&alice, &format!("{}?severity=bad", set_problems(&set.id)))
        .await
        .expect(400);
    s.get(&alice, &format!("{}?kind=bad", set_problems(&set.id)))
        .await
        .expect(400);

    // Other users: 403 on a set they can see, 404 on a private one.
    assert_eq!(
        s.get(&bob, &set_problems(&set.id)).await.expect(403).code(),
        "forbidden"
    );
    s.get(&bob, &set_problems(&private.id)).await.expect(404);
    s.get(&bob, &set_problems("nope")).await.expect(404);
    s.anon_get(&set_problems(&set.id)).await.expect(401);
    s.put(&bob, &dismissal(&set.id, &near[0].id), json!({}))
        .await
        .expect(403);
    s.put(&bob, &dismissal(&private.id, &near[0].id), json!({}))
        .await
        .expect(404);
    s.delete(&bob, &dismissal(&set.id, &near[0].id)).await.expect(403);

    // Bob's own report never shows Alice's sets.
    let report: ProblemReport = s.get(&bob, routes::PROBLEMS).await.expect(200).json();
    assert!(report.sets.is_empty());
    assert_eq!(report.sets_checked, 0);
    let r = s
        .get(&bob, &format!("{}?scope=server", routes::PROBLEMS))
        .await
        .expect(403);
    assert_eq!(r.code(), "forbidden");
}

#[tokio::test]
async fn dismissals_hide_problems_until_the_items_change() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let set = s.create_set(&alice, "words", Visibility::Private).await;
    let item = s.upload_one(&alice, &set.id, "a.png", png(3)).await;
    s.patch(
        &alice,
        &routes::path(routes::ITEM, &[&item.id]),
        json!({"emojis": ["cat", "😺"]}),
    )
    .await
    .expect(200);
    let sp = problems_of(&s, &alice, &set.id, "").await;
    let p = sp
        .problems
        .iter()
        .find(|p| p.kind == ProblemKind::NotEmoji)
        .expect("not_emoji");
    assert_eq!(p.items[0].detail.as_deref(), Some("cat"));

    s.put(&alice, &dismissal(&set.id, &p.id), json!({})).await.expect(204);
    s.put(&alice, &dismissal(&set.id, &p.id), json!({})).await.expect(204); // idempotent
    let after = problems_of(&s, &alice, &set.id, "").await;
    assert!(!after.problems.iter().any(|q| q.id == p.id));
    assert_eq!(after.counts.dismissed, 1);
    let with = problems_of(&s, &alice, &set.id, "?dismissed=true").await;
    assert!(with.problems.iter().any(|q| q.id == p.id && q.dismissed));
    // Restored, then dismissed again.
    s.delete(&alice, &dismissal(&set.id, &p.id)).await.expect(204);
    s.delete(&alice, &dismissal(&set.id, &p.id)).await.expect(204);
    assert!(problems_of(&s, &alice, &set.id, "")
        .await
        .problems
        .iter()
        .any(|q| q.id == p.id));
    s.put(&alice, &dismissal(&set.id, &p.id), json!({})).await.expect(204);

    // Unknown or malformed ids.
    let r = s
        .put(
            &alice,
            &dismissal(&set.id, "near_duplicate-0123456789abcdef"),
            json!({}),
        )
        .await
        .expect(404);
    assert!(r.text().contains("may have changed"), "{}", r.text());
    s.put(&alice, &dismissal(&set.id, "whatever"), json!({}))
        .await
        .expect(404);

    // Editing the emojis makes it a new problem: shown again.
    s.patch(
        &alice,
        &routes::path(routes::ITEM, &[&item.id]),
        json!({"emojis": ["cat", "dog"]}),
    )
    .await
    .expect(200);
    let changed = problems_of(&s, &alice, &set.id, "").await;
    let q = changed
        .problems
        .iter()
        .find(|q| q.kind == ProblemKind::NotEmoji)
        .expect("back");
    assert_ne!(q.id, p.id);
    assert!(!q.dismissed);
}

#[tokio::test]
async fn admins_see_every_set_without_private_items() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let root = s.admin("root").await;
    let private = s.create_set(&alice, "secret", Visibility::Private).await;
    let public = s.create_set(&alice, "shared", Visibility::Public).await;
    for set in [&private, &public] {
        s.upload_one(&alice, &set.id, "a.png", png(4)).await;
        s.upload_one(&alice, &set.id, "b.png", png_near(4)).await;
    }

    let report: ProblemReport = s
        .get(&root, &format!("{}?scope=server", routes::PROBLEMS))
        .await
        .expect(200)
        .json();
    assert_eq!(report.scope, "server");
    assert_eq!(report.sets_checked, 2);
    let by_name = |n: &str| report.sets.iter().find(|sp| sp.set.name == n).expect(n);
    let secret = by_name("secret");
    assert!(!secret.can_dismiss && !secret.can_edit);
    assert_eq!(secret.set.owner_username, "alice");
    assert_eq!(secret.set.cover_item_id, None);
    let items: Vec<_> = secret.problems.iter().flat_map(|p| &p.items).collect();
    assert!(!items.is_empty());
    assert!(items
        .iter()
        .all(|i| i.hidden && i.name.is_empty() && i.detail.is_none()));
    let shared = by_name("shared");
    assert!(shared
        .problems
        .iter()
        .flat_map(|p| &p.items)
        .all(|i| !i.hidden && (i.name == "a" || i.name == "b")));
    // Their own scope has none of Alice's sets.
    let mine: ProblemReport = s.get(&root, routes::PROBLEMS).await.expect(200).json();
    assert_eq!(mine.sets_checked, 0);
    // Per set, read-only; dismissing is for the owner.
    let sp = problems_of(&s, &root, &private.id, "").await;
    assert!(sp.problems.iter().flat_map(|p| &p.items).all(|i| i.hidden));
    let id = &sp.problems[0].id;
    s.put(&root, &dismissal(&private.id, id), json!({})).await.expect(404);
    s.put(&root, &dismissal(&public.id, id), json!({})).await.expect(403);
    // Admins still cannot open the private set or its files.
    s.get(&root, &routes::path(routes::SET, &[&private.id]))
        .await
        .expect(404);
}

#[tokio::test]
async fn same_file_in_another_own_set_is_noted_once() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let first = s.create_set(&alice, "first", Visibility::Private).await;
    let second = s.create_set(&alice, "second", Visibility::Private).await;
    s.upload_one(&alice, &first.id, "a.png", png(7)).await;
    s.upload_one(&alice, &second.id, "a.png", png(7)).await;
    let sp = problems_of(&s, &alice, &second.id, "").await;
    let p = sp
        .problems
        .iter()
        .find(|p| p.kind == ProblemKind::InOtherSet)
        .expect("in_other_set");
    assert_eq!(p.items[0].detail.as_deref(), Some("also in first"));
    assert!(!kinds(&problems_of(&s, &alice, &first.id, "").await).contains(&ProblemKind::InOtherSet));
    // Bob's copy of the same file is not Alice's problem.
    let bob = s.user("bob").await;
    let bobs = s.create_set(&bob, "bobs", Visibility::Public).await;
    s.upload_one(&bob, &bobs.id, "a.png", png(7)).await;
    assert!(!kinds(&problems_of(&s, &bob, &bobs.id, "").await).contains(&ProblemKind::InOtherSet));

    let report: ProblemReport = s.get(&alice, routes::PROBLEMS).await.expect(200).json();
    assert_eq!(report.sets_checked, 2);
    assert!(report.sets.iter().all(|sp| sp.owner_id == alice.id));
}

#[tokio::test]
async fn telegram_blockers_for_a_published_set() {
    let token_dir = tempfile::tempdir().unwrap();
    let token = token_dir.path().join("token");
    std::fs::write(&token, "123:abc\n").unwrap();
    let s = TestServer::with_config(|c| {
        c.telegram.enabled = true;
        c.telegram.bot_token_file = Some(token);
        // Never reached: problems only read the database.
        c.telegram.api_url = "http://127.0.0.1:9".into();
    })
    .await;
    let alice = s.user("alice").await;
    let set = s.create_set(&alice, "pack", Visibility::Private).await;
    let a: Item = s.upload_one(&alice, &set.id, "a.png", png(11)).await;
    let b: Item = s.upload_one(&alice, &set.id, "b.png", png(12)).await;
    s.patch(&alice, &routes::path(routes::ITEM, &[&a.id]), json!({"emojis": ["😼"]}))
        .await
        .expect(200);
    // Not published: missing emojis are fine.
    assert!(!kinds(&problems_of(&s, &alice, &set.id, "").await).contains(&ProblemKind::NoEmoji));
    // Published with only `a` on Telegram, then a failed export.
    let mapping = json!({ a.id.clone(): [a.media.sha256.clone(), "fu-a"] }).to_string();
    sqlx::query("UPDATE sets SET telegram_pack = 'pack_by_bot', telegram_mapping = ? WHERE id = ?")
        .bind(mapping)
        .bind(&set.id)
        .execute(&s.state.db)
        .await
        .unwrap();
    let sp = problems_of(&s, &alice, &set.id, "").await;
    let ks = kinds(&sp);
    assert!(ks.contains(&ProblemKind::NoEmoji), "{ks:?}");
    let no_emoji = sp.problems.iter().find(|p| p.kind == ProblemKind::NoEmoji).unwrap();
    assert_eq!(no_emoji.items.len(), 1);
    assert_eq!(no_emoji.items[0].item_id, b.id);
    let outdated = sp
        .problems
        .iter()
        .find(|p| p.kind == ProblemKind::TelegramOutdated)
        .unwrap();
    assert!(
        outdated.message.contains("1 item not on Telegram yet"),
        "{}",
        outdated.message
    );
    sqlx::query(
        "INSERT INTO jobs (id, kind, payload, subject, status, attempts, error, owner_id, run_after, created_at, finished_at) \
         VALUES ('j1', 'telegram_export', '{}', ?, 'failed', 3, 'Bad Request: STICKER_PNG_DIMENSIONS', ?, 0, 1, 2)",
    )
    .bind(&set.id)
    .bind(&alice.id)
    .execute(&s.state.db)
    .await
    .unwrap();
    let sp = problems_of(&s, &alice, &set.id, "").await;
    assert_eq!(sp.problems[0].kind, ProblemKind::TelegramFailed, "errors first");
    assert!(sp.problems[0].message.contains("STICKER_PNG_DIMENSIONS"));
    assert!(sp.counts.error >= 1);
}

// ---------------------------------------------------------------------------
// Web UI
// ---------------------------------------------------------------------------

#[tokio::test]
async fn set_page_section_and_actions() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let bob = s.user("bob").await;
    let set = s.create_set(&alice, "cats", Visibility::Public).await;
    let private = s.create_set(&alice, "secret", Visibility::Private).await;
    let a = s.upload_one(&alice, &set.id, "a.png", png(21)).await;
    let b = s.upload_one(&alice, &set.id, "b.png", png_near(21)).await;
    let page = format!("/sets/{}", set.id);
    let block = format!("/sets/{}/problems", set.id);

    let ab = Browser::login(&s, &alice).await;
    let html = ab.get(&page).await.expect(200).text();
    assert!(html.contains("id=\"problems\""), "owner sees the section");
    assert!(html.contains(&format!("hx-get=\"{block}\"")), "loaded lazily");
    assert!(html.contains("href=\"/problems\""), "nav entry");

    // The block (htmx) and the page (no JavaScript).
    let r = ab.get_htmx(&block).await.expect(200);
    assert_fragment(&r);
    let frag = r.text();
    assert!(frag.contains(NEAR_TITLE), "{frag}");
    assert!(
        frag.contains(&format!("/api/v1/items/{}/file/thumb", a.id)),
        "thumbnails side by side"
    );
    assert!(frag.contains(&format!("/api/v1/items/{}/file/thumb", b.id)));
    assert!(frag.contains(">Not a problem<span"));
    assert!(frag.contains(&format!("/sets/{}/problems/remove/{}", set.id, b.id)));
    assert_page(&ab.get(&block).await);

    // Others: no section, 403 / 404 for the list.
    let bb = Browser::login(&s, &bob).await;
    let html = bb.get(&page).await.expect(200).text();
    assert!(!html.contains("id=\"problems\""));
    assert_eq!(bb.get_htmx(&block).await.status, StatusCode::FORBIDDEN);
    assert_eq!(
        bb.get(&format!("/sets/{}/problems", private.id)).await.status,
        StatusCode::NOT_FOUND
    );

    // Dismiss with htmx: the block comes back without it.
    let sp = problems_of(&s, &alice, &set.id, "?kind=near_duplicate").await;
    let pid = sp.problems[0].id.clone();
    let action = format!("/sets/{}/problems/{pid}/dismiss", set.id);
    assert_eq!(
        bb.post_htmx(&action, &[("place", "set")]).await.status,
        StatusCode::FORBIDDEN,
        "only the owner"
    );
    let r = ab.post_htmx_from(&action, &[("place", "set")], &page).await.expect(200);
    assert_fragment(&r);
    assert!(r.text().contains("Show 1 dismissed"), "{}", r.text());
    assert!(!r.text().contains(&pid));
    // ...and back, from the "show dismissed" view, without JavaScript.
    let restore = format!("/sets/{}/problems/{pid}/restore", set.id);
    let r = ab
        .post_form(&restore, &[("place", "set"), ("dismissed", "1"), ("next", &block)])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(location(&r), block);
    assert_eq!(problems_of(&s, &alice, &set.id, "").await.counts.dismissed, 0);
    // A plain form without the token is refused.
    assert_eq!(
        ab.post_raw(&action, &[("place", "set")]).await.status,
        StatusCode::FORBIDDEN
    );

    // Deleting one of the pair drops its grid card and the problem.
    let r = ab
        .post_htmx_from(
            &format!("/sets/{}/problems/remove/{}", set.id, b.id),
            &[("place", "set")],
            &page,
        )
        .await
        .expect(200);
    let frag = r.text();
    assert!(
        frag.contains(&format!("id=\"item-{}\" hx-swap-oob=\"delete\"", b.id)),
        "{frag}"
    );
    assert!(!frag.contains(NEAR_TITLE), "{frag}");
    s.get(&alice, &routes::path(routes::ITEM, &[&b.id])).await.expect(404);
    // Only items of this set, only by the owner.
    let other = s.create_set(&alice, "other", Visibility::Private).await;
    let elsewhere = s.upload_one(&alice, &other.id, "x.png", png(22)).await;
    assert_eq!(
        ab.post_htmx(&format!("/sets/{}/problems/remove/{}", set.id, elsewhere.id), &[])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bb.post_htmx(&format!("/sets/{}/problems/remove/{}", set.id, a.id), &[])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn problems_tab_for_users_and_admins() {
    let s = TestServer::start().await;
    let alice = s.user("alice").await;
    let root = s.admin("root").await;
    let set = s.create_set(&alice, "alice-cats", Visibility::Private).await;
    s.upload_one(&alice, &set.id, "a.png", png(31)).await;
    s.upload_one(&alice, &set.id, "b.png", png_near(31)).await;

    let ab = Browser::login(&s, &alice).await;
    let r = ab.get("/problems").await;
    assert_page(&r);
    let html = r.text();
    assert!(html.contains("alice-cats"));
    assert!(html.contains(NEAR_TITLE));
    assert!(html.contains("aria-current=\"page\">Problems<"), "nav entry is current");
    assert!(!html.contains("Whole server"), "no scope switch for users");
    assert_eq!(ab.get("/problems?scope=server").await.status, StatusCode::FORBIDDEN);
    let filtered = ab.get("/problems?severity=error").await.text();
    assert!(!filtered.contains(NEAR_TITLE));
    assert!(filtered.contains("Nothing matches these filters"));

    // Dismiss from the tab: the set's group comes back.
    let sp = problems_of(&s, &alice, &set.id, "?kind=near_duplicate").await;
    let action = format!("/sets/{}/problems/{}/dismiss", set.id, sp.problems[0].id);
    let r = ab
        .post_htmx_from(&action, &[("place", "tab"), ("kind", "near_duplicate")], "/problems")
        .await
        .expect(200);
    assert!(r.text().contains(&format!("id=\"problems-set-{}\"", set.id)));
    assert!(r.text().contains("Nothing left to show here"));

    let rb = Browser::login(&s, &root).await;
    let html = rb.get("/problems").await.text();
    assert!(html.contains("Whole server"));
    assert!(!html.contains("alice-cats"));
    let r = rb.get("/problems?scope=server&dismissed=1").await;
    assert_page(&r);
    let html = r.text();
    assert!(html.contains("alice-cats") && html.contains("by alice"));
    assert!(html.contains("Private item"));
    assert!(!html.contains("/file/thumb"), "no thumbnails of private items");
    assert!(
        !html.contains(">Not a problem<span"),
        "admins cannot dismiss others' problems"
    );
    assert!(
        !html.contains(&format!("href=\"/sets/{}\"", set.id)),
        "no link into a private set"
    );
}

#[tokio::test]
async fn openapi_lists_the_problem_routes() {
    let s = TestServer::start().await;
    let spec = s.anon_get(routes::OPENAPI).await.expect(200).text();
    for p in [routes::PROBLEMS, routes::SET_PROBLEMS, routes::SET_PROBLEM_DISMISSAL] {
        assert!(spec.contains(&format!("\"{p}\"")), "{p}");
    }
    assert!(spec.contains("\"ProblemReport\""));
}
