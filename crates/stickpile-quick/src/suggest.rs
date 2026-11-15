//! Suggestions for the search field. While a `key:value` filter is typed
//! (`vibe:sar`), the values it can take are offered in one line under the
//! field, best first: the vibes, tags, packs and owners of the cached items
//! (most used first) and the fixed words of `type:`, `is:` and `sort:`.
//! A value is matched as typed: its start, then anywhere in it, then with a
//! typo or two forgiven, so `vibe:sarcstic` still offers `sarcastic`.
//!
//! The search itself goes out with every such value [`resolve`]d to its
//! best suggestion, so a half-typed or misspelled filter still finds the
//! stickers.

use std::collections::HashMap;

use stickpile_api::ItemSummary;

/// Values offered in the line at most.
pub const MAX_SUGGESTIONS: usize = 12;

/// The `type:` words (see `stickpile_api::query`).
pub const TYPE_WORDS: [&str; 9] = [
    "gif", "png", "webp", "jpeg", "mp4", "webm", "still", "animated", "video",
];
/// The `is:` words.
pub const IS_WORDS: [&str; 3] = ["mine", "liked", "public"];
/// The `sort:` words.
pub const SORT_WORDS: [&str; 5] = ["relevance", "recent", "likes", "copies", "quality"];

/// The values the viewer's items use, each list most used first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vocabulary {
    pub vibes: Vec<String>,
    pub tags: Vec<String>,
    /// Pack slugs, and the names that are one word.
    pub sets: Vec<String>,
    pub owners: Vec<String>,
}

impl Vocabulary {
    pub fn from_items<'a>(items: impl IntoIterator<Item = &'a ItemSummary>) -> Vocabulary {
        #[derive(Default)]
        struct Counts(HashMap<String, u32>);
        impl Counts {
            fn add(&mut self, value: &str) {
                let v = value.trim().to_lowercase();
                // A value with a space cannot be typed after a `key:`.
                if !v.is_empty() && !v.contains(char::is_whitespace) && !v.contains('"') {
                    *self.0.entry(v).or_default() += 1;
                }
            }
            fn ranked(self) -> Vec<String> {
                let mut v: Vec<(String, u32)> = self.0.into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                v.into_iter().map(|(w, _)| w).collect()
            }
        }
        let (mut vibes, mut tags, mut sets, mut owners) = (
            Counts::default(),
            Counts::default(),
            Counts::default(),
            Counts::default(),
        );
        for it in items {
            if let Some(v) = &it.vibe {
                vibes.add(v);
            }
            for t in &it.tags {
                tags.add(t);
            }
            sets.add(&it.set_slug);
            if !it.set_name.trim().eq_ignore_ascii_case(it.set_slug.trim()) {
                sets.add(&it.set_name);
            }
            owners.add(&it.owner_username);
        }
        Vocabulary {
            vibes: vibes.ranked(),
            tags: tags.ranked(),
            sets: sets.ranked(),
            owners: owners.ranked(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.vibes.is_empty() && self.tags.is_empty() && self.sets.is_empty() && self.owners.is_empty()
    }
}

/// A filter whose values are offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Vibe,
    Tag,
    Set,
    Owner,
    Type,
    Is,
    Sort,
}

impl Field {
    /// The field of a `key` as the query syntax spells it.
    pub fn of(key: &str) -> Option<Field> {
        Some(match key.to_ascii_lowercase().as_str() {
            "vibe" => Field::Vibe,
            "tag" => Field::Tag,
            "set" => Field::Set,
            "by" | "user" => Field::Owner,
            "type" => Field::Type,
            "is" => Field::Is,
            "sort" => Field::Sort,
            _ => return None,
        })
    }

    fn words(self, vocab: &Vocabulary) -> Vec<&str> {
        let fixed = |w: &[&'static str]| w.to_vec();
        match self {
            Field::Vibe => vocab.vibes.iter().map(String::as_str).collect(),
            Field::Tag => vocab.tags.iter().map(String::as_str).collect(),
            Field::Set => vocab.sets.iter().map(String::as_str).collect(),
            Field::Owner => vocab.owners.iter().map(String::as_str).collect(),
            Field::Type => fixed(&TYPE_WORDS),
            Field::Is => fixed(&IS_WORDS),
            Field::Sort => fixed(&SORT_WORDS),
        }
    }
}

/// What the line under the field offers for the filter being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestions {
    /// The key as typed, with its colon and a leading `-`: `vibe:`, `-tag:`.
    pub key: String,
    /// The value typed so far.
    pub typed: String,
    /// Best first; the search uses the first unless `typed` is one of them.
    pub words: Vec<String>,
    /// Byte offset in the query where the value starts.
    pub start: usize,
}

impl Suggestions {
    /// The word the search uses for this value: the typed one when it is a
    /// known value, else the best suggestion; none while nothing is typed
    /// (an empty filter is left out of the search).
    pub fn chosen(&self) -> Option<usize> {
        if self.typed.is_empty() {
            return None;
        }
        let typed = self.typed.to_lowercase();
        match self.words.iter().position(|w| *w == typed) {
            Some(i) => Some(i),
            None => (!self.words.is_empty()).then_some(0),
        }
    }
}

/// The suggestions for the last token of `query` when it is a `key:value`
/// filter of a [`Field`] being typed (no space after it yet).
pub fn suggest(query: &str, vocab: &Vocabulary) -> Option<Suggestions> {
    if query.ends_with(char::is_whitespace) || query.matches('"').count() % 2 == 1 {
        return None;
    }
    let token_start = query
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + query[i..].chars().next().map_or(1, char::len_utf8));
    let token = &query[token_start..];
    let bare = token.strip_prefix('-').unwrap_or(token);
    let (key, typed) = bare.split_once(':')?;
    let field = Field::of(key)?;
    if typed.contains('"') {
        return None;
    }
    let words = ranked(typed, &field.words(vocab));
    if words.is_empty() {
        return None;
    }
    let start = query.len() - typed.len();
    Some(Suggestions {
        key: token[..token.len() - typed.len()].to_string(),
        typed: typed.to_string(),
        words: words.into_iter().take(MAX_SUGGESTIONS).map(str::to_string).collect(),
        start,
    })
}

/// `query` with every filter value of a [`Field`] that is not a known value
/// replaced by its best match, and empty filters (`vibe:`) left out; also
/// the replacements made, as (typed, used).
pub fn resolve(query: &str, vocab: &Vocabulary) -> (String, Vec<(String, String)>) {
    let mut out = String::with_capacity(query.len());
    let mut fixes = Vec::new();
    for (token, quoted) in tokens(query) {
        let resolved = (!quoted).then(|| resolve_token(token, vocab)).flatten();
        let piece = match resolved {
            Some(Resolved::Drop) => continue,
            Some(Resolved::Use(prefix, typed, word)) => {
                fixes.push((typed.to_string(), word.clone()));
                format!("{prefix}{word}")
            }
            None => token.to_string(),
        };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&piece);
    }
    (out, fixes)
}

enum Resolved<'a> {
    Drop,
    Use(&'a str, &'a str, String),
}

fn resolve_token<'a>(token: &'a str, vocab: &Vocabulary) -> Option<Resolved<'a>> {
    let bare = token.strip_prefix('-').unwrap_or(token);
    let (key, typed) = bare.split_once(':')?;
    let field = Field::of(key)?;
    if typed.is_empty() {
        return Some(Resolved::Drop);
    }
    let words = field.words(vocab);
    let lower = typed.to_lowercase();
    if words.is_empty() || words.contains(&lower.as_str()) {
        return None;
    }
    let best = ranked(typed, &words).into_iter().next()?;
    let prefix = &token[..token.len() - typed.len()];
    Some(Resolved::Use(prefix, typed, best.to_string()))
}

/// The query's whitespace-separated tokens; a token with a quote in it
/// (with the spaces inside the quotes) is marked quoted.
fn tokens(query: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut start = None;
    let mut in_quote = false;
    let mut quoted = false;
    for (i, c) in query.char_indices() {
        if c == '"' {
            in_quote = !in_quote;
            quoted = true;
        }
        if c.is_whitespace() && !in_quote {
            if let Some(s) = start.take() {
                out.push((&query[s..i], quoted));
            }
            quoted = false;
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push((&query[s..], quoted));
    }
    out
}

/// Typos forgiven in a value of `len` characters.
fn typo_budget(len: usize) -> usize {
    match len {
        0..=2 => 0,
        3..=5 => 1,
        _ => 2,
    }
}

/// `words` (in their order of use) that match `typed`, best first: those
/// starting with it (the same word first), those containing it, then those
/// a typo or two away, fewer typos first.
fn ranked<'a>(typed: &str, words: &[&'a str]) -> Vec<&'a str> {
    let typed: Vec<char> = typed.to_lowercase().chars().collect();
    if typed.is_empty() {
        return words.to_vec();
    }
    let budget = typo_budget(typed.len());
    let mut scored: Vec<(usize, usize, &'a str)> = Vec::new();
    for (n, w) in words.iter().enumerate() {
        let chars: Vec<char> = w.chars().collect();
        let tier = if chars == typed {
            0
        } else if chars.starts_with(&typed) {
            1
        } else if typed.len() >= 2 && chars.windows(typed.len()).any(|win| win == typed.as_slice()) {
            2
        } else {
            // The typed text against the word's start of about its length,
            // so a typo in a half-typed word counts as well.
            let lo = typed.len().saturating_sub(budget).max(1);
            let hi = (typed.len() + budget).min(chars.len());
            match (lo..=hi).filter_map(|k| osa(&typed, &chars[..k], budget)).min() {
                Some(d) => 3 + d,
                None => continue,
            }
        };
        scored.push((tier, n, w));
    }
    scored.sort_unstable();
    scored.into_iter().map(|(_, _, w)| w).collect()
}

/// Optimal string alignment distance (Damerau-Levenshtein with adjacent
/// transpositions) when it is at most `max`.
fn osa(a: &[char], b: &[char], max: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    let w = b.len() + 1;
    let mut d = vec![0usize; (a.len() + 1) * w];
    for (j, v) in d.iter_mut().enumerate().take(w) {
        *v = j;
    }
    for i in 1..=a.len() {
        d[i * w] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[(i - 1) * w + j] + 1)
                .min(d[i * w + j - 1] + 1)
                .min(d[(i - 1) * w + j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[(i - 2) * w + j - 2] + 1);
            }
            d[i * w + j] = v;
        }
    }
    Some(d[a.len() * w + b.len()]).filter(|v| *v <= max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vocabulary {
        Vocabulary {
            vibes: ["sad", "sarcastic", "happy", "tired", "angry"]
                .map(String::from)
                .to_vec(),
            tags: ["reaction", "nsfw", "cat"].map(String::from).to_vec(),
            sets: ["cats", "work-memes"].map(String::from).to_vec(),
            owners: ["alice", "bob"].map(String::from).to_vec(),
        }
    }

    fn words(q: &str) -> Vec<String> {
        suggest(q, &vocab()).map(|s| s.words).unwrap_or_default()
    }

    #[test]
    fn a_key_alone_offers_every_value_most_used_first() {
        let s = suggest("cat vibe:", &vocab()).unwrap();
        assert_eq!(s.key, "vibe:");
        assert_eq!(s.typed, "");
        assert_eq!(s.words, ["sad", "sarcastic", "happy", "tired", "angry"]);
        assert_eq!(s.start, "cat vibe:".len());
        assert_eq!(s.chosen(), None, "nothing typed: the search has no vibe yet");
    }

    #[test]
    fn the_start_then_inside_then_typos() {
        assert_eq!(words("vibe:sa"), ["sad", "sarcastic"]);
        assert_eq!(words("vibe:sar"), ["sarcastic", "sad"], "sad is a typo away");
        assert_eq!(words("vibe:ppy"), ["happy"], "inside the word");
        assert_eq!(words("vibe:sarcstic"), ["sarcastic"], "a letter left out");
        assert_eq!(words("vibe:sacr"), ["sarcastic"], "half typed, two letters swapped");
        assert_eq!(words("vibe:tierd"), ["tired"], "swapped letters");
        assert_eq!(words("vibe:hapy"), ["happy"]);
        assert!(words("vibe:xyzzy").is_empty());
        assert!(words("vibe:zz").is_empty(), "two letters forgive no typo");
    }

    #[test]
    fn the_same_word_comes_first_and_is_chosen() {
        let s = suggest("vibe:sad", &vocab()).unwrap();
        assert_eq!(s.words[0], "sad");
        assert_eq!(s.chosen(), Some(0));
        let s = suggest("vibe:SAD", &vocab()).unwrap();
        assert_eq!(s.chosen(), Some(0), "case is ignored");
    }

    #[test]
    fn every_field_and_negation() {
        assert_eq!(words("-tag:ns"), ["nsfw"]);
        assert_eq!(suggest("-tag:ns", &vocab()).unwrap().key, "-tag:");
        assert_eq!(words("set:wor"), ["work-memes"]);
        assert_eq!(words("by:al"), ["alice"]);
        assert_eq!(words("user:b"), ["bob"]);
        assert_eq!(words("type:gi"), ["gif"]);
        assert_eq!(words("is:li"), ["liked", "public"]);
        assert_eq!(words("sort:cop"), ["copies"]);
        assert_eq!(words("Vibe:ha"), ["happy"], "keys in any case");
    }

    #[test]
    fn nothing_outside_a_filter_being_typed() {
        assert!(suggest("", &vocab()).is_none());
        assert!(suggest("cat", &vocab()).is_none());
        assert!(suggest("vibe:sad ", &vocab()).is_none(), "the token is finished");
        assert!(suggest("e:x", &vocab()).is_none(), "no emoji list");
        assert!(suggest("\"vibe:sa", &vocab()).is_none(), "inside quotes");
        assert!(suggest("vibe:sa", &Vocabulary::default()).is_none(), "nothing known");
    }

    #[test]
    fn resolve_uses_the_best_match_and_drops_empty_filters() {
        let v = vocab();
        assert_eq!(resolve("cat vibe:sarcstic", &v).0, "cat vibe:sarcastic");
        assert_eq!(
            resolve("cat vibe:sarcstic", &v).1,
            [("sarcstic".to_string(), "sarcastic".to_string())]
        );
        assert_eq!(resolve("vibe:sar -tag:nsf", &v).0, "vibe:sarcastic -tag:nsfw");
        assert_eq!(
            resolve("vibe:sad tag:Cat", &v),
            ("vibe:sad tag:Cat".into(), vec![]),
            "known values stay"
        );
        assert_eq!(resolve("cat vibe:", &v).0, "cat", "an empty filter is left out");
        assert_eq!(resolve("vibe:xyzzy", &v).0, "vibe:xyzzy", "no match: as typed");
        assert_eq!(
            resolve("\"vibe:sar x\" y", &v).0,
            "\"vibe:sar x\" y",
            "quoted text stays"
        );
        assert_eq!(resolve("  q>70   e:😂 ", &v).0, "q>70 e:😂");
        let none = Vocabulary::default();
        assert_eq!(resolve("vibe:sar", &none).0, "vibe:sar", "nothing known yet: as typed");
        assert_eq!(resolve("type:gfi", &none).0, "type:gif", "fixed words need no items");
    }

    #[test]
    fn the_vocabulary_counts_the_items() {
        let item = |vibe: Option<&str>, tags: &[&str], set: (&str, &str), owner: &str| ItemSummary {
            vibe: vibe.map(str::to_string),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            set_slug: set.0.into(),
            set_name: set.1.into(),
            owner_username: owner.into(),
            ..crate::model::tests::item("x")
        };
        let items = [
            item(Some("Happy"), &["cat", "fun"], ("cats", "Cats"), "alice"),
            item(Some("sad"), &["fun"], ("work", "Work memes"), "bob"),
            item(Some("sad "), &["two words", "fun"], ("work", "Work memes"), "bob"),
            item(None, &[], ("kotyky", "Котики"), "bob"),
        ];
        let v = Vocabulary::from_items(&items);
        assert_eq!(v.vibes, ["sad", "happy"]);
        assert_eq!(v.tags, ["fun", "cat"], "a tag with a space cannot be typed");
        assert_eq!(v.sets, ["work", "cats", "kotyky", "котики"]);
        assert_eq!(v.owners, ["bob", "alice"]);
        assert!(!v.is_empty() && Vocabulary::default().is_empty());
    }
}
