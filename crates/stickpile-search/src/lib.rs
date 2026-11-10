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
//!    keywords, emojis, vibe, tags and set name, with exact > prefix >
//!    squashed (`naahh` == `nah`) > fuzzy (edit distance) matches, plus
//!    caller-supplied expansion terms (vibe synonyms, zero-shot related terms)
//!    at reduced weight.
//! 3. Vector score: cosine between the caller-supplied query text embedding
//!    and each item's image embedding.
//! 4. Reciprocal-rank fusion of the two rankings (k = 60); ties broken by
//!    copies, then recency. `sort:` other than relevance overrides the order.

use std::collections::HashSet;
use std::sync::Arc;

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

#[derive(Debug, Default)]
pub struct Index {
    _private: (),
}

impl Index {
    pub fn new() -> Index {
        todo!("search agent")
    }
    pub fn len(&self) -> usize {
        todo!("search agent")
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Insert or replace by `item_id`.
    pub fn upsert(&mut self, doc: Doc) {
        let _ = doc;
        todo!("search agent")
    }
    pub fn remove(&mut self, item_id: &str) {
        let _ = item_id;
        todo!("search agent")
    }
    pub fn remove_set(&mut self, set_id: &str) {
        let _ = set_id;
        todo!("search agent")
    }
    pub fn get(&self, item_id: &str) -> Option<&Doc> {
        let _ = item_id;
        todo!("search agent")
    }
    /// Update the counters of an item without re-indexing text.
    pub fn set_counters(&mut self, item_id: &str, likes: u64, copies: u64) {
        let _ = (item_id, likes, copies);
        todo!("search agent")
    }
    /// Set (or clear) the embedding of every doc with this media id.
    pub fn set_embedding(&mut self, media_id: &str, embedding: Option<Arc<[f32]>>) {
        let _ = (media_id, embedding);
        todo!("search agent")
    }
    pub fn search(&self, query: &Query, viewer: &Viewer, opts: &SearchOptions) -> Outcome {
        let _ = (query, viewer, opts);
        todo!("search agent")
    }
}

/// Lowercase, NFKD, strip combining marks: `Café` -> `cafe`. Non-Latin scripts
/// are kept (Cyrillic stays Cyrillic).
pub fn normalize(text: &str) -> String {
    let _ = text;
    todo!("search agent")
}

/// Collapse runs of the same letter: `naahh` -> `nah`, `sooo` -> `so`.
pub fn squash(word: &str) -> String {
    let _ = word;
    todo!("search agent")
}
