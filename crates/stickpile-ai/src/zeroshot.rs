//! Zero-shot labelling: cosine similarity between an image embedding and
//! pre-computed text embeddings of the vocabulary prompts.

use crate::{AiError, Enricher, Vocabulary};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Suggestions {
    /// (keyword, score) best first.
    pub keywords: Vec<(String, f32)>,
    pub emojis: Vec<(String, f32)>,
    pub vibe: Option<(String, f32)>,
}

pub struct ZeroShot {
    pub vocab: Vocabulary,
    model_id: String,
}

impl ZeroShot {
    /// Embeds every vocabulary prompt once (a few hundred texts).
    pub fn new(enricher: &dyn Enricher, vocab: Vocabulary) -> Result<ZeroShot, AiError> {
        let _ = (enricher, vocab);
        todo!("ai agent")
    }
    pub fn model_id(&self) -> &str {
        &self.model_id
    }
    pub fn suggest(&self, image_embedding: &[f32], max_keywords: usize, max_emojis: usize) -> Suggestions {
        let _ = (image_embedding, max_keywords, max_emojis);
        todo!("ai agent")
    }
    /// Vocabulary terms (keywords and vibe labels) closest to a query text
    /// embedding, for search query expansion.
    pub fn related_terms(&self, text_embedding: &[f32], k: usize) -> Vec<(String, f32)> {
        let _ = (text_embedding, k);
        todo!("ai agent")
    }
}
