//! The text-search round (docs/search.md, "The item text" and
//! "Suggestions"): `text:` matched as plain words are, the last word by
//! its start too, and the words of the texts offered for `x:`.

use std::collections::HashSet;

use stickpile_api::{MediaFormat, MediaKind, Visibility};

use crate::*;

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

fn index(docs: impl IntoIterator<Item = Doc>) -> Index {
    let mut ix = Index::new();
    for d in docs {
        ix.upsert(d);
    }
    ix
}

fn alice() -> Viewer {
    Viewer {
        user_id: "alice".into(),
        ..Viewer::default()
    }
}

fn hits(ix: &Index, q: &str) -> Vec<Hit> {
    let opts = SearchOptions {
        limit: 1000,
        ..Default::default()
    };
    ix.search(&Query::parse(q), &alice(), &opts).hits
}

/// The ids found, in order.
fn ids(ix: &Index, q: &str) -> Vec<String> {
    hits(ix, q).into_iter().map(|h| h.item_id).collect()
}

/// The ids found, sorted.
fn found(ix: &Index, q: &str) -> Vec<String> {
    let mut v = ids(ix, q);
    v.sort();
    v
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Captions as OCR reads them.
fn captions() -> Index {
    index([
        doc("lol", "LOL"),
        doc("lol_what", "lol what"),
        doc("hello", "hello there"),
        doc("no", "no"),
        doc("thoughts", "NO THOUGHTS\nhead empty"),
        doc("joined", "КОЛИВ ДОМА"),
        doc("koly", "коли ти вдома"),
        doc("bug", "FIX THE BUG"),
        doc("build", "fix the build"),
        doc("apos_ascii", "П'ЯТНИЦЯ"),
        doc("apos_2019", "п’ятниця"),
        doc("apos_02bc", "пʼятниця"),
        doc("apos_tick", "п`ятниця"),
    ])
}

// ---------------------------------------------------------------- matching

#[test]
fn x_l_finds_lol_from_the_first_letter() {
    let ix = captions();
    assert_eq!(
        found(&ix, "x:l"),
        strs(&["lol", "lol_what"]),
        "not `hello`: l is inside"
    );
    assert_eq!(found(&ix, "x:lo"), strs(&["lol", "lol_what"]));
    assert_eq!(found(&ix, "x:L"), found(&ix, "x:l"), "case");
    assert_eq!(found(&ix, "text:lol"), strs(&["lol", "lol_what"]));
    // A plain word needs 2 letters for its start: `l` is the word `l`.
    assert!(ids(&ix, "l").is_empty());
    assert_eq!(found(&ix, "x:thou"), strs(&["thoughts"]));
    assert_eq!(found(&ix, "x:hel"), strs(&["hello"]));
}

#[test]
fn x_forgives_typos_and_finds_words_inside_words() {
    let ix = captions();
    assert_eq!(found(&ix, "x:lool"), strs(&["lol", "lol_what"]), "squashed");
    assert_eq!(found(&ix, "x:thoguhts"), strs(&["thoughts"]), "two letters swapped");
    assert_eq!(found(&ix, "x:thougths"), strs(&["thoughts"]));
    assert_eq!(found(&ix, "x:hellp"), strs(&["hello"]), "one letter wrong");
    // `КОЛИ В` read as one word: `оли` is inside it (3 letters on).
    assert_eq!(found(&ix, "x:оли"), strs(&["joined", "koly"]));
    assert_eq!(found(&ix, "x:коли"), strs(&["joined", "koly"]), "the word and a start");
    assert_eq!(
        ids(&ix, "x:КОЛИВ"),
        strs(&["joined", "koly"]),
        "the word, then a typo away"
    );
    // A letter OCR read as its look-alike (a Latin o in a Cyrillic word,
    // й for ї) is a typo from 4 letters.
    let misread = index([doc("latin_o", "кoтик"), doc("short_i", "їжак")]);
    assert_eq!(found(&misread, "x:котик"), strs(&["latin_o"]));
    assert_eq!(found(&misread, "x:йжак"), strs(&["short_i"]));
    assert!(found(&ix, "x:ол").is_empty(), "inside a word from 3 letters");
    assert!(found(&ix, "x:hlo").is_empty(), "no typo below 4 letters");
    assert!(found(&ix, "x:zzzz").is_empty());
}

#[test]
fn x_takes_every_apostrophe_form() {
    let ix = captions();
    let all = strs(&["apos_02bc", "apos_2019", "apos_ascii", "apos_tick"]);
    for q in [
        "x:п'ятн",
        "x:п’ятн",
        "x:пʼятн",
        "x:п`ятн",
        "x:П'ЯТНИЦЯ",
        "x:пятниця",
        "x:пятн",
    ] {
        assert_eq!(found(&ix, q), all, "{q}");
    }
}

#[test]
fn a_phrase_wants_its_words_in_a_row_and_only_the_last_by_its_start() {
    let ix = captions();
    assert_eq!(found(&ix, "x:\"no thou\""), strs(&["thoughts"]));
    assert_eq!(
        found(&ix, "x:\"thoughts he\""),
        strs(&["thoughts"]),
        "over a line break"
    );
    assert_eq!(found(&ix, "x:\"fix the b\""), strs(&["bug", "build"]));
    assert_eq!(found(&ix, "x:\"fix the bu\""), strs(&["bug", "build"]));
    assert_eq!(found(&ix, "x:\"fix the bug\""), strs(&["bug"]));
    assert_eq!(
        found(&ix, "x:\"no thougths head\""),
        strs(&["thoughts"]),
        "a typo in a word before"
    );
    // A word before the last is not matched by its start: `n` and `fi`
    // are only the starts of `no` and `fix`.
    assert!(found(&ix, "x:\"n thoughts\"").is_empty());
    assert!(found(&ix, "x:\"fi the bug\"").is_empty());
    assert!(found(&ix, "x:\"thoughts no\"").is_empty(), "in this order");
    assert!(found(&ix, "x:\"no empty\"").is_empty(), "one after another");
    // Words joined by a hyphen are a phrase too.
    assert_eq!(found(&ix, "x:fix-the-b"), strs(&["bug", "build"]));
}

#[test]
fn not_x_leaves_out_the_words_and_their_starts_only() {
    let ix = captions();
    let all = found(&ix, "");
    let without = |q: &str| -> Vec<String> {
        let got = found(&ix, q);
        all.iter().filter(|id| !got.contains(id)).cloned().collect()
    };
    assert_eq!(without("-x:lo"), strs(&["lol", "lol_what"]), "a start");
    assert_eq!(without("-x:lol"), strs(&["lol", "lol_what"]));
    assert!(without("-x:lool").is_empty(), "squashed: not left out");
    assert!(without("-x:thoguhts").is_empty(), "a typo: not left out");
    assert!(without("-x:оли").is_empty(), "inside a word: not left out");
    assert_eq!(without("-x:коли"), strs(&["joined", "koly"]));
    assert_eq!(without("-x:\"no thou\""), strs(&["thoughts"]));
    assert!(without("-x:\"n thoughts\"").is_empty());
    assert_eq!(without("-x:п'ятн").len(), 4);
    // With a positive value: both hold.
    assert_eq!(found(&ix, "x:fix -x:\"fix the bug\""), strs(&["build"]));
}

/// The best matches of a `text:` value first: the word, a word it
/// starts, squashed, a typo, inside a word; then as without it (here by
/// copies, the most first, so the order without tiers would be the
/// reverse). `sort:` decides alone.
#[test]
fn x_lists_the_best_matches_first() {
    let mut docs = vec![
        doc("exact", "cats"),
        doc("prefix", "catsup"),
        doc("squashed", "caats"),
        doc("typo", "cast"),
        doc("inside", "bobcats"),
        doc("none", "dogs"),
    ];
    for (n, d) in docs.iter_mut().enumerate() {
        d.copies = n as u64;
    }
    let ix = index(docs);
    let order = strs(&["exact", "prefix", "squashed", "typo", "inside"]);
    assert_eq!(ids(&ix, "x:cats"), order);
    let tiers: Vec<u8> = hits(&ix, "x:cats").iter().map(|h| h.text_tier).collect();
    assert_eq!(tiers, [0, 1, 2, 3, 4]);
    // With a word to rank by: the tier first, then the score.
    assert_eq!(ids(&ix, "item x:cats"), order);
    // `sort:` alone.
    assert_eq!(
        ids(&ix, "x:cats sort:copies"),
        strs(&["inside", "typo", "squashed", "prefix", "exact"])
    );
    // Several values: the best of each item (`none` has the most copies).
    assert_eq!(ids(&ix, "x:dogs x:cats")[..3], strs(&["none", "exact", "prefix"])[..]);
    // Without a text: value nothing changes.
    assert!(hits(&ix, "item").iter().all(|h| h.text_tier == 0));
    // The label stays `text` (for words that matched the text); a filter adds none.
    assert!(hits(&ix, "x:cats").iter().all(|h| h.matched.is_empty()));
}

// ---------------------------------------------------------------- punctuation

/// Captions with punctuation, as OCR reads them or people type them.
fn punctuated() -> Index {
    index([
        doc("q", "?"),
        doc("qq", "??"),
        doc("qx", "?!"),
        doc("what", "what?"),
        doc("what_sp", "WHAT ?"),
        doc("what_no", "what"),
        doc("whatever", "whatever?"),
        doc("full", "\u{0449}\u{043e}\u{FF1F}"),
        doc("wait", "wait..."),
        doc("dots4", "...."),
        doc("ellipsis", "\u{2026}"),
        doc("dots2", "so.. ok."),
        doc("wow", "wow!!!"),
        doc("plain", "no punctuation here"),
    ])
}

#[test]
fn x_question_finds_runs_and_word_suffix() {
    let ix = punctuated();
    let questions = strs(&["full", "q", "qq", "qx", "what", "what_sp", "whatever"]);
    assert_eq!(found(&ix, "x:?"), questions, "a token with `?` in it");
    assert_eq!(found(&ix, "x:\u{FF1F}"), questions, "full width");
    assert_eq!(found(&ix, "text:?"), questions);
    assert_eq!(found(&ix, "x:??"), strs(&["qq"]));
    assert_eq!(found(&ix, "x:?!"), strs(&["qx"]));
    assert_eq!(found(&ix, "x:!"), strs(&["qx", "wow"]));
    assert_eq!(found(&ix, "x:!!!!"), Vec::<String>::new());
    // Wildcards there are none: `*` is itself.
    assert!(found(&ix, "x:*").is_empty());
    // A filter adds no label, and every match is as good as another.
    assert!(hits(&ix, "x:?")
        .iter()
        .all(|h| h.matched.is_empty() && h.text_tier == 0));
}

#[test]
fn x_ellipsis_needs_three_dots() {
    let ix = punctuated();
    let ellipses = strs(&["dots4", "ellipsis", "wait"]);
    assert_eq!(found(&ix, "x:..."), ellipses, "not `..`");
    assert_eq!(found(&ix, "x:\u{2026}"), ellipses);
    assert_eq!(found(&ix, "x:...."), strs(&["dots4"]));
    assert_eq!(
        found(&ix, "x:.."),
        strs(&["dots2", "dots4", "ellipsis", "wait"]),
        "two dots are in three"
    );
    assert_eq!(found(&ix, "x:."), strs(&["dots2", "dots4", "ellipsis", "wait"]));
}

#[test]
fn phrase_word_then_punctuation() {
    let ix = punctuated();
    // The word, then a token with `?` in it: right after the word or after
    // a space; not the word alone, not another word it starts.
    assert_eq!(found(&ix, "x:\"what?\""), strs(&["what", "what_sp"]));
    assert_eq!(found(&ix, "x:what?"), strs(&["what", "what_sp"]), "unquoted the same");
    assert_eq!(found(&ix, "x:\"what ?\""), strs(&["what", "what_sp"]));
    assert_eq!(found(&ix, "x:wait..."), strs(&["wait"]));
    assert_eq!(found(&ix, "x:\u{0449}\u{043e}?"), strs(&["full"]));
    // The word as a word is matched: a typo is forgiven (and ranks so).
    assert_eq!(found(&ix, "x:waht?"), strs(&["what", "what_sp"]));
    assert!(hits(&ix, "x:waht?").iter().all(|h| h.text_tier == 3));
    assert_eq!(found(&ix, "x:what"), strs(&["what", "what_no", "what_sp", "whatever"]));
    // In this order, one after the other.
    assert!(found(&ix, "x:\"? what\"").is_empty());
    assert!(found(&ix, "x:\"what !\"").is_empty());
    assert!(found(&ix, "x:\"wait ?\"").is_empty());
    let ix = index([
        doc("a", "wait... what?!"),
        doc("b", "wait what"),
        doc("c", "what? wait..."),
        doc("d", "so ? ... ok"),
    ]);
    assert_eq!(found(&ix, "x:\"wait... what?\""), strs(&["a"]));
    assert_eq!(
        found(&ix, "x:\"wait what\""),
        strs(&["a", "b"]),
        "words alone: as before"
    );
    assert_eq!(found(&ix, "x:\"... wh\""), strs(&["a"]), "the last word by its start");
    // Punctuation alone, several: between the same two words.
    assert_eq!(found(&ix, "x:\"? ...\""), strs(&["d"]), "not c: a word between");
    assert_eq!(found(&ix, "x:\"what? wait\""), strs(&["c"]));
}

/// Items with punctuation in the name or the text (not the keywords).
fn named_punctuation() -> Index {
    let named = |id: &str, name: &str, text: &str| Doc {
        name: name.into(),
        ..doc(id, text)
    };
    let mut ix = index([
        named("name_q", "Really?", ""),
        named("text_q", "plain", "really?"),
        named("name_dots", "wait\u{2026}", ""),
        named("text_dots", "plain", "so..."),
        named("none", "nothing", "nothing"),
    ]);
    ix.upsert(Doc {
        keywords: strs(&["huh?", "hm..."]),
        ..named("kw", "plain", "")
    });
    ix
}

#[test]
fn plain_punctuation_query_searches_text_and_name() {
    let ix = named_punctuation();
    // In the name first, as a word would be.
    assert_eq!(ids(&ix, "?"), strs(&["name_q", "text_q"]), "not the keywords");
    assert_eq!(hits(&ix, "?")[0].matched, ["name"]);
    assert_eq!(hits(&ix, "?")[1].matched, ["text"]);
    assert_eq!(found(&ix, "..."), strs(&["name_dots", "text_dots"]));
    assert_eq!(found(&ix, "\u{2026}"), strs(&["name_dots", "text_dots"]));
    assert_eq!(found(&ix, "\u{FF1F}"), strs(&["name_q", "text_q"]));
    assert_eq!(
        found(&ix, "? ..."),
        strs(&["name_dots", "name_q", "text_dots", "text_q"]),
        "either"
    );
    // `x:` looks at the text alone.
    assert_eq!(found(&ix, "x:?"), strs(&["text_q"]));
    assert!(found(&ix, "!").is_empty());
    // One kind of match, one idf for all the runs with `?`: the rarer
    // `??` does not outrank the `?` of a name.
    let runs = index([
        Doc {
            name: "huh?".into(),
            ..doc("name_q", "")
        },
        doc("text_qq", "??"),
        doc("text_q1", "?"),
        doc("text_q2", "so ?"),
        doc("text_q3", "ok ?"),
    ]);
    assert_eq!(ids(&runs, "?")[0], "name_q");
    assert_eq!(ids(&runs, "?").len(), 5);
    // Before, a query of punctuation found everything (no word to rank by).
    assert_eq!(found(&ix, "").len(), 6);
    // No image side and no expansions: they would bring `none`.
    let opts = SearchOptions {
        limit: 100,
        text_embedding: Some(vec![1.0, 0.0]),
        expansions: vec![("nothing".into(), 1.0)],
        ..Default::default()
    };
    let mut ix = ix;
    let mut none = ix.get("none").cloned().unwrap();
    none.embedding = Some(vec![1.0f32, 0.0].into());
    ix.upsert(none);
    let got: Vec<String> = ix
        .search(&Query::parse("?"), &alice(), &opts)
        .hits
        .into_iter()
        .map(|h| h.item_id)
        .collect();
    assert_eq!(got, strs(&["name_q", "text_q"]));
    let got = ix.search(&Query::parse("nothing"), &alice(), &opts).hits;
    assert!(got[0].matched.contains(&"image"), "the image side is there for words");
}

#[test]
fn mixed_plain_query_keeps_words_fuzzy_and_punctuation_soft() {
    let doc = |id: &str, text: &str| Doc {
        name: "sticker".into(),
        ..doc(id, text)
    };
    let ix = index([
        doc("what_q", "what?"),
        doc("what", "what"),
        doc("waht", "waht"),
        doc("huh", "huh?"),
        doc("none", "nothing"),
    ]);
    // The words find as without the punctuation (a typo too); the
    // punctuation finds nothing by itself (`huh`).
    for (mixed, words) in [
        ("what?", "what"),
        ("waht?", "waht"),
        ("what ?", "what"),
        ("whta?!", "whta"),
    ] {
        assert_eq!(found(&ix, mixed), found(&ix, words), "{mixed}");
    }
    assert_eq!(found(&ix, "what?"), strs(&["waht", "what", "what_q"]));
    // It lifts the items that have it: the same scores otherwise, so
    // `what` comes first by its id without the `?`.
    assert_eq!(ids(&ix, "what"), strs(&["what", "what_q", "waht"]));
    assert_eq!(ids(&ix, "what?"), strs(&["what_q", "what", "waht"]));
    assert_eq!(hits(&ix, "what?")[0].matched, ["text"]);
    // A negated term leaves its punctuation out, as before.
    assert_eq!(found(&ix, "-what?"), found(&ix, "-what"));
}

#[test]
fn minus_x_question_excludes() {
    let ix = punctuated();
    let all = found(&ix, "");
    let without = |q: &str| -> Vec<String> {
        let got = found(&ix, q);
        all.iter().filter(|id| !got.contains(id)).cloned().collect()
    };
    assert_eq!(without("-x:?"), found(&ix, "x:?"));
    assert_eq!(without("-text:?"), found(&ix, "x:?"));
    assert_eq!(without("-x:..."), strs(&["dots4", "ellipsis", "wait"]), "`..` stays");
    assert_eq!(without("-x:\"what?\""), strs(&["what", "what_sp"]));
    assert_eq!(found(&ix, "x:what -x:?"), strs(&["what_no"]));
    // A `-` term of punctuation alone: the name and the text.
    let ix = named_punctuation();
    assert_eq!(
        found(&ix, "-?"),
        strs(&["kw", "name_dots", "none", "text_dots"]),
        "not name_q, text_q; the keywords keep no punctuation"
    );
    assert_eq!(found(&ix, "-\u{2026}"), strs(&["kw", "name_q", "none", "text_q"]));
}

#[test]
fn x_and_plain_words_together() {
    let ix = captions();
    assert_eq!(ids(&ix, "what x:lo"), strs(&["lol_what"]));
    assert_eq!(found(&ix, "x:lo x:thou"), strs(&["lol", "lol_what", "thoughts"]), "or");
    assert_eq!(found(&ix, "x:fi x:bu"), strs(&["bug", "build"]));
}

// ---------------------------------------------------------------- words

fn words(ix: &Index, typed: &str) -> Vec<String> {
    ix.text_words(typed, &alice(), 12)
}

#[test]
fn nothing_typed_offers_the_most_common_words() {
    let ix = index([
        doc("a", "lol lol lol"),
        doc("b", "lol what"),
        doc("c", "what is love"),
        doc("d", "LOVE"),
        doc("e", "I 2024 2nd"),
    ]);
    // `lol` said three times in one picture counts once: lol, love and
    // what are in two items each (alphabetical then), is in one.
    assert_eq!(words(&ix, ""), strs(&["lol", "love", "what", "2nd", "is"]));
    assert_eq!(ix.text_words("", &alice(), 2), strs(&["lol", "love"]));
    assert!(ix.text_words("", &alice(), 0).is_empty());
}

#[test]
fn a_start_offers_the_words_starting_with_it_most_common_first() {
    let ix = index([
        doc("a", "lol"),
        doc("b", "lol"),
        doc("c", "love lmao"),
        doc("d", "hello trololo"),
        doc("e", "кіт котик"),
        doc("f", "котик око"),
        doc("g", "КОЛИВ"),
    ]);
    assert_eq!(
        words(&ix, "l"),
        strs(&["lol", "lmao", "love"]),
        "not hello: l is inside"
    );
    assert_eq!(
        words(&ix, "lo"),
        strs(&["lol", "love", "hello", "trololo"]),
        "then inside"
    );
    assert_eq!(words(&ix, "LO"), words(&ix, "lo"));
    assert_eq!(
        words(&ix, "lol"),
        strs(&["lol", "trololo", "love"]),
        "the word, inside, a typo away"
    );
    assert_eq!(words(&ix, "ко"), strs(&["котик", "колив", "око"]));
    assert_eq!(words(&ix, "оли"), strs(&["колив"]));
    // A typo in a half-typed word, as the suggestion line forgives it.
    assert_eq!(words(&ix, "lvoe"), strs(&["love"]));
    assert_eq!(words(&ix, "кошик"), strs(&["котик"]));
    assert!(words(&ix, "zz").is_empty());
}

#[test]
fn the_words_are_as_the_index_keeps_them() {
    let ix = index([doc("a", "П'ЯТНИЦЯ"), doc("b", "п’ятниця"), doc("c", "Café")]);
    assert_eq!(words(&ix, "п'ят"), strs(&["пятниця"]), "once, all forms counted");
    assert_eq!(words(&ix, "пʼят"), strs(&["пятниця"]));
    assert_eq!(words(&ix, "caf"), strs(&["cafe"]));
    assert_eq!(words(&ix, "CAFÉ"), strs(&["cafe"]));
    assert_eq!(text_word("П'ятн"), Some("пятн".to_string()));
    assert_eq!(text_word(""), Some(String::new()));
    assert_eq!(text_word("a b"), None);
    assert_eq!(text_word("a-b"), None);
    assert_eq!(text_word("!!!"), None);
    assert_eq!(text_word("😂"), None);
    assert!(words(&ix, "a b").is_empty(), "several words: nothing to offer");
}

#[test]
fn short_words_and_numbers_are_not_offered() {
    let ix = index([doc("a", "a b c 2024 1.5 2nd x2 ok"), doc("b", "a ok")]);
    assert_eq!(words(&ix, ""), strs(&["ok", "2nd", "x2"]));
    assert!(words(&ix, "a").is_empty());
    assert!(words(&ix, "20").is_empty());
    assert_eq!(words(&ix, "2"), strs(&["2nd"]));
    // Emojis in the text are no words.
    let ix = index([doc("a", "😂😂 lol")]);
    assert_eq!(words(&ix, ""), strs(&["lol"]));
}

/// Rule 6 of the round: a word only in someone else's private item is
/// never offered; the viewer's own, the public ones and those of the sets
/// the viewer contributes to are.
#[test]
fn only_the_words_of_items_the_viewer_can_search() {
    let theirs = |id: &str, set: &str, vis: Visibility, text: &str| Doc {
        owner_id: "bob".into(),
        owner_username: "bob".into(),
        set_id: set.into(),
        visibility: vis,
        ..doc(id, text)
    };
    let ix = index([
        doc("mine", "zebra"),
        theirs("secret", "bobs-secret", Visibility::Private, "zyzzyva zebra"),
        theirs("unlisted", "bobs-unlisted", Visibility::Unlisted, "zygote"),
        theirs("public", "bobs-public", Visibility::Public, "zany zebra"),
        theirs("public2", "bobs-public", Visibility::Public, "zany"),
        theirs("shared", "bobs-shared", Visibility::Private, "zealot"),
    ]);
    let alice = alice();
    let mut member = alice.clone();
    member.member_sets = HashSet::from(["bobs-shared".to_string()]);
    // zany and zebra are in two items each that alice can search (bob's
    // private one would make zebra three, and first).
    assert_eq!(ix.text_words("z", &alice, 12), strs(&["zany", "zebra"]));
    assert_eq!(ix.text_words("", &alice, 12), strs(&["zany", "zebra"]));
    assert_eq!(ix.text_words("z", &member, 12), strs(&["zany", "zebra", "zealot"]));
    assert!(ix.text_words("zyz", &alice, 12).is_empty());
    assert!(ix.text_words("zyg", &alice, 12).is_empty());
    let bob = Viewer {
        user_id: "bob".into(),
        ..Viewer::default()
    };
    assert_eq!(
        ix.text_words("z", &bob, 12),
        strs(&["zany", "zebra", "zealot", "zygote", "zyzzyva"])
    );
    let nobody = Viewer::default();
    assert_eq!(ix.text_words("z", &nobody, 12), strs(&["zany", "zebra"]));
    // The same through the trait the suggestion line reads.
    let viewed = ViewedIndex {
        index: &ix,
        viewer: &alice,
    };
    assert_eq!(TextWords::text_words(&viewed, "z", 12), strs(&["zany", "zebra"]));
}

#[test]
fn punctuation_not_offered_as_text_word() {
    let ix = index([doc("a", "??? lol"), doc("b", "?! what..."), doc("c", "\u{2026} lol!")]);
    assert_eq!(words(&ix, ""), strs(&["lol", "what"]));
    assert_eq!(words(&ix, "lo"), strs(&["lol"]));
    for typed in ["?", "...", "\u{2026}", "?!", "!"] {
        assert!(words(&ix, typed).is_empty(), "{typed}");
        assert_eq!(text_word(typed), None, "{typed}");
    }
    // A value's punctuation is left out of the word typed, as before.
    assert_eq!(text_word("what?"), Some("what".to_string()));
    assert_eq!(words(&ix, "wha?"), strs(&["what"]));
}

#[test]
fn removed_and_changed_texts_are_no_longer_offered() {
    let mut ix = index([doc("a", "kitten"), doc("b", "kitten mitten")]);
    assert_eq!(words(&ix, "kit"), strs(&["kitten", "mitten"]), "mitten: a typo away");
    ix.remove("a");
    ix.upsert(doc("b", "puppy"));
    assert!(words(&ix, "kit").is_empty());
    assert!(words(&ix, "mit").is_empty());
    assert_eq!(words(&ix, ""), strs(&["puppy"]));
}

/// The tiers of the suggestion line (`stickpile_quick::suggest` ranks
/// tags with the same function).
#[test]
fn offer_tiers() {
    let c = |s: &str| s.chars().collect::<Vec<char>>();
    let tier = |typed: &str, word: &str| offer_tier(&c(typed), &c(word));
    assert_eq!(tier("sad", "sad"), Some(0));
    assert_eq!(tier("sa", "sarcastic"), Some(1));
    assert_eq!(tier("ppy", "happy"), Some(2));
    assert_eq!(tier("p", "happy"), None, "inside from 2 letters");
    assert_eq!(tier("sarcstic", "sarcastic"), Some(4), "a letter left out");
    assert_eq!(tier("sacr", "sarcastic"), Some(4), "half typed, two letters swapped");
    assert_eq!(tier("sar", "sad"), Some(4), "one typo from 3 letters");
    assert_eq!(tier("zz", "sad"), None, "two letters forgive no typo");
    assert_eq!(tier("xyzzy", "happy"), None);
    assert_eq!(tier("", "happy"), Some(0));
    assert_eq!(typo_budget(2), 0);
    assert_eq!(typo_budget(5), 1);
    assert_eq!(typo_budget(6), 2);
}
