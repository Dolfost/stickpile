//! Keyboard-first search syntax, shared by the server and offline client search.
//!
//! ```text
//! human e:😂 set:cats vibe:tired q>70 type:gif tag:reaction -tag:nsfw likes>=2 sort:likes
//! ```
//!
//! * bare words / `"quoted phrases"`: soft text terms, ranked (name, keywords,
//!   emojis, vibe, tags, set name, the words in the picture, image content)
//! * `e:` / `emoji:` - item has this emoji, or one that a name selects
//!   (`e:skull`: letters only; matched in `stickpile_search::emoji_names`)
//! * `set:` / `s:` - set slug (or name, case-insensitive); repeat for OR
//! * `vibe:` / `v:`, `tag:` / `t:` - exact (case-insensitive); repeat for OR
//! * `text:` / `x:` - the words written in the picture (the item's `text`):
//!   the value's words, consecutive (`text:"no thoughts"` is a phrase);
//!   repeat for OR
//! * `type:` - `still|image`, `animated|animation`, `video`, or a format
//!   (`png jpeg jpg webp gif mp4 webm`); repeat for OR
//! * `q` / `likes` / `copies` with `> >= < <= =` and a number - AND
//! * `by:` / `b:` / `user:` - owner username
//! * `is:mine | is:liked | is:public`
//! * `sort:relevance | recent | likes | copies | quality`
//! * a leading `-` negates a filter or a word
//!
//! Keys are case-insensitive; [`Key`] lists them with their one-letter
//! forms. The normalised form (`Query`'s `Display`) writes the long keys
//! (and `e:` for emojis). Unknown `key:value` tokens are plain text.

use std::fmt;

use crate::model::{MediaFormat, MediaKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
}

impl Cmp {
    pub fn test(self, lhs: u64, rhs: u64) -> bool {
        match self {
            Cmp::Gt => lhs > rhs,
            Cmp::Ge => lhs >= rhs,
            Cmp::Lt => lhs < rhs,
            Cmp::Le => lhs <= rhs,
            Cmp::Eq => lhs == rhs,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Cmp::Gt => ">",
            Cmp::Ge => ">=",
            Cmp::Lt => "<",
            Cmp::Le => "<=",
            Cmp::Eq => "=",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeFilter {
    Kind(MediaKind),
    Format(MediaFormat),
}

impl TypeFilter {
    pub fn matches(self, kind: MediaKind, format: MediaFormat) -> bool {
        match self {
            TypeFilter::Kind(k) => k == kind,
            TypeFilter::Format(f) => f == format,
        }
    }
    fn parse(s: &str) -> Option<TypeFilter> {
        Some(match s.to_ascii_lowercase().as_str() {
            "still" | "image" | "static" => TypeFilter::Kind(MediaKind::Still),
            "animated" | "animation" | "anim" => TypeFilter::Kind(MediaKind::Animation),
            "video" => TypeFilter::Kind(MediaKind::Video),
            "png" => TypeFilter::Format(MediaFormat::Png),
            "jpeg" | "jpg" => TypeFilter::Format(MediaFormat::Jpeg),
            "webp" => TypeFilter::Format(MediaFormat::Webp),
            "gif" => TypeFilter::Format(MediaFormat::Gif),
            "mp4" => TypeFilter::Format(MediaFormat::Mp4),
            "webm" => TypeFilter::Format(MediaFormat::Webm),
            _ => return None,
        })
    }
    fn as_str(self) -> &'static str {
        match self {
            TypeFilter::Kind(MediaKind::Still) => "still",
            TypeFilter::Kind(MediaKind::Animation) => "animated",
            TypeFilter::Kind(MediaKind::Video) => "video",
            TypeFilter::Format(f) => f.extension(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Is {
    Mine,
    Liked,
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Relevance,
    Recent,
    Likes,
    Copies,
    Quality,
}

/// The key of a `key:value` filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Emoji,
    Set,
    Vibe,
    Tag,
    By,
    /// The words written in the picture (the item's `text`).
    Text,
    Type,
    Is,
    Sort,
}

impl Key {
    pub const ALL: [Key; 9] = [
        Key::Emoji,
        Key::Set,
        Key::Vibe,
        Key::Tag,
        Key::By,
        Key::Text,
        Key::Type,
        Key::Is,
        Key::Sort,
    ];

    /// The key as written before the `:`, in any case: its long name, its
    /// one-letter form ([`Key::short`]) or an alias (`emoji`, `user`).
    pub fn parse(key: &str) -> Option<Key> {
        Some(match key.to_ascii_lowercase().as_str() {
            "e" | "emoji" => Key::Emoji,
            "s" | "set" => Key::Set,
            "v" | "vibe" => Key::Vibe,
            "t" | "tag" => Key::Tag,
            "b" | "by" | "user" => Key::By,
            "x" | "text" => Key::Text,
            "type" => Key::Type,
            "is" => Key::Is,
            "sort" => Key::Sort,
            _ => return None,
        })
    }

    /// The long name: `emoji`, `set`, `vibe`, `tag`, `by`, `text`, `type`,
    /// `is`, `sort`.
    pub fn name(self) -> &'static str {
        match self {
            Key::Emoji => "emoji",
            Key::Set => "set",
            Key::Vibe => "vibe",
            Key::Tag => "tag",
            Key::By => "by",
            Key::Text => "text",
            Key::Type => "type",
            Key::Is => "is",
            Key::Sort => "sort",
        }
    }

    /// The one-letter form of the keys that have one: `e` for `emoji`,
    /// `s` for `set`, `v` for `vibe`, `t` for `tag`, `b` for `by`, `x` for
    /// `text`.
    pub fn short(self) -> Option<&'static str> {
        match self {
            Key::Emoji => Some("e"),
            Key::Set => Some("s"),
            Key::Vibe => Some("v"),
            Key::Tag => Some("t"),
            Key::By => Some("b"),
            Key::Text => Some("x"),
            Key::Type | Key::Is | Key::Sort => None,
        }
    }
}

/// A filter value list: item must match any of `any` (if non-empty) and none of `none`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter<T> {
    pub any: Vec<T>,
    pub none: Vec<T>,
}

impl<T> Default for Filter<T> {
    fn default() -> Self {
        Filter {
            any: Vec::new(),
            none: Vec::new(),
        }
    }
}

impl<T: PartialEq> Filter<T> {
    pub fn is_empty(&self) -> bool {
        self.any.is_empty() && self.none.is_empty()
    }
    /// `pred(v)` tells whether the item has value `v`.
    pub fn accepts(&self, mut pred: impl FnMut(&T) -> bool) -> bool {
        (self.any.is_empty() || self.any.iter().any(&mut pred)) && !self.none.iter().any(pred)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    /// Soft terms (lowercased, phrases kept whole).
    pub text: Vec<String>,
    /// Words that must not appear in any text field.
    pub exclude_text: Vec<String>,
    /// The `e:` values as typed: emojis (`😂`) or emoji names (`skull`),
    /// which the search turns into the emojis they select.
    pub emojis: Filter<String>,
    /// Lowercased set slugs/names.
    pub sets: Filter<String>,
    pub vibes: Filter<String>,
    pub tags: Filter<String>,
    /// `text:` values, lowercased: each is a run of words that must appear,
    /// consecutive, in the item's text (after normalisation; matched in
    /// stickpile-search).
    pub texts: Filter<String>,
    pub types: Filter<TypeFilter>,
    pub owners: Filter<String>,
    pub is: Filter<Is>,
    pub quality: Vec<(Cmp, u8)>,
    pub likes: Vec<(Cmp, u64)>,
    pub copies: Vec<(Cmp, u64)>,
    pub sort: Sort,
}

impl Query {
    pub fn parse(input: &str) -> Query {
        let mut q = Query::default();
        for raw in tokenize(input) {
            let (neg, tok) = match raw.strip_prefix('-') {
                Some(rest) if !rest.is_empty() && !raw.quoted => (true, rest.to_string()),
                _ => (false, raw.text.clone()),
            };
            if raw.quoted {
                push_text(&mut q, neg, &tok);
                continue;
            }
            if let Some((key, cmp, val)) = split_comparison(&tok).filter(|_| !neg) {
                let ok = match key.as_str() {
                    "q" | "quality" => val
                        .parse::<u8>()
                        .ok()
                        .filter(|v| *v <= 100)
                        .map(|v| q.quality.push((cmp, v))),
                    "likes" | "like" => val.parse::<u64>().ok().map(|v| q.likes.push((cmp, v))),
                    "copies" | "used" => val.parse::<u64>().ok().map(|v| q.copies.push((cmp, v))),
                    _ => None,
                };
                if ok.is_some() {
                    continue;
                }
            }
            if let Some((key, val)) = tok.split_once(':') {
                if !val.is_empty() {
                    let lower = val.to_lowercase();
                    let handled = match Key::parse(key) {
                        Some(Key::Emoji) => push(&mut q.emojis, neg, val.to_string()),
                        Some(Key::Set) => push(&mut q.sets, neg, lower),
                        Some(Key::Vibe) => push(&mut q.vibes, neg, lower),
                        Some(Key::Tag) => push(&mut q.tags, neg, lower),
                        Some(Key::By) => push(&mut q.owners, neg, lower),
                        Some(Key::Text) => push(&mut q.texts, neg, lower),
                        Some(Key::Type) => TypeFilter::parse(val).map(|t| push(&mut q.types, neg, t)).is_some(),
                        Some(Key::Is) => match lower.as_str() {
                            "mine" => push(&mut q.is, neg, Is::Mine),
                            "liked" => push(&mut q.is, neg, Is::Liked),
                            "public" => push(&mut q.is, neg, Is::Public),
                            _ => false,
                        },
                        Some(Key::Sort) => {
                            let s = match lower.as_str() {
                                "relevance" => Some(Sort::Relevance),
                                "recent" | "new" => Some(Sort::Recent),
                                "likes" | "liked" => Some(Sort::Likes),
                                "copies" | "used" | "popular" => Some(Sort::Copies),
                                "quality" | "q" => Some(Sort::Quality),
                                _ => None,
                            };
                            s.map(|s| q.sort = s).is_some()
                        }
                        None => false,
                    };
                    if handled {
                        continue;
                    }
                }
            }
            push_text(&mut q, neg, &tok);
        }
        q
    }

    /// True if the query has no soft terms (pure filter/browse query).
    pub fn is_filter_only(&self) -> bool {
        self.text.is_empty()
    }

    pub fn quality_ok(&self, quality: Option<u8>) -> bool {
        self.quality.is_empty()
            || quality.is_some_and(|v| self.quality.iter().all(|(c, n)| c.test(v as u64, *n as u64)))
    }
    pub fn likes_ok(&self, likes: u64) -> bool {
        self.likes.iter().all(|(c, n)| c.test(likes, *n))
    }
    pub fn copies_ok(&self, copies: u64) -> bool {
        self.copies.iter().all(|(c, n)| c.test(copies, *n))
    }
}

fn push<T: PartialEq>(f: &mut Filter<T>, neg: bool, v: T) -> bool {
    let list = if neg { &mut f.none } else { &mut f.any };
    if !list.contains(&v) {
        list.push(v);
    }
    true
}

fn push_text(q: &mut Query, neg: bool, tok: &str) {
    let t = tok.to_lowercase();
    if t.is_empty() {
        return;
    }
    if neg {
        q.exclude_text.push(t);
    } else {
        q.text.push(t);
    }
}

struct RawToken {
    text: String,
    quoted: bool,
}

impl RawToken {
    fn strip_prefix(&self, p: char) -> Option<&str> {
        self.text.strip_prefix(p)
    }
}

fn tokenize(input: &str) -> Vec<RawToken> {
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut buf = String::new();
        let mut quoted = false;
        // key:"quoted value" or "quoted phrase"
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            if c == '"' {
                chars.next();
                let whole_token_quoted = buf.is_empty();
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                    buf.push(c);
                }
                if whole_token_quoted {
                    quoted = true;
                }
                continue;
            }
            buf.push(c);
            chars.next();
        }
        if !buf.is_empty() {
            out.push(RawToken { text: buf, quoted });
        }
    }
    out
}

fn split_comparison(tok: &str) -> Option<(String, Cmp, String)> {
    let idx = tok.find(['>', '<', '='])?;
    let key = &tok[..idx];
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let rest = &tok[idx..];
    let (cmp, val) = if let Some(v) = rest.strip_prefix(">=") {
        (Cmp::Ge, v)
    } else if let Some(v) = rest.strip_prefix("<=") {
        (Cmp::Le, v)
    } else if let Some(v) = rest.strip_prefix('>') {
        (Cmp::Gt, v)
    } else if let Some(v) = rest.strip_prefix('<') {
        (Cmp::Lt, v)
    } else if let Some(v) = rest.strip_prefix('=') {
        (Cmp::Eq, v)
    } else {
        return None;
    };
    Some((key.to_ascii_lowercase(), cmp, val.to_string()))
}

fn quote(s: &str) -> String {
    if s.chars().any(char::is_whitespace) {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

impl fmt::Display for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        parts.extend(self.text.iter().map(|t| quote(t)));
        parts.extend(self.exclude_text.iter().map(|t| format!("-{}", quote(t))));
        let mut filt = |key: &str, fl: &Filter<String>| {
            parts.extend(fl.any.iter().map(|v| format!("{key}:{}", quote(v))));
            parts.extend(fl.none.iter().map(|v| format!("-{key}:{}", quote(v))));
        };
        filt("e", &self.emojis);
        filt("set", &self.sets);
        filt("vibe", &self.vibes);
        filt("tag", &self.tags);
        filt("text", &self.texts);
        filt("by", &self.owners);
        parts.extend(self.types.any.iter().map(|t| format!("type:{}", t.as_str())));
        parts.extend(self.types.none.iter().map(|t| format!("-type:{}", t.as_str())));
        let is_str = |i: &Is| match i {
            Is::Mine => "mine",
            Is::Liked => "liked",
            Is::Public => "public",
        };
        parts.extend(self.is.any.iter().map(|i| format!("is:{}", is_str(i))));
        parts.extend(self.is.none.iter().map(|i| format!("-is:{}", is_str(i))));
        parts.extend(self.quality.iter().map(|(c, n)| format!("q{}{n}", c.as_str())));
        parts.extend(self.likes.iter().map(|(c, n)| format!("likes{}{n}", c.as_str())));
        parts.extend(self.copies.iter().map(|(c, n)| format!("copies{}{n}", c.as_str())));
        match self.sort {
            Sort::Relevance => {}
            Sort::Recent => parts.push("sort:recent".into()),
            Sort::Likes => parts.push("sort:likes".into()),
            Sort::Copies => parts.push("sort:copies".into()),
            Sort::Quality => parts.push("sort:quality".into()),
        }
        write!(f, "{}", parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_syntax() {
        let q = Query::parse(
            r#"human e:😂 set:Cats vibe:tired q>70 type:gif tag:reaction -tag:nsfw likes>=2 sort:likes "big chungus""#,
        );
        assert_eq!(q.text, vec!["human", "big chungus"]);
        assert_eq!(q.emojis.any, vec!["😂"]);
        assert_eq!(q.sets.any, vec!["cats"]);
        assert_eq!(q.vibes.any, vec!["tired"]);
        assert_eq!(q.quality, vec![(Cmp::Gt, 70)]);
        assert_eq!(q.types.any, vec![TypeFilter::Format(MediaFormat::Gif)]);
        assert_eq!(q.tags.any, vec!["reaction"]);
        assert_eq!(q.tags.none, vec!["nsfw"]);
        assert_eq!(q.likes, vec![(Cmp::Ge, 2)]);
        assert_eq!(q.sort, Sort::Likes);
    }

    #[test]
    fn unknown_keys_are_text() {
        let q = Query::parse("http://x foo:bar type:nonsense");
        assert_eq!(q.text, vec!["http://x", "foo:bar", "type:nonsense"]);
    }

    #[test]
    fn quoted_filter_values() {
        let q = Query::parse(r#"set:"my cats" -naahh"#);
        assert_eq!(q.sets.any, vec!["my cats"]);
        assert_eq!(q.exclude_text, vec!["naahh"]);
        assert!(q.is_filter_only());
    }

    #[test]
    fn quality_bounds() {
        let q = Query::parse("q>=50 q<90");
        assert!(q.quality_ok(Some(50)));
        assert!(!q.quality_ok(Some(90)));
        assert!(!q.quality_ok(None));
        assert!(Query::parse("q>101").quality.is_empty());
    }

    #[test]
    fn filter_accepts() {
        let f = Filter {
            any: vec!["a".to_string()],
            none: vec!["b".to_string()],
        };
        assert!(f.accepts(|v| v == "a"));
        assert!(!f.accepts(|v| v == "b" || v == "a"));
        assert!(Filter::<String>::default().accepts(|_| false));
    }

    #[test]
    fn display_round_trips() {
        let src = r#"human "big chungus" -nope e:😂 set:"my cats" -tag:nsfw text:"no thoughts" type:animated is:liked q>70 likes>=2 sort:recent"#;
        let q = Query::parse(src);
        let again = Query::parse(&q.to_string());
        assert_eq!(q, again);
    }

    #[test]
    fn emoji_names_stay_as_typed() {
        let q = Query::parse("emoji:Skull -e:thumbs_up e:😂");
        assert_eq!(q.emojis.any, vec!["Skull", "😂"]);
        assert_eq!(q.emojis.none, vec!["thumbs_up"]);
        assert_eq!(q.to_string(), "e:Skull e:😂 -e:thumbs_up");
        assert_eq!(Query::parse(&q.to_string()), q);
    }

    #[test]
    fn short_keys_are_the_long_ones() {
        let short = Query::parse("v:tired t:reaction s:cats b:alice e:😂");
        let long = Query::parse("vibe:tired tag:reaction set:cats by:alice emoji:😂");
        assert_eq!(short, long);
        assert_eq!(short.vibes.any, vec!["tired"]);
        assert_eq!(short.tags.any, vec!["reaction"]);
        assert_eq!(short.sets.any, vec!["cats"]);
        assert_eq!(short.owners.any, vec!["alice"]);
        assert_eq!(short.emojis.any, vec!["😂"]);
        assert!(short.text.is_empty());
        assert_eq!(Query::parse("user:alice"), Query::parse("b:alice"), "user: stays");
        assert_eq!(
            Query::parse("V:Tired T:NSFW"),
            Query::parse("vibe:tired tag:nsfw"),
            "any case"
        );
    }

    #[test]
    fn short_keys_negated_and_quoted() {
        let q = Query::parse(r#"-v:tired -t:nsfw -s:"my cats" -b:bob v:"very sad" t:a t:b"#);
        assert_eq!(q.vibes.none, vec!["tired"]);
        assert_eq!(q.tags.none, vec!["nsfw"]);
        assert_eq!(q.sets.none, vec!["my cats"]);
        assert_eq!(q.owners.none, vec!["bob"]);
        assert_eq!(q.vibes.any, vec!["very sad"]);
        assert_eq!(q.tags.any, vec!["a", "b"], "repeated: either");
        assert!(q.text.is_empty() && q.exclude_text.is_empty());
        let q = Query::parse(r#""v:tired""#);
        assert_eq!(q.text, vec!["v:tired"], "a quoted token is a phrase");
        assert!(q.vibes.is_empty());
        let q = Query::parse("v: t:");
        assert_eq!(q.text, vec!["v:", "t:"], "no value: text");
    }

    #[test]
    fn short_keys_normalise_to_the_long_ones() {
        let q = Query::parse(r#"cat v:tired -t:nsfw s:"my cats" b:alice e:😂"#);
        assert_eq!(q.to_string(), r#"cat e:😂 set:"my cats" vibe:tired -tag:nsfw by:alice"#);
        assert_eq!(Query::parse(&q.to_string()), q);
    }

    #[test]
    fn keys_and_their_forms() {
        for key in Key::ALL {
            assert_eq!(Key::parse(key.name()), Some(key));
            assert_eq!(Key::parse(&key.name().to_uppercase()), Some(key));
            if let Some(s) = key.short() {
                assert_eq!(s.len(), 1);
                assert_eq!(Key::parse(s), Some(key));
            }
        }
        let shorts: Vec<_> = Key::ALL.iter().filter_map(|k| k.short()).collect();
        assert_eq!(shorts, ["e", "s", "v", "t", "b", "x"]);
        assert_eq!(Key::parse("user"), Some(Key::By));
        assert_eq!(Key::parse("x"), Some(Key::Text));
        assert_eq!(Key::parse("y"), None);
        assert_eq!(Key::parse("q"), None, "q is a comparison, not a filter key");
    }

    #[test]
    fn text_filter_parses_words_and_phrases() {
        let q = Query::parse(r#"cat text:"No Thoughts" -x:Кіт x:meow"#);
        assert_eq!(q.text, vec!["cat"], "a text: value is no soft term");
        assert_eq!(q.texts.any, vec!["no thoughts", "meow"]);
        assert_eq!(q.texts.none, vec!["кіт"]);
        assert_eq!(Query::parse("x:hi"), Query::parse("TEXT:hi"), "x: is text:, any case");
        assert!(Query::parse("text:hi").is_filter_only());
        // A quoted token stays a phrase of the soft terms.
        let q = Query::parse(r#""text:hi""#);
        assert_eq!(q.text, vec!["text:hi"]);
        assert!(q.texts.is_empty());
        assert_eq!(Query::parse("text: x:").text, vec!["text:", "x:"], "no value: text");
    }

    #[test]
    fn text_filter_displays_as_text() {
        let q = Query::parse(r#"x:"no thoughts" -x:meow cat t:a"#);
        assert_eq!(q.to_string(), r#"cat tag:a text:"no thoughts" -text:meow"#);
        assert_eq!(Query::parse(&q.to_string()), q);
    }

    #[test]
    fn lone_dash_is_text() {
        let q = Query::parse("- -");
        assert_eq!(q.text, vec!["-", "-"]);
    }

    /// Punctuation is a value and a word like any other: the search
    /// (stickpile-search) gives it its meaning.
    #[test]
    fn punctuation_values_and_words() {
        let q = Query::parse(r#"x:? x:... x:"what?" -x:?! ? ... what? -? x:…"#);
        assert_eq!(q.texts.any, vec!["?", "...", "what?", "…"]);
        assert_eq!(q.texts.none, vec!["?!"]);
        assert_eq!(q.text, vec!["?", "...", "what?"]);
        assert_eq!(q.exclude_text, vec!["?"]);
        assert_eq!(Query::parse("x:what?"), Query::parse(r#"x:"what?""#));
        assert_eq!(Query::parse(&q.to_string()), q);
    }
}
