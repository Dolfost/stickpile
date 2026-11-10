//! CLIP ViT-B/32 via ONNX Runtime (`ort`, dynamically loaded).

use crate::{AiError, ClipOptions, Enricher};

pub struct ClipEnricher {
    model_id: String,
}

impl ClipEnricher {
    pub fn load(opts: &ClipOptions) -> Result<ClipEnricher, AiError> {
        let _ = opts;
        todo!("ai agent")
    }
}

impl Enricher for ClipEnricher {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn dim(&self) -> usize {
        512
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
