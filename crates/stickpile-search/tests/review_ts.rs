//! Review of the text-search round (branch review-ts): what the brief and
//! docs/search.md promise about `text:` phrases, checked on the public API.

use stickpile_api::{MediaFormat, MediaKind, Visibility};
use stickpile_search::{Doc, Index, Query, SearchOptions, Viewer};

fn doc(id: &str, text: &str) -> Doc {
    Doc {
        item_id: id.to_string(),
        set_id: "set-alice".into(),
        set_slug: "mine".into(),
        set_name: "Mine".into(),
        owner_id: "alice".into(),
        owner_username: "alice".into(),
        visibility: Visibility::Private,
        name: format!("item {id}"),
        keywords: vec![],
        emojis: vec![],
        vibe: None,
        tags: vec![],
        text: text.into(),
        kind: MediaKind::Still,
        format: MediaFormat::Png,
        quality: None,
        likes: 0,
        copies: 0,
        media_id: format!("media-{id}"),
        embedding: None,
        updated_at: 0,
    }
}

fn found(ix: &Index, q: &str) -> Vec<String> {
    let viewer = Viewer {
        user_id: "alice".into(),
        ..Viewer::default()
    };
    let mut ids: Vec<String> = ix
        .search(&Query::parse(q), &viewer, &SearchOptions::default())
        .hits
        .into_iter()
        .map(|h| h.item_id)
        .collect();
    ids.sort();
    ids
}

/// docs/search.md, "The item text": "A word before it does not match by
/// its start (`x:"n thoughts"` finds nothing)"; the brief (4.1): "only the
/// last may be a prefix (the user is still typing it)". From 3 letters a
/// start of a word before the last still matches, as a word "inside" it
/// (the substring tier starts at position 0 too).
#[test]
fn review_ts_a_word_before_the_last_does_not_match_by_its_start() {
    let mut ix = Index::new();
    ix.upsert(doc("thoughts", "NO THOUGHTS\nHEAD EMPTY"));
    ix.upsert(doc("koly", "КОЛИ ДОМА"));
    // As promised: one or two letters.
    assert!(found(&ix, "x:\"n thoughts\"").is_empty());
    assert!(found(&ix, "x:\"th head\"").is_empty());
    assert!(found(&ix, "x:\"ко дома\"").is_empty());
    // The same rule from three letters ("tho", "thou", "кол" are no typo
    // of "thoughts", "коли": they match as a part from 3 letters).
    // While `-x:` (exact for a word before the last) leaves them in.
    assert_eq!(found(&ix, "-x:\"thou head\""), ["koly", "thoughts"]);
    let wrong: Vec<String> = ["x:\"tho head\"", "x:\"thou head\"", "x:\"кол дома\""]
        .into_iter()
        .filter(|q| !found(&ix, q).is_empty())
        .map(|q| format!("{q} finds {:?}", found(&ix, q)))
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("; "));
}
