//! Hybrid search over item metadata and embeddings (docs/search.md).
//!
//! The index is in memory and independent of the database: the server feeds
//! it from SQLite at startup and on every write; clients feed it from their
//! offline cache (without embeddings). Ranking:
//!
//! 1. Filters from [`Query`] (`e:`, `set:`, `vibe:`, `tag:`, `text:`, `type:`,
//!    `q>`, `likes>`, `by:`, `is:`) and visibility (own items + public sets;
//!    unlisted sets are never searchable by non-owners) cut the candidate
//!    set. An `e:` value is an emoji or a name ([`emoji_names::by_name`]:
//!    `e:skull` is 💀 or ☠️, ...). A `text:` value's words must appear in
//!    the item's text, consecutive, each matched as plain words are (a
//!    typo, squashed, inside a word) and the last also by its start (it is
//!    being typed: `x:l` finds "lol"); `-text:` takes the words or the
//!    start of the last only. A query with a `text:` value lists the best
//!    matches of it first ([`Hit::text_tier`]).
//! 2. Lexical score: per-field weighted BM25-style scoring over name,
//!    keywords, emojis (plus a small built-in emoji vocabulary, 😂 -> laugh),
//!    vibe, tags, set name and the text (the words in the picture, weighted
//!    below name and keywords), with exact > squashed (`naahh` == `nah`) >
//!    prefix > fuzzy (edit distance) > substring matches, a coordination
//!    factor for multi-word queries, plus caller-supplied expansion terms
//!    (vibe synonyms, zero-shot related terms) at reduced weight.
//! 3. Vector score: cosine between the caller-supplied query text embedding
//!    and each item's image embedding.
//! 4. Reciprocal-rank fusion of the two rankings (k = 60); ties broken by
//!    copies, then recency. `sort:` other than relevance overrides the order.
//!
//! The words written on the items are offered while a `text:` value is
//! typed ([`Index::text_words`], [`TextWords`]): those of the items the
//! viewer can search, most common first.

pub mod emoji_names;
mod rank;
#[cfg(test)]
mod review_tests;
mod store;
#[cfg(any(test, feature = "synthetic"))]
#[doc(hidden)]
pub mod synthetic;
#[cfg(test)]
mod tests;
mod text;
#[cfg(test)]
mod text_tests;
mod words;

use std::collections::HashSet;
use std::sync::Arc;

pub use rank::DEFAULT_LIMIT;
pub use stickpile_api::query::{Query, Sort};
use stickpile_api::{MediaFormat, MediaKind, Visibility};
pub use words::{offer_tier, text_word, typo_budget};

/// Everything search needs to know about one item.
#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    pub item_id: String,
    pub set_id: String,
    pub set_slug: String,
    pub set_name: String,
    pub owner_id: String,
    pub owner_username: String,
    pub visibility: Visibility,
    pub name: String,
    pub keywords: Vec<String>,
    pub emojis: Vec<String>,
    pub vibe: Option<String>,
    pub tags: Vec<String>,
    /// The words written in the picture (OCR or typed); may hold line breaks.
    pub text: String,
    pub kind: MediaKind,
    pub format: MediaFormat,
    pub quality: Option<u8>,
    pub likes: u64,
    pub copies: u64,
    pub media_id: String,
    /// L2-normalised image embedding, if computed.
    pub embedding: Option<Arc<[f32]>>,
    pub updated_at: i64,
}

/// Who is searching.
#[derive(Debug, Clone, Default)]
pub struct Viewer {
    pub user_id: String,
    /// Item ids the viewer liked (for `is:liked`).
    pub liked: HashSet<String>,
    /// Sets the viewer contributes to: searchable like their own.
    pub member_sets: HashSet<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// Page size; 0 means [`DEFAULT_LIMIT`]. Not capped here (the server caps it).
    pub limit: usize,
    pub offset: usize,
    /// Query text embedding in the image embedding space (from the AI backend).
    pub text_embedding: Option<Vec<f32>>,
    /// Extra lexical terms with weights in 0..=1 (vibe synonyms, related terms).
    pub expansions: Vec<(String, f32)>,
    /// Minimum cosine for an embedding-only match to count.
    pub min_similarity: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub item_id: String,
    pub score: f32,
    /// `name`, `keyword`, `emoji`, `vibe`, `tag`, `set`, `fuzzy`, `expansion`,
    /// `image`, `text`.
    pub matched: Vec<&'static str>,
    /// How well the item's text matched the query's `text:` values, the
    /// first sort key of the relevance order: 0 the words themselves, 1 the
    /// start of the last one, 2 squashed, 3 a typo, 4 inside a word; 0
    /// without a `text:` value.
    pub text_tier: u8,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outcome {
    pub hits: Vec<Hit>,
    /// Matches before limit/offset.
    pub total: usize,
}

/// The in-memory index. Not internally synchronised: wrap it in a lock to share.
#[derive(Debug, Default, Clone)]
pub struct Index {
    store: store::Store,
}

impl Index {
    pub fn new() -> Index {
        Index::default()
    }
    pub fn len(&self) -> usize {
        self.store.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Insert or replace by `item_id`.
    pub fn upsert(&mut self, doc: Doc) {
        self.store.upsert(doc);
    }
    pub fn remove(&mut self, item_id: &str) {
        self.store.remove(item_id);
    }
    pub fn remove_set(&mut self, set_id: &str) {
        self.store.remove_set(set_id);
    }
    pub fn get(&self, item_id: &str) -> Option<&Doc> {
        self.store.get(item_id)
    }
    /// Every indexed doc, in no particular order.
    pub fn docs(&self) -> impl Iterator<Item = &Doc> {
        self.store.slots.iter().flatten().map(|s| &s.doc)
    }
    /// Update the counters of an item without re-indexing text.
    pub fn set_counters(&mut self, item_id: &str, likes: u64, copies: u64) {
        self.store.set_counters(item_id, likes, copies);
    }
    /// Set (or clear) the embedding of every doc with this media id.
    pub fn set_embedding(&mut self, media_id: &str, embedding: Option<Arc<[f32]>>) {
        self.store.set_embedding(media_id, embedding);
    }
    pub fn search(&self, query: &Query, viewer: &Viewer, opts: &SearchOptions) -> Outcome {
        self.store.search(query, viewer, opts)
    }

    /// The words written on the items `viewer` can search (their `text`)
    /// to offer while the `text:` value `typed` is typed, best first, at
    /// most `limit`: the word itself and those starting with it, those with
    /// it inside, those a typo or two away ([`offer_tier`]), each group the
    /// words in the most items first (a word counts once per item), then
    /// alphabetically. Nothing typed: the words in the most items. Words
    /// of one letter and numbers are not offered, nor anything for a value
    /// of several words or none ([`text_word`]). The words are as the
    /// index keeps them: lower case, without accents and apostrophes
    /// (`пятниця`), which find what they were written as.
    pub fn text_words(&self, typed: &str, viewer: &Viewer, limit: usize) -> Vec<String> {
        self.store.text_words(typed, viewer, limit)
    }
}

/// Where the words written on the items come from for a suggestion line
/// (`x:`): an index as one viewer searches it ([`ViewedIndex`]), a
/// client's offline index, the items of an opened pack. Each call reads
/// the index ([`Index::text_words`]).
pub trait TextWords: Send + Sync {
    /// The words to offer for the `text:` value `typed`, best first, at
    /// most `limit` ([`Index::text_words`]).
    fn text_words(&self, typed: &str, limit: usize) -> Vec<String>;
}

/// An index as one viewer searches it: the words of the items the viewer
/// can search.
#[derive(Debug, Clone, Copy)]
pub struct ViewedIndex<'a> {
    pub index: &'a Index,
    pub viewer: &'a Viewer,
}

impl TextWords for ViewedIndex<'_> {
    fn text_words(&self, typed: &str, limit: usize) -> Vec<String> {
        self.index.text_words(typed, self.viewer, limit)
    }
}

/// Lowercase, NFKD, strip combining marks: `Café` -> `cafe`. Non-Latin scripts
/// are kept (Cyrillic stays Cyrillic).
pub fn normalize(text: &str) -> String {
    text::normalize(text)
}

/// Collapse runs of the same letter: `naahh` -> `nah`, `sooo` -> `so`.
pub fn squash(word: &str) -> String {
    text::squash(word)
}
