//! The collaboration pages through their forms, the way a browser without
//! JavaScript uses them: contributors on the set page, proposing stickers,
//! changes and a cover, and deciding on the Collaboration tab.

mod common;
mod web_common;

use common::{png, TestServer};
use reqwest::StatusCode;
use stickpile_api::{routes, Permissions, Proposal, ProposalStatus, SetDetail, Visibility};
use web_common::*;

fn proposal_id(loc: &str) -> String {
    loc.strip_prefix("/proposals/").expect("a proposal page").to_string()
}

#[tokio::test]
async fn collaborating_through_the_web_ui() {
    let srv = TestServer::start().await;
    let alice_u = srv.user("alice").await;
    let bob_u = srv.user("bob").await;
    let eve_u = srv.user("eve").await;
    let set = srv.create_set(&alice_u, "cats", Visibility::Public).await;
    let item = srv.upload_one(&alice_u, &set.id, "cat.png", png(1)).await;
    let set_page = format!("/sets/{}", set.id);

    let alice = Browser::login(&srv, &alice_u).await;
    let bob = Browser::login(&srv, &bob_u).await;
    let eve = Browser::login(&srv, &eve_u).await;

    // The owner adds bob with two permissions.
    let r = alice
        .post_form(
            &format!("/sets/{}/contributors", set.id),
            &[("username", "bob"), ("add", "on"), ("review", "on")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let page = alice.get(&set_page).await;
    assert_page(&page);
    assert!(page.text().contains("Contributors") && page.text().contains("@bob"));
    let page = bob.get(&set_page).await;
    assert_page(&page);
    assert!(page
        .text()
        .contains("You contribute to this pack: Add stickers, Review proposals."));
    assert!(page.text().contains("Add items"), "bob may upload");
    assert!(
        !page.text().contains("Replace from a set directory"),
        "but not replace the set"
    );
    let r = bob
        .post_multipart(
            &format!("/sets/{}/upload", set.id),
            vec![("file", "bob.png", png(4))],
            true,
        )
        .await;
    assert_page(&r);
    assert!(r.text().contains("1 file added"), "{}", r.text());
    let r = eve
        .post_multipart(
            &format!("/sets/{}/upload", set.id),
            vec![("file", "eve.png", png(5))],
            true,
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let page = eve.get(&set_page).await;
    assert!(
        !page.text().contains("id=\"contributors\""),
        "visitors do not see contributors"
    );
    assert!(page.text().contains("Propose…"));
    // Changing bob's permissions, then the nav badge stays empty for him.
    let r = alice
        .post_form(
            &format!("/sets/{}/contributors/bob", set.id),
            &[("add", "on"), ("review", "on"), ("edit_own", "on")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);

    // eve proposes two stickers (multipart, the token first).
    let r = eve
        .post_multipart_fields(
            &format!("/sets/{}/propose/items", set.id),
            &[("message", "two more")],
            vec![("file", "a.png", png(2)), ("file", "b.png", png(3))],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let stickers = proposal_id(&location(&r));
    // A sticker suggestion and a set change.
    let suggest = eve.get(&format!("/items/{}/suggest", item.id)).await;
    assert_page(&suggest);
    let r = eve
        .post_form(
            &format!("/items/{}/suggest", item.id),
            &[
                ("name", "Grumpy"),
                ("keywords", ""),
                ("emojis", "😾"),
                ("tags", "grumpy, cat"),
                ("vibe", ""),
                ("message", ""),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let suggestion = proposal_id(&location(&r));
    // Unchanged values are an error shown on the form.
    let r = eve
        .post_form(
            &format!("/sets/{}/propose/changes", set.id),
            &[("name", "cats"), ("description", ""), ("cover", ""), ("message", "")],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("nothing to change"), "{}", r.text());
    let r = eve
        .post_form(
            &format!("/sets/{}/propose/changes", set.id),
            &[
                ("name", "Cats!"),
                ("description", ""),
                ("cover", ""),
                ("message", "louder"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let rename = proposal_id(&location(&r));

    // The owner sees three on the tab, with the badge.
    let badge = alice.get_htmx("/collaboration/badge").await;
    assert!(badge.text().contains(">3<"), "{}", badge.text());
    let tab = alice.get("/collaboration").await;
    assert_page(&tab);
    for id in [&stickers, &suggestion, &rename] {
        assert!(tab.text().contains(&format!("proposal-{id}")), "{id} listed");
    }
    // bob reviews too: he accepts the stickers, alice the suggestion, and
    // declines the rename with a note.
    let r = bob
        .post_form(
            &format!("/proposals/{stickers}/accept"),
            &[("note", "welcome"), ("next", "/collaboration")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    assert_eq!(location(&r), "/collaboration");
    let r = alice
        .post_form(&format!("/proposals/{suggestion}/accept"), &[("note", "")])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let r = alice
        .post_form(&format!("/proposals/{rename}/decline"), &[("note", "keep it quiet")])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    // eve's side.
    let mine = eve.get(&format!("/proposals/{rename}")).await;
    assert_page(&mine);
    assert!(mine.text().contains("keep it quiet") && mine.text().contains("Declined"));
    let d: SetDetail = srv
        .get(&alice_u, &routes::path(routes::SET, &[&set.id]))
        .await
        .expect(200)
        .json();
    assert_eq!(d.items.len(), 4);
    assert_eq!(d.set.name, "cats");
    let grumpy = d.items.iter().find(|i| i.id == item.id).unwrap();
    assert_eq!(grumpy.name, "Grumpy");
    assert_eq!(grumpy.tags, ["grumpy", "cat"]);
    let p: Proposal = srv
        .get(&eve_u, &routes::path(routes::PROPOSAL, &[&stickers]))
        .await
        .expect(200)
        .json();
    assert_eq!(p.status, ProposalStatus::Accepted);
    assert_eq!(p.reviewer_username.as_deref(), Some("bob"));

    // A cover image proposal, withdrawn.
    let r = eve
        .post_multipart_fields(
            &format!("/sets/{}/propose/cover", set.id),
            &[],
            vec![("file", "c.png", png(9))],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let cover = proposal_id(&location(&r));
    let r = eve.post_form(&format!("/proposals/{cover}/withdraw"), &[]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let r = alice.post_form(&format!("/proposals/{cover}/accept"), &[]).await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    // Owners cannot propose to themselves; bob leaves.
    let r = alice.get(&format!("/sets/{}/propose", set.id)).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = bob
        .post_form(&format!("/sets/{}/contributors/bob/remove", set.id), &[])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(location(&r), "/collaboration");
    let members: Vec<stickpile_api::Member> = srv
        .get(&alice_u, &routes::path(routes::SET_MEMBERS, &[&set.id]))
        .await
        .expect(200)
        .json();
    assert!(members.is_empty());
    let _ = Permissions::NONE;
}

#[tokio::test]
async fn a_contributor_edits_the_pack() {
    let srv = TestServer::start().await;
    let alice_u = srv.user("alice").await;
    let bob_u = srv.user("bob").await;
    let set = srv.create_set(&alice_u, "team", Visibility::Private).await;
    srv.upload_one(&alice_u, &set.id, "a.png", png(1)).await;
    srv.put(
        &alice_u,
        &routes::path(routes::SET_MEMBER, &[&set.id, "bob"]),
        serde_json::json!({"permissions": Permissions { edit_pack: true, ..Permissions::NONE }}),
    )
    .await
    .expect(200);
    let bob = Browser::login(&srv, &bob_u).await;
    let page = bob.get(&format!("/sets/{}/settings", set.id)).await;
    assert_page(&page);
    assert!(page.text().contains("Edit pack"));
    assert!(!page.text().contains("name=\"slug\""), "no slug field for contributors");
    assert!(!page.text().contains("Delete this set"));
    let r = bob
        .post_form(
            &format!("/sets/{}/settings", set.id),
            &[("name", "Our team"), ("description", "shared")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let d: SetDetail = srv
        .get(&alice_u, &routes::path(routes::SET, &[&set.id]))
        .await
        .expect(200)
        .json();
    assert_eq!(
        (d.set.name.as_str(), d.set.description.as_str()),
        ("Our team", "shared")
    );
    assert_eq!(d.set.slug, "team");
    assert_eq!(d.set.visibility, Visibility::Private);
}
