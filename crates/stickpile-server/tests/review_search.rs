//! Review of the search round (emoji names in `e:`, the item text and
//! `text:`/`x:`, the suggestions): the server, the clients' offline index
//! and each client's suggestions must agree on one list of queries over
//! one library (`review_search_fixture.json`).
//!
//! The server is the real one (`GET /api/v1/search`, `GET
//! /api/v1/search/suggest`); the offline side is the clients' own path: a
//! `stickpile_core` cache synced from the server with `cache::sync`, its
//! `OfflineIndex`, and the vocabulary the desktop backend builds from it
//! (`Vocabulary::for_viewer(index.items(), me)`, `me` the cache's
//! account). The popup's suggestion line is the model's
//! (`Model::suggestions`). Tables go to stdout (run with `--nocapture`).

mod common;

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::time::Duration;

use common::{gif_anim, png, read, TestServer, TestUser};
use serde::Deserialize;
use serde_json::{json, Value};
use stickpile_api::{routes, SearchResults, SearchSuggestions, Set, UploadResult, Visibility};
use stickpile_core::cache::{self, Cache, OfflineIndex};
use stickpile_core::ApiClient;
use stickpile_quick::model::SuggestionLine;
use stickpile_quick::suggest::{self, Vocabulary};
use stickpile_quick::{Event, Model, ModelConfig, OpenInfo};

#[derive(Deserialize)]
struct Fixture {
    sets: Vec<SetSpec>,
    items: Vec<ItemSpec>,
}

#[derive(Deserialize)]
struct SetSpec {
    key: String,
    owner: String,
    slug: String,
    name: String,
    visibility: Visibility,
}

#[derive(Deserialize)]
struct ItemSpec {
    label: String,
    set: String,
    #[serde(default)]
    gif: bool,
    meta: Value,
}

struct World {
    srv: TestServer,
    alice: TestUser,
    /// Item id -> fixture label.
    labels: HashMap<String, String>,
    /// Labels alice can search (all but bob's private set), in upload order.
    visible: Vec<String>,
    offline: OfflineIndex,
    /// As the desktop backend builds it from the offline index.
    vocab: Vocabulary,
    _cache_dir: tempfile::TempDir,
}

async fn world() -> World {
    let fx: Fixture = serde_json::from_str(include_str!("review_search_fixture.json")).unwrap();
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let bob = srv.user("bob").await;
    let owner = |name: &str| if name == "alice" { &alice } else { &bob };
    let mut set_ids = HashMap::new();
    for s in &fx.sets {
        let set: Set = srv
            .post(
                owner(&s.owner),
                routes::SETS,
                json!({"slug": s.slug, "name": s.name, "visibility": s.visibility}),
            )
            .await
            .expect(201)
            .json();
        set_ids.insert(s.key.clone(), (set.id, s.owner.clone(), s.visibility));
    }
    let mut labels = HashMap::new();
    let mut visible = Vec::new();
    for (n, it) in fx.items.iter().enumerate() {
        let (set_id, set_owner, vis) = &set_ids[&it.set];
        let seed = 1000 + n as u64;
        let (file, bytes) = match it.gif {
            true => (format!("{}.gif", it.label), gif_anim(seed)),
            false => (format!("{}.png", it.label), png(seed)),
        };
        let res: Vec<UploadResult> = srv
            .upload(owner(set_owner), set_id, vec![(&file, bytes)], Some(json!([it.meta])))
            .await
            .expect(200)
            .json();
        let item = res.into_iter().next().unwrap().item.expect("item");
        labels.insert(item.id.clone(), it.label.clone());
        if set_owner == "alice" || *vis == Visibility::Public {
            visible.push(it.label.clone());
        }
    }

    // The clients' path: a cache synced from the server, its index.
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
    assert_eq!(offline.len(), visible.len(), "the cache holds what alice can search");
    // As the desktop backend builds it: `e:` offers the viewer's emojis.
    let me = cache.me().unwrap().map(|u| u.username);
    let vocab = Vocabulary::for_viewer(offline.items(), me.as_deref());
    World {
        srv,
        alice,
        labels,
        visible,
        offline,
        vocab,
        _cache_dir: dir,
    }
}

/// Server hits as labels, in order, and the labels reached only by a vibe
/// expansion (`matched == ["semantic"]`); `Err(status)` when not 200.
async fn server(w: &World, q: &str) -> Result<(Vec<String>, Vec<String>), u16> {
    let resp = read(
        w.srv
            .client
            .get(w.srv.url(routes::SEARCH))
            .query(&[("q", q), ("limit", "200")])
            .bearer_auth(&w.alice.token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    if resp.status.as_u16() != 200 {
        return Err(resp.status.as_u16());
    }
    let r: SearchResults = resp.json();
    let mut lexical = Vec::new();
    let mut expansion_only = Vec::new();
    for h in &r.hits {
        let l = w.labels[&h.item.id].clone();
        if h.matched == ["semantic"] {
            expansion_only.push(l);
        } else {
            lexical.push(l);
        }
    }
    Ok((lexical, expansion_only))
}

fn offline(w: &World, q: &str) -> Vec<String> {
    w.offline
        .search(q, None, 200)
        .iter()
        .map(|h| w.labels[&h.item.id].clone())
        .collect()
}

async fn server_suggest(w: &World, q: &str) -> Option<SearchSuggestions> {
    let mut params = form_urlencoded::Serializer::new(String::new());
    params.append_pair("q", q);
    let url = format!("{}?{}", routes::SEARCH_SUGGEST, params.finish());
    w.srv.get(&w.alice, &url).await.expect(200).json()
}

/// The popup's suggestion line for `q` typed on the Search tab.
fn model_line(vocab: &Vocabulary, q: &str) -> Option<SuggestionLine> {
    let mut m = Model::new(ModelConfig::default());
    m.handle(Event::Open(OpenInfo::default()));
    m.handle(Event::VocabularyLoaded(vocab.clone()));
    m.handle(Event::QueryChanged(q.into()));
    m.suggestions()
}

fn set(v: &[String]) -> BTreeSet<String> {
    v.iter().cloned().collect()
}

fn labels(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// The queries, and the hits the docs promise where they say exactly.
fn queries() -> Vec<(String, Option<BTreeSet<String>>)> {
    let skulls = ["skull", "bones", "bones_plain", "xray", "cap", "pub_skull", "gif"];
    let long_value = format!("text:{}", "a".repeat(900));
    let too_long = format!("text:{}", "a".repeat(1500));
    let q: Vec<(&str, Option<Vec<&str>>)> = vec![
        // emoji names
        ("e:skull", Some(skulls.to_vec())),
        ("e:SKULL", Some(skulls.to_vec())),
        ("emoji:Skull", Some(skulls.to_vec())),
        ("e:sku", Some(skulls.to_vec())),
        (
            "e:sklul",
            Some(vec!["skull", "bones", "bones_plain", "xray", "pub_skull", "gif"]),
        ),
        ("e:thumbs", Some(vec!["thumb_tone", "thumb", "thumbdown"])),
        ("e:thumbs_up", Some(vec!["thumb_tone", "thumb"])),
        ("e:thumbs-up", Some(vec!["thumb_tone", "thumb"])),
        ("e:Thumbs_Up", Some(vec!["thumb_tone", "thumb"])),
        ("e:thumbs_u", Some(vec!["thumb_tone", "thumb"])),
        ("e:thumbz", None),
        ("e:thunbs_up", Some(vec!["thumb_tone", "thumb"])),
        ("e:ok", None),
        ("e:hat", None),
        ("e:skl", Some(vec![])),
        ("e:xqzv", Some(vec![])),
        ("e:череп", Some(vec![])),
        ("e:1st", Some(vec![])),
        ("e:skull e:😂", Some([&skulls[..], &["joy"]].concat())),
        ("e:skull -e:☠", Some(vec!["skull", "xray", "cap", "pub_skull", "gif"])),
        ("e:👍", Some(vec!["thumb_tone", "thumb"])),
        ("e:👍🏽", Some(vec!["thumb_tone"])),
        ("e:👍🏿", Some(vec![])),
        ("e:❤", Some(vec!["heart_vs", "heart"])),
        ("e:❤️", Some(vec!["heart_vs", "heart"])),
        ("e:fire", None),
        ("e:heart_on_fire", Some(vec!["heartfire"])),
        ("e:dead", None),
        ("e:lol", None),
        ("e:", None),
        ("cat e:cat", None),
        // text:
        (
            "text:thoughts",
            Some(vec!["text_thoughts", "reversed", "long_text", "pub_skull"]),
        ),
        ("TEXT:thoughts", None),
        ("text:thought", Some(vec![])),
        ("text:\"no thoughts\"", Some(vec!["text_thoughts", "pub_skull"])),
        ("x:\"no thoughts\"", Some(vec!["text_thoughts", "pub_skull"])),
        ("text:\"thoughts head\"", Some(vec!["text_thoughts"])),
        ("text:\"no empty\"", Some(vec![])),
        ("x:\"head empty\"", Some(vec!["text_thoughts"])),
        ("text:no_thoughts", None),
        ("text:\"no thoughts", Some(vec!["text_thoughts", "pub_skull"])),
        ("x:кіт", Some(vec!["ukr", "mixed"])),
        ("text:Кіт", Some(vec!["ukr", "mixed"])),
        ("text:КІТ", Some(vec!["ukr", "mixed"])),
        ("text:\"кіт у мішку\"", Some(vec!["ukr"])),
        ("text:п'ятниця", Some(vec!["apos_ascii", "apos_2019", "apos_02bc"])),
        ("text:п’ятниця", Some(vec!["apos_ascii", "apos_2019", "apos_02bc"])),
        ("text:пʼятниця", Some(vec!["apos_ascii", "apos_2019", "apos_02bc"])),
        ("text:\"cat кіт\"", Some(vec!["mixed"])),
        ("text:\"meow мяу\"", Some(vec!["mixed"])),
        ("text:😂", Some(vec!["mixed", "emoji_text"])),
        ("text:!!!", Some(vec![])),
        ("text:", None),
        ("x:", None),
        // plain words against the text
        ("thoughts", None),
        ("кіт", None),
        ("пятниця", None),
        ("\"no thoughts\"", None),
        ("quack", Some(vec!["duck"])),
        ("danger", Some(vec!["bones_plain"])),
        ("😂", Some(vec!["joy", "gif", "mixed", "emoji_text"])),
        ("sleepy", None),
        ("tired", None),
        ("e:skull -e:skull", Some(vec![])),
        ("e:+1", Some(vec![])),
        ("e:ok_hand", Some(vec![])),
        // negations
        ("-e:skull", None),
        ("-e:xqzv", None),
        ("-text:thoughts", None),
        ("-x:\"no thoughts\"", None),
        ("-text:!!!", None),
        ("thoughts -text:thoughts", Some(vec!["name_thoughts", "kw_thoughts"])),
        ("-thoughts", None),
        // combinations
        ("v:happy e:thumbs", Some(vec!["thumb_tone"])),
        ("t:reaction -e:skull", Some(vec!["joy"])),
        (
            "set:mine text:thoughts",
            Some(vec!["text_thoughts", "reversed", "long_text"]),
        ),
        ("type:gif e:skull", Some(vec!["gif"])),
        ("type:gif -e:skull", Some(vec![])),
        ("by:bob x:кря", Some(vec!["duck"])),
        ("-t:nsfw e:😂", Some(vec!["joy"])),
        ("is:mine e:skull", None),
        // Offline: see `offline_differs`.
        ("is:public text:thoughts", Some(vec!["pub_skull"])),
        ("is:public e:skull", Some(vec!["pub_skull", "gif"])),
        ("e:skull text:\"no thoughts\"", Some(vec!["pub_skull"])),
        ("sort:recent e:skull", Some(skulls.to_vec())),
        ("x:кіт -tag:nsfw", Some(vec!["ukr", "mixed"])),
        ("likes>=0 e:thumbs", Some(vec!["thumb_tone", "thumb", "thumbdown"])),
    ];
    let mut out: Vec<(String, Option<BTreeSet<String>>)> = q
        .into_iter()
        .map(|(q, want)| (q.to_string(), want.map(|w| labels(&w))))
        .collect();
    out.push((long_value, Some(BTreeSet::new())));
    out.push((too_long, Some(BTreeSet::new())));
    out
}

/// The queries the offline index answers otherwise by design, and what it
/// finds instead (docs/search.md, "Offline behaviour"): an `ItemSummary`
/// carries no set visibility, so the cache counts the viewer's own items
/// as private and `is:public` finds only other people's public items
/// offline (bob's `gif`), not alice's in her public set (`pub_skull`).
fn offline_differs(q: &str) -> Option<BTreeSet<String>> {
    match q {
        "is:public text:thoughts" => Some(labels(&[])),
        "is:public e:skull" => Some(labels(&["gif"])),
        _ => None,
    }
}

fn short(q: &str) -> String {
    if q.chars().count() > 40 {
        format!("{}... ({} bytes)", q.chars().take(12).collect::<String>(), q.len())
    } else {
        q.to_string()
    }
}

/// Every query of the list through the server and the offline index:
/// the hit sets must agree (a vibe expansion of the server is documented
/// and reported apart), and match what the docs promise where they say.
#[tokio::test]
async fn server_and_offline_agree_on_the_query_list() {
    let w = world().await;
    let all: BTreeSet<String> = w.visible.iter().cloned().collect();
    let mut table = String::new();
    let mut problems = Vec::new();
    writeln!(
        table,
        "| query | server hits | offline hits | agree | order agrees | docs |"
    )
    .unwrap();
    writeln!(table, "|---|---|---|---|---|---|").unwrap();
    for (q, want) in queries() {
        let off = offline(&w, &q);
        let (srv, srv_text, agree, order) = match server(&w, &q).await {
            Ok((lex, exp)) => {
                let agree = set(&lex) == set(&off);
                let order = lex == off;
                let mut s = lex.join(" ");
                if !exp.is_empty() {
                    s.push_str(&format!(" [+expansion: {}]", exp.join(" ")));
                }
                (Some(lex), s, agree, order)
            }
            Err(code) => (None, format!("HTTP {code}"), false, false),
        };
        let differs = offline_differs(&q);
        let mut docs = String::from("-");
        if let Some(want) = &want {
            let want_off = differs.as_ref().unwrap_or(want);
            let ok_off = set(&off) == *want_off;
            let ok_srv = srv.as_ref().is_none_or(|s| set(s) == *want);
            docs = if ok_off && ok_srv { "ok".into() } else { "WRONG".into() };
            if !ok_off {
                problems.push(format!("{q:?}: offline {:?}, docs want {want_off:?}", set(&off)));
            }
            if !ok_srv {
                problems.push(format!(
                    "{q:?}: server {:?}, docs want {want:?}",
                    set(srv.as_ref().unwrap())
                ));
            }
        }
        // docs/api.md: `q` at most 1000 bytes (400); the offline index has
        // no such limit. docs/search.md: offline `is:public`. Documented,
        // not counted.
        let documented = match () {
            _ if srv.is_none() && q.len() > 1000 => Some("q over 1000 bytes"),
            _ if differs.is_some() => Some("offline is:public"),
            _ => None,
        };
        if !agree && documented.is_none() {
            problems.push(format!("{q:?}: server {srv_text} / offline {}", off.join(" ")));
        }
        let off_text = if off.len() == all.len() {
            "(all visible)".to_string()
        } else {
            off.join(" ")
        };
        let srv_text = if srv.as_ref().is_some_and(|s| s.len() == all.len()) {
            "(all visible)".to_string()
        } else {
            srv_text
        };
        writeln!(
            table,
            "| `{}` | {} | {} | {} | {} | {} |",
            short(&q),
            srv_text,
            off_text,
            match (agree, documented) {
                (true, _) => "yes".to_string(),
                (false, Some(why)) => format!("no (documented: {why})"),
                (false, None) => "NO".to_string(),
            },
            if order { "yes" } else { "no" },
            docs
        )
        .unwrap();
    }
    println!("{table}");
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// Plain words that hit the text rank below the name and the keywords,
/// on the server and offline alike.
#[tokio::test]
async fn text_ranks_below_name_and_keywords_on_both_sides() {
    let w = world().await;
    let (srv, _) = server(&w, "thoughts").await.unwrap();
    let off = offline(&w, "thoughts");
    for (side, hits) in [("server", &srv), ("offline", &off)] {
        assert_eq!(hits[..2], ["name_thoughts", "kw_thoughts"], "{side}: {hits:?}");
        assert!(!hits.contains(&"secret".to_string()), "{side}");
    }
}

/// The typed prefixes: what the popup's `suggest`, its model's
/// suggestion line and the server endpoint offer, from the same items.
#[tokio::test]
async fn every_surface_offers_the_same_values() {
    let w = world().await;
    let prefixes = [
        "e:",
        "cat e:",
        "e:sk",
        "e:skul",
        "e:sklul",
        "e:SKULL",
        "-e:sk",
        "emoji:thumbs",
        "e:thumbs_u",
        "e:xqzv",
        "e:👍",
        "e:❤",
        "e:1st",
        "v:",
        "v:sa",
        "vibe:sarcstic",
        "-v:ha",
        "t:",
        "t:re",
        "s:",
        "s:мо",
        "b:",
        "type:",
        "is:",
        "sort:",
        "x:",
        "text:",
        "x:кі",
        "set:\"my",
        "e:sk ",
    ];
    let mut table = String::new();
    let mut problems = Vec::new();
    writeln!(
        table,
        "| typed | popup (suggest) | popup model line | server endpoint | agree |"
    )
    .unwrap();
    writeln!(table, "|---|---|---|---|---|").unwrap();
    let show = |words: &[String], labels: &[String], chosen: Option<usize>| -> String {
        if words.is_empty() {
            return "none".into();
        }
        words
            .iter()
            .zip(labels.iter().chain(std::iter::repeat(&String::new())))
            .enumerate()
            .map(|(i, (w, l))| {
                let mark = if chosen == Some(i) { "*" } else { "" };
                if l.is_empty() {
                    format!("{mark}{w}")
                } else {
                    format!("{mark}{w}({l})")
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    for p in prefixes {
        let popup = suggest::suggest(p, &w.vocab);
        let line = model_line(&w.vocab, p);
        let srv = server_suggest(&w, p).await;
        let popup_t = popup
            .as_ref()
            .map_or("none".into(), |s| show(&s.words, &s.labels, s.chosen()));
        let line_t = line
            .as_ref()
            .map_or("none".into(), |s| show(&s.words, &s.labels, s.chosen));
        let srv_t = srv.as_ref().map_or("none".into(), |s| {
            show(&s.words, &s.labels, s.chosen.map(|c| c as usize))
        });
        let agree = popup_t == line_t && popup_t == srv_t;
        if !agree {
            problems.push(format!(
                "{p:?}:\n  popup  {popup_t}\n  model  {line_t}\n  server {srv_t}"
            ));
        }
        if let (Some(a), Some(b)) = (&popup, &srv) {
            assert_eq!(a.start as u64, b.start, "{p:?}");
            assert_eq!(a.key, b.key, "{p:?}");
            assert_eq!(a.typed, b.typed, "{p:?}");
            assert_eq!(a.words.len(), a.labels.len(), "{p:?}: labels parallel");
        }
        writeln!(
            table,
            "| `{p}` | {popup_t} | {line_t} | {srv_t} | {} |",
            if agree { "yes" } else { "NO" }
        )
        .unwrap();
    }
    println!("{table}");
    assert!(
        problems.is_empty(),
        "{} disagreements:\n{}",
        problems.len(),
        problems.join("\n")
    );
    // `e:` alone offers the emojis of alice's own items (her public set's
    // too), not those of bob's public ones (🦆 is only his); `e:duck` still
    // offers 🦆, from the table.
    let own = server_suggest(&w, "e:").await.unwrap().words;
    assert!(
        own.contains(&"💀".to_string()) && !own.contains(&"🦆".to_string()),
        "{own:?}"
    );
    assert_eq!(server_suggest(&w, "e:duck").await.unwrap().words[0], "🦆");
}

/// Taking an `e:` offer puts the emoji in (`e:<glyph>`); the query then
/// finds the items with that emoji, on the server and offline alike.
#[tokio::test]
async fn a_chosen_emoji_offer_finds_its_items() {
    let w = world().await;
    let mut problems = Vec::new();
    for typed in ["e:", "cat e:", "e:sk", "e:thumbs", "e:❤", "-e:sk"] {
        let Some(s) = server_suggest(&w, typed).await else {
            problems.push(format!("{typed:?}: no offers"));
            continue;
        };
        for word in &s.words {
            let q = format!("{}{word}", &typed[..s.start as usize]);
            assert!(q.ends_with(&format!("e:{word}")), "{q}");
            // `resolve` leaves an e: value alone.
            assert_eq!(suggest::resolve(&q, &w.vocab).0, q);
            let (srv, _) = server(&w, &q).await.unwrap();
            let off = offline(&w, &q);
            if set(&srv) != set(&off) {
                problems.push(format!("{q:?}: server {srv:?} offline {off:?}"));
            }
        }
    }
    // resolve: x: goes out as text:, e: names stay.
    for (typed, sent) in [
        ("x:кіт", "text:кіт"),
        ("-x:\"no thoughts\"", "-text:\"no thoughts\""),
        ("X:кіт", "text:кіт"),
        ("e:sklul -e:Skull", "e:sklul -e:Skull"),
        ("e:skull e:😂", "e:skull e:😂"),
    ] {
        assert_eq!(suggest::resolve(typed, &w.vocab).0, sent, "{typed}");
        let (a, _) = server(&w, typed).await.unwrap();
        let (b, _) = server(&w, sent).await.unwrap();
        assert_eq!(set(&a), set(&b), "{typed} and {sent} find the same");
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// An empty `x:` (being typed) is no filter: like `v:` and `e:` it should
/// be left out of the search, not go out as `text:`, which the parser
/// reads as the plain word "text".
#[tokio::test]
async fn an_empty_x_is_left_out_like_the_other_empty_filters() {
    let w = world().await;
    assert_eq!(suggest::resolve("cat v: e:", &w.vocab).0, "cat");
    // What the popup and Android send while `x:` is typed.
    let sent = suggest::resolve("cat x:", &w.vocab).0;
    let q = stickpile_api::query::Query::parse(&sent);
    assert_eq!(
        (sent.as_str(), q.text.clone()),
        ("cat", vec!["cat".to_string()]),
        "`cat x:` goes out as {sent:?}: the words {:?}",
        q.text
    );
}

/// Inside an opened pack the clients search its items themselves
/// (`stickpile_quick::within`): the filters and `text:` find what the
/// server finds with `set:` added. (Words may find more inside: the
/// pack's substring matches, documented.)
#[tokio::test]
async fn the_search_inside_a_pack_agrees_with_the_server() {
    let w = world().await;
    let set_id = w.offline.items().find(|i| i.set_slug == "mine").unwrap().set_id.clone();
    let mut items: Vec<stickpile_api::ItemSummary> =
        w.offline.items().filter(|i| i.set_id == set_id).cloned().collect();
    items.sort_by(|a, b| a.id.cmp(&b.id));
    let within = stickpile_quick::within::Within::new(&items, Some(Visibility::Private), Some("alice"));
    let mut problems = Vec::new();
    let mut table =
        String::from("| query (in pack mine) | server (with set:mine) | within | agree |\n|---|---|---|---|\n");
    for q in [
        "e:skull",
        "-e:skull",
        "e:sklul",
        "e:thumbs",
        "e:👍🏽",
        "e:skull e:😂",
        "e:xqzv",
        "text:\"no thoughts\"",
        "text:\"thoughts head\"",
        "x:кіт",
        "text:п’ятниця",
        "-text:thoughts",
        "text:😂",
        "thoughts",
        "кіт",
        "is:mine e:skull",
    ] {
        let (srv, _) = server(&w, &format!("{q} set:mine")).await.unwrap();
        let inside: Vec<String> = within
            .search(q, Some("alice"), &Default::default())
            .iter()
            .map(|f| w.labels[&items[f.index].id].clone())
            .collect();
        let agree = set(&srv) == set(&inside);
        if !agree {
            problems.push(format!("{q:?}: server {srv:?} within {inside:?}"));
        }
        writeln!(
            table,
            "| `{q}` | {} | {} | {} |",
            srv.join(" "),
            inside.join(" "),
            if agree { "yes" } else { "NO" }
        )
        .unwrap();
    }
    println!("{table}");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
