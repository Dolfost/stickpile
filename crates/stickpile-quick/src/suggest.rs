//! Suggestions for the search field. While a `key:value` filter is typed
//! (`vibe:sar`), the values it can take are offered in one line under the
//! field, best first: the vibes, tags, packs and owners of the cached items
//! (most used first) and the fixed words of `type:`, `is:` and `sort:`.
//! A value is matched as typed: its start, then anywhere in it, then with a
//! typo or two forgiven, so `vibe:sarcstic` still offers `sarcastic`.
//!
//! `e:` offers emojis, each with its name ([`Suggestions::labels`]): the
//! items' emojis (most used first), and while a name is typed (`e:sk`) the
//! emojis it selects (`stickpile_search::emoji_names::by_name`), the
//! items' first. Taking one puts the emoji in place of the name.
//!
//! The search itself goes out with every such value [`resolve`]d to its
//! best suggestion, so a half-typed or misspelled filter still finds the
//! stickers. An `e:` value goes out as typed: the search matches a name
//! itself, typos included.
//!
//! The one-letter keys work as the long ones (`v:sar` as `vibe:sar`, see
//! `stickpile_api::query::Key`); the key stays as typed.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use stickpile_api::query::Key;
use stickpile_api::ItemSummary;
use stickpile_search::emoji_names;

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
    /// The emojis of the items, most used first (ties in the order first
    /// seen), each once however its variation selectors are written (see
    /// [`emoji_names::toned_key`]), as first seen.
    pub emojis: Vec<String>,
    /// The viewer's username, as far as the cache knows it: what `is:mine`
    /// means where the client searches by itself (inside an opened pack,
    /// see [`crate::within`]). `None`: `is:mine` finds nothing there.
    pub me: Option<String>,
}

/// What one item gives a [`Vocabulary`]: the values its filters match.
/// The server builds one from its search index, the clients from their
/// cached [`ItemSummary`]s.
#[derive(Debug, Clone, Copy)]
pub struct ItemValues<'a> {
    pub vibe: Option<&'a str>,
    pub tags: &'a [String],
    pub set_slug: &'a str,
    pub set_name: &'a str,
    pub owner_username: &'a str,
    pub emojis: &'a [String],
}

impl<'a> From<&'a ItemSummary> for ItemValues<'a> {
    fn from(it: &'a ItemSummary) -> Self {
        ItemValues {
            vibe: it.vibe.as_deref(),
            tags: &it.tags,
            set_slug: &it.set_slug,
            set_name: &it.set_name,
            owner_username: &it.owner_username,
            emojis: &it.emojis,
        }
    }
}

impl Vocabulary {
    pub fn from_items<'a>(items: impl IntoIterator<Item = &'a ItemSummary>) -> Vocabulary {
        Vocabulary::from_values(items.into_iter().map(ItemValues::from))
    }

    /// [`Vocabulary::from_items`] for items given by their values.
    pub fn from_values<'a>(items: impl IntoIterator<Item = ItemValues<'a>>) -> Vocabulary {
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
        // (emoji as first seen, uses), in the order first seen.
        let mut emojis: Vec<(String, u32)> = Vec::new();
        let mut emoji_at: HashMap<String, usize> = HashMap::new();
        for it in items {
            for e in it.emojis {
                let e = e.trim();
                if e.is_empty() || e.contains(char::is_whitespace) {
                    continue;
                }
                match emoji_at.entry(emoji_names::toned_key(e)) {
                    Entry::Occupied(at) => emojis[*at.get()].1 += 1,
                    Entry::Vacant(at) => {
                        at.insert(emojis.len());
                        emojis.push((e.to_string(), 1));
                    }
                }
            }
            if let Some(v) = it.vibe {
                vibes.add(v);
            }
            for t in it.tags {
                tags.add(t);
            }
            sets.add(it.set_slug);
            if !it.set_name.trim().eq_ignore_ascii_case(it.set_slug.trim()) {
                sets.add(it.set_name);
            }
            owners.add(it.owner_username);
        }
        // Stable: ties stay in the order first seen.
        emojis.sort_by_key(|e| std::cmp::Reverse(e.1));
        Vocabulary {
            vibes: vibes.ranked(),
            tags: tags.ranked(),
            sets: sets.ranked(),
            owners: owners.ranked(),
            emojis: emojis.into_iter().map(|(e, _)| e).collect(),
            me: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.vibes.is_empty()
            && self.tags.is_empty()
            && self.sets.is_empty()
            && self.owners.is_empty()
            && self.emojis.is_empty()
    }

    /// These values first, then those of `more` not among them (each list
    /// in its order); the viewer of `more` when this one names none.
    pub fn then(&self, more: &Vocabulary) -> Vocabulary {
        fn join(first: &[String], more: &[String], key: impl Fn(&str) -> String) -> Vec<String> {
            let seen: HashSet<String> = first.iter().map(|w| key(w)).collect();
            first
                .iter()
                .chain(more.iter().filter(|w| !seen.contains(&key(w))))
                .cloned()
                .collect()
        }
        let same = |w: &str| w.to_string();
        Vocabulary {
            vibes: join(&self.vibes, &more.vibes, same),
            tags: join(&self.tags, &more.tags, same),
            sets: join(&self.sets, &more.sets, same),
            owners: join(&self.owners, &more.owners, same),
            emojis: join(&self.emojis, &more.emojis, emoji_names::toned_key),
            me: self.me.clone().or_else(|| more.me.clone()),
        }
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
    /// `e:`: emojis, offered with their names.
    Emoji,
}

impl Field {
    /// The field of a `key` as the query syntax spells it, its one-letter
    /// form included (`v` is `vibe`); `None` for anything that is no key,
    /// and for `text:` (`x:`), whose values are free words with nothing to
    /// offer.
    pub fn of(key: &str) -> Option<Field> {
        Some(match Key::parse(key)? {
            Key::Vibe => Field::Vibe,
            Key::Tag => Field::Tag,
            Key::Set => Field::Set,
            Key::By => Field::Owner,
            Key::Type => Field::Type,
            Key::Is => Field::Is,
            Key::Sort => Field::Sort,
            Key::Emoji => Field::Emoji,
            Key::Text => return None,
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
            Field::Emoji => vocab.emojis.iter().map(String::as_str).collect(),
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
    /// Best first; the search uses the first unless `typed` is one of them
    /// (for `e:`, the search uses what is typed: see [`Suggestions::chosen`]).
    /// Taking one puts it in place of `typed`: for `e:`, an emoji.
    pub words: Vec<String>,
    /// Parallel to `words`: the English short name of each emoji `e:`
    /// offers (`skull` for 💀); empty for the other fields, and for an
    /// emoji without a name.
    pub labels: Vec<String>,
    /// Byte offset in the query where the value starts.
    pub start: usize,
}

impl Suggestions {
    /// The filter: [`Field::of`] the key.
    pub fn field(&self) -> Option<Field> {
        let key = self.key.strip_prefix('-').unwrap_or(&self.key);
        Field::of(key.strip_suffix(':').unwrap_or(key))
    }

    /// The word the search uses for this value: the typed one when it is a
    /// known value, else the best suggestion; none while nothing is typed
    /// (an empty filter is left out of the search). For `e:`, the search
    /// uses the value as typed: the emoji typed when it is offered, none
    /// for a name (`e:sk`, which the search matches itself).
    pub fn chosen(&self) -> Option<usize> {
        if self.typed.is_empty() {
            return None;
        }
        if self.field() == Some(Field::Emoji) {
            let typed = emoji_names::toned_key(&self.typed);
            return self.words.iter().position(|w| emoji_names::toned_key(w) == typed);
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
    let (words, labels) = if field == Field::Emoji {
        let words = emojis(typed, vocab);
        let labels = words
            .iter()
            .map(|e| emoji_names::short_name(e).unwrap_or_default().to_string())
            .collect();
        (words, labels)
    } else {
        let words: Vec<String> = ranked(typed, &field.words(vocab))
            .into_iter()
            .take(MAX_SUGGESTIONS)
            .map(str::to_string)
            .collect();
        let labels = vec![String::new(); words.len()];
        (words, labels)
    };
    if words.is_empty() {
        return None;
    }
    let start = query.len() - typed.len();
    Some(Suggestions {
        key: token[..token.len() - typed.len()].to_string(),
        typed: typed.to_string(),
        words,
        labels,
        start,
    })
}

/// The emojis offered for `e:<typed>`, at most [`MAX_SUGGESTIONS`]:
///
/// - nothing typed: the items' emojis, most used first;
/// - a name (`sk`): the emojis it selects
///   ([`emoji_names::by_name`]), the items' first (in their order);
/// - an emoji, maybe still being typed (`❤` of `❤️‍🔥`): the items'
///   emojis that start with it, the same one first (else that emoji
///   from the names table), then longer ones (another skin tone, a ZWJ
///   sequence).
fn emojis(typed: &str, vocab: &Vocabulary) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut put = |e: &str| {
        if out.len() < MAX_SUGGESTIONS && seen.insert(emoji_names::toned_key(e)) {
            out.push(e.to_string());
        }
    };
    if typed.is_empty() {
        vocab.emojis.iter().for_each(|e| put(e));
    } else if emoji_names::is_name(typed) {
        let named = emoji_names::by_name(typed);
        let keys: HashSet<&str> = named.iter().map(|e| e.key()).collect();
        vocab
            .emojis
            .iter()
            .filter(|e| keys.contains(emoji_names::key(e).as_str()))
            .for_each(|e| put(e));
        named.iter().for_each(|e| put(e.glyph));
    } else {
        let typed = emoji_names::toned_key(typed);
        let (same, longer): (Vec<&String>, Vec<&String>) = vocab
            .emojis
            .iter()
            .filter(|e| emoji_names::toned_key(e).starts_with(&typed))
            .partition(|e| emoji_names::toned_key(e) == typed);
        same.into_iter().for_each(|e| put(e));
        if let Some(e) = emoji_names::find(&typed).filter(|e| e.key() == typed) {
            put(e.glyph);
        }
        longer.into_iter().for_each(|e| put(e));
    }
    out
}

/// `query` with every filter value of a [`Field`] that is not a known value
/// replaced by its best match, and empty filters (`vibe:`, `e:`) left out;
/// also the replacements made, as (typed, used). An `e:` value is never
/// replaced: the search matches an emoji name (`e:skull`) itself, typos
/// included. The one-letter keys go out long (`v:sad` as `vibe:sad`), so
/// a server from before them reads them too.
pub fn resolve(query: &str, vocab: &Vocabulary) -> (String, Vec<(String, String)>) {
    let mut out = String::with_capacity(query.len());
    let mut fixes = Vec::new();
    for (token, quoted) in tokens(query) {
        let resolved = (!quoted).then(|| resolve_token(token, vocab)).flatten();
        let piece = match resolved {
            Some(Resolved::Drop) => continue,
            Some(Resolved::Use(prefix, typed, word)) => {
                fixes.push((typed.to_string(), word.clone()));
                long_key(&format!("{prefix}{word}"))
            }
            None => long_key(token),
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

/// `token` with its key written long when it is a one-letter key other
/// than `e:` (which every server reads): `-v:x` is `-vibe:x`.
fn long_key(token: &str) -> String {
    let (neg, bare) = match token.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", token),
    };
    if let Some((key, value)) = bare.split_once(':') {
        if let Some(k) = Key::parse(key) {
            if k != Key::Emoji && k.short().is_some_and(|s| s.eq_ignore_ascii_case(key)) {
                return format!("{neg}{}:{value}", k.name());
            }
        }
    }
    token.to_string()
}

fn resolve_token<'a>(token: &'a str, vocab: &Vocabulary) -> Option<Resolved<'a>> {
    let bare = token.strip_prefix('-').unwrap_or(token);
    let (key, typed) = bare.split_once(':')?;
    let field = Field::of(key)?;
    if typed.is_empty() {
        return Some(Resolved::Drop);
    }
    if field == Field::Emoji {
        return None;
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
            emojis: ["😂", "🦨", "🐱", "💀", "❤️"].map(String::from).to_vec(),
            me: None,
        }
    }

    fn words(q: &str) -> Vec<String> {
        suggest(q, &vocab()).map(|s| s.words).unwrap_or_default()
    }

    #[test]
    fn e_alone_offers_the_emojis_most_used_first_with_their_names() {
        let s = suggest("cat e:", &vocab()).unwrap();
        assert_eq!(s.key, "e:");
        assert_eq!(s.field(), Some(Field::Emoji));
        assert_eq!(s.words, ["😂", "🦨", "🐱", "💀", "❤️"]);
        assert_eq!(
            s.labels,
            ["face with tears of joy", "skunk", "cat face", "skull", "red heart"]
        );
        assert_eq!(s.start, "cat e:".len());
        assert_eq!(s.chosen(), None, "nothing typed");
        assert!(suggest("e:", &Vocabulary::default()).is_none(), "no emojis known");
    }

    #[test]
    fn an_emoji_name_offers_the_own_emojis_first_then_the_others() {
        let s = suggest("e:sk", &vocab()).unwrap();
        assert_eq!(s.words[..3], ["🦨", "💀", "☠️"], "the own ones in their order");
        assert_eq!(s.labels[..3], ["skunk", "skull", "skull and crossbones"]);
        assert_eq!(s.words.len(), MAX_SUGGESTIONS);
        assert_eq!(s.labels.len(), s.words.len());
        assert_eq!(s.chosen(), None, "a name goes out as typed");
        let s = suggest("e:sk", &Vocabulary::default()).unwrap();
        assert_eq!(s.words[..2], ["💀", "☠️"], "the table alone");

        let s = suggest("-emoji:SKULL", &vocab()).unwrap();
        assert_eq!(s.key, "-emoji:");
        assert_eq!(s.words, ["💀", "☠️", "🩻", "👲"]);
        assert_eq!(
            s.labels,
            ["skull", "skull and crossbones", "x-ray", "person with skullcap"]
        );
        assert_eq!(
            words("e:sklul"),
            ["💀", "☠️", "🩻"],
            "typos as the search forgives them"
        );
        assert_eq!(words("e:thumbs_up"), ["👍"]);
        assert!(suggest("e:xqzv", &vocab()).is_none(), "a name that selects nothing");
    }

    #[test]
    fn an_emoji_typed_is_offered_and_chosen() {
        let s = suggest("e:💀", &vocab()).unwrap();
        assert_eq!(
            (s.words.as_slice(), s.chosen()),
            (["💀".to_string()].as_slice(), Some(0))
        );
        assert_eq!(s.labels, ["skull"]);
        let s = suggest("e:❤", &vocab()).unwrap();
        assert_eq!(s.words, ["❤️"], "without its variation selector");
        assert_eq!(s.chosen(), Some(0));
        let s = suggest("e:👍", &vocab()).unwrap();
        assert_eq!((s.words[0].as_str(), s.labels[0].as_str()), ("👍", "thumbs up"));
        assert_eq!(s.chosen(), Some(0), "not an own one, still the emoji typed");
        let v = Vocabulary {
            emojis: ["👍🏽", "❤️\u{200d}🔥", "❤️"].map(String::from).to_vec(),
            ..Vocabulary::default()
        };
        let s = suggest("e:❤", &v).unwrap();
        assert_eq!(s.words, ["❤️", "❤️\u{200d}🔥"], "the same one first, then longer ones");
        assert_eq!(s.labels, ["red heart", "heart on fire"]);
        let s = suggest("e:👍", &v).unwrap();
        assert_eq!(
            s.words,
            ["👍", "👍🏽"],
            "the emoji typed, then the own one with a skin tone"
        );
        assert_eq!(s.chosen(), Some(0), "another skin tone is another filter");
        assert!(suggest("e:👍🏿", &v).is_none());
    }

    #[test]
    fn choosing_an_emoji_puts_it_in() {
        let q = "cat e:sk";
        let s = suggest(q, &vocab()).unwrap();
        assert_eq!(format!("{}{}", &q[..s.start], s.words[1]), "cat e:💀");
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
    fn short_keys_offer_what_the_long_ones_do() {
        for (short, long) in [
            ("v:sar", "vibe:sar"),
            ("t:ns", "tag:ns"),
            ("s:wor", "set:wor"),
            ("b:al", "by:al"),
            ("v:", "vibe:"),
            ("-t:", "-tag:"),
        ] {
            assert_eq!(words(short), words(long), "{short}");
            assert!(!words(short).is_empty(), "{short}");
        }
        let s = suggest("cat v:sar", &vocab()).unwrap();
        assert_eq!(s.key, "v:", "the key as typed");
        assert_eq!(s.typed, "sar");
        assert_eq!(s.start, "cat v:".len());
        assert_eq!(s.words[0], "sarcastic");
        assert_eq!(suggest("-T:ns", &vocab()).unwrap().key, "-T:");
        assert_eq!(words("e:sk"), words("emoji:sk"));
        assert_eq!(words("E:"), words("emoji:"));
        assert!(!words("e:").is_empty());
    }

    #[test]
    fn resolve_short_keys_and_send_them_long() {
        let v = vocab();
        assert_eq!(resolve("cat v:sarcstic", &v).0, "cat vibe:sarcastic");
        assert_eq!(
            resolve("v:sar -t:nsf s:wor b:al", &v).0,
            "vibe:sarcastic -tag:nsfw set:work-memes by:alice"
        );
        assert_eq!(resolve("cat v: t:", &v).0, "cat", "empty filters are left out");
        assert_eq!(resolve("v:sad", &v), ("vibe:sad".into(), vec![]), "a known value stays");
        assert_eq!(
            resolve("V:xyzzy", &v).0,
            "vibe:xyzzy",
            "no match: as typed, the key long"
        );
        let none = Vocabulary::default();
        assert_eq!(
            resolve(r#"v:"very sad" -T:a e:😂 "v:x" user:bob"#, &none).0,
            r#"vibe:"very sad" -tag:a e:😂 "v:x" user:bob"#,
            "quoted values too; e:, phrases and long keys stay"
        );
        assert_eq!(resolve("http://x q>70", &none).0, "http://x q>70");
    }

    #[test]
    fn resolve_leaves_emoji_names_alone() {
        let v = vocab();
        assert_eq!(
            resolve("e:skull -emoji:sklul e:sk E:Skull e:💀", &v),
            ("e:skull -emoji:sklul e:sk E:Skull e:💀".into(), vec![]),
            "the search matches names itself"
        );
        assert_eq!(resolve("cat e: -e:", &v).0, "cat", "empty ones are left out");
    }

    #[test]
    fn then_puts_the_first_values_first() {
        let pack = Vocabulary {
            vibes: vec!["tired".into(), "new".into()],
            tags: vec![],
            sets: vec!["cats".into()],
            owners: vec![],
            emojis: vec!["❤".into(), "🐶".into()],
            me: None,
        };
        let all = Vocabulary {
            me: Some("alice".into()),
            ..vocab()
        };
        let both = pack.then(&all);
        assert_eq!(both.vibes, ["tired", "new", "sad", "sarcastic", "happy", "angry"]);
        assert_eq!(both.tags, all.tags);
        assert_eq!(both.sets, ["cats", "work-memes"]);
        assert_eq!(
            both.emojis,
            ["❤", "🐶", "😂", "🦨", "🐱", "💀"],
            "❤️ is ❤ with a variation selector"
        );
        assert_eq!(both.me.as_deref(), Some("alice"));
    }

    #[test]
    fn nothing_outside_a_filter_being_typed() {
        assert!(suggest("", &vocab()).is_none());
        assert!(suggest("cat", &vocab()).is_none());
        assert!(suggest("vibe:sad ", &vocab()).is_none(), "the token is finished");
        assert!(suggest("e:sk ", &vocab()).is_none(), "the token is finished");
        assert!(suggest("\"vibe:sa", &vocab()).is_none(), "inside quotes");
        assert!(suggest("vibe:sa", &Vocabulary::default()).is_none(), "nothing known");
        // The words in the picture are free text: nothing to offer or to correct.
        assert_eq!(Field::of("text"), None);
        assert_eq!(Field::of("X"), None);
        assert!(suggest("text:", &vocab()).is_none());
        assert!(suggest("cat x:sa", &vocab()).is_none());
        assert_eq!(
            resolve("x:sad text:cat", &vocab()).0,
            "text:sad text:cat",
            "x: goes out long"
        );
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

    #[test]
    fn the_emojis_are_counted_from_the_items() {
        let item = |emojis: &[&str]| ItemSummary {
            emojis: emojis.iter().map(|e| e.to_string()).collect(),
            ..crate::model::tests::item("x")
        };
        let items = [
            item(&["🐱", "😂"]),
            item(&["😂", " ", ""]),
            item(&["😂", "🐶"]),
            item(&["❤️"]),
            item(&["❤", "👍🏽", "👍", "two words"]),
        ];
        let v = Vocabulary::from_items(&items);
        assert_eq!(
            v.emojis,
            ["😂", "❤️", "🐱", "🐶", "👍🏽", "👍"],
            "most used first, ties as first seen; ❤ and ❤️ are one"
        );
        let only = Vocabulary {
            emojis: vec!["😂".into()],
            ..Vocabulary::default()
        };
        assert!(!only.is_empty());
    }
}
