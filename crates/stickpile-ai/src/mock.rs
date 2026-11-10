//! Deterministic, model-free [`Enricher`] for tests: text embeddings are
//! hashed bags of words, image embeddings derive from coarse colour
//! statistics. Same input always gives the same normalised vector.
//!
//! The first [`MockEnricher::COLOUR_DIMS`] dimensions are shared between the
//! two modalities: an image puts the fraction of its pixels nearest to each
//! named colour there, and a colour word in a text ("red", "a red square")
//! puts weight on the same dimension. So "red" is close to an all-red image
//! and far from an all-blue one, which makes zero-shot tests meaningful
//! without a model. Every other word hashes (FNV-1a) into the remaining
//! dimensions; a few prompt filler words ("a", "meme of", ...) are ignored so
//! prompt templates do not dilute the signal.

use image::RgbaImage;

use crate::{l2_normalize, AiError, Enricher};

pub struct MockEnricher {
    pub dim: usize,
}

impl Default for MockEnricher {
    fn default() -> Self {
        MockEnricher { dim: 64 }
    }
}

/// Named colours and their reference RGB (the image side picks the nearest).
const PALETTE: [(&str, [u8; 3]); 10] = [
    ("red", [220, 30, 30]),
    ("orange", [245, 140, 20]),
    ("yellow", [245, 225, 40]),
    ("green", [40, 170, 50]),
    ("blue", [30, 70, 220]),
    ("purple", [140, 50, 190]),
    ("pink", [245, 150, 200]),
    ("brown", [120, 70, 30]),
    ("white", [250, 250, 250]),
    ("black", [10, 10, 10]),
];

/// Words that carry no meaning in prompts.
const STOPWORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "of",
    "meme",
    "sticker",
    "picture",
    "photo",
    "image",
    "emoji",
    "reaction",
    "with",
    "and",
    "that",
    "is",
    "it",
    "feels",
    "saying",
    "someone",
    "something",
];

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

fn colour_index(word: &str) -> Option<usize> {
    let word = if word == "grey" || word == "gray" {
        "black"
    } else {
        word
    };
    PALETTE.iter().position(|(n, _)| *n == word)
}

impl MockEnricher {
    /// Dimensions reserved for colour features.
    pub const COLOUR_DIMS: usize = PALETTE.len();
    /// Smallest supported `dim`.
    pub const MIN_DIM: usize = 16;

    fn check_dim(&self) -> Result<(), AiError> {
        if self.dim < Self::MIN_DIM {
            return Err(AiError::Inference(format!("mock dim {} < {}", self.dim, Self::MIN_DIM)));
        }
        Ok(())
    }
}

impl Enricher for MockEnricher {
    fn model_id(&self) -> &str {
        match self.dim {
            16 => "mock-16",
            32 => "mock-32",
            64 => "mock-64",
            128 => "mock-128",
            256 => "mock-256",
            512 => "mock-512",
            _ => "mock",
        }
    }
    fn dim(&self) -> usize {
        self.dim
    }
    fn embed_image(&self, rgba: &RgbaImage) -> Result<Vec<f32>, AiError> {
        self.check_dim()?;
        if rgba.width() == 0 || rgba.height() == 0 {
            return Err(AiError::Inference("empty image".into()));
        }
        let flat = crate::preprocess::flatten_alpha(rgba);
        let mut v = vec![0f32; self.dim];
        let total = (flat.width() * flat.height()) as f32;
        let (mut sum_r, mut sum_g, mut sum_b) = (0f32, 0f32, 0f32);
        for p in flat.pixels() {
            let nearest = PALETTE
                .iter()
                .enumerate()
                .min_by_key(|(_, (_, c))| (0..3).map(|i| (p[i] as i32 - c[i] as i32).pow(2)).sum::<i32>())
                .map(|(i, _)| i)
                .unwrap_or(0);
            v[nearest] += 1.0 / total;
            sum_r += p[0] as f32;
            sum_g += p[1] as f32;
            sum_b += p[2] as f32;
        }
        // A weak brightness/tint signal in the hashed region keeps different
        // images with the same palette distinguishable without hurting colour matches.
        let mean = [sum_r, sum_g, sum_b].map(|s| s / total / 255.0);
        for (i, m) in mean.iter().enumerate() {
            let slot = Self::COLOUR_DIMS + (fnv1a(&format!("#img-mean-{i}")) as usize % (self.dim - Self::COLOUR_DIMS));
            v[slot] += 0.05 * m;
        }
        l2_normalize(&mut v);
        Ok(v)
    }
    fn embed_text(&self, text: &str) -> Result<Vec<f32>, AiError> {
        self.check_dim()?;
        let mut v = vec![0f32; self.dim];
        let lower = text.to_lowercase();
        let mut any = false;
        for word in lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
            if STOPWORDS.contains(&word) {
                continue;
            }
            any = true;
            match colour_index(word) {
                Some(i) => v[i] += 1.0,
                None => {
                    let h = fnv1a(word);
                    v[Self::COLOUR_DIMS + (h as usize % (self.dim - Self::COLOUR_DIMS))] += 1.0;
                }
            }
        }
        if !any {
            // Symbols only / stopwords only: hash the whole text so the result is still a unit vector.
            let h = fnv1a(&lower);
            v[Self::COLOUR_DIMS + (h as usize % (self.dim - Self::COLOUR_DIMS))] = 1.0;
        }
        l2_normalize(&mut v);
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cosine;
    use image::Rgba;

    fn solid(c: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(32, 32, Rgba(c))
    }

    fn norm(v: &[f32]) -> f32 {
        v.iter().map(|x| x * x).sum::<f32>().sqrt()
    }

    #[test]
    fn deterministic_and_normalised() {
        let m = MockEnricher::default();
        let a = m.embed_text("a cat on a red sofa").unwrap();
        assert_eq!(a, m.embed_text("a cat on a red sofa").unwrap());
        assert_eq!(a.len(), 64);
        assert!((norm(&a) - 1.0).abs() < 1e-5);
        for t in ["", "a", "!!!", "😂"] {
            assert!((norm(&m.embed_text(t).unwrap()) - 1.0).abs() < 1e-5, "{t:?}");
        }
        let img = m.embed_image(&solid([200, 10, 10, 255])).unwrap();
        assert_eq!(img, m.embed_image(&solid([200, 10, 10, 255])).unwrap());
        assert!((norm(&img) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn colour_words_match_colour_images() {
        let m = MockEnricher::default();
        let red = m.embed_image(&solid([230, 20, 20, 255])).unwrap();
        let blue = m.embed_image(&solid([20, 40, 230, 255])).unwrap();
        let t_red = m.embed_text("a red square").unwrap();
        let t_blue = m.embed_text("a blue square").unwrap();
        assert!(cosine(&red, &t_red) > cosine(&red, &t_blue));
        assert!(cosine(&blue, &t_blue) > cosine(&blue, &t_red));
        assert!(cosine(&red, &m.embed_text("red").unwrap()) > 0.99);
        // Transparent pixels count as white.
        let clear = m.embed_image(&solid([255, 0, 0, 0])).unwrap();
        assert!(cosine(&clear, &m.embed_text("white").unwrap()) > 0.99);
    }

    #[test]
    fn bag_of_words_similarity() {
        let m = MockEnricher::default();
        let a = m.embed_text("cat").unwrap();
        assert!(cosine(&a, &m.embed_text("a meme of cat").unwrap()) > 0.99);
        assert!(cosine(&a, &m.embed_text("CAT!").unwrap()) > 0.99);
        assert!(cosine(&a, &m.embed_text("cat dog").unwrap()) > 0.5);
    }

    #[test]
    fn small_dim_is_an_error() {
        let m = MockEnricher { dim: 8 };
        assert!(m.embed_text("x").is_err());
        assert!(m.embed_image(&solid([0, 0, 0, 255])).is_err());
        assert_eq!(MockEnricher { dim: 512 }.model_id(), "mock-512");
    }
}
