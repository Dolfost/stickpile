//! Hybrid search over item metadata and embeddings (docs/search.md).
//!
//! The index is in memory and independent of the database: the server feeds
//! it from SQLite at startup and on every write; clients feed it from their
//! offline cache (without embeddings). Ranking:
//!
//! 1. Filters from [`Query`] (`e:`, `set:`, `vibe:`, `tag:`, `type:`, `q>`,
//!    `likes>`, `by:`, `is:`) and visibility (own items + public sets; unlisted
//!    sets are never searchable by non-owners) cut the candidate set.
//! 2. Lexical score: per-field weighted BM25-style scoring over name,
//!    keywords, emojis (plus a small built-in emoji vocabulary, 😂 -> laugh),
//!    vibe, tags and set name, with exact > squashed (`naahh` == `nah`) >
//!    prefix > fuzzy (edit distance) > substring matches, a coordination
//!    factor for multi-word queries, plus caller-supplied expansion terms
//!    (vibe synonyms, zero-shot related terms) at reduced weight.
//! 3. Vector score: cosine between the caller-supplied query text embedding
//!    and each item's image embedding.
//! 4. Reciprocal-rank fusion of the two rankings (k = 60); ties broken by
//!    copies, then recency. `sort:` other than relevance overrides the order.

mod emoji_names;
mod rank;
mod store;
#[cfg(test)]
mod tests;
mod text;

use std::collections::HashSet;
use std::sync::Arc;

pub use rank::DEFAULT_LIMIT;
pub use stickpile_api::query::{Query, Sort};
use stickpile_api::{MediaFormat, MediaKind, Visibility};

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
    /// `name`, `keyword`, `emoji`, `vibe`, `tag`, `set`, `fuzzy`, `expansion`, `image`.
    pub matched: Vec<&'static str>,
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
