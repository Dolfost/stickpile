//! Deterministic, model-free [`Enricher`] for tests: text embeddings are
//! hashed bags of words, image embeddings derive from coarse colour
//! statistics. Same input always gives the same normalised vector.

use crate::{AiError, Enricher};

pub struct MockEnricher {
    pub dim: usize,
}

impl Default for MockEnricher {
    fn default() -> Self {
        MockEnricher { dim: 64 }
    }
}

impl Enricher for MockEnricher {
    fn model_id(&self) -> &str {
        "mock-64"
    }
    fn dim(&self) -> usize {
        self.dim
    }
    fn embed_image(&self, rgba: &image::RgbaImage) -> Result<Vec<f32>, AiError> {
        let _ = rgba;
        todo!("ai agent")
    }
    fn embed_text(&self, text: &str) -> Result<Vec<f32>, AiError> {
        let _ = text;
        todo!("ai agent")
    }
}
