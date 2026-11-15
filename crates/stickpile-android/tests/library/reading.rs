//! Search, the lists and the suggestions: online, offline and signed out.

use stickpile_android::{LibraryErrorKind, MediaKind, Target};
use stickpile_core::mock::dead_url;

use super::*;

#[tokio::test]
async fn search_online_offline_and_signed_out() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());

    let found = lib.search("cat".into(), Target::Discord, 10).await.unwrap();
    assert!(!found.offline);
    let mut got = ids(&found.items);
    got.sort();
    let mut want = vec![items[0].id.clone(), items[1].id.clone()];
    want.sort();
    assert_eq!(got, want);
    let grumpy = found.items.iter().find(|i| i.id == items[0].id).unwrap();
    assert_eq!(grumpy.name, "grumpy cat");
    assert_eq!(grumpy.set_name, "Cats");
    assert_eq!(grumpy.keywords, vec!["cat", "angry"]);
    assert_eq!((grumpy.kind, grumpy.width, grumpy.height), (MediaKind::Still, 20, 20));
    let gif = found.items.iter().find(|i| i.id == items[1].id).unwrap();
    assert_eq!(gif.kind, MediaKind::Animation);
    assert_eq!(
        lib.search("cat".into(), Target::Generic, 1).await.unwrap().items.len(),
        1
    );

    // Offline: the cache answers once a sync filled it.
    lib.sync_now().await.unwrap();
    server.stop();
    let offline = lib.search("fire".into(), Target::Generic, 10).await.unwrap();
    assert!(offline.offline);
    assert_eq!(ids(&offline.items), vec![items[3].id.clone()]);
    // Also in a library opened while the server is down.
    let again = open(dir.path(), &server.url);
    let offline = again.search("sleepy".into(), Target::Telegram, 10).await.unwrap();
    assert!(offline.offline);
    assert_eq!(ids(&offline.items), vec![items[2].id.clone()]);

    // Signed out (the server up, no token): refused.
    server.restart();
    let out = tempfile::tempdir().unwrap();
    let lib = open(out.path(), &server.url);
    assert_eq!(
        kind(lib.search("cat".into(), Target::Generic, 5).await),
        LibraryErrorKind::NotSignedIn
    );
    // Signed out and offline: the (empty) cache answers.
    let lib = open(out.path(), &dead_url());
    let empty = lib.search("cat".into(), Target::Generic, 5).await.unwrap();
    assert!(empty.offline && empty.items.is_empty());
}

#[tokio::test]
async fn search_uses_the_suggested_filter_value() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    lib.sync_now().await.unwrap();
    // `set:dog` is half typed: the search goes out for `set:dogs`.
    let found = lib.search("set:dog".into(), Target::Generic, 10).await.unwrap();
    assert_eq!(ids(&found.items), vec![items[4].id.clone()]);
    let q = server
        .state()
        .requests
        .iter()
        .filter(|r| r.starts_with("GET /api/v1/search"))
        .count();
    assert_eq!(q, 1);
}

#[tokio::test]
async fn suggestions_come_from_the_cache() {
    let (server, _) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    assert_eq!(lib.suggestions("set:ca".into()), None, "nothing cached yet");
    let is = lib.suggestions("is:li".into()).unwrap();
    assert_eq!(
        (is.key.as_str(), is.words[0].as_str(), is.chosen),
        ("is:", "liked", Some(0))
    );

    lib.sync_now().await.unwrap();
    let s = lib.suggestions("grumpy set:ca".into()).unwrap();
    assert_eq!(s.key, "set:");
    assert_eq!(s.words[0], "cats");
    assert_eq!(s.chosen, Some(0));
    let typed = lib.suggestions("-set:".into()).unwrap();
    assert_eq!(typed.key, "-set:");
    assert!(typed.words.contains(&"dogs".to_string()), "{:?}", typed.words);
    assert_eq!(typed.chosen, None, "nothing typed yet");
    assert_eq!(lib.suggestions("cat".into()), None);
    assert_eq!(lib.suggestions("set:cats ".into()), None);
}

#[tokio::test]
async fn recent_is_this_devices_history() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    assert!(lib.recent(10).is_empty());
    lib.search("cat".into(), Target::Generic, 10).await.unwrap();
    lib.record_use(items[0].id.clone(), "original".into(), "com.discord".into())
        .unwrap();
    lib.record_use(items[1].id.clone(), "original".into(), "com.discord".into())
        .unwrap();
    assert_eq!(ids(&lib.recent(10)), vec![items[1].id.clone(), items[0].id.clone()]);
    assert_eq!(lib.recent(1).len(), 1);

    // Signed out: the history went with the account.
    lib.logout().await.unwrap();
    assert!(lib.recent(10).is_empty());
}

#[tokio::test]
async fn favorites_online_remembered_offline_and_signed_out() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    lib.sync_now().await.unwrap();
    assert!(lib.favorites(10).await.unwrap().items.is_empty());
    assert!(lib.toggle_like(items[2].id.clone()).await.unwrap());
    let online = lib.favorites(10).await.unwrap();
    assert!(!online.offline);
    assert_eq!(ids(&online.items), vec![items[2].id.clone()]);
    // A like after the last read is remembered too.
    assert!(lib.toggle_like(items[0].id.clone()).await.unwrap());

    server.stop();
    let offline = lib.favorites(10).await.unwrap();
    assert!(offline.offline);
    assert_eq!(ids(&offline.items), vec![items[0].id.clone(), items[2].id.clone()]);
    assert_eq!(lib.favorites(1).await.unwrap().items.len(), 1);

    // Never read online: nothing to answer with.
    let other = tempfile::tempdir().unwrap();
    server.restart();
    let fresh = signed_in(&server, other.path());
    fresh.sync_now().await.unwrap();
    server.stop();
    assert_eq!(kind(fresh.favorites(10).await), LibraryErrorKind::Unreachable);

    server.restart();
    let out = tempfile::tempdir().unwrap();
    let lib = open(out.path(), &server.url);
    assert_eq!(kind(lib.favorites(10).await), LibraryErrorKind::NotSignedIn);
}

#[tokio::test]
async fn most_used_online_and_offline() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    lib.sync_now().await.unwrap();
    for id in [&items[1].id, &items[1].id, &items[0].id] {
        lib.record_use(id.clone(), "original".into(), "".into()).unwrap();
    }
    assert!(wait_until(|| server.state().usage.len() == 3).await);
    let online = lib.most_used(10).await.unwrap();
    assert!(!online.offline);
    assert_eq!(ids(&online.items)[..2], [items[1].id.clone(), items[0].id.clone()]);

    server.stop();
    let offline = lib.most_used(10).await.unwrap();
    assert!(offline.offline);
    assert_eq!(ids(&offline.items), vec![items[1].id.clone(), items[0].id.clone()]);

    server.restart();
    let out = tempfile::tempdir().unwrap();
    let lib = open(out.path(), &server.url);
    assert_eq!(kind(lib.most_used(10).await), LibraryErrorKind::NotSignedIn);
}

#[tokio::test]
async fn packs_online_offline_and_signed_out() {
    let (mut server, _) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    // Before any sync: whose packs are mine is asked from the server.
    let online = lib.packs().await.unwrap();
    assert!(!online.offline);
    let names: Vec<(&str, bool, u32)> = online
        .packs
        .iter()
        .map(|p| (p.name.as_str(), p.mine, p.item_count))
        .collect();
    assert_eq!(names.len(), 3);
    assert!(
        names[..2].contains(&("Cats", true, 3)) && names[..2].contains(&("Memes", true, 1)),
        "{names:?}"
    );
    assert_eq!(names[2], ("Dogs", false, 1));
    assert_eq!(online.packs[2].owner, "bob");
    assert_eq!(online.packs[2].slug, "dogs");

    lib.sync_now().await.unwrap();
    server.stop();
    let offline = lib.packs().await.unwrap();
    assert!(offline.offline);
    let mut names: Vec<(String, bool)> = offline.packs.iter().map(|p| (p.name.clone(), p.mine)).collect();
    names.sort();
    assert_eq!(
        names,
        vec![("Cats".into(), true), ("Dogs".into(), false), ("Memes".into(), true)]
    );

    // Offline with nothing cached.
    let empty = tempfile::tempdir().unwrap();
    let lib = open(empty.path(), &server.url);
    assert_eq!(kind(lib.packs().await), LibraryErrorKind::Unreachable);

    server.restart();
    let lib = open(empty.path(), &server.url);
    assert_eq!(kind(lib.packs().await), LibraryErrorKind::NotSignedIn);
}

#[tokio::test]
async fn own_packs_include_empty_ones_online() {
    let (mut server, _) = seeded_server();
    server.state().add_set("alice", "new", "New", Visibility::Private);
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    // packs leaves the empty set out; own_packs keeps it, and only alice's.
    assert!(!lib.packs().await.unwrap().packs.iter().any(|p| p.name == "New"));
    let own = lib.own_packs().await.unwrap();
    assert!(!own.offline);
    let mut names: Vec<(String, bool, u32)> = own
        .packs
        .iter()
        .map(|p| (p.name.clone(), p.mine, p.item_count))
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            ("Cats".into(), true, 3),
            ("Memes".into(), true, 1),
            ("New".into(), true, 0)
        ]
    );

    // Offline: the own packs of the cached items (not the empty one).
    lib.sync_now().await.unwrap();
    server.stop();
    let offline = lib.own_packs().await.unwrap();
    assert!(offline.offline);
    let mut names: Vec<String> = offline.packs.iter().map(|p| p.name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["Cats".to_string(), "Memes".to_string()]);

    server.restart();
    let empty = tempfile::tempdir().unwrap();
    let lib = open(empty.path(), &server.url);
    assert_eq!(kind(lib.own_packs().await), LibraryErrorKind::NotSignedIn);
}

#[tokio::test]
async fn pack_items_online_offline_and_unknown() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let cats = items[0].set_id.clone();
    let online = lib.pack_items(cats.clone()).await.unwrap();
    assert!(!online.offline);
    assert_eq!(
        ids(&online.items),
        vec![items[0].id.clone(), items[1].id.clone(), items[2].id.clone()],
        "pack order"
    );
    assert_eq!(kind(lib.pack_items("nope".into()).await), LibraryErrorKind::NotFound);

    lib.sync_now().await.unwrap();
    server.stop();
    let offline = lib.pack_items(cats.clone()).await.unwrap();
    assert!(offline.offline);
    assert_eq!(
        ids(&offline.items),
        vec![items[0].id.clone(), items[1].id.clone(), items[2].id.clone()],
        "by name"
    );
    assert_eq!(kind(lib.pack_items("nope".into()).await), LibraryErrorKind::Unreachable);

    server.restart();
    let out = tempfile::tempdir().unwrap();
    let lib = open(out.path(), &server.url);
    assert_eq!(kind(lib.pack_items(cats).await), LibraryErrorKind::NotSignedIn);
}
