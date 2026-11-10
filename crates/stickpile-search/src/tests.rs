use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use stickpile_api::{MediaFormat, MediaKind, Visibility};

use crate::text::{osa_within, tokenize, Token};
use crate::*;

// ---------------------------------------------------------------- helpers

fn doc(id: &str) -> Doc {
    Doc {
        item_id: id.to_string(),
        set_id: "set-misc".into(),
        set_slug: "misc".into(),
        set_name: "Misc".into(),
        owner_id: "alice".into(),
        owner_username: "Alice".into(),
        visibility: Visibility::Private,
        name: String::new(),
        keywords: vec![],
        emojis: vec![],
        vibe: None,
        tags: vec![],
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

fn with(id: &str, f: impl FnOnce(&mut Doc)) -> Doc {
    let mut d = doc(id);
    f(&mut d);
    d
}

fn named(id: &str, name: &str) -> Doc {
    with(id, |d| d.name = name.into())
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn index(docs: impl IntoIterator<Item = Doc>) -> Index {
    let mut ix = Index::new();
    for d in docs {
        ix.upsert(d);
    }
    ix
}

fn viewer(id: &str) -> Viewer {
    Viewer {
        user_id: id.into(),
        liked: HashSet::new(),
    }
}

fn alice() -> Viewer {
    viewer("alice")
}

fn run(ix: &Index, q: &str, v: &Viewer, opts: &SearchOptions) -> Outcome {
    ix.search(&Query::parse(q), v, opts)
}

fn ids_opts(ix: &Index, q: &str, v: &Viewer, opts: &SearchOptions) -> Vec<String> {
    run(ix, q, v, opts).hits.into_iter().map(|h| h.item_id).collect()
}

/// Ranked ids for `q` as alice.
fn ids(ix: &Index, q: &str) -> Vec<String> {
    ids_opts(ix, q, &alice(), &SearchOptions::default())
}

fn sorted_ids(ix: &Index, q: &str, v: &Viewer) -> Vec<String> {
    let mut v = ids_opts(
        ix,
        q,
        v,
        &SearchOptions {
            limit: 10_000,
            ..Default::default()
        },
    );
    v.sort();
    v
}

fn hit<'a>(o: &'a Outcome, id: &str) -> &'a Hit {
    o.hits
        .iter()
        .find(|h| h.item_id == id)
        .unwrap_or_else(|| panic!("{id} not in {:?}", o.hits))
}

fn emb(v: &[f32]) -> Option<Arc<[f32]>> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    Some(v.iter().map(|x| x / n).collect::<Vec<_>>().into())
}

/// Tiny deterministic PRNG (xorshift64*), no dependency needed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, self.below(i + 1));
        }
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
    }
}

// ---------------------------------------------------------------- text

#[test]
fn normalize_table() {
    let cases = [
        ("Café", "cafe"),
        ("CAFÉ", "cafe"),
        ("cafe\u{301}", "cafe"),
        ("naïve Ångström", "naive angstrom"),
        ("Кіт", "кіт"),
        ("ЁЖИК ёлка", "ежик елка"),
        ("Їжак", "їжак"),
        ("Йой", "йой"),
        ("и\u{306}", "й"),
        ("Ўсё", "ўсе"),
        ("ﬁre", "fire"),
        ("ｃａｔ", "cat"),
        ("❤️", "❤"),
        ("İstanbul", "istanbul"),
        ("Straße", "straße"),
        ("がっこう", "がっこう"),
        ("plain ascii 123", "plain ascii 123"),
    ];
    for (input, want) in cases {
        assert_eq!(normalize(input), want, "normalize({input:?})");
    }
}

#[test]
fn squash_table() {
    let cases = [
        ("naahh", "nah"),
        ("sooo", "so"),
        ("nah", "nah"),
        ("noooope", "nope"),
        ("1000", "1000"),
        ("", ""),
        ("аааа", "а"),
        ("Aa", "Aa"),
    ];
    for (input, want) in cases {
        assert_eq!(squash(input), want, "squash({input:?})");
    }
}

#[test]
fn squash_key_table() {
    let cases = [
        ("hahaha", "ha"),
        ("haha", "ha"),
        ("hahaaa", "ha"),
        ("lolol", "lol"),
        ("lololol", "lol"),
        ("lol", "lol"),
        ("naahh", "nah"),
        ("хахаха", "ха"),
        ("1212", "1212"),
    ];
    for (input, want) in cases {
        assert_eq!(text::squash_key(input), want, "squash_key({input:?})");
    }
}

#[test]
fn tokenize_words_and_emojis() {
    let toks = tokenize("Big-Chungus 😂😂 can't my_cats file.png 👍🏽 ❤️ п'ять");
    let words: Vec<(&str, bool)> = toks.iter().map(|t| (t.text.as_str(), t.word)).collect();
    assert_eq!(
        words,
        vec![
            ("big", true),
            ("chungus", true),
            ("😂", false),
            ("😂", false),
            ("cant", true),
            ("my", true),
            ("cats", true),
            ("file", true),
            ("png", true),
            ("👍", false),
            ("❤", false),
            ("пять", true),
        ]
    );
    assert_eq!(
        tokenize("🤷‍♂️"),
        vec![Token {
            text: "🤷\u{200d}♂".into(),
            word: false
        }]
    );
    assert_eq!(tokenize("🇺🇦").len(), 1);
    assert!(tokenize(" - !? ").is_empty());
}

#[test]
fn osa_distance() {
    let c = |s: &str| s.chars().collect::<Vec<_>>();
    assert_eq!(osa_within(&c("chungs"), &c("chungus"), 1), Some(1));
    assert_eq!(osa_within(&c("abcd"), &c("bacd"), 1), Some(1));
    assert_eq!(osa_within(&c("cat"), &c("cat"), 0), Some(0));
    assert_eq!(osa_within(&c("kitten"), &c("sitting"), 2), None);
    assert_eq!(osa_within(&c("kitten"), &c("sitting"), 3), Some(3));
    assert_eq!(osa_within(&c("a"), &c("abcd"), 2), None);
    // Words longer than the stack buffer take the heap path.
    let long = "x".repeat(60);
    assert_eq!(osa_within(&c(&long), &c(&format!("{long}y")), 1), Some(1));
    assert_eq!(
        osa_within(&c(&format!("ab{long}")), &c(&format!("ba{long}")), 1),
        Some(1)
    );
}

#[test]
fn emoji_names_lookup() {
    assert!(emoji_names::names("😂").unwrap().contains("laugh"));
    assert!(emoji_names::names("🤷\u{200d}♂").unwrap().contains("shrug"));
    assert!(emoji_names::names("🦖").is_none());
}

// ---------------------------------------------------------------- visibility

#[test]
fn visibility_matrix() {
    let cases = [
        ("own-private", "alice", Visibility::Private, true),
        ("own-unlisted", "alice", Visibility::Unlisted, true),
        ("own-public", "alice", Visibility::Public, true),
        ("other-private", "bob", Visibility::Private, false),
        ("other-unlisted", "bob", Visibility::Unlisted, false),
        ("other-public", "bob", Visibility::Public, true),
    ];
    let ix = index(cases.iter().map(|(id, owner, vis, _)| {
        with(id, |d| {
            d.name = "grumpy cat".into();
            d.owner_id = owner.to_string();
            d.visibility = *vis;
            d.embedding = emb(&[1.0, 0.0]);
        })
    }));
    let mut want: Vec<String> = cases.iter().filter(|c| c.3).map(|c| c.0.to_string()).collect();
    want.sort();
    for q in ["grumpy", "", "type:png", "\"grumpy cat\"", "gru", "grumpi"] {
        assert_eq!(sorted_ids(&ix, q, &alice()), want, "query {q:?}");
    }
    // The vector side obeys the same rules.
    let opts = SearchOptions {
        text_embedding: Some(vec![1.0, 0.0]),
        min_similarity: 0.5,
        ..Default::default()
    };
    let mut got = ids_opts(&ix, "zzz", &alice(), &opts);
    got.sort();
    assert_eq!(got, want);
    // Anonymous viewers (empty user id) only see public items, even of ownerless docs.
    let mut anon = sorted_ids(&ix, "grumpy", &viewer(""));
    anon.sort();
    assert_eq!(anon, strs(&["other-public", "own-public"]));
    let ix2 = index([with("x", |d| d.owner_id = String::new())]);
    assert!(sorted_ids(&ix2, "", &viewer("")).is_empty());
}

// ---------------------------------------------------------------- filters

fn filter_fixture() -> (Index, Viewer) {
    let docs = [
        with("a", |d| {
            d.name = "Sleepy cat".into();
            d.emojis = strs(&["😴"]);
            d.vibe = Some("tired".into());
            d.tags = strs(&["Reaction"]);
            (d.set_id, d.set_slug, d.set_name) = ("s-cats".into(), "cats".into(), "My Cats".into());
            d.kind = MediaKind::Animation;
            d.format = MediaFormat::Gif;
            d.quality = Some(80);
            (d.likes, d.copies) = (5, 10);
            d.visibility = Visibility::Public;
        }),
        with("b", |d| {
            d.name = "Angry dog".into();
            d.emojis = strs(&["😡", "❤️"]);
            d.vibe = Some("Angry".into());
            d.tags = strs(&["nsfw"]);
            (d.set_id, d.set_slug, d.set_name) = ("s-dogs".into(), "dogs".into(), "Dogs".into());
            d.quality = Some(40);
            (d.likes, d.copies) = (0, 2);
        }),
        with("c", |d| {
            d.name = "Dancing frog".into();
            d.emojis = strs(&["👍🏽"]);
            d.vibe = Some("happy".into());
            d.tags = strs(&["reaction", "dance"]);
            (d.set_id, d.set_slug, d.set_name) = ("s-frogs".into(), "frog-pack".into(), "Frögs".into());
            (d.owner_id, d.owner_username) = ("bob".into(), "Bob".into());
            d.kind = MediaKind::Video;
            d.format = MediaFormat::Mp4;
            (d.likes, d.copies) = (12, 0);
            d.visibility = Visibility::Public;
        }),
        with("d", |d| {
            d.name = "Bob private cat".into();
            (d.owner_id, d.owner_username) = ("bob".into(), "Bob".into());
        }),
        with("e", |d| {
            d.name = "Heart cat".into();
            d.emojis = strs(&["❤"]);
            (d.set_id, d.set_slug, d.set_name) = ("s-cats".into(), "cats".into(), "My Cats".into());
            d.format = MediaFormat::Webp;
            d.quality = Some(95);
            (d.likes, d.copies) = (1, 1);
            d.visibility = Visibility::Unlisted;
        }),
    ];
    let mut v = alice();
    v.liked.insert("c".into());
    (index(docs), v)
}

#[test]
fn every_filter_and_negation() {
    let (ix, v) = filter_fixture();
    let cases: &[(&str, &[&str])] = &[
        ("", &["a", "b", "c", "e"]),
        // emojis, with variation selector and skin tone handling
        ("e:😴", &["a"]),
        ("e:❤️", &["b", "e"]),
        ("e:❤", &["b", "e"]),
        ("emoji:❤", &["b", "e"]),
        ("-e:❤", &["a", "c"]),
        ("e:👍", &["c"]),
        ("e:👍🏽", &["c"]),
        ("e:👍🏿", &[]),
        ("e:😴 e:😡", &["a", "b"]),
        // sets: slug, normalised name, id
        ("set:cats", &["a", "e"]),
        ("set:\"my cats\"", &["a", "e"]),
        ("set:MY-CATS", &[]),
        ("set:frog-pack", &["c"]),
        ("set:frogs", &["c"]),
        ("set:Frögs", &["c"]),
        ("set:s-dogs", &["b"]),
        ("-set:cats", &["b", "c"]),
        ("set:cats set:dogs", &["a", "b", "e"]),
        // vibes
        ("vibe:tired", &["a"]),
        ("vibe:TIRED", &["a"]),
        ("vibe:angry", &["b"]),
        ("-vibe:tired", &["b", "c", "e"]),
        ("vibe:tire", &[]),
        // tags
        ("tag:reaction", &["a", "c"]),
        ("-tag:nsfw", &["a", "c", "e"]),
        ("tag:dance tag:nsfw", &["b", "c"]),
        ("tag:reaction -tag:dance", &["a"]),
        // types
        ("type:gif", &["a"]),
        ("type:animated", &["a"]),
        ("type:still", &["b", "e"]),
        ("-type:still", &["a", "c"]),
        ("type:video", &["c"]),
        ("type:mp4", &["c"]),
        ("type:webp", &["e"]),
        ("type:gif type:webp", &["a", "e"]),
        // owners
        ("by:bob", &["c"]),
        ("by:BOB", &["c"]),
        ("user:alice", &["a", "b", "e"]),
        ("-by:alice", &["c"]),
        // is:
        ("is:mine", &["a", "b", "e"]),
        ("-is:mine", &["c"]),
        ("is:liked", &["c"]),
        ("-is:liked", &["a", "b", "e"]),
        ("is:public", &["a", "c"]),
        ("-is:public", &["b", "e"]),
        // comparisons (items without a quality never pass a quality filter)
        ("q>70", &["a", "e"]),
        ("q<50", &["b"]),
        ("q>=40 q<=80", &["a", "b"]),
        ("likes>=5", &["a", "c"]),
        ("likes=0", &["b"]),
        ("likes<2", &["b", "e"]),
        ("copies>1", &["a", "b"]),
        ("copies=0", &["c"]),
        // excluded words: whole tokens in any own text field
        ("-cat", &["b", "c"]),
        ("-cats", &["b", "c"]),
        ("-\"angry dog\"", &["a", "c", "e"]),
        ("-\"dog angry\"", &["a", "b", "c", "e"]),
        ("-angr", &["a", "b", "c", "e"]),
        ("-dancing -sleepy", &["b", "e"]),
        ("-😡", &["a", "c", "e"]),
        // combined with text
        ("cat", &["a", "e"]),
        ("cat e:😴 q>50", &["a"]),
        ("frog by:bob", &["c"]),
        ("cat -sleepy", &["e"]),
        ("dog is:public", &[]),
    ];
    for (q, want) in cases {
        assert_eq!(sorted_ids(&ix, q, &v), strs(want), "query {q:?}");
    }
}

// ---------------------------------------------------------------- lexical matching

#[test]
fn squash_matches_elongated_words() {
    let ix = index([
        with("nah", |d| d.keywords = strs(&["nah"])),
        with("nope", |d| d.name = "nope".into()),
        with("long", |d| d.keywords = strs(&["naaaaah"])),
    ]);
    let o = run(&ix, "naahh", &alice(), &SearchOptions::default());
    let mut got: Vec<&str> = o.hits.iter().map(|h| h.item_id.as_str()).collect();
    got.sort();
    assert_eq!(got, vec!["long", "nah"]);
    assert_eq!(hit(&o, "nah").matched, vec!["keyword", "fuzzy"]);
    // Repeated letter pairs squash too.
    let laughs = index([
        with("lol", |d| d.keywords = strs(&["lol"])),
        with("haha", |d| d.vibe = Some("haha".into())),
    ]);
    assert_eq!(ids(&laughs, "lolololol"), strs(&["lol"]));
    assert_eq!(ids(&laughs, "hahahaha"), strs(&["haha"]));
    // Exact beats squashed.
    assert_eq!(ids(&ix, "nah"), strs(&["nah", "long"]));
    assert_eq!(
        hit(&run(&ix, "nah", &alice(), &SearchOptions::default()), "nah").matched,
        vec!["keyword"]
    );
}

#[test]
fn expansions_reach_synonyms() {
    let ix = index([
        with("x", |d| d.keywords = strs(&["nah"])),
        with("y", |d| d.vibe = Some("dismissive".into())),
        with("z", |d| d.vibe = Some("happy".into())),
    ]);
    assert_eq!(ids(&ix, "naahh"), strs(&["x"]));
    let opts = SearchOptions {
        expansions: vec![("dismissive".into(), 0.6)],
        ..Default::default()
    };
    let o = run(&ix, "naahh", &alice(), &opts);
    assert_eq!(
        o.hits.iter().map(|h| h.item_id.as_str()).collect::<Vec<_>>(),
        vec!["x", "y"]
    );
    assert_eq!(hit(&o, "y").matched, vec!["expansion"]);
    assert_eq!(hit(&o, "x").matched, vec!["keyword", "fuzzy"]);
    // Zero / NaN weights are ignored; an expansion equal to a user term adds nothing.
    for w in [0.0, f32::NAN, -1.0] {
        let opts = SearchOptions {
            expansions: vec![("dismissive".into(), w)],
            ..Default::default()
        };
        assert_eq!(ids_opts(&ix, "naahh", &alice(), &opts), strs(&["x"]));
    }
    let plain = run(&ix, "nah", &alice(), &SearchOptions::default());
    let dup = run(
        &ix,
        "nah",
        &alice(),
        &SearchOptions {
            expansions: vec![("nah".into(), 1.0)],
            ..Default::default()
        },
    );
    assert_eq!(plain, dup);
    // Multi-word expansions are phrases.
    let ix = index([
        with("t", |d| d.keywords = strs(&["thumbs up"])),
        with("u", |d| d.keywords = strs(&["up"])),
    ]);
    let opts = SearchOptions {
        expansions: vec![("thumbs up".into(), 0.5)],
        ..Default::default()
    };
    assert_eq!(ids_opts(&ix, "approve", &alice(), &opts), strs(&["t"]));
}

#[test]
fn expansion_only_ranks_below_user_term_match() {
    let ix = index([
        with("syn", |d| d.name = "dismissive".into()),
        with("user", |d| d.tags = strs(&["nah"])),
    ]);
    let opts = SearchOptions {
        expansions: vec![("dismissive".into(), 0.6)],
        ..Default::default()
    };
    assert_eq!(ids_opts(&ix, "nah", &alice(), &opts), strs(&["user", "syn"]));
}

#[test]
fn fuzzy_matches() {
    let ix = index([
        named("chungus", "Big Chungus"),
        named("cat", "cat"),
        named("year", "2024 recap"),
        named("dog", "dog"),
    ]);
    for q in ["chungs", "chungsu", "chxngus", "cxungsu", "chunguss"] {
        let o = run(&ix, q, &alice(), &SearchOptions::default());
        assert_eq!(o.hits.len(), 1, "{q}");
        assert_eq!(o.hits[0].item_id, "chungus", "{q}");
        assert_eq!(o.hits[0].matched, vec!["name", "fuzzy"], "{q}");
    }
    // Too many edits, too short, or numeric: no fuzzy match.
    assert!(ids(&ix, "chxxxus").is_empty());
    assert_eq!(ids(&ix, "chxxgus"), strs(&["chungus"]));
    assert!(ids(&ix, "car").is_empty());
    assert!(ids(&ix, "dgo").is_empty());
    assert!(ids(&ix, "2025").is_empty());
}

#[test]
fn fuzzy_finds_every_single_edit() {
    // Property: the trigram pre-filter never hides a word one edit away.
    let words = [
        "chungus",
        "sleepy",
        "wholesome",
        "tired",
        "cringe",
        "sarcastic",
        "kitten",
        "кішка",
        "frog",
    ];
    let ix = index(words.iter().map(|w| named(w, w)));
    for w in words {
        let c: Vec<char> = w.chars().collect();
        let mut variants: Vec<String> = Vec::new();
        for i in 0..c.len() {
            let mut sub = c.clone();
            sub[i] = 'x';
            variants.push(sub.iter().collect());
            let mut del = c.clone();
            del.remove(i);
            variants.push(del.iter().collect());
            let mut ins = c.clone();
            ins.insert(i, 'x');
            variants.push(ins.iter().collect());
            if i + 1 < c.len() {
                let mut tr = c.clone();
                tr.swap(i, i + 1);
                variants.push(tr.iter().collect());
            }
        }
        for v in variants {
            if v.chars().count() < 4 {
                continue;
            }
            assert!(ids(&ix, &v).contains(&w.to_string()), "{v:?} should find {w:?}");
        }
    }
}

#[test]
fn prefix_and_substring() {
    let ix = index([named("chungus", "Big Chungus"), named("other", "chat")]);
    let o = run(&ix, "chun", &alice(), &SearchOptions::default());
    assert_eq!(o.hits.len(), 1);
    assert_eq!(o.hits[0].matched, vec!["name"]);
    // Two characters are enough for a prefix; one is not.
    assert_eq!(ids(&ix, "ch").len(), 2);
    assert!(ids(&ix, "c").is_empty());
    // Substring inside a token (>= 3 chars).
    assert_eq!(ids(&ix, "ungu"), strs(&["chungus"]));
    assert_eq!(ids(&ix, "gus"), strs(&["chungus"]));
    assert!(ids(&ix, "us").is_empty());
}

#[test]
fn phrases_match_token_sequences() {
    let ix = index([
        named("p1", "Big Chungus"),
        with("p2", |d| d.keywords = strs(&["chungus", "big"])),
        named("p3", "big fat chungus"),
    ]);
    assert_eq!(ids(&ix, "\"big chungus\""), strs(&["p1"]));
    assert_eq!(ids(&ix, "big-chungus"), strs(&["p1"]));
    assert_eq!(ids(&ix, "\"big chun\""), strs(&["p1"]));
    assert!(ids(&ix, "\"chungus big\"").is_empty());
    let o = run(&ix, "\"big chungs\"", &alice(), &SearchOptions::default());
    assert_eq!(o.hits.len(), 1);
    assert_eq!(o.hits[0].matched, vec!["name", "fuzzy"]);
    // Unquoted words match independently.
    assert_eq!(sorted_ids(&ix, "big chungus", &alice()), strs(&["p1", "p2", "p3"]));
}

#[test]
fn cyrillic_matching() {
    let ix = index([
        named("kit", "Кіт у мішку"),
        with("yizhak", |d| d.keywords = strs(&["Їжак"])),
        named("yolka", "Ёлка"),
        named("kyt", "кит"),
    ]);
    assert_eq!(ids(&ix, "кіт"), strs(&["kit"]));
    assert_eq!(ids(&ix, "КІТ"), strs(&["kit"]));
    assert_eq!(ids(&ix, "мішк"), strs(&["kit"]));
    assert_eq!(ids(&ix, "їжак"), strs(&["yizhak"]));
    assert_eq!(ids(&ix, "елка"), strs(&["yolka"]));
    assert_eq!(ids(&ix, "ёлка"), strs(&["yolka"]));
    // One letter off: fuzzy, labelled.
    let o = run(&ix, "іжак", &alice(), &SearchOptions::default());
    assert_eq!(o.hits[0].item_id, "yizhak");
    assert!(o.hits[0].matched.contains(&"fuzzy"));
}

#[test]
fn diacritics_fold_both_ways() {
    let ix = index([named("accent", "Café time"), named("plain", "cafe")]);
    for q in ["cafe", "café", "CAFÉ"] {
        assert_eq!(sorted_ids(&ix, q, &alice()), strs(&["accent", "plain"]), "{q}");
    }
}

#[test]
fn emoji_as_text() {
    let ix = index([
        with("field", |d| d.emojis = strs(&["😂"])),
        named("name", "lol 😂"),
        with("cat", |d| d.emojis = strs(&["🐱"])),
        with("heart", |d| d.emojis = strs(&["❤️"])),
        with("thumb", |d| d.emojis = strs(&["👍🏽"])),
    ]);
    let o = run(&ix, "😂", &alice(), &SearchOptions::default());
    assert_eq!(o.total, 2);
    assert_eq!(hit(&o, "field").matched, vec!["emoji"]);
    assert_eq!(hit(&o, "name").matched, vec!["name"]);
    // Emoji field outranks the name containing the same emoji among other words.
    assert_eq!(ids(&ix, "❤"), strs(&["heart"]));
    assert_eq!(ids(&ix, "❤️"), strs(&["heart"]));
    assert_eq!(ids(&ix, "👍"), strs(&["thumb"]));
    // Emoji vocabulary: words reach the emoji.
    let o = run(&ix, "laugh", &alice(), &SearchOptions::default());
    assert_eq!(
        o.hits.iter().map(|h| h.item_id.as_str()).collect::<Vec<_>>(),
        vec!["field"]
    );
    assert_eq!(o.hits[0].matched, vec!["emoji"]);
    assert_eq!(ids(&ix, "kitty"), strs(&["cat"]));
    assert_eq!(ids(&ix, "\"thumbs up\""), strs(&["thumb"]));
    // Emoji names are not "contained text" for exclusions.
    assert_eq!(
        sorted_ids(&ix, "-cat", &alice()),
        strs(&["cat", "field", "heart", "name", "thumb"])
    );
}

// ---------------------------------------------------------------- ranking

#[test]
fn ranking_sanity() {
    // Exact name beats a keyword substring.
    let ix = index([
        with("kw", |d| d.keywords = strs(&["concatenate"])),
        named("name", "cat"),
    ]);
    assert_eq!(ids(&ix, "cat"), strs(&["name", "kw"]));

    // Field weights: name > keyword > tag > vibe > set for the same exact token.
    let ix = index([
        with("set", |d| (d.set_name, d.set_slug) = ("Wow".into(), "wow".into())),
        with("vibe", |d| d.vibe = Some("wow".into())),
        with("tag", |d| d.tags = strs(&["wow"])),
        with("kw", |d| d.keywords = strs(&["wow"])),
        named("name", "wow"),
    ]);
    assert_eq!(ids(&ix, "wow"), strs(&["name", "kw", "tag", "vibe", "set"]));

    // Match quality: exact > squash > prefix > fuzzy > substring.
    let ix = index([
        named("substring", "megachungus"),
        named("fuzzy", "chungas"),
        named("prefix", "chungusland"),
        named("squash", "chunguss"),
        named("exact", "chungus"),
    ]);
    assert_eq!(
        ids(&ix, "chungus"),
        strs(&["exact", "squash", "prefix", "fuzzy", "substring"])
    );

    // Coordination: matching more query words wins.
    let ix = index([
        named("one", "cat"),
        named("both", "sad cat"),
        with("split", |d| {
            d.name = "cat".into();
            d.vibe = Some("sad".into())
        }),
    ]);
    assert_eq!(ids(&ix, "sad cat"), strs(&["both", "split", "one"]));

    // Rare words weigh more than common ones (IDF).
    let mut docs: Vec<Doc> = (0..20).map(|i| named(&format!("common{i:02}"), "meme")).collect();
    docs.push(named("rare", "chungus"));
    docs.push(named("both", "meme chungus"));
    let ix = index(docs);
    let got = ids(&ix, "meme chungus");
    assert_eq!(got[0], "both");
    assert_eq!(got[1], "rare");

    // Scores are non-increasing in relevance order.
    let o = run(
        &ix,
        "meme chungus",
        &alice(),
        &SearchOptions {
            limit: 100,
            ..Default::default()
        },
    );
    assert!(o.hits.windows(2).all(|w| w[0].score >= w[1].score));
}

#[test]
fn text_ties_break_by_copies_then_recency_then_id() {
    let ix = index([
        with("b", |d| (d.name, d.copies, d.updated_at) = ("cat".into(), 1, 5)),
        with("a", |d| (d.name, d.copies, d.updated_at) = ("cat".into(), 1, 5)),
        with("new", |d| (d.name, d.copies, d.updated_at) = ("cat".into(), 1, 9)),
        with("used", |d| (d.name, d.copies, d.updated_at) = ("cat".into(), 7, 0)),
    ]);
    assert_eq!(ids(&ix, "cat"), strs(&["used", "new", "a", "b"]));
}

// ---------------------------------------------------------------- vector

#[test]
fn vector_hits_and_fusion() {
    let ix = index([
        with("img", |d| d.embedding = emb(&[1.0, 0.0, 0.0])),
        with("lex", |d| {
            d.name = "cat".into();
            d.embedding = emb(&[0.0, 1.0, 0.0]);
        }),
        with("near", |d| d.embedding = emb(&[0.8, 0.6, 0.0])),
        with("both", |d| {
            d.name = "cat".into();
            d.embedding = emb(&[0.9, 0.1, 0.0]);
        }),
        with("hidden", |d| {
            d.owner_id = "bob".into();
            d.embedding = emb(&[1.0, 0.0, 0.0]);
        }),
        with("dim", |d| d.embedding = emb(&[1.0, 0.0])),
        with("filtered", |d| {
            d.embedding = emb(&[1.0, 0.0, 0.0]);
            d.format = MediaFormat::Gif;
        }),
    ]);
    let opts = SearchOptions {
        text_embedding: Some(vec![2.0, 0.0, 0.0]),
        min_similarity: 0.5,
        ..Default::default()
    };
    let o = run(&ix, "cat -type:gif", &alice(), &opts);
    let got: Vec<&str> = o.hits.iter().map(|h| h.item_id.as_str()).collect();
    assert_eq!(got[0], "both", "found by both sides ranks first: {got:?}");
    let mut rest = got[1..].to_vec();
    rest.sort();
    assert_eq!(rest, vec!["img", "lex", "near"]);
    assert_eq!(o.total, 4);
    assert_eq!(hit(&o, "both").matched, vec!["name", "image"]);
    assert_eq!(hit(&o, "img").matched, vec!["image"]);
    assert_eq!(hit(&o, "lex").matched, vec!["name"]);
    // A higher threshold drops weak image matches.
    let strict = SearchOptions {
        min_similarity: 0.9,
        ..opts.clone()
    };
    let mut got = ids_opts(&ix, "cat -type:gif", &alice(), &strict);
    got.sort();
    assert_eq!(got, strs(&["both", "img", "lex"]));
    // Vector-only hits for a word nobody used.
    assert_eq!(sorted_ids_opts(&ix, "zebra -type:gif", &strict), strs(&["both", "img"]));
    // Filter-only queries ignore the embedding.
    assert_eq!(run(&ix, "", &alice(), &opts).total, 6);
    // set_embedding updates the vector side.
    let mut ix = ix;
    ix.set_embedding("media-lex", emb(&[1.0, 0.0, 0.0]));
    let o = run(&ix, "cat -type:gif", &alice(), &strict);
    assert_eq!(hit(&o, "lex").matched, vec!["name", "image"]);
    ix.set_embedding("media-img", None);
    assert!(!ids_opts(&ix, "zebra", &alice(), &strict).contains(&"img".to_string()));
}

fn sorted_ids_opts(ix: &Index, q: &str, opts: &SearchOptions) -> Vec<String> {
    let mut v = ids_opts(ix, q, &alice(), opts);
    v.sort();
    v
}

#[test]
fn vector_order_follows_similarity() {
    let ix = index([
        with("far", |d| d.embedding = emb(&[0.6, 0.8])),
        with("close", |d| d.embedding = emb(&[0.99, 0.1])),
        with("mid", |d| d.embedding = emb(&[0.8, 0.6])),
    ]);
    let opts = SearchOptions {
        text_embedding: Some(vec![1.0, 0.0]),
        min_similarity: 0.1,
        ..Default::default()
    };
    assert_eq!(
        ids_opts(&ix, "anything", &alice(), &opts),
        strs(&["close", "mid", "far"])
    );
}

// ---------------------------------------------------------------- ordering and paging

fn sort_fixture() -> Index {
    index([
        with("old-popular", |d| {
            (d.name, d.likes, d.copies, d.updated_at, d.quality) = ("cat".into(), 1, 50, 10, Some(30));
        }),
        with("new", |d| {
            (d.name, d.likes, d.copies, d.updated_at, d.quality) = ("cat".into(), 2, 5, 99, None);
        }),
        with("liked", |d| {
            (d.name, d.likes, d.copies, d.updated_at, d.quality) = ("cat cat".into(), 40, 5, 50, Some(60));
        }),
        with("pretty", |d| {
            (d.name, d.likes, d.copies, d.updated_at, d.quality) = ("a cat".into(), 0, 0, 1, Some(99));
        }),
        with("unrelated", |d| (d.name, d.copies) = ("dog".into(), 1000)),
    ])
}

#[test]
fn sort_modes() {
    let ix = sort_fixture();
    let cases: &[(&str, &[&str])] = &[
        ("cat sort:recent", &["new", "liked", "old-popular", "pretty"]),
        ("cat sort:likes", &["liked", "new", "old-popular", "pretty"]),
        ("cat sort:copies", &["old-popular", "liked", "new", "pretty"]),
        ("cat sort:quality", &["pretty", "liked", "old-popular", "new"]),
        // Filter-only relevance: copies, likes, recency.
        ("", &["unrelated", "old-popular", "liked", "new", "pretty"]),
        ("sort:recent", &["new", "liked", "old-popular", "pretty", "unrelated"]),
        ("-dog sort:likes", &["liked", "new", "old-popular", "pretty"]),
    ];
    for (q, want) in cases {
        assert_eq!(ids(&ix, q), strs(want), "{q}");
    }
}

#[test]
fn browse_ties_use_likes_then_recency() {
    let ix = index([
        with("a", |d| (d.copies, d.likes, d.updated_at) = (3, 1, 1)),
        with("b", |d| (d.copies, d.likes, d.updated_at) = (3, 2, 0)),
        with("c", |d| (d.copies, d.likes, d.updated_at) = (3, 1, 7)),
        with("d", |d| (d.copies, d.likes, d.updated_at) = (3, 1, 7)),
    ]);
    assert_eq!(ids(&ix, ""), strs(&["b", "c", "d", "a"]));
}

#[test]
fn pagination_and_total() {
    let ix = index((0..120).map(|i| with(&format!("i{i:03}"), |d| (d.name, d.copies) = ("cat".into(), i))));
    for q in ["cat", ""] {
        let o = run(&ix, q, &alice(), &SearchOptions::default());
        assert_eq!((o.total, o.hits.len()), (120, 50), "{q}");
        let o = run(
            &ix,
            q,
            &alice(),
            &SearchOptions {
                offset: 100,
                limit: 50,
                ..Default::default()
            },
        );
        assert_eq!((o.total, o.hits.len()), (120, 20), "{q}");
        let o = run(
            &ix,
            q,
            &alice(),
            &SearchOptions {
                offset: 500,
                limit: 10,
                ..Default::default()
            },
        );
        assert_eq!((o.total, o.hits.len()), (120, 0), "{q}");
        let all = ids_opts(
            &ix,
            q,
            &alice(),
            &SearchOptions {
                limit: 1000,
                ..Default::default()
            },
        );
        let mut paged = Vec::new();
        for page in 0..13 {
            paged.extend(ids_opts(
                &ix,
                q,
                &alice(),
                &SearchOptions {
                    offset: page * 10,
                    limit: 10,
                    ..Default::default()
                },
            ));
        }
        assert_eq!(paged, all, "{q}");
        assert_eq!(all[0], "i119");
    }
    assert_eq!(run(&ix, "dog", &alice(), &SearchOptions::default()).total, 0);
}

#[test]
fn degenerate_queries() {
    let empty = Index::new();
    assert!(empty.is_empty());
    assert_eq!(
        run(&empty, "cat", &alice(), &SearchOptions::default()),
        Outcome::default()
    );
    assert_eq!(run(&empty, "", &alice(), &SearchOptions::default()), Outcome::default());
    let ix = index([named("a", "cat"), named("b", "dog")]);
    // Punctuation-only text behaves like a filter-only query.
    assert_eq!(ids(&ix, "- -").len(), 2);
    assert_eq!(ids(&ix, "cat cat"), ids(&ix, "cat"));
    assert!(ids(&ix, "zzzz").is_empty());
    // Many terms do not overflow the matched-term mask.
    let long: String = (0..40).map(|i| format!("w{i} ")).collect::<String>() + "cat";
    assert!(ids(&ix, &long).is_empty() || ids(&ix, &long) == strs(&["a"]));
}

// ---------------------------------------------------------------- maintenance

#[test]
fn index_is_shareable() {
    fn assert_send_sync<T: Send + Sync + Clone>() {}
    assert_send_sync::<Index>();
    assert_eq!(
        run(
            &index((0..60).map(|i| doc(&format!("{i}")))),
            "",
            &alice(),
            &SearchOptions::default()
        )
        .hits
        .len(),
        DEFAULT_LIMIT
    );
}

#[test]
fn upsert_replaces_old_tokens() {
    let mut ix = index([named("x", "grumpy cat")]);
    ix.upsert(named("x", "happy dog"));
    assert_eq!(ix.len(), 1);
    assert!(ids(&ix, "grumpy").is_empty());
    assert!(ids(&ix, "cat").is_empty());
    assert!(ids(&ix, "grum").is_empty());
    assert!(ids(&ix, "grumpi").is_empty());
    assert_eq!(ids(&ix, "happy"), strs(&["x"]));
    assert_eq!(ix.get("x").unwrap().name, "happy dog");
    // Old vocabulary is gone from every side index.
    assert!(!ix.store.vocab.contains_key("grumpy"));
    assert_eq!(ix.store.vocab_len(), 2 + 1); // happy, dog, misc (set)
}

#[test]
fn removal_cleans_everything() {
    let mut ix = index([
        with("a", |d| {
            d.name = "Sleepy cat 😴".into();
            d.keywords = strs(&["tired", "nap"]);
            d.emojis = strs(&["😴"]);
            d.tags = strs(&["reaction"]);
            d.vibe = Some("tired".into());
        }),
        with("b", |d| {
            d.name = "Other".into();
            (d.set_id, d.set_slug, d.set_name) = ("s2".into(), "dogs".into(), "Dogs".into());
        }),
        with("c", |d| {
            d.name = "Third cat".into();
            (d.set_id, d.set_slug, d.set_name) = ("s2".into(), "dogs".into(), "Dogs".into());
        }),
    ]);
    ix.remove("missing");
    assert_eq!(ix.len(), 3);
    ix.remove("a");
    assert_eq!(ix.len(), 2);
    assert!(ix.get("a").is_none());
    assert!(ids(&ix, "sleepy").is_empty());
    assert_eq!(ids(&ix, "cat"), strs(&["c"]));
    ix.remove_set("s2");
    assert!(ix.is_empty());
    assert!(ids(&ix, "").is_empty());
    let s = &ix.store;
    assert_eq!(s.vocab_len(), 0);
    assert!(s.squashed.is_empty());
    assert!(s.trigrams.is_empty());
    assert_eq!(s.field_len_sum, [0; store::NFIELDS]);
    assert_eq!(s.field_nonempty, [0; store::NFIELDS]);
    assert!(s.tokens.iter().all(|t| t.postings.is_empty() && t.df == 0));
    // Slots and token ids are reused.
    ix.upsert(named("d", "fresh cat"));
    assert_eq!(ids(&ix, "cat"), strs(&["d"]));
    assert_eq!(ix.store.slots.len(), 3);
}

#[test]
fn counters_and_embeddings_update_in_place() {
    let mut ix = index([
        with("a", |d| (d.name, d.media_id) = ("cat".into(), "shared".into())),
        with("b", |d| {
            (d.name, d.media_id) = ("cat".into(), "shared".into());
            d.set_id = "other".into();
        }),
        named("c", "cat"),
    ]);
    ix.set_counters("c", 7, 3);
    ix.set_counters("missing", 1, 1);
    assert_eq!(ids(&ix, "likes>5"), strs(&["c"]));
    assert_eq!(ids(&ix, "cat")[0], "c");
    assert_eq!(ix.get("c").map(|d| (d.likes, d.copies)), Some((7, 3)));
    ix.set_embedding("shared", emb(&[1.0, 0.0]));
    ix.set_embedding("missing", emb(&[1.0, 0.0]));
    let opts = SearchOptions {
        text_embedding: Some(vec![1.0, 0.0]),
        min_similarity: 0.5,
        ..Default::default()
    };
    let o = run(&ix, "zebra", &alice(), &opts);
    let mut got: Vec<&str> = o.hits.iter().map(|h| h.item_id.as_str()).collect();
    got.sort();
    assert_eq!(got, vec!["a", "b"]);
    // Re-upserting one doc keeps the media mapping of the other.
    ix.upsert(with("a", |d| (d.name, d.media_id) = ("cat".into(), "own".into())));
    ix.set_embedding("shared", None);
    assert!(ix.get("b").unwrap().embedding.is_none());
    assert!(ix.get("a").unwrap().embedding.is_none());
    ix.set_embedding("own", emb(&[1.0, 0.0]));
    assert_eq!(ids_opts(&ix, "zebra", &alice(), &opts), strs(&["a"]));
}

fn random_doc(rng: &mut Rng, id: usize, words: &[&str]) -> Doc {
    let pick = |n: usize, rng: &mut Rng| (0..n).map(|_| rng.pick(words).to_string()).collect::<Vec<_>>();
    let n = 1 + rng.below(3);
    let name = pick(n, rng).join(" ");
    let n = rng.below(4);
    let keywords = pick(n, rng);
    let n = rng.below(2);
    let tags = pick(n, rng);
    let set = rng.below(4);
    let vis = [Visibility::Private, Visibility::Unlisted, Visibility::Public][rng.below(3)];
    let owner = ["alice", "bob"][rng.below(2)];
    let emojis = [vec![], strs(&["😂"]), strs(&["🐱", "❤️"])][rng.below(3)].clone();
    let vibe = if rng.below(2) == 0 {
        Some(rng.pick(words).to_string())
    } else {
        None
    };
    let likes = rng.below(5) as u64;
    let updated_at = rng.below(1000) as i64;
    with(&format!("d{id}"), |d| {
        d.name = name;
        d.keywords = keywords;
        d.tags = tags;
        (d.set_id, d.set_slug, d.set_name) = (format!("s{set}"), format!("set-{set}"), format!("Set {}", words[set]));
        d.visibility = vis;
        d.owner_id = owner.into();
        d.emojis = emojis;
        d.vibe = vibe;
        d.likes = likes;
        d.updated_at = updated_at;
    })
}

#[test]
fn incremental_index_equals_fresh_rebuild() {
    let words = [
        "cat", "cats", "sleepy", "sleepi", "naah", "nah", "кіт", "café", "chungus", "big", "dog", "doge",
    ];
    let mut rng = Rng(0x5eed);
    let mut ix = Index::new();
    let mut live: std::collections::BTreeMap<String, Doc> = Default::default();
    for step in 0..600 {
        match rng.below(10) {
            0..=5 => {
                let id = rng.below(60);
                let d = random_doc(&mut rng, id, &words);
                live.insert(d.item_id.clone(), d.clone());
                ix.upsert(d);
            }
            6..=7 => {
                let id = format!("d{}", rng.below(60));
                live.remove(&id);
                ix.remove(&id);
            }
            8 => {
                let set = format!("s{}", rng.below(4));
                if step % 7 == 0 {
                    live.retain(|_, d| d.set_id != set);
                    ix.remove_set(&set);
                }
            }
            _ => {
                let id = format!("d{}", rng.below(60));
                let (likes, copies) = (rng.below(9) as u64, rng.below(9) as u64);
                if let Some(d) = live.get_mut(&id) {
                    (d.likes, d.copies) = (likes, copies);
                }
                ix.set_counters(&id, likes, copies);
            }
        }
    }
    let fresh = index(live.values().cloned());
    assert_eq!(ix.len(), fresh.len());
    assert_eq!(
        ix.store.vocab.keys().collect::<Vec<_>>(),
        fresh.store.vocab.keys().collect::<Vec<_>>()
    );
    assert_eq!(ix.store.field_len_sum, fresh.store.field_len_sum);
    let queries = [
        "cat",
        "sleepy cat",
        "naahh",
        "кіт",
        "cafe",
        "\"big chungus\"",
        "chungs",
        "do",
        "",
        "-cat",
        "e:❤ cat",
    ];
    for q in queries {
        for v in [alice(), viewer("bob")] {
            let opts = SearchOptions {
                limit: 1000,
                ..Default::default()
            };
            let a = run(&ix, q, &v, &opts);
            let b = run(&fresh, q, &v, &opts);
            assert_eq!(a.total, b.total, "{q}");
            let ai: Vec<_> = a.hits.iter().map(|h| (&h.item_id, &h.matched)).collect();
            let bi: Vec<_> = b.hits.iter().map(|h| (&h.item_id, &h.matched)).collect();
            assert_eq!(ai, bi, "{q}");
        }
    }
}

// ---------------------------------------------------------------- performance

fn synthetic_library(n: usize, dim: usize, rng: &mut Rng) -> (Vec<Doc>, Vec<String>) {
    let syll = [
        "ka", "to", "mi", "ne", "ru", "sa", "lo", "pe", "chu", "gus", "ba", "di", "fo", "zi", "wa", "qui", "ko", "ra",
        "te", "ni", "mo", "su", "ve", "li", "da", "ho", "je", "ya", "pu", "xe",
    ];
    let mut words: Vec<String> = (0..40_000)
        .map(|_| (0..2 + rng.below(3)).map(|_| *rng.pick(&syll)).collect::<String>())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    words.sort();
    rng.shuffle(&mut words);
    let emojis = ["😂", "😭", "🐱", "❤️", "👍🏽", "🔥", "😴", "🤔", "💀", "🥺"];
    let vibes = [
        "happy",
        "sad",
        "sarcastic",
        "angry",
        "tired",
        "cringe",
        "wholesome",
        "dismissive",
    ];
    let docs = (0..n)
        .map(|i| {
            let w = |k: usize, rng: &mut Rng| (0..k).map(|_| rng.pick(&words).clone()).collect::<Vec<_>>();
            let k = 1 + rng.below(3);
            let name = w(k, rng).join(" ");
            let k = 2 + rng.below(5);
            let keywords = w(k, rng);
            let k = rng.below(3);
            let tags = w(k, rng);
            let set = rng.below(300);
            let mut e: Vec<f32> = (0..dim).map(|_| rng.unit()).collect();
            let norm = e.iter().map(|x| x * x).sum::<f32>().sqrt();
            e.iter_mut().for_each(|x| *x /= norm);
            Doc {
                item_id: format!("item{i:06}"),
                set_id: format!("set{set}"),
                set_slug: format!("set-{set}"),
                set_name: format!("Set {}", words[set]),
                owner_id: format!("user{}", set % 20),
                owner_username: format!("User{}", set % 20),
                visibility: [Visibility::Private, Visibility::Unlisted, Visibility::Public][set % 3],
                name,
                keywords,
                emojis: (0..rng.below(3)).map(|_| rng.pick(&emojis).to_string()).collect(),
                vibe: Some(rng.pick(&vibes).to_string()),
                tags,
                kind: MediaKind::Animation,
                format: MediaFormat::Gif,
                quality: Some(rng.below(101) as u8),
                likes: rng.below(50) as u64,
                copies: rng.below(500) as u64,
                media_id: format!("media{i}"),
                embedding: Some(e.into()),
                updated_at: rng.below(1_000_000) as i64,
            }
        })
        .collect();
    (docs, words)
}

/// `cargo test -p stickpile-search --release -- --ignored --nocapture perf`
#[test]
#[ignore]
fn perf_synthetic_library() {
    let dim = 512;
    for n in [10_000, 50_000] {
        let mut rng = Rng(42);
        let (docs, words) = synthetic_library(n, dim, &mut rng);
        let start = Instant::now();
        let mut ix = Index::new();
        for d in docs {
            ix.upsert(d);
        }
        let build = start.elapsed();
        println!(
            "n={n}: indexed in {:.2?} ({:.0} docs/s), vocab {}",
            build,
            n as f64 / build.as_secs_f64(),
            ix.store.vocab_len()
        );
        // Two adjacent words of a real name, so the phrase query has hits.
        let v = viewer("user0");
        let (w0, w1) = (0..n)
            .find_map(|i| {
                let d = ix.get(&format!("item{i:06}")).filter(|d| d.owner_id == v.user_id)?;
                let mut it = d.name.split(' ');
                Some((it.next()?.to_string(), it.next()?.to_string()))
            })
            .unwrap();
        let (w0, w1) = (&w0, &w1);
        let mut typo: Vec<char> = w0.chars().collect();
        typo.swap(1, 2);
        let typo: String = typo.into_iter().collect();
        let queries = [
            ("exact word", w0.clone()),
            ("2-char prefix", w0.chars().take(2).collect()),
            ("3-char prefix", w0.chars().take(3).collect()),
            ("typo", typo),
            ("squash", format!("{w0}{}", w0.chars().last().unwrap())),
            ("two words", format!("{w0} {w1}")),
            ("phrase", format!("\"{w0} {w1}\"")),
            ("word + filters", format!("{w0} vibe:happy q>30 -tag:{w1}")),
            ("filter-only", "type:gif q>50".to_string()),
            ("browse all", String::new()),
        ];
        let mut qv: Vec<f32> = (0..dim).map(|_| rng.unit()).collect();
        let norm = qv.iter().map(|x| x * x).sum::<f32>().sqrt();
        qv.iter_mut().for_each(|x| *x /= norm);
        let with_vec = SearchOptions {
            text_embedding: Some(qv),
            min_similarity: 0.05,
            ..Default::default()
        };
        let expanded = SearchOptions {
            expansions: vec![("sad".into(), 0.6), ("tired".into(), 0.5), (words[7].clone(), 0.4)],
            ..Default::default()
        };
        let mut cases: Vec<(String, String, SearchOptions)> = queries
            .iter()
            .map(|(l, q)| (l.to_string(), q.clone(), SearchOptions::default()))
            .collect();
        cases.push(("word + embedding".into(), w0.clone(), with_vec));
        cases.push(("word + 3 expansions".into(), w0.clone(), expanded));
        for (label, q, opts) in cases {
            let query = Query::parse(&q);
            let runs = 20;
            let start = Instant::now();
            let mut total = 0;
            for _ in 0..runs {
                total = ix.search(&query, &v, &opts).total;
            }
            let per = start.elapsed() / runs;
            println!("  {label:<22} {q:<28} {per:>10.2?}  total={total}");
        }
    }
}
