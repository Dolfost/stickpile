//! Sets, items, uploads, files, import/export and the authorization matrix.

mod common;

use common::{gif_anim, jpeg, png, png_near, TestServer, TestUser};
use reqwest::Method;
use serde_json::json;
use stickpile_api::{
    routes, FieldSource, ImportReport, Item, ItemDetail, ItemPage, ItemSummary, LikeState, MediaFormat, MediaKind, Set,
    SetDetail, SetSummary, UploadResult, Visibility,
};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

struct World {
    srv: TestServer,
    owner: TestUser,
    other: TestUser,
    admin: TestUser,
    /// (visibility, set, item)
    sets: Vec<(Visibility, Set, Item)>,
}

async fn world() -> World {
    let srv = TestServer::start().await;
    let owner = srv.user("owner").await;
    let other = srv.user("other").await;
    let admin = srv.admin("admin").await;
    let mut sets = Vec::new();
    for (i, vis) in [Visibility::Private, Visibility::Unlisted, Visibility::Public]
        .into_iter()
        .enumerate()
    {
        let set = srv.create_set(&owner, &format!("set-{i}"), vis).await;
        let item = srv
            .upload_one(&owner, &set.id, &format!("item{i}.png"), png(100 + i as u64))
            .await;
        sets.push((vis, set, item));
    }
    World {
        srv,
        owner,
        other,
        admin,
        sets,
    }
}

#[tokio::test]
async fn set_crud_and_validation() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let set: Set = srv
        .post(&u, routes::SETS, json!({"slug": "cats", "name": "My cats", "description": "meow", "aspect_ratio": [1, 1], "recommended_px": 512}))
        .await
        .expect(201)
        .json();
    assert_eq!(set.version, 1);
    assert_eq!(set.visibility, Visibility::Private);
    assert_eq!(set.aspect_ratio, Some((1, 1)));
    assert!(set.editable);
    assert_eq!(set.owner_username, "alice");
    assert_eq!(set.id.len(), 26);

    srv.post(&u, routes::SETS, json!({"slug": "cats", "name": "dup"}))
        .await
        .expect(409);
    for bad in ["Cats", "../x", "", "-x", "a b"] {
        srv.post(&u, routes::SETS, json!({"slug": bad, "name": "x"}))
            .await
            .expect(400);
    }
    srv.post(&u, routes::SETS, json!({"slug": "ok", "name": ""}))
        .await
        .expect(400);

    let path = routes::path(routes::SET, &[&set.id]);
    let up: Set = srv
        .patch(
            &u,
            &path,
            json!({"name": "Cats!", "visibility": "public", "aspect_ratio": null}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(up.name, "Cats!");
    assert_eq!(up.aspect_ratio, None);
    assert_eq!(up.recommended_px, Some(512));
    assert_eq!(up.version, 2);
    // No-op patch does not bump the version.
    let same: Set = srv.patch(&u, &path, json!({})).await.expect(200).json();
    assert_eq!(same.version, 2);
    let other = srv.create_set(&u, "dogs", Visibility::Private).await;
    srv.patch(&u, &routes::path(routes::SET, &[&other.id]), json!({"slug": "cats"}))
        .await
        .expect(409);

    let mine: Vec<SetSummary> = srv.get(&u, routes::SETS).await.expect(200).json();
    assert_eq!(mine.len(), 2);
    let detail: SetDetail = srv.get(&u, &path).await.expect(200).json();
    assert!(detail.items.is_empty());
    srv.delete(&u, &path).await.expect(204);
    srv.get(&u, &path).await.expect(404);
    let actions: Vec<(String,)> = sqlx::query_as("SELECT action FROM audit_log WHERE action = 'set.delete'")
        .fetch_all(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(actions.len(), 1);
}

#[tokio::test]
async fn set_visibility_matrix() {
    let w = world().await;
    let srv = &w.srv;
    for (vis, set, _) in &w.sets {
        let path = routes::path(routes::SET, &[&set.id]);
        let readable = *vis != Visibility::Private;
        // owner
        srv.get(&w.owner, &path).await.expect(200);
        // other user
        let r = srv.get(&w.other, &path).await;
        assert_eq!(r.status, if readable { 200 } else { 404 }, "{vis:?}");
        if readable {
            let d: SetDetail = r.json();
            assert!(!d.set.editable);
            assert_eq!(d.items.len(), 1);
        }
        // admin: no implicit content access
        let r = srv.get(&w.admin, &path).await;
        assert_eq!(r.status, if readable { 200 } else { 404 }, "admin {vis:?}");
        // anonymous
        srv.anon_get(&path).await.expect(401);
        // export follows read access
        let r = srv.get(&w.other, &routes::path(routes::SET_EXPORT, &[&set.id])).await;
        assert_eq!(r.status, if readable { 200 } else { 404 });
        // edits: never by others (403 when they can see it, 404 otherwise)
        let expect = if readable { 403 } else { 404 };
        srv.patch(&w.other, &path, json!({"name": "pwned"}))
            .await
            .expect(expect);
        srv.patch(&w.admin, &path, json!({"name": "pwned"}))
            .await
            .expect(expect);
        srv.delete(&w.other, &path).await.expect(expect);
        srv.put(
            &w.other,
            &routes::path(routes::SET_ORDER, &[&set.id]),
            json!({"item_ids": []}),
        )
        .await
        .expect(expect);
        let r = srv.upload(&w.other, &set.id, vec![("x.png", png(999))], None).await;
        assert_eq!(r.status, expect);
        let r = read_import(srv, &w.other, Some(&set.id)).await;
        assert_eq!(r.status, expect);
    }
    // Browse lists: others see only public sets.
    let mine: Vec<SetSummary> = srv.get(&w.owner, routes::SETS).await.expect(200).json();
    assert_eq!(mine.len(), 3);
    let theirs: Vec<SetSummary> = srv.get(&w.other, routes::SETS).await.expect(200).json();
    assert!(theirs.is_empty());
    let public: Vec<SetSummary> = srv
        .get(&w.other, &format!("{}?scope=public", routes::SETS))
        .await
        .expect(200)
        .json();
    assert_eq!(public.len(), 1);
    assert_eq!(public[0].visibility, Visibility::Public);
    assert_eq!(public[0].cover_item_id.as_deref(), Some(w.sets[2].2.id.as_str()));
    let all: Vec<SetSummary> = srv
        .get(&w.other, &format!("{}?scope=all", routes::SETS))
        .await
        .expect(200)
        .json();
    assert_eq!(all.len(), 1);
    let all_owner: Vec<SetSummary> = srv
        .get(&w.owner, &format!("{}?scope=all", routes::SETS))
        .await
        .expect(200)
        .json();
    assert_eq!(all_owner.len(), 3);
    srv.get(&w.owner, &format!("{}?scope=bogus", routes::SETS))
        .await
        .expect(400);
}

async fn read_import(srv: &TestServer, user: &TestUser, into: Option<&str>) -> common::Resp {
    let mut url = srv.url(routes::SETS_IMPORT);
    if let Some(i) = into {
        url.push_str(&format!("?into={i}"));
    }
    let form = reqwest::multipart::Form::new().text("set.toml", "name = \"x\"\n").part(
        "file",
        reqwest::multipart::Part::bytes(png(4242)).file_name("x/001-a.png"),
    );
    common::read(
        srv.client
            .post(url)
            .bearer_auth(&user.token)
            .multipart(form)
            .send()
            .await
            .unwrap(),
    )
    .await
}

#[tokio::test]
async fn item_visibility_matrix() {
    let w = world().await;
    let srv = &w.srv;
    for (vis, _set, item) in &w.sets {
        let readable = *vis != Visibility::Private;
        let code = if readable { 200 } else { 404 };
        let ip = routes::path(routes::ITEM, &[&item.id]);
        srv.get(&w.owner, &ip).await.expect(200);
        srv.get(&w.other, &ip).await.expect(code);
        srv.get(&w.admin, &ip).await.expect(code);
        srv.anon_get(&ip).await.expect(401);
        // files
        for variant in ["original", "thumb"] {
            let fp = routes::path(routes::ITEM_FILE, &[&item.id, variant]);
            srv.get(&w.owner, &fp).await.expect(200);
            srv.get(&w.other, &fp).await.expect(code);
            srv.anon_get(&fp).await.expect(401);
        }
        srv.get(&w.other, &routes::path(routes::ITEM_BETTER, &[&item.id]))
            .await
            .expect(code);
        // likes: allowed wherever readable
        let lp = routes::path(routes::ITEM_LIKE, &[&item.id]);
        let r = srv.put(&w.other, &lp, json!({})).await.expect(code);
        if readable {
            let l: LikeState = r.json();
            assert!(l.liked);
            assert_eq!(l.likes, 1);
            let l: LikeState = srv.delete(&w.other, &lp).await.expect(200).json();
            assert_eq!(l.likes, 0);
        } else {
            srv.delete(&w.other, &lp).await.expect(404);
        }
        // usage: accepted wherever readable
        let ack = srv
            .post(
                &w.other,
                routes::USAGE,
                json!({"events": [{"id": format!("ev-{}", item.id), "item_id": item.id, "copied_at": now()}]}),
            )
            .await
            .expect(200)
            .value();
        assert_eq!(ack["accepted"], if readable { 1 } else { 0 });
        assert_eq!(ack["rejected"].as_array().unwrap().len(), if readable { 0 } else { 1 });
        // edit / delete / enrich: owner only
        let expect = if readable { 403 } else { 404 };
        srv.patch(&w.other, &ip, json!({"name": "pwned"})).await.expect(expect);
        srv.patch(&w.admin, &ip, json!({"name": "pwned"})).await.expect(expect);
        srv.delete(&w.other, &ip).await.expect(expect);
        srv.post(&w.other, &routes::path(routes::ITEM_ENRICH, &[&item.id]), json!({}))
            .await
            .expect(expect);
    }
    // Garbage ids are plain 404s.
    srv.get(&w.owner, &routes::path(routes::ITEM, &["../../etc/passwd"]))
        .await
        .expect(404);
    srv.get(&w.owner, &routes::path(routes::ITEM_FILE, &[&w.sets[0].2.id, "nope"]))
        .await
        .expect(404);
}

#[tokio::test]
async fn search_and_sync_visibility() {
    let w = world().await;
    let srv = &w.srv;
    let (_, _priv_set, priv_item) = &w.sets[0];
    let (_, _unl_set, unl_item) = &w.sets[1];
    let (_, pub_set, pub_item) = &w.sets[2];
    let ids = |r: &serde_json::Value| -> Vec<String> {
        r["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["item"]["id"].as_str().unwrap().to_string())
            .collect()
    };
    let mine = srv
        .get(&w.owner, &format!("{}?q=item", routes::SEARCH))
        .await
        .expect(200)
        .value();
    let mine = ids(&mine);
    for it in [priv_item, unl_item, pub_item] {
        assert!(mine.contains(&it.id), "owner finds own {}", it.name);
    }
    for user in [&w.other, &w.admin] {
        let r = srv
            .get(user, &format!("{}?q=item", routes::SEARCH))
            .await
            .expect(200)
            .value();
        assert_eq!(ids(&r), vec![pub_item.id.clone()], "only public items for others");
        let r = srv
            .get(user, &format!("{}?q=", routes::SEARCH))
            .await
            .expect(200)
            .value();
        assert_eq!(ids(&r), vec![pub_item.id.clone()]);
    }
    let r = srv
        .get(&w.other, &format!("{}?q=set:set-0", routes::SEARCH))
        .await
        .expect(200)
        .value();
    assert!(ids(&r).is_empty());
    let r = srv
        .get(
            &w.owner,
            &format!("{}?q=is:mine type:png&target=discord", routes::SEARCH),
        )
        .await
        .expect(200)
        .value();
    assert_eq!(ids(&r).len(), 3);
    assert_eq!(r["semantic"], false);
    srv.get(&w.owner, &format!("{}?q=x&target=myspace", routes::SEARCH))
        .await
        .expect(400);

    // Client cache sync: own + public, never others' unlisted.
    let page: ItemPage = srv.get(&w.owner, routes::ITEMS).await.expect(200).json();
    assert_eq!(page.items.len(), 3);
    let page: ItemPage = srv.get(&w.other, routes::ITEMS).await.expect(200).json();
    assert_eq!(
        page.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
        vec![pub_item.id.clone()]
    );
    let since = page.server_time;

    // Making the public set private: tombstone for others, not for the owner.
    srv.patch(
        &w.owner,
        &routes::path(routes::SET, &[&pub_set.id]),
        json!({"visibility": "private"}),
    )
    .await
    .expect(200);
    let page: ItemPage = srv
        .get(&w.other, &format!("{}?updated_since={since}", routes::ITEMS))
        .await
        .expect(200)
        .json();
    assert!(page.items.is_empty());
    assert_eq!(page.removed, vec![pub_item.id.clone()]);
    let page: ItemPage = srv
        .get(&w.owner, &format!("{}?updated_since={since}", routes::ITEMS))
        .await
        .expect(200)
        .json();
    assert!(page.removed.is_empty());
    // `updated_since` is inclusive (same-second changes are never missed), so
    // items created in that second may show up again; the changed one must.
    assert!(
        page.items.iter().any(|i| i.id == pub_item.id),
        "visibility change bumps the owner's items too"
    );
    let r = srv
        .get(&w.other, &format!("{}?q=item", routes::SEARCH))
        .await
        .expect(200)
        .value();
    assert!(ids(&r).is_empty(), "index follows visibility changes");

    // Public again: it comes back and the tombstone is gone.
    srv.patch(
        &w.owner,
        &routes::path(routes::SET, &[&pub_set.id]),
        json!({"visibility": "public"}),
    )
    .await
    .expect(200);
    let page: ItemPage = srv
        .get(&w.other, &format!("{}?updated_since={since}", routes::ITEMS))
        .await
        .expect(200)
        .json();
    assert_eq!(page.items.len(), 1);
    assert!(page.removed.is_empty());

    // Deleting a public item: everyone gets the tombstone.
    let t2 = page.server_time;
    srv.delete(&w.owner, &routes::path(routes::ITEM, &[&pub_item.id]))
        .await
        .expect(204);
    let page: ItemPage = srv
        .get(&w.other, &format!("{}?updated_since={t2}", routes::ITEMS))
        .await
        .expect(200)
        .json();
    assert_eq!(page.removed, vec![pub_item.id.clone()]);
    let page: ItemPage = srv
        .get(&w.owner, &format!("{}?updated_since={t2}", routes::ITEMS))
        .await
        .expect(200)
        .json();
    assert_eq!(page.removed, vec![pub_item.id.clone()]);
    // Deleting a private item: only the owner learns about it.
    srv.delete(&w.owner, &routes::path(routes::ITEM, &[&priv_item.id]))
        .await
        .expect(204);
    let page: ItemPage = srv
        .get(&w.other, &format!("{}?updated_since={t2}", routes::ITEMS))
        .await
        .expect(200)
        .json();
    assert!(!page.removed.contains(&priv_item.id));
    let page: ItemPage = srv
        .get(&w.owner, &format!("{}?updated_since={t2}", routes::ITEMS))
        .await
        .expect(200)
        .json();
    assert!(page.removed.contains(&priv_item.id));
    // Without updated_since there are no tombstones.
    let page: ItemPage = srv.get(&w.owner, routes::ITEMS).await.expect(200).json();
    assert!(page.removed.is_empty());
}

#[tokio::test]
async fn sync_pagination() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let files: Vec<(String, Vec<u8>)> = (0..5).map(|i| (format!("f{i}.png"), png(i))).collect();
    srv.upload(
        &u,
        &set.id,
        files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect(),
        None,
    )
    .await
    .expect(200);
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let url = match &cursor {
            Some(c) => format!("{}?limit=2&cursor={c}", routes::ITEMS),
            None => format!("{}?limit=2", routes::ITEMS),
        };
        let page: ItemPage = srv.get(&u, &url).await.expect(200).json();
        assert!(page.items.len() <= 2);
        seen.extend(page.items.into_iter().map(|i| i.id));
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 5);
    srv.get(&u, &format!("{}?cursor=!!!", routes::ITEMS)).await.expect(400);
}

#[tokio::test]
async fn uploads_sniff_sanitise_and_dedup() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;

    // PNG bytes named .gif are stored as PNG.
    let item = srv.upload_one(&u, &set.id, "funny cat.gif", png(1)).await;
    assert_eq!(item.media.format, MediaFormat::Png);
    assert_eq!(item.media.kind, MediaKind::Still);
    assert_eq!(item.name, "funny cat");
    assert_eq!(item.filename, "funny-cat");
    assert_eq!(item.position, 0);
    assert!(item
        .media
        .variants
        .iter()
        .any(|v| v.variant == stickpile_api::Variant::Thumb));
    let f = srv
        .get(&u, &routes::path(routes::ITEM_FILE, &[&item.id, "original"]))
        .await
        .expect(200);
    assert_eq!(f.header("content-type").unwrap(), "image/png");
    assert!(f.body.starts_with(b"\x89PNG"));

    // Text named .png -> 415.
    let r = srv
        .upload(
            &u,
            &set.id,
            vec![("evil.png", b"<script>alert(1)</script>".to_vec())],
            None,
        )
        .await
        .expect(415);
    assert_eq!(r.code(), "unsupported_media");
    // HTML/SVG are never accepted.
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#.to_vec();
    srv.upload(&u, &set.id, vec![("x.svg", svg)], None).await.expect(415);

    // Polyglot: trailing payload after IEND is stripped from the stored original.
    let mut poly = png(2);
    let marker = b"PK\x03\x04TRAILING-PAYLOAD-MARKER";
    poly.extend_from_slice(marker);
    let item2 = srv.upload_one(&u, &set.id, "poly.png", poly.clone()).await;
    let f = srv
        .get(&u, &routes::path(routes::ITEM_FILE, &[&item2.id, "original"]))
        .await
        .expect(200);
    assert!(
        !f.body.windows(marker.len()).any(|w| w == marker),
        "trailing data survived"
    );
    assert_eq!(
        item2.media.sha256,
        stickpile_server::ids::sha256_hex(&poly),
        "dedup key is the upload hash"
    );

    // JPEG metadata (an APP1/Exif-like comment) is stripped too.
    let jpg = jpeg(3);
    let mut with_com = jpg[..2].to_vec();
    let comment = b"SECRET-GPS-COMMENT";
    with_com.extend_from_slice(&[0xFF, 0xFE, 0x00, (comment.len() + 2) as u8]);
    with_com.extend_from_slice(comment);
    with_com.extend_from_slice(&jpg[2..]);
    let item3 = srv.upload_one(&u, &set.id, "photo.jpg", with_com).await;
    assert_eq!(item3.media.format, MediaFormat::Jpeg);
    let f = srv
        .get(&u, &routes::path(routes::ITEM_FILE, &[&item3.id, "original"]))
        .await
        .expect(200);
    assert!(
        !f.body.windows(comment.len()).any(|w| w == comment),
        "JPEG comment survived"
    );

    // Same bytes in the same set: no new item.
    let res: Vec<UploadResult> = srv
        .upload(&u, &set.id, vec![("again.png", png(1))], None)
        .await
        .expect(200)
        .json();
    assert!(res[0].already_in_set);
    assert_eq!(res[0].item.as_ref().unwrap().id, item.id);
    let detail: SetDetail = srv
        .get(&u, &routes::path(routes::SET, &[&set.id]))
        .await
        .expect(200)
        .json();
    assert_eq!(detail.items.len(), 3);
    assert_eq!(
        detail.items.iter().map(|i| i.position).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );

    // Same bytes in another set: new item, shared media, reported as same content.
    let set2 = srv.create_set(&u, "s2", Visibility::Private).await;
    let res: Vec<UploadResult> = srv
        .upload(&u, &set2.id, vec![("copy.png", png(1))], None)
        .await
        .expect(200)
        .json();
    let copy = res[0].item.clone().unwrap();
    assert!(!res[0].already_in_set);
    assert_eq!(copy.media.id, item.media.id);
    assert!(res[0]
        .near_duplicates
        .iter()
        .any(|n| n.existing.id == item.id && n.same_content && n.distance == 0));
    let media_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(media_rows, 3);

    // Multi-file upload with metadata and one bad file: per-file errors, 200.
    let res: Vec<UploadResult> = srv
        .upload(
            &u,
            &set2.id,
            vec![("a.png", png(10)), ("bad.png", b"nope".to_vec()), ("anim.gif", gif_anim(20))],
            Some(json!([{"name": "Alpha", "keywords": ["first", " first ", ""], "emojis": ["😀"], "tags": ["t"]}, {}, {"name": "Anim"}])),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(res.len(), 3);
    let a = res[0].item.as_ref().unwrap();
    assert_eq!(a.name, "Alpha");
    assert_eq!(a.keywords, vec!["first"]);
    assert_eq!(a.emojis, vec!["😀"]);
    assert_eq!(a.tags, vec!["t"]);
    assert_eq!(a.keywords_source, FieldSource::User);
    assert!(res[1].item.is_none() && res[1].error.is_some());
    let anim = res[2].item.as_ref().unwrap();
    assert_eq!(anim.media.kind, MediaKind::Animation);
    assert_eq!(anim.media.format, MediaFormat::Gif);
    assert_eq!(anim.media.frame_count, Some(3));

    // Bodies that are not multipart.
    let r = srv
        .post(&u, &routes::path(routes::SET_ITEMS, &[&set.id]), json!({}))
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.code(), "bad_request");
}

#[tokio::test]
async fn upload_size_limits() {
    let srv = TestServer::with_config(|c| {
        c.limits.max_upload_mb = 1;
        c.limits.max_request_mb = 2;
    })
    .await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let big = vec![0x89u8; 1024 * 1024 + 10];
    let r = srv.upload(&u, &set.id, vec![("big.png", big)], None).await.expect(413);
    assert_eq!(r.code(), "too_large");
    // Whole request over max_request_mb.
    let chunk = vec![0u8; 900 * 1024];
    let r = srv
        .upload(
            &u,
            &set.id,
            vec![("a.png", chunk.clone()), ("b.png", chunk.clone()), ("c.png", chunk)],
            None,
        )
        .await;
    assert_eq!(r.status, 413, "{}", r.text());
    assert_eq!(r.code(), "too_large");
    // Too many decoded pixels.
    let srv = TestServer::with_config(|c| c.limits.max_pixels = 1000).await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let r = srv.upload(&u, &set.id, vec![("big.png", png(1))], None).await;
    assert!(r.status == 413 || r.status == 415, "{} {}", r.status, r.text());
}

#[tokio::test]
async fn near_duplicates_stay_within_own_library() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let a_set = srv.create_set(&alice, "a", Visibility::Public).await;
    let b_set = srv.create_set(&bob, "b", Visibility::Public).await;
    let original = srv.upload_one(&alice, &a_set.id, "orig.png", png(7)).await;

    // Bob uploads the same picture: alice's public item is never reported to him.
    let res: Vec<UploadResult> = srv
        .upload(&bob, &b_set.id, vec![("same.png", png_near(7))], None)
        .await
        .expect(200)
        .json();
    assert!(res[0].near_duplicates.is_empty(), "{:?}", res[0].near_duplicates);
    let res: Vec<UploadResult> = srv
        .upload(&bob, &b_set.id, vec![("exact.png", png(7))], None)
        .await
        .expect(200)
        .json();
    assert!(res[0]
        .near_duplicates
        .iter()
        .all(|n| n.existing.owner_username == "bob"));

    // Alice uploading a near-duplicate into another of her sets gets a hint.
    let a2 = srv.create_set(&alice, "a2", Visibility::Private).await;
    let res: Vec<UploadResult> = srv
        .upload(&alice, &a2.id, vec![("near.png", png_near(7))], None)
        .await
        .expect(200)
        .json();
    let near = &res[0].near_duplicates;
    assert!(
        near.iter().any(|n| n.existing.id == original.id && !n.same_content),
        "{near:?}"
    );
    // And unrelated pictures are not reported.
    let res: Vec<UploadResult> = srv
        .upload(&alice, &a2.id, vec![("other.png", png(8))], None)
        .await
        .expect(200)
        .json();
    assert!(res[0].near_duplicates.iter().all(|n| n.existing.id != original.id));

    // Item detail shows the same, only from the caller's library.
    let d: ItemDetail = srv
        .get(&bob, &routes::path(routes::ITEM, &[&original.id]))
        .await
        .expect(200)
        .json();
    assert!(d.near_duplicates.iter().all(|n| n.existing.owner_username == "bob"));
    let d: ItemDetail = srv
        .get(&alice, &routes::path(routes::ITEM, &[&original.id]))
        .await
        .expect(200)
        .json();
    assert!(d.near_duplicates.iter().all(|n| n.existing.owner_username == "alice"));
    assert!(!d.near_duplicates.is_empty());
    assert!(d.quality.is_some());
}

#[tokio::test]
async fn file_serving_headers_variants_and_jobs() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let other = srv.user("bob").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let item = srv.upload_one(&u, &set.id, "cat.png", png(5)).await;
    let fp = routes::path(routes::ITEM_FILE, &[&item.id, "thumb"]);
    let r = srv.get(&u, &fp).await.expect(200);
    assert_eq!(r.header("content-type").unwrap(), "image/webp");
    assert_eq!(r.header("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(
        r.header("content-disposition").unwrap(),
        "inline; filename=\"cat-thumb.webp\""
    );
    assert!(r.header("cache-control").unwrap().contains("immutable"));
    let etag = r.header("etag").expect("etag");
    assert!(etag.starts_with('"') && !etag.starts_with("W/"));
    // Conditional GET.
    let r = common::read(
        srv.client
            .get(srv.url(&fp))
            .bearer_auth(&u.token)
            .header("if-none-match", &etag)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, 304);
    // HEAD.
    let r = common::read(
        srv.client
            .head(srv.url(&fp))
            .bearer_auth(&u.token)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200);
    assert!(r.body.is_empty());
    assert_eq!(r.header("content-type").unwrap(), "image/webp");
    // Range.
    let op = routes::path(routes::ITEM_FILE, &[&item.id, "original"]);
    let r = common::read(
        srv.client
            .get(srv.url(&op))
            .bearer_auth(&u.token)
            .header("range", "bytes=0-3")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, 206);
    assert_eq!(r.body, b"\x89PNG");
    assert_eq!(r.header("content-disposition").unwrap(), "inline; filename=\"cat.png\"");

    // Variants are generated by the job; 404 until then.
    let sticker = routes::path(routes::ITEM_FILE, &[&item.id, "tg_sticker"]);
    srv.get(&u, &sticker).await.expect(404);
    let jobs: Vec<stickpile_api::Job> = {
        let admin = srv.admin("root").await;
        srv.get(&admin, routes::ADMIN_JOBS).await.expect(200).json()
    };
    let job = jobs
        .iter()
        .find(|j| j.kind == stickpile_api::JobKind::Variants)
        .expect("variants job queued");
    assert_eq!(job.status, stickpile_api::JobStatus::Queued);
    // JOB endpoint: owner yes, others no.
    srv.get(&u, &routes::path(routes::JOB, &[&job.id])).await.expect(200);
    srv.get(&other, &routes::path(routes::JOB, &[&job.id]))
        .await
        .expect(404);
    assert!(srv.run_jobs().await >= 1);
    let r = srv.get(&u, &sticker).await.expect(200);
    assert_eq!(r.header("content-type").unwrap(), "image/webp");
    let done: stickpile_api::Job = srv
        .get(&u, &routes::path(routes::JOB, &[&job.id]))
        .await
        .expect(200)
        .json();
    assert_eq!(done.status, stickpile_api::JobStatus::Done);
    let d: ItemDetail = srv
        .get(&u, &routes::path(routes::ITEM, &[&item.id]))
        .await
        .expect(200)
        .json();
    assert!(d
        .item
        .media
        .variants
        .iter()
        .any(|v| v.variant == stickpile_api::Variant::TgSticker));
    assert!(
        d.item
            .media
            .variants
            .iter()
            .any(|v| v.variant == stickpile_api::Variant::Png)
            || item.media.format == MediaFormat::Png
    );
    // Telegram can take it as a sticker; incompatibilities only list real problems.
    assert!(d.incompatible.iter().all(|i| !i.reason.is_empty()));
}

#[tokio::test]
async fn background_workers_process_jobs() {
    let srv = TestServer::start_with(
        common::Opts {
            workers: true,
            ..Default::default()
        },
        |_| {},
    )
    .await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let item = srv.upload_one(&u, &set.id, "cat.png", png(5)).await;
    let sticker = routes::path(routes::ITEM_FILE, &[&item.id, "tg_sticker"]);
    for _ in 0..100 {
        if srv.get(&u, &sticker).await.status == 200 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("variants were not generated by the workers");
}

#[tokio::test]
async fn video_upload() {
    if !common::ffmpeg_available() {
        eprintln!("ffmpeg not on PATH; skipping");
        return;
    }
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("clip.mp4");
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=64x64:rate=10:duration=1",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
        ])
        .arg(&out)
        .status();
    let Ok(status) = status else { return };
    if !status.success() {
        eprintln!("ffmpeg cannot encode h264 here; skipping");
        return;
    }
    let bytes = std::fs::read(&out).unwrap();
    let item = srv.upload_one(&u, &set.id, "clip.mp4", bytes).await;
    assert_eq!(item.media.kind, MediaKind::Video);
    assert_eq!(item.media.format, MediaFormat::Mp4);
    srv.get(&u, &routes::path(routes::ITEM_FILE, &[&item.id, "thumb"]))
        .await
        .expect(200);
}

#[tokio::test]
async fn video_upload_without_ffmpeg_fails_cleanly() {
    let srv = TestServer::with_config(|c| {
        c.media.ffmpeg = "/nonexistent/ffmpeg".into();
        c.media.ffprobe = "/nonexistent/ffprobe".into();
    })
    .await;
    assert!(!srv.state.ffmpeg_available);
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    // Minimal ISO-BMFF header with an mp4 brand.
    let mut fake = vec![0, 0, 0, 0x18];
    fake.extend_from_slice(b"ftypisom\0\0\0\0isommp41");
    fake.extend_from_slice(&[0u8; 64]);
    let r = srv.upload(&u, &set.id, vec![("v.mp4", fake)], None).await;
    assert!(r.status.is_client_error(), "{} {}", r.status, r.text());
    // Stills still work.
    srv.upload_one(&u, &set.id, "ok.png", png(1)).await;
}

#[tokio::test]
async fn item_edit_confirm_move_delete_reorder() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let other = srv.user("bob").await;
    let set = srv.create_set(&u, "s", Visibility::Public).await;
    let set2 = srv.create_set(&u, "s2", Visibility::Private).await;
    let bob_set = srv.create_set(&other, "b", Visibility::Public).await;
    let a = srv.upload_one(&u, &set.id, "a.png", png(1)).await;
    let b = srv.upload_one(&u, &set.id, "b.png", png(2)).await;
    let c = srv.upload_one(&u, &set.id, "c.png", png(3)).await;
    let v0 = srv
        .get(&u, &routes::path(routes::SET, &[&set.id]))
        .await
        .expect(200)
        .json::<SetDetail>()
        .set
        .version;

    // Pretend AI filled some fields.
    sqlx::query("UPDATE items SET keywords = '[\"cat\"]', keywords_source = 'ai', vibe = 'smug', vibe_source = 'ai' WHERE id = ?")
        .bind(&a.id)
        .execute(&srv.state.db)
        .await
        .unwrap();
    let ip = routes::path(routes::ITEM, &[&a.id]);
    let it: Item = srv
        .patch(&u, &ip, json!({"confirm": ["keywords"]}))
        .await
        .expect(200)
        .json();
    assert_eq!(it.keywords, vec!["cat"]);
    assert_eq!(it.keywords_source, FieldSource::User);
    assert_eq!(it.vibe_source, FieldSource::Ai);
    let it: Item = srv
        .patch(
            &u,
            &ip,
            json!({"vibe": "tired", "emojis": ["😴"], "name": "Sleepy", "tags": ["mood"]}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(it.vibe.as_deref(), Some("tired"));
    assert_eq!(it.vibe_source, FieldSource::User);
    assert_eq!(it.name, "Sleepy");
    assert_eq!(it.filename, "sleepy");
    assert_eq!(it.tags, vec!["mood"]);
    let it: Item = srv.patch(&u, &ip, json!({"vibe": null})).await.expect(200).json();
    assert_eq!(it.vibe, None);
    srv.patch(&u, &ip, json!({"confirm": ["owner_id"]})).await.expect(400);
    srv.patch(&u, &ip, json!({"name": ""})).await.expect(400);
    let v1 = srv
        .get(&u, &routes::path(routes::SET, &[&set.id]))
        .await
        .expect(200)
        .json::<SetDetail>()
        .set
        .version;
    assert!(v1 > v0, "edits bump the set version");
    // Search sees the edit.
    let r = srv
        .get(&u, &format!("{}?q=sleepy", routes::SEARCH))
        .await
        .expect(200)
        .value();
    assert_eq!(r["hits"][0]["item"]["id"], a.id.as_str());

    // Reorder.
    let order = routes::path(routes::SET_ORDER, &[&set.id]);
    srv.put(&u, &order, json!({"item_ids": [c.id, a.id]})).await.expect(400);
    srv.put(&u, &order, json!({"item_ids": [c.id, a.id, a.id]}))
        .await
        .expect(400);
    let d: SetDetail = srv
        .put(&u, &order, json!({"item_ids": [c.id, a.id, b.id]}))
        .await
        .expect(200)
        .json();
    assert_eq!(
        d.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
        vec![c.id.clone(), a.id.clone(), b.id.clone()]
    );

    // Move to another own set, not to someone else's.
    srv.patch(&u, &ip, json!({"set_id": bob_set.id})).await.expect(403);
    let moved: Item = srv.patch(&u, &ip, json!({"set_id": set2.id})).await.expect(200).json();
    assert_eq!(moved.set_id, set2.id);
    assert_eq!(moved.position, 0);
    let d: SetDetail = srv
        .get(&u, &routes::path(routes::SET, &[&set.id]))
        .await
        .expect(200)
        .json();
    assert_eq!(d.items.iter().map(|i| i.position).collect::<Vec<_>>(), vec![0, 1]);
    // Moving the same media into a set that has it -> 409.
    srv.upload_one(&u, &set.id, "a-again.png", png(1)).await;
    srv.patch(&u, &ip, json!({"set_id": set.id})).await.expect(409);

    // Delete.
    srv.delete(&u, &ip).await.expect(204);
    srv.get(&u, &ip).await.expect(404);
    srv.delete(&u, &ip).await.expect(404);
}

#[tokio::test]
async fn likes_usage_popular_and_stats() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let pub_set = srv.create_set(&alice, "pub", Visibility::Public).await;
    let unl_set = srv.create_set(&alice, "unl", Visibility::Unlisted).await;
    let a = srv.upload_one(&alice, &pub_set.id, "a.png", png(1)).await;
    let b = srv.upload_one(&alice, &pub_set.id, "b.png", png(2)).await;
    let u = srv.upload_one(&alice, &unl_set.id, "u.png", png(3)).await;

    // Likes are idempotent.
    let lp = routes::path(routes::ITEM_LIKE, &[&a.id]);
    srv.put(&bob, &lp, json!({})).await.expect(200);
    let l: LikeState = srv.put(&bob, &lp, json!({})).await.expect(200).json();
    assert_eq!((l.liked, l.likes), (true, 1));
    let l: LikeState = srv.put(&alice, &lp, json!({})).await.expect(200).json();
    assert_eq!(l.likes, 2);
    let d: ItemDetail = srv
        .get(&bob, &routes::path(routes::ITEM, &[&a.id]))
        .await
        .expect(200)
        .json();
    assert!(d.item.liked_by_me);
    assert_eq!(d.item.likes, 2);
    // Unlisted items can be liked by link.
    srv.put(&bob, &routes::path(routes::ITEM_LIKE, &[&u.id]), json!({}))
        .await
        .expect(200);

    // Usage: idempotent by id, clamped timestamps, invisible items rejected.
    let t = now();
    let batch = json!({"events": [
        {"id": "e1", "item_id": a.id, "copied_at": t, "target": "telegram", "variant": "tg_sticker", "client": "cli/0.1"},
        {"id": "e2", "item_id": a.id, "copied_at": t - 3600, "target": "discord"},
        {"id": "e3", "item_id": b.id, "copied_at": t - 20 * 86400},
        {"id": "e4", "item_id": b.id, "copied_at": t + 10 * 365 * 86400},
        {"id": "e5", "item_id": "aaaaaaaaaaaaaaaaaaaaaaaaaa", "copied_at": t},
        {"id": "e6", "item_id": u.id, "copied_at": 0}
    ]});
    let ack = srv.post(&bob, routes::USAGE, batch.clone()).await.expect(200).value();
    assert_eq!(ack["accepted"], 5);
    assert_eq!(ack["duplicates"], 0);
    assert_eq!(ack["rejected"], json!(["e5"]));
    let ack = srv.post(&bob, routes::USAGE, batch).await.expect(200).value();
    assert_eq!(ack["accepted"], 0);
    assert_eq!(ack["duplicates"], 5);
    let (max_t, min_t): (i64, i64) = sqlx::query_as("SELECT MAX(copied_at), MIN(copied_at) FROM usage_events")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert!(max_t <= now() + 86400, "future timestamps clamped");
    assert!(min_t >= t - 366 * 86400, "ancient timestamps clamped");
    srv.post(
        &bob,
        routes::USAGE,
        json!({"events": [{"id": "bad id!", "item_id": a.id, "copied_at": t}]}),
    )
    .await
    .expect(200);
    let d: ItemDetail = srv
        .get(&alice, &routes::path(routes::ITEM, &[&a.id]))
        .await
        .expect(200)
        .json();
    assert_eq!(d.item.copies, 2);

    // Popular: server scope for bob = own + public (not alice's unlisted).
    let p = srv
        .get(&bob, &format!("{}?window=all", routes::POPULAR))
        .await
        .expect(200)
        .value();
    let ids = |list: &serde_json::Value| -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|e| e["item"]["id"].as_str().unwrap().to_string())
            .collect()
    };
    let copied = ids(&p["most_copied"]);
    // a and b both have 2 copies (ties broken by id); alice's unlisted item is not listed.
    assert_eq!(copied.len(), 2);
    assert!(copied.contains(&a.id) && copied.contains(&b.id));
    assert!(!copied.contains(&u.id));
    assert_eq!(p["most_copied"][0]["value"], 2);
    assert_eq!(ids(&p["most_liked"]), vec![a.id.clone()]);
    assert_eq!(p["most_liked"][0]["value"], 2);
    assert!(ids(&p["trending"]).contains(&a.id));
    // Window: the 20-day-old copy of b is outside the week.
    let p = srv
        .get(&bob, &format!("{}?window=week", routes::POPULAR))
        .await
        .expect(200)
        .value();
    assert_eq!(
        p["most_copied"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["item"]["id"] == b.id.as_str())
            .map(|e| e["value"].clone()),
        Some(json!(1))
    );
    // Set scope works for readable sets only.
    let p = srv
        .get(
            &bob,
            &format!("{}?scope=set&set={}&window=all", routes::POPULAR, unl_set.id),
        )
        .await
        .expect(200)
        .value();
    assert_eq!(ids(&p["most_copied"]), vec![u.id.clone()]);
    let priv_set = srv.create_set(&alice, "priv", Visibility::Private).await;
    srv.get(&bob, &format!("{}?scope=set&set={}", routes::POPULAR, priv_set.id))
        .await
        .expect(404);
    srv.get(&bob, &format!("{}?scope=set", routes::POPULAR))
        .await
        .expect(400);
    let p = srv
        .get(&bob, &format!("{}?scope=mine", routes::POPULAR))
        .await
        .expect(200)
        .value();
    assert!(p["most_copied"].as_array().unwrap().is_empty());
    let p = srv
        .get(&alice, &format!("{}?scope=mine&window=all", routes::POPULAR))
        .await
        .expect(200)
        .value();
    assert_eq!(ids(&p["most_copied"]).len(), 3);

    // Stats.
    let s: stickpile_api::UserStats = srv.get(&bob, routes::STATS_ME).await.expect(200).json();
    assert_eq!(s.copies_total, 5);
    assert!(s.copies_last_7d >= 3);
    assert_eq!(s.likes_given, 2);
    assert_eq!(s.daily_copies.len(), 30);
    // Days are UTC days: in the first hour after midnight the copy made an
    // hour ago (e2) counts for yesterday.
    let e2_today = t.rem_euclid(86400) >= 3600;
    assert!(s.daily_copies[29] > u64::from(e2_today));
    assert!(s.daily_copies[28] + s.daily_copies[29] >= 2);
    assert!(s
        .top_targets
        .iter()
        .any(|(t, n)| *t == stickpile_api::TargetApp::Telegram && *n == 1));
    assert!(s.top_items.iter().take(2).any(|e| e.item.id == a.id && e.value == 2));
    let s: stickpile_api::UserStats = srv.get(&alice, routes::STATS_ME).await.expect(200).json();
    assert_eq!(s.sets, 3);
    assert_eq!(s.items, 3);
    assert!(s.storage_bytes > 0);
    assert_eq!(s.likes_received, 2, "own likes do not count as received");

    // Profiles.
    let p: Vec<stickpile_api::profiles::TargetProfile> = srv.get(&bob, routes::PROFILES).await.expect(200).json();
    assert_eq!(p.len(), stickpile_api::TargetApp::ALL.len());
}

#[tokio::test]
async fn better_versions() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let a_set = srv.create_set(&alice, "a", Visibility::Private).await;
    let b_set = srv.create_set(&bob, "b", Visibility::Public).await;
    let b_priv = srv.create_set(&bob, "bp", Visibility::Private).await;
    let low = srv.upload_one(&alice, &a_set.id, "low.png", png(9)).await;
    let hi = srv.upload_one(&bob, &b_set.id, "hi.png", png_near(9)).await;
    let hidden = srv.upload_one(&bob, &b_priv.id, "hidden.png", png_near(9)).await;
    // Force qualities.
    for (id, q) in [(&low.media.id, 20), (&hi.media.id, 90), (&hidden.media.id, 95)] {
        sqlx::query("UPDATE media SET quality = ? WHERE id = ?")
            .bind(q)
            .bind(id)
            .execute(&srv.state.db)
            .await
            .unwrap();
    }
    let better: Vec<ItemSummary> = srv
        .get(&alice, &routes::path(routes::ITEM_BETTER, &[&low.id]))
        .await
        .expect(200)
        .json();
    let ids: Vec<&str> = better.iter().map(|b| b.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![hi.id.as_str()],
        "public better version found, private one hidden"
    );
    let none: Vec<ItemSummary> = srv
        .get(&bob, &routes::path(routes::ITEM_BETTER, &[&hidden.id]))
        .await
        .expect(200)
        .json();
    assert!(none.is_empty());
}

fn untar(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    use std::io::Read;
    let mut ar = tar::Archive::new(bytes);
    let mut out = Vec::new();
    for e in ar.entries().unwrap() {
        let mut e = e.unwrap();
        if e.header().entry_type().is_dir() {
            continue;
        }
        let p = e.path().unwrap().to_string_lossy().into_owned();
        let mut b = Vec::new();
        e.read_to_end(&mut b).unwrap();
        out.push((p, b));
    }
    out
}

async fn import_tar(srv: &TestServer, user: &TestUser, tar: Vec<u8>, into: Option<&str>) -> common::Resp {
    let mut url = srv.url(routes::SETS_IMPORT);
    if let Some(i) = into {
        url.push_str(&format!("?into={i}"));
    }
    let form =
        reqwest::multipart::Form::new().part("archive", reqwest::multipart::Part::bytes(tar).file_name("set.tar"));
    common::read(
        srv.client
            .post(url)
            .bearer_auth(&user.token)
            .multipart(form)
            .send()
            .await
            .unwrap(),
    )
    .await
}

#[tokio::test]
async fn import_export_round_trip() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;

    // Import a directory as loose files + set.toml (AI markers, unknown keys).
    let manifest = r#"
version = 7
name = "My cats"
slug = "my-cats"
description = "the best"
visibility = "unlisted"
aspect_ratio = "1:1"
recommended_px = 512
custom_top = "kept"

[items."001-cat.png"]
name = "cat"
keywords = ["cat", "stare"]
keywords_source = "ai"
emojis = ["😼"]
vibe = "smug"
vibe_source = "ai"
tags = ["reaction"]
x_custom = 42

[items."002-naahh.gif"]
name = "naahh"
"#;
    let form = reqwest::multipart::Form::new()
        .text("set.toml", manifest.to_string())
        .part(
            "file",
            reqwest::multipart::Part::bytes(png(1)).file_name("my-cats/001-cat.png"),
        )
        .part(
            "file",
            reqwest::multipart::Part::bytes(gif_anim(2)).file_name("my-cats/002-naahh.gif"),
        )
        .part(
            "file",
            reqwest::multipart::Part::bytes(jpeg(3)).file_name("my-cats/003-sigh.jpg"),
        );
    let report: ImportReport = common::read(
        srv.client
            .post(srv.url(routes::SETS_IMPORT))
            .bearer_auth(&alice.token)
            .multipart(form)
            .send()
            .await
            .unwrap(),
    )
    .await
    .expect(200)
    .json();
    assert_eq!(report.created, 3, "{report:?}");
    let set = report.set.clone().unwrap();
    assert_eq!(set.slug, "my-cats");
    assert_eq!(set.version, 7);
    assert_eq!(set.visibility, Visibility::Unlisted);
    assert_eq!(set.aspect_ratio, Some((1, 1)));
    let d: SetDetail = srv
        .get(&alice, &routes::path(routes::SET, &[&set.id]))
        .await
        .expect(200)
        .json();
    let names: Vec<&str> = d.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["cat", "naahh", "sigh"]);
    assert_eq!(d.items[0].keywords_source, FieldSource::Ai);
    assert_eq!(d.items[0].vibe.as_deref(), Some("smug"));
    assert_eq!(d.items[0].emojis_source, FieldSource::User);

    // Same slug again -> 409.
    let tar1 = srv
        .get(&alice, &routes::path(routes::SET_EXPORT, &[&set.id]))
        .await
        .expect(200);
    assert_eq!(tar1.header("content-type").unwrap(), "application/x-tar");
    assert!(tar1.header("content-disposition").unwrap().contains("my-cats.tar"));
    let r = import_tar(&srv, &alice, tar1.body.clone(), None).await.expect(409);
    assert_eq!(r.code(), "conflict");

    // Bob imports the export as a fresh set and exports it again: identical.
    let rep: ImportReport = import_tar(&srv, &bob, tar1.body.clone(), None).await.expect(200).json();
    assert_eq!(rep.created, 3);
    let bob_set = rep.set.unwrap();
    let tar2 = srv
        .get(&bob, &routes::path(routes::SET_EXPORT, &[&bob_set.id]))
        .await
        .expect(200);
    let e1 = untar(&tar1.body);
    let e2 = untar(&tar2.body);
    assert_eq!(
        e1.iter().map(|e| &e.0).collect::<Vec<_>>(),
        e2.iter().map(|e| &e.0).collect::<Vec<_>>()
    );
    let names: Vec<&str> = e1.iter().map(|e| e.0.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "my-cats/set.toml",
            "my-cats/001-cat.png",
            "my-cats/002-naahh.gif",
            "my-cats/003-sigh.jpg"
        ]
    );
    for ((n1, b1), (_, b2)) in e1.iter().zip(&e2) {
        if n1.ends_with("set.toml") {
            let m1 = stickpile_setfmt::Manifest::parse(std::str::from_utf8(b1).unwrap()).unwrap();
            let m2 = stickpile_setfmt::Manifest::parse(std::str::from_utf8(b2).unwrap()).unwrap();
            assert_eq!(m1, m2, "manifest data identical");
            assert_eq!(m1.version, 7);
            assert_eq!(m1.extra.get("custom_top").and_then(|v| v.as_str()), Some("kept"));
            let cat = &m1.items["001-cat.png"];
            assert_eq!(cat.keywords_source, FieldSource::Ai);
            assert_eq!(cat.extra.get("x_custom").and_then(|v| v.as_integer()), Some(42));
        } else {
            assert_eq!(b1, b2, "{n1} byte-identical");
        }
    }
    // Exported originals are the stored originals.
    let orig = srv
        .get(&alice, &routes::path(routes::ITEM_FILE, &[&d.items[0].id, "original"]))
        .await
        .expect(200);
    assert_eq!(e1[1].1, orig.body);

    // Re-import into the same set: nothing changes, version stays.
    let rep: ImportReport = import_tar(&srv, &alice, tar1.body.clone(), Some(&set.id))
        .await
        .expect(200)
        .json();
    assert_eq!(
        (rep.created, rep.updated, rep.removed, rep.unchanged),
        (0, 0, 0, 3),
        "{rep:?}"
    );
    assert_eq!(rep.set.unwrap().version, 7);

    // Edit the directory: drop one file, rename one, add one, reorder via filenames.
    let mut entries = untar(&tar1.body);
    entries.retain(|(n, _)| !n.ends_with("002-naahh.gif"));
    let mut m = stickpile_setfmt::Manifest::parse(std::str::from_utf8(&entries[0].1).unwrap()).unwrap();
    m.items.remove("002-naahh.gif");
    m.items.get_mut("001-cat.png").unwrap().name = "Cat!".into();
    m.description = "changed".into();
    let sigh = m.items.remove("003-sigh.jpg").unwrap();
    m.items.insert("000-sigh.jpg".into(), sigh);
    let mut builder = tar::Builder::new(Vec::new());
    let mut add = |name: &str, bytes: &[u8]| {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        builder.append_data(&mut h, name, bytes).unwrap();
    };
    add("set.toml", m.to_toml().as_bytes());
    add("001-cat.png", &entries[1].1);
    add("000-sigh.jpg", &entries[2].1);
    add("004-new.png", &png(44));
    let tar3 = builder.into_inner().unwrap();
    let rep: ImportReport = import_tar(&srv, &alice, tar3, Some(&set.id)).await.expect(200).json();
    // sigh only moved (prefix change): unchanged metadata, new position.
    assert_eq!(
        (rep.created, rep.updated, rep.unchanged, rep.removed),
        (1, 1, 1, 1),
        "{rep:?}"
    );
    let s = rep.set.unwrap();
    assert_eq!(s.version, 8);
    assert_eq!(s.description, "changed");
    let d: SetDetail = srv
        .get(&alice, &routes::path(routes::SET, &[&set.id]))
        .await
        .expect(200)
        .json();
    let names: Vec<&str> = d.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["sigh", "Cat!", "new"]);
    assert_eq!(d.items[1].keywords_source, FieldSource::Ai, "AI markers preserved");

    // Import into someone else's set is refused; malformed archives are 400.
    import_tar(&srv, &bob, tar1.body.clone(), Some(&set.id))
        .await
        .expect(403);
    import_tar(&srv, &alice, b"not a tar".to_vec(), None).await.expect(400);
    // Path traversal inside an archive is rejected.
    let mut builder = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    let data = png(5);
    h.set_size(data.len() as u64);
    h.set_mode(0o644);
    h.as_gnu_mut().unwrap().name[..12].copy_from_slice(b"../evil.png\0");
    h.set_cksum();
    builder.append(&h, data.as_slice()).unwrap();
    let evil = builder.into_inner().unwrap();
    let r = import_tar(&srv, &alice, evil, None).await;
    assert!(
        r.status == 400 || r.value()["created"] == 0,
        "{} {}",
        r.status,
        r.text()
    );
}

#[tokio::test]
async fn read_only_git_pack_is_not_editable() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "pack", Visibility::Public).await;
    let item = srv.upload_one(&u, &set.id, "a.png", png(1)).await;
    sqlx::query(
        "UPDATE sets SET source = 'git', git_mode = 'read_only', git_url = 'https://github.com/x/y.git' WHERE id = ?",
    )
    .bind(&set.id)
    .execute(&srv.state.db)
    .await
    .unwrap();
    let d: SetDetail = srv
        .get(&u, &routes::path(routes::SET, &[&set.id]))
        .await
        .expect(200)
        .json();
    assert!(!d.set.editable);
    assert_eq!(
        d.set.git.as_ref().unwrap().web_url.as_deref(),
        Some("https://github.com/x/y")
    );
    srv.patch(&u, &routes::path(routes::SET, &[&set.id]), json!({"name": "x"}))
        .await
        .expect(403);
    srv.patch(&u, &routes::path(routes::ITEM, &[&item.id]), json!({"name": "x"}))
        .await
        .expect(403);
    srv.upload(&u, &set.id, vec![("b.png", png(2))], None).await.expect(403);
    // Likes and usage still work; the owner may delete (unsubscribe).
    srv.put(&u, &routes::path(routes::ITEM_LIKE, &[&item.id]), json!({}))
        .await
        .expect(200);
    srv.delete(&u, &routes::path(routes::SET, &[&set.id])).await.expect(204);
}

#[tokio::test]
async fn orphan_media_gc_removes_files() {
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    let item = srv.upload_one(&u, &set.id, "a.png", png(1)).await;
    let sha = item.media.sha256.clone();
    let dir = srv.state.config.media_dir().join(&sha[..2]).join(&sha);
    assert!(dir.join("original.png").is_file());
    assert!(dir.join("thumb.webp").is_file());
    stickpile_server::cleanup_once(&srv.state).await.unwrap();
    assert!(dir.is_dir(), "referenced media kept");
    srv.delete(&u, &routes::path(routes::ITEM, &[&item.id]))
        .await
        .expect(204);
    stickpile_server::cleanup_once(&srv.state).await.unwrap();
    assert!(!dir.exists(), "orphaned media removed");
    // Re-uploading works afterwards.
    srv.upload_one(&u, &set.id, "a.png", png(1)).await;
    assert!(dir.join("original.png").is_file());
    let _ = Method::GET;
}
