//! Review of the search round: what docs/search.md promises about the item
//! text, checked where a fixture of ordinary items does not reach.

use stickpile_api::{MediaFormat, MediaKind, Visibility};

use crate::*;

fn doc(id: &str, f: impl FnOnce(&mut Doc)) -> Doc {
    let mut d = Doc {
        item_id: id.to_string(),
        set_id: "set-misc".into(),
        set_slug: "misc".into(),
        set_name: "Misc".into(),
        owner_id: "alice".into(),
        owner_username: "alice".into(),
        visibility: Visibility::Private,
        name: String::new(),
        keywords: vec![],
        emojis: vec![],
        vibe: None,
        tags: vec![],
        text: String::new(),
        kind: MediaKind::Still,
        format: MediaFormat::Png,
        quality: None,
        likes: 0,
        copies: 0,
        media_id: format!("media-{id}"),
        embedding: None,
        updated_at: 0,
    };
    f(&mut d);
    d
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Ordinary items around the ones compared, so the field averages are
/// those of a library rather than of two items.
fn library(more: Vec<Doc>) -> Index {
    let mut ix = Index::new();
    for (n, (name, kws, text)) in [
        ("sleepy cat", vec!["cat", "sleep", "tired"], "monday again"),
        ("grumpy dog", vec!["dog", "angry"], ""),
        ("big chungus", vec!["rabbit", "meme"], "big chungus"),
        ("this is fine", vec!["fire", "dog", "fine"], "this is fine"),
        ("drake", vec!["no", "yes", "meme"], ""),
        ("surprised pikachu", vec!["shock", "pokemon"], ""),
    ]
    .into_iter()
    .enumerate()
    {
        ix.upsert(doc(&format!("filler{n}"), |d| {
            d.name = name.into();
            d.keywords = strs(&kws);
            d.text = text.into();
        }));
    }
    for d in more {
        ix.upsert(d);
    }
    ix
}

fn ids(ix: &Index, q: &str) -> Vec<String> {
    let v = Viewer {
        user_id: "alice".into(),
        ..Viewer::default()
    };
    ix.search(&Query::parse(q), &v, &SearchOptions::default())
        .hits
        .into_iter()
        .map(|h| h.item_id)
        .filter(|id| !id.starts_with("filler"))
        .collect()
}

/// "name > keyword > text for the same word" (docs/search.md, Ranking):
/// also when the picture says the word a few times, as captions do, and
/// the item has the five keywords the AI fills in.
#[test]
fn a_word_said_again_in_the_text_still_ranks_below_the_keyword() {
    let ix = library(vec![
        doc("keyword", |d| {
            d.name = "toddler refusing".into();
            d.keywords = strs(&["no", "child", "refuse", "stubborn", "meme"]);
        }),
        doc("text", |d| {
            d.name = "angry baby".into();
            d.text = "NO NO NO".into();
        }),
    ]);
    assert_eq!(ids(&ix, "no"), strs(&["keyword", "text"]));
}

/// "`😂`: Items with 😂 in their emojis (ranked first) or in their text."
#[test]
fn an_emoji_in_the_emojis_ranks_above_the_same_emoji_in_the_text() {
    let ix = library(vec![
        doc("emojis", |d| {
            d.name = "laughing guy".into();
            d.emojis = strs(&["😂", "🤣"]);
        }),
        doc("text", |d| {
            d.name = "caption".into();
            d.text = "😂😂😂".into();
        }),
    ]);
    assert_eq!(ids(&ix, "😂"), strs(&["emojis", "text"]));
}

/// The tokenizer removes a backtick as an apostrophe (`is_apostrophe`), but
/// UAX #29 splits the word at it first: "п`ятниця", as typed on a
/// keyboard without the apostrophe, is not "п'ятниця".
#[test]
fn a_backtick_is_an_apostrophe_as_the_tokenizer_means() {
    let ix = library(vec![
        doc("backtick", |d| d.text = "п`ятниця".into()),
        doc("ascii", |d| d.text = "п'ятниця".into()),
    ]);
    let mut got = ids(&ix, "text:п'ятниця");
    got.sort();
    assert_eq!(got, strs(&["ascii", "backtick"]));
}
