//! Contributors with permissions, proposals to other users' sets, and what
//! both mean for search, client caches, media cleanup and backups.

mod common;

use common::{png, TestServer, TestUser};
use serde_json::{json, Value};
use stickpile_api::{
    routes, Item, ItemPage, Member, Permissions, Proposal, ProposalKind, ProposalStatus, Set, SetDetail, SetRole,
    Visibility,
};

fn set_path(set: &Set) -> String {
    routes::path(routes::SET, &[&set.id])
}

fn member_path(set: &Set, username: &str) -> String {
    routes::path(routes::SET_MEMBER, &[&set.id, username])
}

fn item_path(item: &Item) -> String {
    routes::path(routes::ITEM, &[&item.id])
}

async fn add_member(srv: &TestServer, owner: &TestUser, set: &Set, who: &TestUser, p: Permissions) -> Member {
    srv.put(owner, &member_path(set, &who.username), json!({ "permissions": p }))
        .await
        .expect(200)
        .json()
}

async fn set_as(srv: &TestServer, user: &TestUser, set: &Set) -> Set {
    let d: SetDetail = srv.get(user, &set_path(set)).await.expect(200).json();
    d.set
}

/// POST a multipart proposal of new stickers.
async fn propose_files(
    srv: &TestServer,
    user: &TestUser,
    set: &Set,
    files: Vec<(&str, Vec<u8>)>,
    meta: Option<Value>,
    message: &str,
) -> common::Resp {
    let mut form = reqwest::multipart::Form::new().text("message", message.to_string());
    for (name, bytes) in files {
        form = form.part(
            "file",
            reqwest::multipart::Part::bytes(bytes).file_name(name.to_string()),
        );
    }
    if let Some(m) = meta {
        form = form.text("meta", m.to_string());
    }
    let rb = srv
        .client
        .post(srv.url(&routes::path(routes::SET_PROPOSAL_ITEMS, &[&set.id])))
        .bearer_auth(&user.token);
    common::read(rb.multipart(form).send().await.expect("proposal")).await
}

async fn propose_cover(srv: &TestServer, user: &TestUser, set: &Set, bytes: Vec<u8>) -> common::Resp {
    let form = reqwest::multipart::Form::new()
        .text("message", "a nicer cover")
        .part("file", reqwest::multipart::Part::bytes(bytes).file_name("cover.png"));
    let rb = srv
        .client
        .post(srv.url(&routes::path(routes::SET_PROPOSAL_COVER, &[&set.id])))
        .bearer_auth(&user.token);
    common::read(rb.multipart(form).send().await.expect("cover proposal")).await
}

fn decide(id: &str, what: &str) -> String {
    let template = match what {
        "accept" => routes::PROPOSAL_ACCEPT,
        "decline" => routes::PROPOSAL_DECLINE,
        _ => routes::PROPOSAL_WITHDRAW,
    };
    routes::path(template, &[id])
}

// ---------------------------------------------------------------------------
// Contributors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn contributor_permissions() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let carol = srv.user("carol").await;
    let dave = srv.user("dave").await;
    let eve = srv.user("eve").await;
    let set = srv.create_set(&alice, "team", Visibility::Private).await;
    let alices = srv.upload_one(&alice, &set.id, "alice.png", png(1)).await;
    assert_eq!(alices.added_by_username.as_deref(), Some("alice"));

    // Nobody but the owner sees a private set, until added.
    srv.get(&bob, &set_path(&set)).await.expect(404);
    add_member(&srv, &alice, &set, &bob, Permissions::CONTRIBUTOR_DEFAULT).await;
    let all_but_settings = Permissions {
        add: false,
        ..Permissions::ALL
    };
    add_member(&srv, &alice, &set, &carol, all_but_settings).await;
    add_member(&srv, &alice, &set, &dave, Permissions::NONE).await;

    let as_bob = set_as(&srv, &bob, &set).await;
    assert_eq!(as_bob.role, SetRole::Contributor);
    assert_eq!(as_bob.permissions, Permissions::CONTRIBUTOR_DEFAULT);
    assert!(!as_bob.editable, "settings stay the owner's");
    let as_alice = set_as(&srv, &alice, &set).await;
    assert_eq!(
        (as_alice.role, as_alice.permissions),
        (SetRole::Owner, Permissions::ALL)
    );
    srv.get(&eve, &set_path(&set)).await.expect(404);

    // bob: add, and edit or remove only his own.
    let bobs = srv.upload_one(&bob, &set.id, "bob.png", png(2)).await;
    assert_eq!(bobs.added_by_username.as_deref(), Some("bob"));
    srv.patch(&bob, &item_path(&bobs), json!({"name": "bob's"}))
        .await
        .expect(200);
    let r = srv
        .patch(&bob, &item_path(&alices), json!({"name": "mine now"}))
        .await
        .expect(403);
    assert!(r.text().contains("may not edit this sticker"), "{}", r.text());
    srv.delete(&bob, &item_path(&alices)).await.expect(403);
    srv.patch(&bob, &set_path(&set), json!({"name": "renamed"}))
        .await
        .expect(403);
    srv.patch(&bob, &set_path(&set), json!({"visibility": "public"}))
        .await
        .expect(403);

    // dave: read only.
    srv.upload(&dave, &set.id, vec![("d.png", png(3))], None)
        .await
        .expect(403);
    srv.patch(&dave, &item_path(&bobs), json!({"name": "x"}))
        .await
        .expect(403);

    // carol: everything but adding and the settings.
    srv.upload(&carol, &set.id, vec![("c.png", png(4))], None)
        .await
        .expect(403);
    srv.patch(&carol, &item_path(&alices), json!({"tags": ["shared"]}))
        .await
        .expect(200);
    srv.patch(
        &carol,
        &set_path(&set),
        json!({"name": "Team pack", "description": "ours"}),
    )
    .await
    .expect(200);
    srv.patch(&carol, &set_path(&set), json!({"cover_item_id": bobs.id}))
        .await
        .expect(200);
    srv.patch(&carol, &set_path(&set), json!({"slug": "taken-over"}))
        .await
        .expect(403);
    srv.put(
        &carol,
        &routes::path(routes::SET_ORDER, &[&set.id]),
        json!({"item_ids": [bobs.id, alices.id]}),
    )
    .await
    .expect(200);
    srv.put(
        &carol,
        &member_path(&set, "eve"),
        json!({"permissions": Permissions::ALL}),
    )
    .await
    .expect(403);
    srv.delete(&carol, &item_path(&alices)).await.expect(204);
    srv.delete(&carol, &set_path(&set)).await.expect(403);

    // bob removes his own; moves need removal here and adding there.
    srv.delete(&bob, &item_path(&bobs)).await.expect(204);
    let bobs = srv.upload_one(&bob, &set.id, "bob2.png", png(5)).await;
    let own = srv.create_set(&bob, "bobs-own", Visibility::Private).await;
    srv.patch(&bob, &item_path(&bobs), json!({"set_id": own.id}))
        .await
        .expect(200);
    let alices2 = srv.upload_one(&alice, &set.id, "alice2.png", png(6)).await;
    srv.patch(&bob, &item_path(&alices2), json!({"set_id": own.id}))
        .await
        .expect(403);
    srv.patch(&dave, &item_path(&alices2), json!({"set_id": own.id}))
        .await
        .expect(403);

    // The contributor list: owner and contributors only.
    let members: Vec<Member> = srv
        .get(&bob, &routes::path(routes::SET_MEMBERS, &[&set.id]))
        .await
        .expect(200)
        .json();
    let names: Vec<&str> = members.iter().map(|m| m.username.as_str()).collect();
    assert_eq!(names, ["bob", "carol", "dave"]);
    srv.get(&eve, &routes::path(routes::SET_MEMBERS, &[&set.id]))
        .await
        .expect(404);

    // Adding: only existing other users.
    srv.put(
        &alice,
        &member_path(&set, "alice"),
        json!({"permissions": Permissions::ALL}),
    )
    .await
    .expect(400);
    srv.put(
        &alice,
        &member_path(&set, "nobody"),
        json!({"permissions": Permissions::ALL}),
    )
    .await
    .expect(400);
    // Changing permissions takes effect at once.
    add_member(&srv, &alice, &set, &dave, Permissions::CONTRIBUTOR_DEFAULT).await;
    srv.upload_one(&dave, &set.id, "d.png", png(3)).await;

    // Leaving, and being removed.
    srv.delete(&bob, &member_path(&set, "bob")).await.expect(204);
    srv.get(&bob, &set_path(&set)).await.expect(404);
    srv.delete(&dave, &member_path(&set, "carol")).await.expect(403);
    srv.delete(&alice, &member_path(&set, "carol")).await.expect(204);
    srv.get(&carol, &set_path(&set)).await.expect(404);

    // Contributed sets in browse and in the upload target list.
    let all: Vec<stickpile_api::SetSummary> = srv
        .get(&dave, &format!("{}?scope=all", routes::SETS))
        .await
        .expect(200)
        .json();
    assert!(all.iter().any(|s| s.id == set.id));
    let contributing: Vec<stickpile_api::SetSummary> = srv
        .get(&dave, &format!("{}?scope=contributing", routes::SETS))
        .await
        .expect(200)
        .json();
    assert_eq!(contributing.len(), 1);
    let mine: Vec<stickpile_api::SetSummary> = srv.get(&dave, routes::SETS).await.expect(200).json();
    assert!(mine.is_empty(), "contributed sets are not the user's own");
}

#[tokio::test]
async fn read_only_git_packs_take_no_contributions() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let set = srv.create_set(&alice, "upstream", Visibility::Private).await;
    sqlx::query("UPDATE sets SET source = 'git', git_mode = 'read_only' WHERE id = ?")
        .bind(&set.id)
        .execute(&srv.state.db)
        .await
        .unwrap();
    add_member(&srv, &alice, &set, &bob, Permissions::ALL).await;
    let as_bob = set_as(&srv, &bob, &set).await;
    assert_eq!(as_bob.permissions, Permissions::NONE);
    let r = srv
        .upload(&bob, &set.id, vec![("b.png", png(1))], None)
        .await
        .expect(403);
    assert!(r.text().contains("read-only git pack"), "{}", r.text());
}

#[tokio::test]
async fn contributors_search_and_cache_the_sets() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let set = srv.create_set(&alice, "secret", Visibility::Private).await;
    let a = srv.upload_one(&alice, &set.id, "grumpy.png", png(1)).await;
    let b = srv.upload_one(&alice, &set.id, "sleepy.png", png(2)).await;

    let first: ItemPage = srv.get(&bob, routes::ITEMS).await.expect(200).json();
    assert!(first.items.is_empty());
    let since = first.server_time;

    // Added: search finds the items and the cache sync brings them (even
    // though they did not change).
    add_member(&srv, &alice, &set, &bob, Permissions::NONE).await;
    let hits = srv
        .get(&bob, &format!("{}?q=grumpy", routes::SEARCH))
        .await
        .expect(200)
        .value();
    assert_eq!(hits["hits"][0]["item"]["id"], a.id.as_str());
    let page: ItemPage = srv
        .get(&bob, &format!("{}?updated_since={since}", routes::ITEMS))
        .await
        .expect(200)
        .json();
    let mut got: Vec<String> = page.items.iter().map(|i| i.id.clone()).collect();
    got.sort();
    let mut want = vec![a.id.clone(), b.id.clone()];
    want.sort();
    assert_eq!(got, want);
    let since = page.server_time;

    // An item removed from the set reaches the contributor's cache.
    srv.delete(&alice, &item_path(&b)).await.expect(204);
    let page: ItemPage = srv
        .get(&bob, &format!("{}?updated_since={since}", routes::ITEMS))
        .await
        .expect(200)
        .json();
    assert_eq!(page.removed, vec![b.id.clone()]);

    // Removed from the contributors: the rest goes too, and search stops.
    srv.delete(&alice, &member_path(&set, "bob")).await.expect(204);
    let page: ItemPage = srv
        .get(&bob, &format!("{}?updated_since={since}", routes::ITEMS))
        .await
        .expect(200)
        .json();
    let mut removed = page.removed.clone();
    removed.sort();
    assert_eq!(removed, want);
    let hits = srv
        .get(&bob, &format!("{}?q=grumpy", routes::SEARCH))
        .await
        .expect(200)
        .value();
    assert!(hits["hits"].as_array().unwrap().is_empty());
    // The owner's own cache is not told to drop anything still there.
    let page: ItemPage = srv
        .get(&alice, &format!("{}?updated_since={since}", routes::ITEMS))
        .await
        .expect(200)
        .json();
    assert_eq!(page.removed, vec![b.id.clone()]);
}

// ---------------------------------------------------------------------------
// Proposals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn proposing_stickers_and_accepting_them() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let eve = srv.user("eve").await;
    let frank = srv.user("frank").await;
    let set = srv.create_set(&alice, "public", Visibility::Public).await;
    srv.upload_one(&alice, &set.id, "first.png", png(1)).await;

    let meta = json!([{"name": "wave", "emojis": ["👋"], "tags": ["hello"]}, {"keywords": ["two"]}]);
    let p: Proposal = propose_files(
        &srv,
        &eve,
        &set,
        vec![("wave.png", png(10)), ("second.png", png(11))],
        Some(meta),
        "two for your pack",
    )
    .await
    .expect(201)
    .json();
    assert_eq!((p.kind, p.status), (ProposalKind::AddItems, ProposalStatus::Open));
    assert_eq!(p.proposer_username, "eve");
    assert_eq!(p.message, "two for your pack");
    assert_eq!(p.items.len(), 2);
    assert_eq!(p.items[0].name, "wave");
    assert_eq!(p.items[0].emojis, ["👋"]);
    assert_eq!(p.items[1].name, "second");
    assert!(!p.can_decide && p.can_withdraw);
    let thumb = p.items[0].thumb_url.clone().expect("thumbnail while open");
    srv.get(&eve, &thumb).await.expect(200);
    srv.get(&alice, &thumb).await.expect(200);
    srv.get(&frank, &thumb).await.expect(404);
    srv.get(&frank, &routes::path(routes::PROPOSAL, &[&p.id]))
        .await
        .expect(404);
    // Not in the set yet.
    let d: SetDetail = srv.get(&alice, &set_path(&set)).await.expect(200).json();
    assert_eq!(d.items.len(), 1);

    let incoming: Vec<Proposal> = srv
        .get(&alice, &format!("{}?box=incoming&status=open", routes::PROPOSALS))
        .await
        .expect(200)
        .json();
    assert_eq!(incoming.len(), 1);
    assert!(incoming[0].can_decide);
    let outgoing: Vec<Proposal> = srv
        .get(&eve, &format!("{}?box=outgoing", routes::PROPOSALS))
        .await
        .expect(200)
        .json();
    assert_eq!(outgoing[0].id, p.id);
    let none: Vec<Proposal> = srv.get(&eve, routes::PROPOSALS).await.expect(200).json();
    assert!(none.is_empty(), "eve reviews nothing");

    // Only the owner (or a reviewer) decides.
    srv.post(&eve, &decide(&p.id, "accept"), json!({})).await.expect(403);
    let done: Proposal = srv
        .post(&alice, &decide(&p.id, "accept"), json!({"note": "thanks!"}))
        .await
        .expect(200)
        .json();
    assert_eq!(done.status, ProposalStatus::Accepted);
    assert_eq!(done.reviewer_username.as_deref(), Some("alice"));
    assert_eq!(done.review_note, "thanks!");
    assert!(done.items.iter().all(|i| i.item_id.is_some() && i.thumb_url.is_none()));
    let d: SetDetail = srv.get(&alice, &set_path(&set)).await.expect(200).json();
    assert_eq!(d.items.len(), 3);
    let wave = d.items.iter().find(|i| i.name == "wave").unwrap();
    assert_eq!(wave.added_by_username.as_deref(), Some("eve"));
    assert_eq!(wave.tags, ["hello"]);
    // Decided once.
    srv.post(&alice, &decide(&p.id, "decline"), json!({})).await.expect(409);
    srv.post(&eve, &decide(&p.id, "withdraw"), json!({})).await.expect(409);
    srv.get(&eve, &thumb).await.expect(404);
}

#[tokio::test]
async fn proposing_changes_to_stickers_and_sets() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let carol = srv.user("carol").await;
    let eve = srv.user("eve").await;
    let set = srv.create_set(&alice, "public", Visibility::Public).await;
    let cat = srv.upload_one(&alice, &set.id, "cat.png", png(1)).await;
    let dog = srv.upload_one(&alice, &set.id, "dog.png", png(2)).await;
    // carol reviews but may not edit the pack herself.
    let reviewer = Permissions {
        review: true,
        ..Permissions::NONE
    };
    add_member(&srv, &alice, &set, &carol, reviewer).await;

    let item_proposals = routes::path(routes::ITEM_PROPOSALS, &[&cat.id]);
    // Nothing that is already so.
    srv.post(&eve, &item_proposals, json!({"name": "cat"}))
        .await
        .expect(400);
    srv.post(&eve, &item_proposals, json!({"description": "x"}))
        .await
        .expect(400);
    let p: Proposal = srv
        .post(
            &eve,
            &item_proposals,
            json!({"message": "better tags", "name": "Grumpy cat", "tags": ["grumpy"], "emojis": ["😾"]}),
        )
        .await
        .expect(201)
        .json();
    assert_eq!(p.kind, ProposalKind::EditItem);
    assert_eq!(p.item.as_ref().map(|i| i.id.as_str()), Some(cat.id.as_str()));
    assert_eq!(p.changes.name.as_deref(), Some("Grumpy cat"));
    assert_eq!(p.before.name.as_deref(), Some("cat"));
    assert_eq!(p.before.tags.as_deref(), Some(&[][..]));
    // carol may review, so she accepts: the change is made.
    srv.post(&carol, &decide(&p.id, "accept"), json!({})).await.expect(200);
    let d: SetDetail = srv.get(&alice, &set_path(&set)).await.expect(200).json();
    let changed = d.items.iter().find(|i| i.id == cat.id).unwrap();
    assert_eq!(changed.name, "Grumpy cat");
    assert_eq!(changed.tags, ["grumpy"]);

    // Set changes: name and a sticker as the cover; declined, then again.
    let set_proposals = routes::path(routes::SET_PROPOSALS, &[&set.id]);
    srv.post(&eve, &set_proposals, json!({"cover_item_id": "nope"}))
        .await
        .expect(400);
    srv.post(&eve, &set_proposals, json!({"tags": ["x"]})).await.expect(400);
    let p: Proposal = srv
        .post(&eve, &set_proposals, json!({"name": "Pets", "cover_item_id": dog.id}))
        .await
        .expect(201)
        .json();
    let declined: Proposal = srv
        .post(&alice, &decide(&p.id, "decline"), json!({"note": "not yet"}))
        .await
        .expect(200)
        .json();
    assert_eq!(declined.status, ProposalStatus::Declined);
    srv.post(&alice, &decide(&p.id, "accept"), json!({})).await.expect(409);
    let p: Proposal = srv
        .post(&eve, &set_proposals, json!({"name": "Pets", "cover_item_id": dog.id}))
        .await
        .expect(201)
        .json();
    srv.post(&carol, &decide(&p.id, "accept"), json!({})).await.expect(200);
    let s = set_as(&srv, &alice, &set).await;
    assert_eq!(s.name, "Pets");
    assert_eq!(s.cover_item_id.as_deref(), Some(dog.id.as_str()));

    // A cover image.
    let p: Proposal = propose_cover(&srv, &eve, &set, png(30)).await.expect(201).json();
    let cover_url = p.cover_image_url.clone().expect("cover thumbnail");
    srv.get(&alice, &cover_url).await.expect(200);
    srv.post(&alice, &decide(&p.id, "accept"), json!({})).await.expect(200);
    assert!(set_as(&srv, &alice, &set).await.cover_image.is_some());

    // Withdrawing: the proposer only.
    let p: Proposal = srv
        .post(&eve, &item_proposals, json!({"keywords": ["meow"]}))
        .await
        .expect(201)
        .json();
    srv.post(&alice, &decide(&p.id, "withdraw"), json!({}))
        .await
        .expect(403);
    let w: Proposal = srv
        .post(&eve, &decide(&p.id, "withdraw"), json!({}))
        .await
        .expect(200)
        .json();
    assert_eq!(w.status, ProposalStatus::Withdrawn);

    // A removed sticker cannot take its changes; the proposal stays open.
    let p: Proposal = srv
        .post(
            &eve,
            &routes::path(routes::ITEM_PROPOSALS, &[&dog.id]),
            json!({"name": "Doggo"}),
        )
        .await
        .expect(201)
        .json();
    srv.delete(&alice, &item_path(&dog)).await.expect(204);
    let r = srv.post(&alice, &decide(&p.id, "accept"), json!({})).await.expect(409);
    assert!(r.text().contains("removed"), "{}", r.text());
    let still: Proposal = srv
        .get(&eve, &routes::path(routes::PROPOSAL, &[&p.id]))
        .await
        .expect(200)
        .json();
    assert_eq!(still.status, ProposalStatus::Open);
    assert!(still.item.is_none());
}

#[tokio::test]
async fn who_may_propose() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let eve = srv.user("eve").await;
    let private = srv.create_set(&alice, "private", Visibility::Private).await;
    let unlisted = srv.create_set(&alice, "unlisted", Visibility::Unlisted).await;
    let item = srv.upload_one(&alice, &unlisted.id, "x.png", png(1)).await;

    // Unreadable: 404, like everywhere else.
    propose_files(&srv, &eve, &private, vec![("a.png", png(2))], None, "")
        .await
        .expect(404);
    // Own set: change it directly.
    propose_files(&srv, &alice, &unlisted, vec![("a.png", png(2))], None, "")
        .await
        .expect(400);
    // An unlisted set can be read by link, so it takes proposals.
    propose_files(&srv, &eve, &unlisted, vec![("a.png", png(2))], None, "")
        .await
        .expect(201);
    // A contributor who may edit stickers edits them; one who may not proposes.
    add_member(
        &srv,
        &alice,
        &unlisted,
        &bob,
        Permissions {
            edit_all: true,
            ..Permissions::NONE
        },
    )
    .await;
    srv.post(
        &bob,
        &routes::path(routes::ITEM_PROPOSALS, &[&item.id]),
        json!({"name": "y"}),
    )
    .await
    .expect(400);
    srv.post(
        &bob,
        &routes::path(routes::SET_PROPOSALS, &[&unlisted.id]),
        json!({"name": "Y"}),
    )
    .await
    .expect(201);
    // Too long a message.
    let long = "x".repeat(1001);
    srv.post(
        &eve,
        &routes::path(routes::SET_PROPOSALS, &[&unlisted.id]),
        json!({"name": "Z", "message": long}),
    )
    .await
    .expect(400);
    // The proposer keeps seeing their proposal after the set goes private.
    srv.patch(&alice, &set_path(&unlisted), json!({"visibility": "private"}))
        .await
        .expect(200);
    let mine: Vec<Proposal> = srv
        .get(&eve, &format!("{}?box=outgoing", routes::PROPOSALS))
        .await
        .expect(200)
        .json();
    assert_eq!(mine.len(), 1);
}

#[tokio::test]
async fn proposal_media_is_kept_while_needed() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let eve = srv.user("eve").await;
    let set = srv.create_set(&alice, "public", Visibility::Public).await;
    let p: Proposal = propose_files(&srv, &eve, &set, vec![("a.png", png(5))], None, "")
        .await
        .expect(201)
        .json();
    let media = || async {
        sqlx::query_scalar::<_, Option<String>>("SELECT media_id FROM proposal_items WHERE proposal_id = ?")
            .bind(&p.id)
            .fetch_one(&srv.state.db)
            .await
            .unwrap()
    };
    let media_id = media().await.expect("stored");
    assert_eq!(stickpile_server::app::gc_orphan_media(&srv.state).await.unwrap(), 0);
    srv.post(&alice, &decide(&p.id, "decline"), json!({})).await.expect(200);
    assert_eq!(
        stickpile_server::app::gc_orphan_media(&srv.state).await.unwrap(),
        0,
        "kept for a day after the decision"
    );
    sqlx::query("UPDATE proposals SET decided_at = decided_at - 2 * 86400 WHERE id = ?")
        .bind(&p.id)
        .execute(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(stickpile_server::app::gc_orphan_media(&srv.state).await.unwrap(), 1);
    assert_eq!(media().await, None, "the proposal itself stays, without its file");
    assert!(stickpile_server::db::media_by_id(&srv.state.db, &media_id)
        .await
        .unwrap()
        .is_none());
    let p: Proposal = srv
        .get(&eve, &routes::path(routes::PROPOSAL, &[&p.id]))
        .await
        .expect(200)
        .json();
    assert_eq!(p.items.len(), 1);
}

#[tokio::test]
async fn backups_keep_open_proposals_and_contributors() {
    use stickpile_server::service::backup;
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let eve = srv.user("eve").await;
    let set = srv.create_set(&alice, "team", Visibility::Public).await;
    srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
    add_member(&srv, &alice, &set, &bob, Permissions::CONTRIBUTOR_DEFAULT).await;
    let p: Proposal = propose_files(&srv, &eve, &set, vec![("new.png", png(7))], None, "hi")
        .await
        .expect(201)
        .json();

    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("backup");
    let info = backup::run(&srv.state.config, &target).await.unwrap();
    assert!(info.errors.is_empty(), "{:?}", info.errors);
    let files: Vec<_> = std::fs::read_dir(target.join(backup::PROPOSALS_DIR)).unwrap().collect();
    assert_eq!(files.len(), 1);

    let restored_dir = out.path().join("restored");
    let mut cfg = srv.state.config.clone();
    cfg.server.data_dir = restored_dir.clone();
    let r = backup::restore(&cfg, &target).await.unwrap();
    assert!(r.missing.is_empty(), "{:?}", r.missing);
    let restored = TestServer::with_config(|c| c.server.data_dir = restored_dir.clone()).await;
    let again = |u: &TestUser| TestUser {
        id: u.id.clone(),
        username: u.username.clone(),
        password: u.password.clone(),
        token: u.token.clone(),
    };
    let (alice, bob) = (again(&alice), again(&bob));
    let p: Proposal = restored
        .get(&alice, &routes::path(routes::PROPOSAL, &[&p.id]))
        .await
        .expect(200)
        .json();
    restored
        .get(&alice, p.items[0].thumb_url.as_ref().unwrap())
        .await
        .expect(200);
    restored
        .post(&alice, &decide(&p.id, "accept"), json!({}))
        .await
        .expect(200);
    restored.upload_one(&bob, &set.id, "b.png", png(2)).await;
    let d: SetDetail = restored.get(&alice, &set_path(&set)).await.expect(200).json();
    assert_eq!(d.items.len(), 3);
}

// ---------------------------------------------------------------------------
// From the review of the first version
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reviewers_do_not_decide_their_own_proposals() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let carol = srv.user("carol").await;
    let set = srv.create_set(&alice, "team", Visibility::Private).await;
    srv.upload_one(&alice, &set.id, "a.png", png(1)).await;
    let review_only = Permissions {
        review: true,
        ..Permissions::NONE
    };
    add_member(&srv, &alice, &set, &carol, review_only).await;
    // Reviewing must not turn into editing: carol proposes a rename...
    let p: Proposal = srv
        .post(
            &carol,
            &routes::path(routes::SET_PROPOSALS, &[&set.id]),
            json!({"name": "Mine"}),
        )
        .await
        .expect(201)
        .json();
    assert!(!p.can_decide && p.can_withdraw);
    // ...and cannot accept (or decline) it herself, nor see it as incoming.
    srv.post(&carol, &decide(&p.id, "accept"), json!({})).await.expect(403);
    srv.post(&carol, &decide(&p.id, "decline"), json!({})).await.expect(403);
    let incoming: Vec<Proposal> = srv.get(&carol, routes::PROPOSALS).await.expect(200).json();
    assert!(incoming.is_empty());
    assert_eq!(set_as(&srv, &alice, &set).await.name, "team");
    // The owner decides.
    let incoming: Vec<Proposal> = srv.get(&alice, routes::PROPOSALS).await.expect(200).json();
    assert_eq!(incoming.len(), 1);
    srv.post(&alice, &decide(&p.id, "accept"), json!({})).await.expect(200);
    assert_eq!(set_as(&srv, &alice, &set).await.name, "Mine");
}

#[tokio::test]
async fn a_sticker_moved_away_is_gone_for_its_proposal() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let eve = srv.user("eve").await;
    let public = srv.create_set(&alice, "public", Visibility::Public).await;
    let private = srv.create_set(&alice, "private", Visibility::Private).await;
    let item = srv.upload_one(&alice, &public.id, "secret-later.png", png(1)).await;
    let p: Proposal = srv
        .post(
            &eve,
            &routes::path(routes::ITEM_PROPOSALS, &[&item.id]),
            json!({"name": "x"}),
        )
        .await
        .expect(201)
        .json();
    srv.patch(&alice, &item_path(&item), json!({"set_id": private.id}))
        .await
        .expect(200);
    for user in [&eve, &alice] {
        let seen: Proposal = srv
            .get(user, &routes::path(routes::PROPOSAL, &[&p.id]))
            .await
            .expect(200)
            .json();
        assert!(seen.item.is_none(), "nothing of the private set shows");
    }
    let r = srv.post(&alice, &decide(&p.id, "accept"), json!({})).await.expect(409);
    assert!(r.text().contains("moved"), "{}", r.text());
}

#[tokio::test]
async fn a_move_to_another_owner_reaches_the_old_owners_cache() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let team = srv.create_set(&alice, "team", Visibility::Private).await;
    add_member(&srv, &alice, &team, &bob, Permissions::CONTRIBUTOR_DEFAULT).await;
    let bobs = srv.upload_one(&bob, &team.id, "b.png", png(1)).await;
    let page: ItemPage = srv.get(&alice, routes::ITEMS).await.expect(200).json();
    assert!(page.items.iter().any(|i| i.id == bobs.id));
    let since = page.server_time;
    let own = srv.create_set(&bob, "own", Visibility::Private).await;
    srv.patch(&bob, &item_path(&bobs), json!({"set_id": own.id}))
        .await
        .expect(200);
    let page: ItemPage = srv
        .get(&alice, &format!("{}?updated_since={since}", routes::ITEMS))
        .await
        .expect(200)
        .json();
    assert_eq!(page.removed, vec![bobs.id.clone()]);
    let page: ItemPage = srv
        .get(&bob, &format!("{}?updated_since={since}", routes::ITEMS))
        .await
        .expect(200)
        .json();
    assert!(page.removed.is_empty(), "bob still has it, in his own set");
    assert!(page.items.iter().any(|i| i.id == bobs.id));
}
