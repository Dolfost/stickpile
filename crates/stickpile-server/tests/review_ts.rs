//! Independent review of the text-search round (fuzzy `text:`/`x:` and
//! the words `x:` offers): the server (`GET /api/v1/search`, `GET
//! /api/v1/search/suggest`), the clients' offline index (a
//! `stickpile_core` cache synced from the server, its `OfflineIndex`), the
//! in-pack search (`stickpile_quick::within::Within` over the cached
//! items) and the popup's offers (the desktop backend's vocabulary, the
//! model's line) on a library and a query list written for the review.
//! Tables go to stdout (`--nocapture`).

mod common;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use common::{png, read, TestServer, TestUser};
use serde_json::json;
use stickpile_api::{
    routes, ItemSummary, Permissions, SearchResults, SearchSuggestions, Set, UploadResult, Visibility,
};
use stickpile_core::cache::{self, Cache, OfflineIndex};
use stickpile_core::ApiClient;
use stickpile_quick::model::SuggestionLine;
use stickpile_quick::suggest::{self, Vocabulary};
use stickpile_quick::within::Within;
use stickpile_quick::{Event, Model, ModelConfig, OpenInfo};
use stickpile_search::{Index, Query, SearchOptions, Viewer};

/// (key, owner, visibility, alice is a member).
const SETS: [(&str, &str, Visibility, bool); 8] = [
    ("mine", "alice", Visibility::Private, false),
    ("apub", "alice", Visibility::Public, false),
    ("bobs", "bob", Visibility::Public, false),
    ("bsecret", "bob", Visibility::Private, false),
    ("bunl", "bob", Visibility::Unlisted, false),
    ("cteam", "carol", Visibility::Private, true),
    ("cunlteam", "carol", Visibility::Unlisted, true),
    ("cpriv", "carol", Visibility::Private, false),
];

/// (label, set, text).
const ITEMS: &[(&str, &str, &str)] = &[
    ("lol1", "mine", "LOL"),
    ("lolwhat", "mine", "lol what"),
    ("love", "mine", "what is love"),
    ("literally", "mine", "Literally me"),
    ("hello", "mine", "hello there"),
    ("nothoughts", "mine", "NO THOUGHTS\nHEAD EMPTY"),
    ("nothou", "mine", "no thou shalt"),
    ("fixbug", "mine", "fix the bug"),
    ("fixedbugs", "mine", "fixed the bugs"),
    ("joined", "mine", "КОЛИВ ДОМА"),
    ("koly", "mine", "коли ти вдома"),
    ("apos_ascii", "mine", "П'ЯТНИЦЯ"),
    ("apos_2019", "mine", "п\u{2019}ятниця вечір"),
    ("apos_02bc", "mine", "п\u{02BC}ятниця"),
    ("apos_tick", "mine", "п`ятниця"),
    ("ukr", "mine", "Їжак ґанок Єнот іграшка"),
    ("kyiv", "mine", "Київ"),
    ("kava_latin", "mine", "К\u{0041}ВА"),
    ("kot_latin", "mine", "к\u{006F}т"),
    ("digits", "mine", "404 not found 2024"),
    ("first", "mine", "1st place"),
    ("naah", "mine", "naaah"),
    ("lool", "mine", "lool"),
    ("emoji", "mine", "😂😂 lol"),
    ("cest", "mine", "C'est la vie"),
    ("dont", "mine", "don't panic"),
    ("cafe", "mine", "CAFÉ olé"),
    ("whenyou", "mine", "WHENYOU see it"),
    ("thoughts_q", "mine", "thoughts?"),
    ("lmao", "mine", "lmao lmao lmao"),
    ("lmfao1", "mine", "lmfao"),
    ("lmfao2", "mine", "lmfao x"),
    ("glorp_vis", "mine", "glorp"),
    ("glorb1", "mine", "glorb"),
    ("glorb2", "apub", "glorb"),
    ("pub_skull", "apub", "no thoughts"),
    ("duck", "bobs", "quack кря lol"),
    ("code", "bobs", "when the code works"),
    ("secret", "bsecret", "zyzzyva lol lol секретик"),
    ("glorp_h1", "bsecret", "glorp hidden"),
    ("glorp_h2", "bsecret", "glorp hidden"),
    ("glorp_h3", "bsecret", "glorp hidden"),
    ("glorp_h4", "bsecret", "glorp hidden"),
    ("unl", "bunl", "quokka unlisted"),
    ("team", "cteam", "platypus teamwork"),
    ("unlteam", "cunlteam", "echidna"),
    ("cpriv", "cpriv", "wombat lol"),
];

pub struct World {
    pub srv: TestServer,
    pub alice: TestUser,
    pub bob: TestUser,
    pub carol: TestUser,
    /// Item id -> label, and back.
    pub labels: HashMap<String, String>,
    pub ids: HashMap<String, String>,
    pub sets: HashMap<String, Set>,
    pub cache: Cache,
    pub client: ApiClient,
    pub offline: Arc<OfflineIndex>,
    /// As the desktop backend builds it.
    pub vocab: Vocabulary,
    /// The cached items, by id.
    pub items: Vec<ItemSummary>,
    pub within: Within,
    _dir: tempfile::TempDir,
}

pub async fn world() -> World {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let carol = srv.user("carol").await;
    let mut sets = HashMap::new();
    for (key, owner, vis, member) in SETS {
        let u = match owner {
            "alice" => &alice,
            "bob" => &bob,
            _ => &carol,
        };
        let set: Set = srv
            .post(u, routes::SETS, json!({"slug": key, "name": key, "visibility": vis}))
            .await
            .expect(201)
            .json();
        if member {
            srv.put(
                u,
                &routes::path(routes::SET_MEMBER, &[&set.id, "alice"]),
                json!({ "permissions": Permissions::CONTRIBUTOR_DEFAULT }),
            )
            .await
            .expect(200);
        }
        sets.insert(key.to_string(), set);
    }
    let mut labels = HashMap::new();
    let mut ids = HashMap::new();
    for (n, (label, set, text)) in ITEMS.iter().enumerate() {
        let owner = SETS.iter().find(|s| s.0 == *set).unwrap().1;
        let u = match owner {
            "alice" => &alice,
            "bob" => &bob,
            _ => &carol,
        };
        let res: Vec<UploadResult> = srv
            .upload(
                u,
                &sets[*set].id,
                vec![(&format!("{label}.png"), png(7000 + n as u64))],
                Some(json!([{"name": format!("item {n}"), "text": text}])),
            )
            .await
            .expect(200)
            .json();
        let item = res.into_iter().next().unwrap().item.expect("item");
        labels.insert(item.id.clone(), label.to_string());
        ids.insert(label.to_string(), item.id.clone());
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
    let (offline, vocab, items, within) = client_side(&cache);
    World {
        srv,
        alice,
        bob,
        carol,
        labels,
        ids,
        sets,
        cache,
        client,
        offline,
        vocab,
        items,
        within,
        _dir: dir,
    }
}

/// What the desktop backend builds from the cache.
pub fn client_side(cache: &Cache) -> (Arc<OfflineIndex>, Vocabulary, Vec<ItemSummary>, Within) {
    let offline = Arc::new(cache.offline_index().unwrap());
    let me = cache.me().unwrap().map(|u| u.username);
    let vocab = Vocabulary::for_viewer(offline.items(), me.as_deref()).with_texts(offline.clone());
    let mut items: Vec<ItemSummary> = offline.items().cloned().collect();
    items.sort_by(|a, b| a.id.cmp(&b.id));
    let within = Within::new(&items, None, me.as_deref());
    (offline, vocab, items, within)
}

pub async fn server(w: &World, user: &TestUser, q: &str) -> Result<Vec<String>, u16> {
    let resp = read(
        w.srv
            .client
            .get(w.srv.url(routes::SEARCH))
            .query(&[("q", q), ("limit", "200")])
            .bearer_auth(&user.token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    if resp.status.as_u16() != 200 {
        return Err(resp.status.as_u16());
    }
    let r: SearchResults = resp.json();
    Ok(r.hits.iter().map(|h| w.labels[&h.item.id].clone()).collect())
}

pub fn offline(w: &World, q: &str) -> Vec<String> {
    w.offline
        .search(q, None, 200)
        .iter()
        .map(|h| w.labels[&h.item.id].clone())
        .collect()
}

pub fn within(w: &World, q: &str) -> Vec<String> {
    w.within
        .search(q, Some("alice"), &HashSet::new())
        .iter()
        .map(|f| w.labels[&w.items[f.index].id].clone())
        .collect()
}

/// `Hit::text_tier` of each hit of the offline index's docs.
pub fn tiers(w: &World, q: &str) -> HashMap<String, u8> {
    let mut index = Index::new();
    for it in &w.items {
        index.upsert(cache::doc_for(it, Some("alice")));
    }
    let viewer = Viewer {
        user_id: "alice".into(),
        ..Viewer::default()
    };
    let opts = SearchOptions {
        limit: 500,
        ..Default::default()
    };
    index
        .search(&Query::parse(q), &viewer, &opts)
        .hits
        .into_iter()
        .map(|h| (w.labels[&h.item_id].clone(), h.text_tier))
        .collect()
}

pub async fn server_suggest(w: &World, user: &TestUser, q: &str) -> Option<SearchSuggestions> {
    let mut params = form_urlencoded::Serializer::new(String::new());
    params.append_pair("q", q);
    let url = format!("{}?{}", routes::SEARCH_SUGGEST, params.finish());
    w.srv.get(user, &url).await.expect(200).json()
}

pub fn model_line(vocab: &Vocabulary, q: &str) -> Option<SuggestionLine> {
    let mut m = Model::new(ModelConfig::default());
    m.handle(Event::Open(OpenInfo::default()));
    m.handle(Event::VocabularyLoaded(vocab.clone()));
    m.handle(Event::QueryChanged(q.into()));
    m.suggestions()
}

fn set(v: &[String]) -> BTreeSet<String> {
    v.iter().cloned().collect()
}

fn nondecreasing(hits: &[String], t: &HashMap<String, u8>) -> bool {
    let ts: Vec<u8> = hits.iter().map(|h| t.get(h).copied().unwrap_or(255)).collect();
    ts.windows(2).all(|p| p[0] <= p[1])
}

const QUERIES: &[&str] = &[
    "x:l",
    "x:lo",
    "x:lol",
    "x:LOL",
    "x:lool",
    "x:lolz",
    "x:h",
    "x:hel",
    "x:helo",
    "x:hlelo",
    "x:thoguhts",
    "x:thuoghst",
    "x:though",
    "x:\"no thou\"",
    "x:\"n thoughts\"",
    "x:\"no thoughts head\"",
    "x:\"thoughts head e\"",
    "x:\"thou head\"",
    "x:\"fix the b\"",
    "x:\"fixed the bug\"",
    "x:\"the bug\"",
    "x:коли",
    "x:оли",
    "x:ко",
    "x:КО",
    "x:к",
    "x:кава",
    "x:кот",
    "x:п'ятн",
    "x:п\u{2019}ятн",
    "x:п\u{02BC}ятн",
    "x:п`ятн",
    "x:пятн",
    "x:п'",
    "x:п'ятниця",
    "x:п'ятница",
    "x:їжак",
    "x:іжак",
    "x:ґанок",
    "x:ганок",
    "x:єнот",
    "x:енот",
    "x:київ",
    "x:кийв",
    "x:40",
    "x:404",
    "x:2025",
    "x:1s",
    "x:1st",
    "x:naah",
    "x:nah",
    "x:dont",
    "x:don't",
    "x:cest",
    "x:cafe",
    "x:CAFÉ",
    "x:whenyou",
    "x:when",
    "x:hen",
    "x:you",
    "x:😂",
    "-x:lo",
    "-x:lool",
    "-x:lolz",
    "-x:оли",
    "-x:\"no thou\"",
    "-x:thoguhts",
    "x:lo -x:lol",
    "x:lo x:ко",
    "what x:lo",
    "x:lo sort:recent",
    "x:lo is:mine",
    "x:lo set:bobs",
    "x:lo by:bob",
    "x:lo -by:bob",
    "x:!!!",
    "x:a-b",
    "x:''",
    "-x:!!!",
    "x:zyz",
    "x:zyzzyva",
    "x:секрет",
    "x:quokka",
    "x:quo",
    "x:wombat",
    "x:womb",
    "x:hidden",
    "x:platypus",
    "x:echidna",
    "x:glo",
    "text:lo",
    "TEXT:Lo",
    "x:\"fixe the bug\"",
    "x:\"th head\"",
    "-x:\"thou head\"",
    "x:thoguh",
    "x:lvo",
    "x:lvoe",
    "x:helol",
    "x:hlelo there",
    "x:tehre",
    "x:thrre",
    "x:litreally",
    "x:litrealy",
    "x:ltireally",
    "-lol x:lo",
    "x:lo -what",
    "x:\"no thou\" sort:likes",
    "x:lol -x:lol",
    "x:п'ятниц",
    "x:пятнниця",
];

/// Every query through the server, the offline index and the in-pack
/// search over all cached items.
#[tokio::test]
async fn review_query_table() {
    let w = world().await;
    let all = w.items.len();
    let mut table = String::from(
        "| query | server | offline | within | sets agree | server=offline order | tiers ordered (server/within) |\n|---|---|---|---|---|---|---|\n",
    );
    let mut problems = Vec::new();
    let show = |v: &[String]| {
        if v.len() == all {
            "(all)".to_string()
        } else if v.is_empty() {
            "none".to_string()
        } else {
            v.join(" ")
        }
    };
    for q in QUERIES {
        let srv = server(&w, &w.alice, q).await;
        let off = offline(&w, q);
        let inn = within(&w, q);
        let t = tiers(&w, q);
        let (srv_t, agree, order, ord_s) = match &srv {
            Ok(s) => (
                show(s),
                set(s) == set(&off) && set(s) == set(&inn),
                *s == off,
                nondecreasing(s, &t),
            ),
            Err(c) => (format!("HTTP {c}"), false, false, false),
        };
        let ord_w = nondecreasing(&inn, &t);
        if !agree {
            problems.push(format!(
                "{q:?}: server {srv_t} / offline {} / within {}",
                show(&off),
                show(&inn)
            ));
        }
        writeln!(
            table,
            "| `{q}` | {srv_t} | {} | {} | {} | {} | {}/{} |",
            show(&off),
            show(&inn),
            if agree { "yes" } else { "NO" },
            if order { "yes" } else { "no" },
            ord_s,
            ord_w,
        )
        .unwrap();
    }
    println!("{table}");
    std::fs::write("/data/stickpile/.claude/scratch/review-ts/query_table.md", &table).ok();
    println!("{}", problems.join("\n"));
}

const PREFIXES: &[&str] = &[
    "x:",
    "x:l",
    "x:lo",
    "x:lol",
    "x:LOL",
    "x:lool",
    "x:ко",
    "x:п'ят",
    "x:п\u{2019}ятниця",
    "-x:",
    "text:",
    "text:LO",
    "cat -x:lo",
    "x:оли",
    "x:lm",
    "x:glo",
    "x:2",
    "x:40",
    "x:1",
    "x:п",
    "x:zy",
    "x:qu",
    "x:wo",
    "x:pla",
    "x:ech",
    "x:hlelo",
    "x:thoguhts",
    "x:ґ",
    "x:г",
    "x:кава",
    "x:a-b",
    "x:!!!",
    "x:😂",
    "x:\"no",
    "x:lo ",
];

/// The offers of the server endpoint, the popup's `suggest` (the desktop
/// backend's vocabulary) and the popup model's line.
#[tokio::test]
async fn review_offers_table() {
    let w = world().await;
    let mut table = String::from("| typed | server | popup suggest | popup line | agree |\n|---|---|---|---|---|\n");
    let show = |words: &[String], chosen: Option<usize>| -> String {
        if words.is_empty() {
            return "none".into();
        }
        words
            .iter()
            .enumerate()
            .map(|(i, w)| if chosen == Some(i) { format!("*{w}") } else { w.clone() })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut problems = Vec::new();
    for p in PREFIXES {
        let srv = server_suggest(&w, &w.alice, p).await;
        let pop = suggest::suggest(p, &w.vocab);
        let line = model_line(&w.vocab, p);
        let s = srv
            .as_ref()
            .map_or("none".into(), |s| show(&s.words, s.chosen.map(|c| c as usize)));
        let a = pop.as_ref().map_or("none".into(), |s| show(&s.words, s.chosen()));
        let b = line.as_ref().map_or("none".into(), |s| show(&s.words, s.chosen));
        let agree = s == a && a == b;
        if !agree {
            problems.push(format!("{p:?}: server {s} / popup {a} / line {b}"));
        }
        writeln!(
            table,
            "| `{p}` | {s} | {a} | {b} | {} |",
            if agree { "yes" } else { "NO" }
        )
        .unwrap();
    }
    println!("{table}");
    std::fs::write("/data/stickpile/.claude/scratch/review-ts/offers_table.md", &table).ok();
    println!("{}", problems.join("\n"));
}

async fn leak_row(w: &World, stage: &str, typed: &str, full: &str, table: &mut String) -> (bool, bool, bool, bool) {
    let s = server_suggest(w, &w.alice, typed)
        .await
        .map(|s| s.words)
        .unwrap_or_default();
    let c = suggest::suggest(typed, &w.vocab).map(|s| s.words).unwrap_or_default();
    let sh = server(w, &w.alice, full).await.unwrap();
    let oh = offline(w, full);
    writeln!(table, "| {stage} | `{typed}` | {s:?} | {c:?} | {sh:?} | {oh:?} |").unwrap();
    (!s.is_empty(), !c.is_empty(), !sh.is_empty(), !oh.is_empty())
}

/// The words a viewer is offered and finds by `x:` after the items or the
/// viewer's access change: on the server at once, in the client's cache
/// after its next (incremental) sync.
#[tokio::test]
async fn review_leaks_after_changes() {
    let mut w = world().await;
    let bpub2 = w.srv.create_set(&w.bob, "bpub2", Visibility::Public).await;
    let mut new_ids = HashMap::new();
    for (n, label) in ["narwhal", "axolotl", "ocelot"].into_iter().enumerate() {
        let res: Vec<UploadResult> = w
            .srv
            .upload(
                &w.bob,
                &bpub2.id,
                vec![(&format!("{label}.png"), png(9000 + n as u64))],
                Some(json!([{"name": format!("new {n}"), "text": label}])),
            )
            .await
            .expect(200)
            .json();
        let id = res.into_iter().next().unwrap().item.unwrap().id;
        w.labels.insert(id.clone(), label.to_string());
        new_ids.insert(label, id);
    }
    cache::sync(&w.cache, &w.client, 500).await.unwrap();
    (w.offline, w.vocab, w.items, w.within) = client_side(&w.cache);
    let probes = [
        ("x:narw", "x:narwhal"),
        ("x:axo", "x:axolotl"),
        ("x:ocel", "x:ocelot"),
        ("x:platy", "x:platypus"),
        ("x:echid", "x:echidna"),
        ("x:qua", "x:quack"),
    ];
    let mut table = String::from(
        "| stage | typed | server offers | client offers | server x: hits | offline x: hits |\n|---|---|---|---|---|---|\n",
    );
    for (typed, full) in probes {
        let r = leak_row(&w, "before", typed, full, &mut table).await;
        assert_eq!(r, (true, true, true, true), "{typed} before the changes");
    }
    // The changes: an item moved to a private set, one deleted, one's text
    // edited; alice removed from carol's private and unlisted teams; bob's
    // public set made private.
    w.srv
        .patch(
            &w.bob,
            &routes::path(routes::ITEM, &[&new_ids["narwhal"]]),
            json!({"set_id": w.sets["bsecret"].id}),
        )
        .await
        .expect(200);
    w.srv
        .delete(&w.bob, &routes::path(routes::ITEM, &[&new_ids["axolotl"]]))
        .await
        .expect(204);
    w.srv
        .patch(
            &w.bob,
            &routes::path(routes::ITEM, &[&new_ids["ocelot"]]),
            json!({"text": "other words"}),
        )
        .await
        .expect(200);
    for key in ["cteam", "cunlteam"] {
        w.srv
            .delete(&w.carol, &routes::path(routes::SET_MEMBER, &[&w.sets[key].id, "alice"]))
            .await
            .expect(204);
    }
    w.srv
        .patch(
            &w.bob,
            &routes::path(routes::SET, &[&w.sets["bobs"].id]),
            json!({"visibility": "private"}),
        )
        .await
        .expect(200);
    let mut leaks = Vec::new();
    for (typed, full) in probes {
        let (so, _, sh, _) = leak_row(&w, "after, cache not synced", typed, full, &mut table).await;
        if so || sh {
            leaks.push(format!("server still offers/finds {typed}/{full}"));
        }
    }
    let report = cache::sync(&w.cache, &w.client, 500).await.unwrap();
    (w.offline, w.vocab, w.items, w.within) = client_side(&w.cache);
    for (typed, full) in probes {
        let (so, co, sh, oh) = leak_row(&w, "after sync", typed, full, &mut table).await;
        if so || co || sh || oh {
            leaks.push(format!(
                "after sync: {typed}/{full}: server offers {so} client offers {co} server finds {sh} offline finds {oh}"
            ));
        }
    }
    println!("sync after the changes: {report:?}\n{table}");
    std::fs::write("/data/stickpile/.claude/scratch/review-ts/leaks_table.md", &table).ok();
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

/// This branch's client against a running server (a 0.8.1 one, by hand):
/// `STICKPILE_REVIEW_URL`, `STICKPILE_REVIEW_TOKEN` (a token file). The
/// cache synced from it, the popup's offers from that cache, what the
/// popup sends (`resolve`) and what the server and the cache find.
#[tokio::test]
#[ignore]
async fn review_compat_this_client_against_a_server() {
    let url = std::env::var("STICKPILE_REVIEW_URL").unwrap();
    let token = std::fs::read_to_string(std::env::var("STICKPILE_REVIEW_TOKEN").unwrap())
        .unwrap()
        .trim()
        .to_string();
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path(), 64).unwrap();
    let client = ApiClient::new(&url, Duration::from_secs(10))
        .unwrap()
        .with_token(Some(token));
    let me = client.me().await.unwrap();
    cache.bind_server(&url).unwrap();
    cache.set_me(&me).unwrap();
    let report = cache::sync(&cache, &client, 500).await.unwrap();
    println!("sync: {report:?}");
    let (offline, vocab, _, _) = client_side(&cache);
    for typed in ["x:", "x:l", "x:lo", "x:ко", "x:п'ят", "-x:", "x:zy"] {
        let s = suggest::suggest(typed, &vocab);
        println!(
            "popup offers {typed:<8} {:?} chosen {:?}",
            s.as_ref().map(|s| s.words.clone()),
            s.as_ref().and_then(|s| s.chosen())
        );
    }
    for q in ["x:lo", "x:lol", "cat x:", "x:\"no thou\"", "-x:lo", "x:п'ятн"] {
        let sent = suggest::resolve(q, &vocab).0;
        let params = stickpile_core::client::SearchParams {
            q: sent.clone(),
            limit: Some(200),
            offset: None,
            target: None,
        };
        let srv = client.search(&params).await.map(|r| {
            let mut t: Vec<String> = r.hits.into_iter().map(|h| h.item.text).collect();
            t.sort();
            t
        });
        let mut off: Vec<String> = offline.search(q, None, 200).into_iter().map(|h| h.item.text).collect();
        off.sort();
        println!("{q:<14} sent {sent:<16} server {srv:?}\n{:<14} offline {off:?}", "");
    }
}
