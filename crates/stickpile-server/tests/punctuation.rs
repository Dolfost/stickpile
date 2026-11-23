//! Punctuation in the item text and the name (docs/search.md, "Tokens"):
//! the same queries give the same items through the server (`GET
//! /api/v1/search`), the clients' offline index (a `stickpile_core` cache
//! synced from the server, its `OfflineIndex`) and the in-pack search
//! (`stickpile_quick::within::Within` over the cached items), as in
//! `review_ts.rs`.

mod common;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

use common::{png, read};
use serde_json::json;
use stickpile_api::{routes, ItemSummary, SearchResults, UploadResult, Visibility};
use stickpile_core::cache::{self, Cache};
use stickpile_core::ApiClient;
use stickpile_quick::within::Within;

/// (label, owner, visibility of the set, name, text).
const ITEMS: &[(&str, &str, Visibility, &str, &str)] = &[
    ("q", "alice", Visibility::Private, "first", "?"),
    ("qq", "alice", Visibility::Private, "second", "??"),
    ("qx", "alice", Visibility::Private, "third", "?!"),
    ("what", "alice", Visibility::Private, "fourth", "what?"),
    ("what_sp", "alice", Visibility::Private, "fifth", "WHAT ?"),
    ("what_no", "alice", Visibility::Private, "sixth", "what"),
    ("whatever", "alice", Visibility::Private, "seventh", "whatever?"),
    (
        "full",
        "alice",
        Visibility::Private,
        "eighth",
        "\u{0447}\u{043e}\u{FF1F}",
    ),
    ("wait", "alice", Visibility::Private, "ninth", "wait..."),
    ("dots4", "alice", Visibility::Private, "tenth", "...."),
    ("ellipsis", "alice", Visibility::Private, "eleventh", "\u{2026}"),
    ("dots2", "alice", Visibility::Private, "twelfth", "so.. ok."),
    ("name_q", "alice", Visibility::Private, "Really?", ""),
    ("name_dots", "alice", Visibility::Private, "wait\u{2026}", ""),
    (
        "phrase",
        "alice",
        Visibility::Private,
        "thirteenth",
        "wait... what?! no",
    ),
    ("none", "alice", Visibility::Private, "nothing", "nothing"),
    ("bob_pub", "bob", Visibility::Public, "bobs", "huh?"),
    ("bob_priv", "bob", Visibility::Private, "secret?", "hidden?"),
];

const QUERIES: &[&str] = &[
    "x:?",
    "x:\u{FF1F}",
    "x:??",
    "x:?!",
    "x:!",
    "x:...",
    "x:\u{2026}",
    "x:..",
    "x:\"what?\"",
    "x:what?",
    "x:\"waht?\"",
    "x:\"wait... what?\"",
    "-x:?",
    "-x:...",
    "x:what -x:?",
    "?",
    "...",
    "\u{2026}",
    "? ...",
    "what?",
    "wait...",
    "-?",
    "-\u{2026}",
    "x:? sort:recent",
];

#[tokio::test]
async fn punctuation_queries_agree_on_server_offline_and_within() {
    let srv = common::TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut sets = HashMap::new();
    for (n, (label, owner, vis, name, text)) in ITEMS.iter().enumerate() {
        let u = if *owner == "alice" { &alice } else { &bob };
        let key = format!("{owner}-{vis:?}").to_lowercase();
        if !sets.contains_key(&key) {
            sets.insert(key.clone(), srv.create_set(u, &key, *vis).await);
        }
        let res: Vec<UploadResult> = srv
            .upload(
                u,
                &sets[&key].id,
                vec![(&format!("{label}.png"), png(8000 + n as u64))],
                Some(json!([{"name": name, "text": text}])),
            )
            .await
            .expect(200)
            .json();
        let item = res.into_iter().next().unwrap().item.expect("item");
        assert_eq!(item.text, *text, "{label}: the text as given");
        labels.insert(item.id, label.to_string());
    }
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path(), 64).unwrap();
    let client = ApiClient::new(&srv.base, Duration::from_secs(10))
        .unwrap()
        .with_token(Some(alice.token.clone()));
    let me = client.me().await.unwrap();
    cache.bind_server(&srv.base).unwrap();
    cache.set_me(&me).unwrap();
    cache::sync(&cache, &client, 500).await.unwrap();
    let offline = cache.offline_index().unwrap();
    let mut items: Vec<ItemSummary> = offline.items().cloned().collect();
    items.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(items.len(), ITEMS.len() - 1, "all but bob's private one");
    let within = Within::new(&items, None, Some("alice"));

    let mut problems = Vec::new();
    for q in QUERIES {
        let resp = read(
            srv.client
                .get(srv.url(routes::SEARCH))
                .query(&[("q", *q), ("limit", "200")])
                .bearer_auth(&alice.token)
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{q}");
        let r: SearchResults = resp.json();
        let server: Vec<String> = r.hits.iter().map(|h| labels[&h.item.id].clone()).collect();
        let off: Vec<String> = offline
            .search(q, None, 200)
            .iter()
            .map(|h| labels[&h.item.id].clone())
            .collect();
        let inn: Vec<String> = within
            .search(q, Some("alice"), &HashSet::new())
            .iter()
            .map(|f| labels[&items[f.index].id].clone())
            .collect();
        println!(
            "{q:<22} server {server:?}\n{:<22} offline {off:?}\n{:<22} within {inn:?}",
            "", ""
        );
        let set = |v: &[String]| v.iter().cloned().collect::<BTreeSet<_>>();
        if server != off || set(&server) != set(&inn) {
            problems.push(format!("{q:?}: server {server:?} offline {off:?} within {inn:?}"));
        }
        assert!(!server.contains(&"bob_priv".to_string()), "{q}");
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));

    // What they find, as docs/search.md says.
    let found = |q: &str| -> BTreeSet<String> {
        offline
            .search(q, None, 200)
            .iter()
            .map(|h| labels[&h.item.id].clone())
            .collect()
    };
    let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
    assert_eq!(
        found("x:?"),
        set(&["q", "qq", "qx", "what", "what_sp", "whatever", "full", "phrase", "bob_pub"])
    );
    assert_eq!(found("x:..."), set(&["wait", "dots4", "ellipsis", "phrase"]));
    assert_eq!(found("x:\"what?\""), set(&["what", "what_sp", "phrase"]));
    assert_eq!(
        found("?"),
        set(&["q", "qq", "qx", "what", "what_sp", "whatever", "full", "phrase", "bob_pub", "name_q"])
    );
    assert_eq!(found("..."), set(&["wait", "dots4", "ellipsis", "phrase", "name_dots"]));
}
