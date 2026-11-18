//! Suggestions for the search field. While a `key:value` filter is typed
//! (`vibe:sar`), the values it can take are offered in one line under the
//! field, best first: the vibes, tags, packs and owners of the cached items
//! (most used first) and the fixed words of `type:`, `is:` and `sort:`.
//! A value is matched as typed: its start, then anywhere in it, then with a
//! typo or two forgiven, so `vibe:sarcstic` still offers `sarcastic`.
//!
//! `e:` offers emojis, each with its name ([`Suggestions::labels`]): the
//! emojis of the viewer's own items (most used first, see
//! [`Vocabulary::emojis`]), and while a name is typed (`e:sk`) the emojis
//! it selects (`stickpile_search::emoji_names::by_name`), the viewer's
//! first. Taking one puts the emoji in place of the name.
//!
//! The values do not depend on the order the items come in (the server
//! reads its index, the clients a `HashMap` of their cache): ties go by
//! the value, emojis by the names table's order, and an emoji written in
//! several ways is offered in one ([`emoji_names::spelling`]).
//!
//! The search itself goes out with every such value [`resolve`]d to its
//! best suggestion, so a half-typed or misspelled filter still finds the
//! stickers. An `e:` value goes out as typed: the search matches a name
//! itself, typos included.
//!
//! The one-letter keys work as the long ones (`v:sar` as `vibe:sar`, see
//! `stickpile_api::query::Key`); the key stays as typed.
//!
//! `x:` (`text:`) offers the words written on the items the viewer can
//! search, read from the search index on each keystroke
//! ([`Vocabulary::texts`], `stickpile_search::Index::text_words`): those
//! the typed value starts, has inside or is a typo or two away from, in
//! that order, each group the words in the most items first; nothing
//! typed, the words in the most items. A `text:` value goes out as typed:
//! the search forgives a half-typed or misspelled word itself.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use stickpile_api::query::Key;
use stickpile_api::ItemSummary;
use stickpile_search::{emoji_names, offer_tier, text_word, TextWords};

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

/// The values the viewer's items use, each list most used first, ties in
/// a fixed order, so the same items in any order give the same lists: the
/// vibes, tags, packs and owners of every item the viewer can search, ties
/// alphabetical, and the emojis of the viewer's own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vocabulary {
    pub vibes: Vec<String>,
    pub tags: Vec<String>,
    /// Pack slugs, and the names that are one word.
    pub sets: Vec<String>,
    pub owners: Vec<String>,
    /// The emojis of the viewer's own items ([`ItemValues::mine`]), most
    /// used first, ties in the names table's order
    /// ([`emoji_names::rank`]), then by [`emoji_names::toned_key`]. Each
    /// is there once however it is written, in its one spelling
    /// ([`emoji_names::spelling`]: `☠️` for `☠`, `👍🏽` with its tone).
    pub emojis: Vec<String>,
    /// The viewer's username, as far as the cache knows it: what `is:mine`
    /// means where the client searches by itself (inside an opened pack,
    /// see [`crate::within`]). `None`: `is:mine` finds nothing there.
    pub me: Option<String>,
    /// Where `x:` finds the words written on the items: the index the
    /// viewer's search reads ([`Texts`]); none: `x:` offers nothing.
    pub texts: Texts,
}

/// The words written on the items, for `x:`: the index the viewer's search
/// reads (a client's offline index, the items of an opened pack), asked on
/// each keystroke ([`TextWords`]), so the texts are not copied here. Two
/// are equal when they read the same index.
#[derive(Clone, Default)]
pub struct Texts(Option<Arc<dyn TextWords>>);

impl Texts {
    pub fn new(words: Arc<dyn TextWords>) -> Texts {
        Texts(Some(words))
    }

    pub fn get(&self) -> Option<&dyn TextWords> {
        self.0.as_deref()
    }

    /// These words first, then those of `more` not among them.
    pub fn then(&self, more: &Texts) -> Texts {
        match (&self.0, &more.0) {
            (Some(first), Some(more)) => Texts::new(Arc::new(Then(first.clone(), more.clone()))),
            (Some(_), None) => self.clone(),
            (None, _) => more.clone(),
        }
    }
}

impl PartialEq for Texts {
    fn eq(&self, other: &Texts) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(a), Some(b)) => std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)),
            _ => false,
        }
    }
}

impl Eq for Texts {}

impl fmt::Debug for Texts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() {
            "Texts(index)"
        } else {
            "Texts(none)"
        })
    }
}

/// [`Texts::then`]: the words of one, then those of the other.
struct Then(Arc<dyn TextWords>, Arc<dyn TextWords>);

impl TextWords for Then {
    fn text_words(&self, typed: &str, limit: usize) -> Vec<String> {
        let mut words = self.0.text_words(typed, limit);
        if words.len() < limit {
            for w in self.1.text_words(typed, limit) {
                if words.len() == limit {
                    break;
                }
                if !words.contains(&w) {
                    words.push(w);
                }
            }
        }
        words
    }
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
    /// The viewer owns the item (or who the viewer is is not known): its
    /// emojis count for [`Vocabulary::emojis`].
    pub mine: bool,
}

impl<'a> ItemValues<'a> {
    /// The values of a cached item, for the viewer with the username `me`
    /// (`None`: not known, every item counts as the viewer's).
    pub fn of(it: &'a ItemSummary, me: Option<&str>) -> ItemValues<'a> {
        ItemValues {
            vibe: it.vibe.as_deref(),
            tags: &it.tags,
            set_slug: &it.set_slug,
            set_name: &it.set_name,
            owner_username: &it.owner_username,
            emojis: &it.emojis,
            mine: me.is_none_or(|me| me == it.owner_username),
        }
    }
}

impl Vocabulary {
    /// The values of `items`, the emojis of all of them: for items with
    /// no viewer to tell apart (a pack's own values, a test). The clients'
    /// vocabulary is [`Vocabulary::for_viewer`].
    pub fn from_items<'a>(items: impl IntoIterator<Item = &'a ItemSummary>) -> Vocabulary {
        Vocabulary::for_viewer(items, None)
    }

    /// The values of `items` (the cache) for the viewer with the username
    /// `me`: the emojis of the items `me` owns, of all while `me` is not
    /// known; the rest of all of them. [`Vocabulary::me`] is `me`.
    pub fn for_viewer<'a>(items: impl IntoIterator<Item = &'a ItemSummary>, me: Option<&str>) -> Vocabulary {
        Vocabulary {
            me: me.map(str::to_string),
            ..Vocabulary::from_values(items.into_iter().map(|it| ItemValues::of(it, me)))
        }
    }

    /// The values of items given by their values (no [`Vocabulary::me`]).
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
        // Uses per toned key.
        let mut emojis: HashMap<String, u32> = HashMap::new();
        for it in items {
            for e in it.emojis.iter().filter(|_| it.mine) {
                let e = e.trim();
                if e.is_empty() || e.contains(char::is_whitespace) {
                    continue;
                }
                *emojis.entry(emoji_names::toned_key(e)).or_default() += 1;
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
        let mut emojis: Vec<(std::cmp::Reverse<u32>, u64, String)> = emojis
            .into_iter()
            .map(|(e, uses)| {
                let rank = emoji_names::rank(&e).unwrap_or(u64::MAX);
                (std::cmp::Reverse(uses), rank, e)
            })
            .collect();
        emojis.sort_unstable();
        Vocabulary {
            vibes: vibes.ranked(),
            tags: tags.ranked(),
            sets: sets.ranked(),
            owners: owners.ranked(),
            emojis: emojis.into_iter().map(|(_, _, e)| emoji_names::spelling(&e)).collect(),
            me: None,
            texts: Texts::default(),
        }
    }

    /// These values, `x:` offering the words of `words` (the index the
    /// viewer's search reads).
    pub fn with_texts(self, words: Arc<dyn TextWords>) -> Vocabulary {
        Vocabulary {
            texts: Texts::new(words),
            ..self
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
            texts: self.texts.then(&more.texts),
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
    /// `x:` (`text:`): the words written on the items
    /// ([`Vocabulary::texts`]), the value going out as typed.
    Text,
}

impl Field {
    /// The field of a `key` as the query syntax spells it, its one-letter
    /// form included (`v` is `vibe`, `x` is `text`); `None` for anything
    /// that is no key.
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
            Key::Text => Field::Text,
        })
    }

    /// The field of the `key:value` filter being typed at the end of
    /// `query` ([`suggest`] offers its values), if any.
    pub fn typed(query: &str) -> Option<Field> {
        let token = query.rsplit(char::is_whitespace).next()?;
        let (key, _) = token.strip_prefix('-').unwrap_or(token).split_once(':')?;
        Field::of(key)
    }

    /// The values of the vocabulary's lists; none for `x:`, whose words
    /// are looked up as typed ([`Vocabulary::texts`]).
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
            Field::Text => Vec::new(),
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
    /// (for `e:` and `x:`, the search uses what is typed: see
    /// [`Suggestions::chosen`]). Taking one puts it in place of `typed`:
    /// for `e:`, an emoji.
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
    /// for a name (`e:sk`, which the search matches itself). For `x:` too:
    /// the word typed when it is offered (written in any case, with any
    /// apostrophe), else none (`x:lo` finds what starts with it).
    pub fn chosen(&self) -> Option<usize> {
        if self.typed.is_empty() {
            return None;
        }
        match self.field() {
            Some(Field::Emoji) => {
                let typed = emoji_names::toned_key(&self.typed);
                return self.words.iter().position(|w| emoji_names::toned_key(w) == typed);
            }
            Some(Field::Text) => {
                let typed = text_word(&self.typed)?;
                return self.words.iter().position(|w| *w == typed);
            }
            _ => {}
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
    suggest_with(query, vocab, vocab.texts.get())
}

/// [`suggest`], `x:` offering the words of `texts` rather than of
/// [`Vocabulary::texts`]: the server's, its index as the caller searches
/// it (`stickpile_search::ViewedIndex`).
pub fn suggest_with(query: &str, vocab: &Vocabulary, texts: Option<&dyn TextWords>) -> Option<Suggestions> {
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
    let (words, labels) = match field {
        Field::Emoji => {
            let words = emojis(typed, vocab);
            let labels = words
                .iter()
                .map(|e| emoji_names::short_name(e).unwrap_or_default().to_string())
                .collect();
            (words, labels)
        }
        Field::Text => {
            let words = texts.map_or_else(Vec::new, |t| t.text_words(typed, MAX_SUGGESTIONS));
            let labels = vec![String::new(); words.len()];
            (words, labels)
        }
        _ => {
            let words: Vec<String> = ranked(typed, &field.words(vocab))
                .into_iter()
                .take(MAX_SUGGESTIONS)
                .map(str::to_string)
                .collect();
            let labels = vec![String::new(); words.len()];
            (words, labels)
        }
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
/// - nothing typed: the [`Vocabulary::emojis`] (the viewer's), most used
///   first;
/// - a name (`sk`): the emojis it selects
///   ([`emoji_names::by_name`]), the vocabulary's first (in its order);
/// - an emoji, maybe still being typed (`❤` of `❤️‍🔥`): the vocabulary's
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
/// replaced by its best match, and empty filters of any key (`vibe:`,
/// `e:`, `x:`: the parser would read an empty `text:` as the word "text")
/// left out; also the replacements made, as (typed, used). An `e:` or
/// `x:` value is never replaced: the search matches an emoji name
/// (`e:skull`) itself, typos included, and finds the words a `text:`
/// value starts or is a typo of (`x:lo` is not narrowed to one word). The
/// one-letter keys go out long (`v:sad` as `vibe:sad`), so a server from
/// before them reads them too.
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
    // An empty filter of any key (`v:`, `e:`, `x:`) is no filter yet.
    Key::parse(key)?;
    if typed.is_empty() {
        return Some(Resolved::Drop);
    }
    let field = Field::of(key)?;
    if matches!(field, Field::Emoji | Field::Text) {
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

/// `words` (in their order of use) that match `typed`, best first: those
/// starting with it (the same word first), those containing it, then those
/// a typo or two away, fewer typos first (`stickpile_search::offer_tier`,
/// which ranks the words written on the items the same way).
fn ranked<'a>(typed: &str, words: &[&'a str]) -> Vec<&'a str> {
    let typed: Vec<char> = typed.to_lowercase().chars().collect();
    if typed.is_empty() {
        return words.to_vec();
    }
    let mut scored: Vec<(usize, usize, &'a str)> = Vec::new();
    for (n, w) in words.iter().enumerate() {
        let chars: Vec<char> = w.chars().collect();
        if let Some(tier) = offer_tier(&typed, &chars) {
            scored.push((tier, n, w));
        }
    }
    scored.sort_unstable();
    scored.into_iter().map(|(_, _, w)| w).collect()
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
            texts: Texts::default(),
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
            texts: Texts::default(),
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
        // No index to read the words in the pictures from: nothing for x:.
        assert_eq!(Field::of("text"), Some(Field::Text));
        assert_eq!(Field::of("X"), Some(Field::Text));
        assert!(suggest("text:", &vocab()).is_none());
        assert!(suggest("cat x:sa", &vocab()).is_none());
        assert_eq!(
            resolve("x:sad text:cat", &vocab()).0,
            "text:sad text:cat",
            "x: goes out long"
        );
    }

    /// Items with the words written on them, as a pack holds them.
    fn texts() -> Vocabulary {
        let item = |id: &str, text: &str| ItemSummary {
            id: id.into(),
            text: text.into(),
            ..crate::model::tests::item(id)
        };
        let items = [
            item("a", "LOL"),
            item("b", "lol what"),
            item("c", "what is love"),
            item("d", "КОЛИВ ДОМА"),
            item("e", "коли ти вдома"),
            item("f", "П'ЯТНИЦЯ"),
            item("g", "п’ятниця 2024"),
            item("h", "hello"),
        ];
        vocab().with_texts(Arc::new(crate::within::Within::new(&items, None, None)))
    }

    #[test]
    fn x_alone_offers_the_most_common_words() {
        let s = suggest("cat x:", &texts()).unwrap();
        assert_eq!((s.key.as_str(), s.typed.as_str(), s.start), ("x:", "", 6));
        assert_eq!(s.field(), Some(Field::Text));
        // In two items each, then in one, alphabetically; no numbers.
        assert_eq!(
            s.words[..6],
            ["lol", "what", "пятниця", "hello", "is", "love"].map(String::from)
        );
        assert!(!s.words.contains(&"2024".to_string()));
        assert!(s.labels.iter().all(String::is_empty) && s.labels.len() == s.words.len());
        assert_eq!(s.chosen(), None, "nothing typed");
        assert_eq!(suggest("text:", &texts()).unwrap().words, s.words);
        assert_eq!(suggest("-x:", &texts()).unwrap().key, "-x:");
    }

    #[test]
    fn x_offers_the_words_a_value_starts_has_inside_or_is_a_typo_of() {
        let v = texts();
        assert_eq!(words_in("x:l", &v), ["lol", "love"]);
        assert_eq!(words_in("x:lo", &v), ["lol", "love", "hello"], "then inside");
        assert_eq!(words_in("X:LO", &v), words_in("x:lo", &v));
        assert_eq!(words_in("text:lo", &v), words_in("x:lo", &v));
        assert_eq!(words_in("x:ко", &v), ["коли", "колив"]);
        assert_eq!(words_in("x:оли", &v), ["коли", "колив"], "inside the words");
        assert_eq!(words_in("x:lvoe", &v), ["love"], "a typo");
        assert_eq!(words_in("x:п'ятн", &v), ["пятниця"], "as the index keeps it");
        assert!(suggest("x:zzzz", &v).is_none());
        assert!(suggest("x:a-b", &v).is_none(), "two words");
        let s = suggest("cat -x:lo", &v).unwrap();
        assert_eq!((s.key.as_str(), s.typed.as_str(), s.start), ("-x:", "lo", 7));
    }

    fn words_in(q: &str, v: &Vocabulary) -> Vec<String> {
        suggest(q, v).map(|s| s.words).unwrap_or_default()
    }

    /// The search uses an `x:` value as typed: only a word typed whole is
    /// chosen (a hint), and `resolve` leaves the value alone.
    #[test]
    fn an_x_value_goes_out_as_typed() {
        let v = texts();
        assert_eq!(suggest("x:lo", &v).unwrap().chosen(), None);
        assert_eq!(suggest("x:lol", &v).unwrap().chosen(), Some(0));
        assert_eq!(suggest("x:LOL", &v).unwrap().chosen(), Some(0));
        assert_eq!(suggest("x:П’ЯТНИЦЯ", &v).unwrap().chosen(), Some(0));
        assert_eq!(resolve("x:lo", &v), ("text:lo".to_string(), vec![]));
        assert_eq!(resolve("cat x:lool -x:ко", &v).0, "cat text:lool -text:ко");
        assert_eq!(resolve("x:\"no thou\"", &v).0, "text:\"no thou\"");
        assert_eq!(resolve("cat x:", &v).0, "cat", "an empty x: is left out");
    }

    #[test]
    fn choosing_a_text_word_puts_it_in_with_a_space() {
        let q = "cat x:lo";
        let s = suggest(q, &texts()).unwrap();
        assert_eq!(format!("{}{} ", &q[..s.start], s.words[0]), "cat x:lol ");
    }

    #[test]
    fn the_field_being_typed() {
        assert_eq!(Field::typed("cat x:lo"), Some(Field::Text));
        assert_eq!(Field::typed("-t:re"), Some(Field::Tag));
        assert_eq!(Field::typed("e:"), Some(Field::Emoji));
        assert_eq!(Field::typed("cat"), None);
        assert_eq!(Field::typed("x:lo "), None);
        assert_eq!(Field::typed("http://x"), None);
    }

    #[test]
    fn texts_compare_by_the_index_they_read() {
        let a = texts();
        assert_eq!(a, a.clone());
        assert_ne!(a, texts(), "another index");
        assert_ne!(a, vocab());
        assert_eq!(vocab(), vocab());
        // A pack's words first, then the others.
        let item = |id: &str, text: &str| ItemSummary {
            id: id.into(),
            text: text.into(),
            ..crate::model::tests::item(id)
        };
        let pack =
            Vocabulary::default().with_texts(Arc::new(crate::within::Within::new(&[item("p", "loud")], None, None)));
        let both = pack.then(&a);
        assert_eq!(words_in("x:lo", &both), ["loud", "lol", "love", "hello"]);
        assert_eq!(format!("{:?}", both.texts), "Texts(index)");
        assert_eq!(format!("{:?}", vocab().texts), "Texts(none)");
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
        assert_eq!(
            resolve("cat x: -TEXT: is: by:", &v).0,
            "cat",
            "of every key, text: too (not the word text)"
        );
        assert_eq!(resolve("cat http: x:", &v).0, "cat http:", "no key: a word");
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
            ["😂", "❤️", "🐱", "🐶", "👍", "👍🏽"],
            "most used first, ties in the table's order; ❤ and ❤️ are one, spelled ❤️"
        );
        let mut reversed = items.clone();
        reversed.reverse();
        assert_eq!(Vocabulary::from_items(&reversed), v, "in any order");

        // The viewer's own items give the emojis; all give the rest.
        let theirs = ItemSummary {
            owner_username: "bob".into(),
            vibe: Some("tired".into()),
            ..item(&["💀", "💀", "☠"])
        };
        let mine = [item(&["🐸"]), theirs.clone(), item(&["☠️", "🐸"])];
        let v = Vocabulary::for_viewer(&mine, Some("me"));
        assert_eq!(v.emojis, ["🐸", "☠️"]);
        assert_eq!(v.owners, ["me", "bob"]);
        assert_eq!(v.vibes, ["tired"]);
        assert_eq!(v.me.as_deref(), Some("me"));
        assert_eq!(
            Vocabulary::for_viewer(&mine, None).emojis,
            ["💀", "☠️", "🐸"],
            "nobody known: every item's, ties in the table's order"
        );
        assert_eq!(Vocabulary::for_viewer(&mine, None), Vocabulary::from_items(&mine));
        let only = Vocabulary {
            emojis: vec!["😂".into()],
            ..Vocabulary::default()
        };
        assert!(!only.is_empty());
    }
}
