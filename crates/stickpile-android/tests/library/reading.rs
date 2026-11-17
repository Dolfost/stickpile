//! Search, the lists and the suggestions: online, offline and signed out.

use stickpile_android::{HintKind, LibraryErrorKind, MediaFormat, MediaKind, Target};
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

/// The user's complaint: with the server away every list waited for the
/// timeout. Once a request finds it unreachable, the next reads answer
/// from the cache at once, without a request, until a refresh asks for it.
#[tokio::test]
async fn a_server_found_unreachable_is_not_waited_for_again() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    lib.sync_now().await.unwrap();
    assert!(!lib.packs().await.unwrap().offline);

    server.stop();
    assert!(lib.favorites(10).await.is_err() || lib.packs().await.unwrap().offline);
    server.restart();
    server.state().requests.clear();
    let started = std::time::Instant::now();
    let packs = lib.packs().await.unwrap();
    let found = lib.search("fire".into(), Target::Generic, 10).await.unwrap();
    let used = lib.most_used(10).await.unwrap();
    assert!(packs.offline && found.offline && used.offline);
    assert_eq!(ids(&found.items), vec![items[3].id.clone()]);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "answered at once: {:?}",
        started.elapsed()
    );
    assert!(
        server.state().requests.is_empty(),
        "nothing was sent: {:?}",
        server.state().requests
    );

    // A refresh tries the server, and then the lists come from it again.
    lib.sync_now().await.unwrap();
    assert!(!lib.packs().await.unwrap().offline);
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

    // A short list online still remembers them all for offline.
    server.restart();
    assert!(lib.toggle_like(items[4].id.clone()).await.unwrap());
    let one = lib.favorites(1).await.unwrap();
    assert!(!one.offline);
    assert_eq!(one.items.len(), 1);
    server.stop();
    let offline = lib.favorites(10).await.unwrap();
    assert!(offline.offline);
    let mut got = ids(&offline.items);
    got.sort();
    let mut want = vec![items[0].id.clone(), items[2].id.clone(), items[4].id.clone()];
    want.sort();
    assert_eq!(got, want);

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

/// The ids `pack_search` finds in `pack` for `query`, and whether the
/// cache answered.
async fn in_pack(lib: &StickpileLibrary, pack: &str, query: &str) -> (Vec<String>, bool) {
    let list = lib.pack_search(pack.into(), query.into()).await.unwrap();
    (ids(&list.items), list.offline)
}

/// The search inside an open pack (the app's Packs tab, the keyboard's
/// Sets): the whole syntax over the pack's items, here and not on the
/// server, online and from the cache.
#[tokio::test]
async fn pack_search_online_offline_and_unknown() {
    let (mut server, items) = seeded_server();
    {
        let mut st = server.state();
        for (item, vibe, tags) in [
            (0, "angry", ["reaction"]),
            (1, "tired", ["nsfw"]),
            (2, "tired", ["reaction"]),
        ] {
            let m = st.items.iter_mut().find(|m| m.item.id == items[item].id).unwrap();
            m.item.vibe = Some(vibe.into());
            m.item.tags = tags.map(String::from).to_vec();
        }
    }
    let (grumpy, naahh, sleepy) = (items[0].id.clone(), items[1].id.clone(), items[2].id.clone());
    let cats = items[0].set_id.clone();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());

    let all = vec![grumpy.clone(), naahh.clone(), sleepy.clone()];
    assert_eq!(in_pack(&lib, &cats, "").await, (all.clone(), false), "blank: the pack");
    assert_eq!(in_pack(&lib, &cats, "  ").await.0, all);
    assert_eq!(
        in_pack(&lib, &cats, "vibe:tired").await,
        (vec![naahh.clone(), sleepy.clone()], false)
    );
    assert_eq!(in_pack(&lib, &cats, "v:tired").await.0, [naahh.clone(), sleepy.clone()]);
    assert_eq!(in_pack(&lib, &cats, "v:tired -t:nsfw").await.0, [sleepy.as_str()]);
    assert_eq!(
        in_pack(&lib, &cats, "t:reaction").await.0,
        [grumpy.clone(), sleepy.clone()]
    );
    assert_eq!(in_pack(&lib, &cats, "type:gif").await.0, [naahh.as_str()]);
    assert_eq!(
        in_pack(&lib, &cats, "v:tir").await.0,
        [naahh.clone(), sleepy.clone()],
        "half typed"
    );
    assert_eq!(in_pack(&lib, &cats, "grumpy").await.0, [grumpy.as_str()]);
    assert_eq!(in_pack(&lib, &cats, "grumpy type:gif").await.0, Vec::<String>::new());
    assert!(
        in_pack(&lib, &cats, "dog").await.0.is_empty(),
        "other packs' items stay out"
    );
    assert_eq!(in_pack(&lib, &cats, "set:cats").await.0, all);
    let searched = server
        .state()
        .requests
        .iter()
        .filter(|r| r.starts_with("GET /api/v1/search"))
        .count();
    assert_eq!(searched, 0, "the pack is searched here");

    // The pack's visibility once listed; the account and its likes once synced and read.
    lib.packs().await.unwrap();
    assert_eq!(in_pack(&lib, &cats, "is:public").await.0, all);
    lib.sync_now().await.unwrap();
    assert_eq!(in_pack(&lib, &cats, "is:mine").await.0, all);
    assert!(lib.toggle_like(sleepy.clone()).await.unwrap());
    lib.favorites(10).await.unwrap();
    assert_eq!(in_pack(&lib, &cats, "is:liked").await.0, [sleepy.as_str()]);

    // Offline: the cached items of the pack.
    server.stop();
    assert_eq!(
        in_pack(&lib, &cats, "v:tired -t:nsfw").await,
        (vec![sleepy.clone()], true)
    );
    assert_eq!(
        in_pack(&lib, &cats, "is:mine -is:liked").await,
        (vec![grumpy, naahh], true)
    );
    assert_eq!(in_pack(&lib, &cats, "").await, (all, true));
    assert_eq!(
        kind(lib.pack_search("nope".into(), "cat".into()).await),
        LibraryErrorKind::Unreachable
    );

    server.restart();
    let out = tempfile::tempdir().unwrap();
    let lib = open(out.path(), &server.url);
    assert_eq!(
        kind(lib.pack_search(cats, "cat".into()).await),
        LibraryErrorKind::NotSignedIn
    );
}

/// What the tiles show: the owner, the format, the nsfw tag, the words in
/// the picture, the account's likes once the favorites were read, and the
/// item's page.
#[tokio::test]
async fn items_carry_what_the_tiles_show() {
    let (server, items) = seeded_server();
    {
        let mut st = server.state();
        let grumpy = st.items.iter_mut().find(|m| m.item.id == items[0].id).unwrap();
        grumpy.item.tags = vec!["fun".into(), " NSFW ".into()];
        st.set_text(&items[0].id, "NO THOUGHTS\nhead empty", stickpile_api::FieldSource::Ai);
    }
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let find = |list: &[stickpile_android::Item], id: &str| list.iter().find(|i| i.id == id).cloned().unwrap();

    let found = lib.search("cat".into(), Target::Generic, 10).await.unwrap().items;
    let grumpy = find(&found, &items[0].id);
    let gif = find(&found, &items[1].id);
    assert!(grumpy.nsfw, "tagged NSFW, any case");
    assert!(!gif.nsfw);
    assert_eq!((grumpy.owner.as_str(), grumpy.format), ("alice", MediaFormat::Png));
    assert_eq!(gif.format, MediaFormat::Gif);
    assert_eq!(grumpy.text, "NO THOUGHTS\nhead empty", "line breaks kept");
    assert_eq!(gif.text, "", "none");
    assert!(!grumpy.liked && !gif.liked, "the favorites were never read");

    // The favorites are saved under the account the cache knows: after a sync.
    lib.sync_now().await.unwrap();
    assert!(lib.toggle_like(items[1].id.clone()).await.unwrap());
    lib.favorites(1).await.unwrap();
    let found = lib.search("cat".into(), Target::Generic, 10).await.unwrap().items;
    assert!(find(&found, &items[1].id).liked);
    assert!(!find(&found, &items[0].id).liked);

    // A like made here shows at once, without reading the favorites again.
    assert!(lib.toggle_like(items[0].id.clone()).await.unwrap());
    let found = lib.search("cat".into(), Target::Generic, 10).await.unwrap().items;
    assert!(find(&found, &items[0].id).liked);
    assert!(!lib.toggle_like(items[1].id.clone()).await.unwrap());
    let pack = lib.pack_items(items[0].set_id.clone()).await.unwrap().items;
    assert!(!find(&pack, &items[1].id).liked);
    assert!(find(&pack, &items[0].id).liked);

    assert_eq!(
        lib.item_url(items[0].id.clone()),
        Some(format!("{}/items/{}", server.url.trim_end_matches('/'), items[0].id))
    );
}

#[tokio::test]
async fn hints_come_from_the_cache() {
    let (server, _) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let keys = lib.hints(String::new());
    assert_eq!(keys[0].label, "tag: (t:)");
    assert_eq!(keys[0].query, "tag:");
    assert!(keys.iter().all(|h| h.kind == HintKind::Key));

    lib.sync_now().await.unwrap();
    let values = lib.hints("grumpy set:ca".into());
    assert_eq!(values[0].label, "cats");
    assert_eq!(values[0].query, "grumpy set:cats ");
    assert!(values[0].chosen);
    let filters = lib.hints("do".into());
    assert!(
        filters
            .iter()
            .any(|h| h.label == "set:dogs" && h.query == "set:dogs " && h.kind == HintKind::Filter),
        "{filters:?}"
    );
    assert!(lib.hints("\"half a phr".into()).is_empty());
}
