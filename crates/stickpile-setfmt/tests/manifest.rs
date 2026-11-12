//! set.toml parsing and round-trip guarantees.

use stickpile_setfmt::{FieldSource, ItemMeta, Manifest, SetFmtError, Visibility};

const EXAMPLE: &str = r#"
# A comment that is not preserved (comments are not data).
version = 3
name = "My cats"
description = "Cats, mostly staring."
visibility = "unlisted"
aspect_ratio = "1:1"
recommended_px = 512
author = "someone"                 # unknown top-level key
created = 2024-05-01T12:00:00Z     # unknown datetime

[upstream]                          # unknown top-level table
homepage = "https://example.org"
mirrors = ["a", "b"]

[items."001-cat.png"]
name = "cat"
keywords = ["cat", "stare"]
keywords_source = "ai"
emojis = ["😼"]
vibe = "smug"
vibe_source = "ai"
tags = ["reaction"]
rating = 5                          # unknown per-item key
source_url = "https://example.org/cat"

[items."001-cat.png".history]
first_seen = 2020-01-01

[[items."001-cat.png".credits]]
who = "a"

[[items."001-cat.png".credits]]
who = "b"

[items."002-naahh.gif"]
name = "naahh"
name_source = "ai"
emojis = ["🙅", "😤"]
emojis_source = "ai"
"#;

#[test]
fn example_parses() {
    let m = Manifest::parse(EXAMPLE).unwrap();
    assert_eq!(m.version, 3);
    assert_eq!(m.name, "My cats");
    assert_eq!(m.visibility, Visibility::Unlisted);
    assert_eq!(m.aspect(), Some((1, 1)));
    assert_eq!(m.recommended_px, Some(512));
    assert_eq!(m.extra["author"].as_str(), Some("someone"));
    assert!(m.extra["created"].as_datetime().is_some());
    assert!(m.extra["upstream"].is_table());
    let cat = &m.items["001-cat.png"];
    assert_eq!(cat.keywords, ["cat", "stare"]);
    assert_eq!(cat.keywords_source, FieldSource::Ai);
    assert_eq!(cat.name_source, FieldSource::User);
    assert_eq!(cat.vibe.as_deref(), Some("smug"));
    assert_eq!(cat.vibe_source, FieldSource::Ai);
    assert_eq!(cat.extra["rating"].as_integer(), Some(5));
    assert_eq!(cat.extra["credits"].as_array().unwrap().len(), 2);
    let naahh = &m.items["002-naahh.gif"];
    assert_eq!(naahh.name_source, FieldSource::Ai);
    assert_eq!(naahh.emojis_source, FieldSource::Ai);
    assert_eq!(naahh.keywords_source, FieldSource::User);
}

#[test]
fn example_round_trips() {
    let m = Manifest::parse(EXAMPLE).unwrap();
    let text = m.to_toml();
    let again = Manifest::parse(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    assert_eq!(again, m, "{text}");
    assert_eq!(again.to_toml(), text, "serialising twice must give identical text");
    // AI markers and unknown keys are in the output.
    for needle in [
        "keywords_source = \"ai\"",
        "vibe_source = \"ai\"",
        "name_source = \"ai\"",
        "emojis_source = \"ai\"",
        "author = \"someone\"",
        "rating = 5",
        "first_seen = 2020-01-01",
        "homepage = ",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in\n{text}");
    }
    // User sources are the default and not written.
    assert!(!text.contains("\"user\""), "{text}");
}

#[test]
fn golden_output() {
    let mut m = Manifest::new("My cats");
    m.version = 2;
    m.visibility = Visibility::Public;
    m.set_aspect(Some((1, 1)));
    m.recommended_px = Some(512);
    m.items.insert(
        "001-cat.png".into(),
        ItemMeta {
            name: "cat".into(),
            keywords: vec!["cat".into(), "stare".into()],
            keywords_source: FieldSource::Ai,
            emojis: vec!["😼".into()],
            ..Default::default()
        },
    );
    m.items.insert(
        "002-dog.gif".into(),
        ItemMeta {
            name: "dog".into(),
            ..Default::default()
        },
    );
    let want = r#"version = 2
name = "My cats"
visibility = "public"
aspect_ratio = "1:1"
recommended_px = 512

[items."001-cat.png"]
name = "cat"
keywords = ["cat", "stare"]
keywords_source = "ai"
emojis = ["😼"]

[items."002-dog.gif"]
name = "dog"
"#;
    assert_eq!(m.to_toml(), want);
}

#[test]
fn minimal_and_defaults() {
    let m = Manifest::parse("name = \"x\"").unwrap();
    assert_eq!(m.version, 1);
    assert_eq!(m.visibility, Visibility::Private);
    assert!(m.items.is_empty());
    assert_eq!(Manifest::parse(&m.to_toml()).unwrap(), m);

    // A missing name is allowed (the readers fill it from the directory name).
    let m = Manifest::parse("").unwrap();
    assert_eq!(m.name, "");
    assert_eq!(Manifest::parse(&m.to_toml()).unwrap(), m);

    let empty = Manifest::new("e");
    assert_eq!(empty.version, 1);
    assert_eq!(Manifest::parse(&empty.to_toml()).unwrap(), empty);
}

#[test]
fn parse_errors() {
    for bad in [
        "name = ",
        "name = \"x\"\nvisibility = \"secret\"",
        "name = \"x\"\nversion = -1",
        "name = \"x\"\n[items.\"a.png\"]\nkeywords_source = \"robot\"",
        "name = \"x\"\n[items.\"a.png\"]\nkeywords = \"not an array\"",
        "name = 5",
    ] {
        match Manifest::parse(bad) {
            Err(SetFmtError::Parse(msg)) => assert!(!msg.is_empty()),
            other => panic!("{bad:?} -> {other:?}"),
        }
    }
}

#[test]
fn shadowing_extra_keys_and_huge_version_do_not_panic() {
    let mut m = Manifest::new("x");
    m.extra.insert("name".into(), toml::Value::String("shadow".into()));
    m.extra.insert("items".into(), toml::Value::Integer(1));
    m.version = u64::MAX;
    let mut item = ItemMeta::default();
    item.extra.insert("keywords".into(), toml::Value::Integer(1));
    m.items.insert("a.png".into(), item);
    let back = Manifest::parse(&m.to_toml()).unwrap();
    assert_eq!(back.name, "x");
    assert_eq!(back.version, i64::MAX as u64);
    assert!(back.extra.is_empty());
    assert!(back.items["a.png"].extra.is_empty());
}

// ---------------------------------------------------------------------------
// Property-style round trip over generated manifests
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

const FRAGMENTS: &[&str] = &[
    "cat",
    "Ünïcödé",
    "😼",
    "\"quoted\"",
    "back\\slash",
    "new\nline",
    "tab\t",
    "'single'",
    "#hash",
    "[bracket]",
    " = ",
    " ",
    "",
    "ключ",
    "\u{7f}",
    "a.b",
    "\r\n",
    "'''",
    "\"\"\"",
    "🙅‍♂️",
];

const KNOWN: &[&str] = &[
    "version",
    "name",
    "slug",
    "description",
    "visibility",
    "aspect_ratio",
    "recommended_px",
    "items",
    "name_source",
    "keywords",
    "keywords_source",
    "emojis",
    "emojis_source",
    "vibe",
    "vibe_source",
    "tags",
];

fn string(rng: &mut Rng) -> String {
    (0..rng.below(4)).map(|_| *rng.pick(FRAGMENTS)).collect()
}

fn strings(rng: &mut Rng) -> Vec<String> {
    (0..rng.below(4)).map(|_| string(rng)).collect()
}

fn key(rng: &mut Rng) -> String {
    loop {
        let k = string(rng);
        if !KNOWN.contains(&k.as_str()) {
            return k;
        }
    }
}

fn value(rng: &mut Rng, depth: u32) -> toml::Value {
    use toml::Value;
    let kinds = if depth >= 2 { 5 } else { 7 };
    match rng.below(kinds) {
        0 => Value::String(string(rng)),
        1 => Value::Integer(*rng.pick(&[0, 1, -1, 42, i64::MAX, i64::MIN, 1 << 40])),
        2 => Value::Float(*rng.pick(&[0.5, -1.25e10, 3.0, 1e-7, f64::INFINITY, -0.0, 6.02e23])),
        3 => Value::Boolean(rng.chance(50)),
        4 => Value::Datetime(
            rng.pick(&[
                "1979-05-27T07:32:00Z",
                "1979-05-27",
                "07:32:00",
                "1979-05-27T00:32:00.999999-07:00",
            ])
            .parse()
            .unwrap(),
        ),
        5 => Value::Array((0..rng.below(4)).map(|_| value(rng, depth + 1)).collect()),
        _ => Value::Table(table(rng, depth + 1)),
    }
}

fn table(rng: &mut Rng, depth: u32) -> toml::Table {
    let mut t = toml::Table::new();
    for _ in 0..rng.below(4) {
        let k = if depth == 0 { key(rng) } else { string(rng) };
        t.insert(k, value(rng, depth));
    }
    t
}

fn source(rng: &mut Rng) -> FieldSource {
    if rng.chance(50) {
        FieldSource::Ai
    } else {
        FieldSource::User
    }
}

fn manifest(rng: &mut Rng) -> Manifest {
    let mut m = Manifest::new(string(rng));
    m.version = rng.next() >> 1;
    m.slug = rng.chance(50).then(|| string(rng));
    m.description = string(rng);
    m.visibility = *rng.pick(&[Visibility::Private, Visibility::Unlisted, Visibility::Public]);
    m.aspect_ratio = rng
        .chance(50)
        .then(|| if rng.chance(80) { "16:9".into() } else { string(rng) });
    m.recommended_px = rng.chance(50).then(|| rng.next() as u32);
    m.cover = rng.chance(30).then(|| string(rng));
    m.cover_image = rng.chance(30).then(|| string(rng));
    m.extra = table(rng, 0);
    for _ in 0..rng.below(6) {
        let filename = format!(
            "{:03}-{}.{}",
            rng.below(1000),
            string(rng),
            rng.pick(&["png", "gif", "webm"])
        );
        let item = ItemMeta {
            name: string(rng),
            name_source: source(rng),
            keywords: strings(rng),
            keywords_source: source(rng),
            emojis: strings(rng),
            emojis_source: source(rng),
            vibe: rng.chance(50).then(|| string(rng)),
            vibe_source: source(rng),
            tags: strings(rng),
            extra: table(rng, 0),
        };
        m.items.insert(filename, item);
    }
    m
}

#[test]
fn generated_manifests_round_trip() {
    let mut rng = Rng(0x5eed_cafe_f00d_d00d);
    for case in 0..500 {
        let m = manifest(&mut rng);
        let text = m.to_toml();
        let back = Manifest::parse(&text).unwrap_or_else(|e| panic!("case {case}: {e}\n{text}"));
        assert_eq!(back, m, "case {case}:\n{text}");
        assert_eq!(back.to_toml(), text, "case {case}: not stable");
    }
}
