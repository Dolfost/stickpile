//! Review of the search round: the Android hints for the typed prefixes
//! of the review's query list, over the items of
//! `crates/stickpile-server/tests/review_search_fixture.json` that alice
//! can search, against the desktop's `suggest` over the same items. The
//! server endpoint's answers for these prefixes (upload order, its index
//! order) are in `crates/stickpile-server/tests/review_search.rs`; the
//! app builds its words from `OfflineIndex::items()`, a `HashMap`, so
//! the order the items come in is not the server's.

use stickpile_api::ItemSummary;
use stickpile_quick::suggest::{self, Vocabulary};

use super::tests::summary;
use super::{hints, Words};

/// Alice's searchable items of the fixture, in its (upload) order.
fn fixture_items() -> Vec<ItemSummary> {
    let fx: serde_json::Value = serde_json::from_str(include_str!(
        "../../../stickpile-server/tests/review_search_fixture.json"
    ))
    .unwrap();
    let sets = fx["sets"].as_array().unwrap();
    let strs = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for it in fx["items"].as_array().unwrap() {
        let set = sets.iter().find(|s| s["key"] == it["set"]).unwrap();
        let owner = set["owner"].as_str().unwrap();
        if owner != "alice" && set["visibility"] != "public" {
            continue;
        }
        let meta = &it["meta"];
        out.push(ItemSummary {
            id: it["label"].as_str().unwrap().into(),
            set_id: set["key"].as_str().unwrap().into(),
            set_slug: set["slug"].as_str().unwrap().into(),
            set_name: set["name"].as_str().unwrap().into(),
            owner_username: owner.into(),
            name: meta["name"].as_str().unwrap_or_default().into(),
            keywords: strs(&meta["keywords"]),
            emojis: strs(&meta["emojis"]),
            vibe: meta["vibe"].as_str().map(str::to_string),
            tags: strs(&meta["tags"]),
            text: meta["text"].as_str().unwrap_or_default().into(),
            ..summary()
        });
    }
    out
}

const PREFIXES: [&str; 17] = [
    "e:",
    "cat e:",
    "e:sk",
    "e:skul",
    "e:sklul",
    "-e:sk",
    "emoji:thumbs",
    "e:xqzv",
    "e:👍",
    "e:❤",
    "v:",
    "v:sa",
    "vibe:sarcstic",
    "t:",
    "x:",
    "text:",
    "x:кі",
];

/// The hints for a `key:` prefix are the desktop's offers: an emoji with
/// its name, the query with the emoji in, `chosen` where the desktop's is.
#[test]
fn hints_are_the_desktops_offers_for_the_review_prefixes() {
    let items = fixture_items();
    let words = Words::for_viewer(items.iter(), Some("alice"));
    let vocab = Vocabulary::for_viewer(&items, Some("alice"));
    let mut table = String::from("| typed | Android hints (fixture order) |\n|---|---|\n");
    for p in PREFIXES {
        let h = hints(p, &words);
        let want: Vec<(String, String, bool)> = match suggest::suggest(p, &vocab) {
            Some(s) => {
                let chosen = s.chosen();
                s.words
                    .iter()
                    .zip(&s.labels)
                    .enumerate()
                    .map(|(i, (w, l))| {
                        let label = if l.is_empty() { w.clone() } else { format!("{w} {l}") };
                        (label, format!("{}{w} ", &p[..s.start]), chosen == Some(i))
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        let got: Vec<(String, String, bool)> = h.iter().map(|h| (h.label.clone(), h.query.clone(), h.chosen)).collect();
        assert_eq!(got, want, "{p}");
        let shown: Vec<String> = h
            .iter()
            .map(|h| format!("{}{}", if h.chosen { "*" } else { "" }, h.label))
            .collect();
        table.push_str(&format!(
            "| `{p}` | {} |\n",
            if shown.is_empty() {
                "none".into()
            } else {
                shown.join(", ")
            }
        ));
    }
    println!("{table}");
}

/// The server's `e:` offers for alice over the fixture (its index order is
/// the upload order): `GET /api/v1/search/suggest?q=e:` in
/// `review_search.rs`, the emojis of alice's own items (bob's 🦆 is not
/// among them), ties in the names table's order. The app's words come
/// from a `HashMap` of the cached items, for the cache's account: the
/// same items in another order must offer the same.
#[test]
fn the_emoji_hints_do_not_depend_on_the_order_of_the_cached_items() {
    let items = fixture_items();
    let server = [
        "💀",
        "☠️",
        "🐱",
        "❤️",
        "😂",
        "🐸",
        "🔥",
        "👍",
        "👍🏽",
        "👎🏿",
        "🗿",
        "❤️\u{200d}🔥",
    ];
    let glyphs = |items: &[ItemSummary]| -> Vec<String> {
        hints("e:", &Words::for_viewer(items.iter(), Some("alice")))
            .iter()
            .map(|h| h.label.split(' ').next().unwrap().to_string())
            .collect()
    };
    assert_eq!(glyphs(&items), server, "in the server's order: the server's offers");
    let mut reversed = items.clone();
    reversed.reverse();
    assert_eq!(glyphs(&reversed), server, "the same items in another order");
}
