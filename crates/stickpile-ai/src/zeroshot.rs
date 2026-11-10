//! Zero-shot labelling: cosine similarity between an image embedding and
//! pre-computed text embeddings of the vocabulary prompts.
//!
//! * Keywords: the mean of the (normalised) embeddings of
//!   [`KEYWORD_TEMPLATES`] filled with the keyword (prompt ensembling).
//! * Emojis: the mean over [`EMOJI_TEMPLATES`] filled with the description.
//! * Vibes: a prototype per label, mixing the label templates
//!   ([`VIBE_TEMPLATES`]) with the bare synonyms; synonyms get
//!   [`ZeroShotParams::synonym_weight`] of the total.
//!
//! Selection (calibration in docs/ai.md): keywords and emojis are kept while
//! they are within `*_margin` cosine of the best match and above
//! `*_min_similarity`; the vibe is a softmax over the prototypes with CLIP's
//! logit scale (100) and is only reported above `vibe_min_confidence`.
//!
//! Templates and thresholds were chosen on CLIP ViT-B/32 (both text
//! encoders) with a photo and 30 transparent emoji stickers; see docs/ai.md.

use crate::{cosine, l2_normalize, AiError, Enricher, Vocabulary};

/// Prompts for keywords; `{}` is replaced by the keyword.
pub const KEYWORD_TEMPLATES: [&str; 3] = ["a meme of {}", "a sticker of {}", "a picture of {}"];
/// Prompts for emojis; `{}` is replaced by the description.
pub const EMOJI_TEMPLATES: [&str; 2] = ["{}", "a sticker of {}"];
/// Prompts for vibe labels; `{}` is replaced by the label. Synonyms are
/// embedded bare (they double as [`ZeroShot::related_terms`] targets).
pub const VIBE_TEMPLATES: [&str; 2] = ["a {} meme", "a {} reaction sticker"];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Suggestions {
    /// (keyword, score) best first.
    pub keywords: Vec<(String, f32)>,
    pub emojis: Vec<(String, f32)>,
    pub vibe: Option<(String, f32)>,
}

/// Selection thresholds. Scores of keywords and emojis are raw cosine
/// similarities (CLIP ViT-B/32 image-text pairs typically land in 0.15..0.35);
/// the vibe score is a softmax probability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZeroShotParams {
    /// Keep keywords scoring at least `best - keyword_margin`.
    pub keyword_margin: f32,
    /// ... and at least this absolute cosine.
    pub keyword_min_similarity: f32,
    pub emoji_margin: f32,
    pub emoji_min_similarity: f32,
    /// Softmax temperature for vibes (CLIP's learned logit scale is 100).
    pub vibe_logit_scale: f32,
    /// Report a vibe only if its probability is at least this.
    pub vibe_min_confidence: f32,
    /// Share of the synonym prompts in a vibe prototype (0 = label only).
    pub synonym_weight: f32,
}

impl Default for ZeroShotParams {
    fn default() -> Self {
        ZeroShotParams {
            keyword_margin: 0.025,
            keyword_min_similarity: 0.22,
            emoji_margin: 0.02,
            emoji_min_similarity: 0.22,
            vibe_logit_scale: 100.0,
            vibe_min_confidence: 0.25,
            synonym_weight: 0.3,
        }
    }
}

/// Row-major matrix of unit vectors.
#[derive(Debug, Clone, Default)]
struct Matrix {
    dim: usize,
    data: Vec<f32>,
}

impl Matrix {
    fn push(&mut self, v: &[f32]) {
        debug_assert_eq!(v.len(), self.dim);
        self.data.extend_from_slice(v);
    }
    fn len(&self) -> usize {
        self.data.len().checked_div(self.dim).unwrap_or(0)
    }
    fn scores(&self, q: &[f32]) -> Vec<f32> {
        self.data.chunks_exact(self.dim).map(|row| cosine(row, q)).collect()
    }
}

pub struct ZeroShot {
    pub vocab: Vocabulary,
    model_id: String,
    dim: usize,
    params: ZeroShotParams,
    /// One prompt-ensemble prototype per keyword.
    keywords: Matrix,
    /// The bare keyword, for text-to-text [`ZeroShot::related_terms`].
    keyword_terms: Matrix,
    emojis: Matrix,
    vibes: Matrix,
    /// Bare label and synonyms of each vibe: (vibe index, embedding row).
    vibe_terms: Vec<usize>,
    vibe_term_emb: Matrix,
}

/// Mean of unit vectors, renormalised.
fn mean_unit(rows: &[Vec<f32>], dim: usize) -> Vec<f32> {
    let mut acc = vec![0f32; dim];
    for r in rows {
        acc.iter_mut().zip(r).for_each(|(a, x)| *a += x);
    }
    l2_normalize(&mut acc);
    acc
}

fn fill(template: &str, term: &str) -> String {
    template.replace("{}", term)
}

impl ZeroShot {
    /// Embeds every vocabulary prompt once (a few hundred texts).
    pub fn new(enricher: &dyn Enricher, vocab: Vocabulary) -> Result<ZeroShot, AiError> {
        Self::with_params(enricher, vocab, ZeroShotParams::default())
    }

    pub fn with_params(
        enricher: &dyn Enricher,
        vocab: Vocabulary,
        params: ZeroShotParams,
    ) -> Result<ZeroShot, AiError> {
        vocab.validate()?;
        let dim = enricher.dim();
        // Collect every prompt, embed in one batch call, then slice.
        let mut texts: Vec<String> = Vec::new();
        let mut take = |items: Vec<String>| {
            let start = texts.len();
            texts.extend(items);
            start..texts.len()
        };
        let kw_ranges: Vec<_> = vocab
            .keywords
            .iter()
            .map(|k| take(KEYWORD_TEMPLATES.iter().map(|t| fill(t, k)).collect()))
            .collect();
        let kw_term_ranges: Vec<_> = vocab.keywords.iter().map(|k| take(vec![k.clone()])).collect();
        let emoji_ranges: Vec<_> = vocab
            .emojis
            .iter()
            .map(|e| take(EMOJI_TEMPLATES.iter().map(|t| fill(t, &e.description)).collect()))
            .collect();
        let vibe_label_ranges: Vec<_> = vocab
            .vibes
            .iter()
            .map(|v| take(VIBE_TEMPLATES.iter().map(|t| fill(t, &v.label)).collect()))
            .collect();
        let mut vibe_terms = Vec::new();
        let vibe_term_ranges: Vec<_> = vocab
            .vibes
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let terms: Vec<String> = std::iter::once(v.label.clone())
                    .chain(v.synonyms.iter().cloned())
                    .collect();
                vibe_terms.extend(std::iter::repeat_n(i, terms.len()));
                take(terms)
            })
            .collect();

        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let emb = enricher.embed_texts(&refs)?;
        if emb.len() != texts.len() || emb.iter().any(|e| e.len() != dim) {
            return Err(AiError::Inference(
                "enricher returned embeddings of the wrong shape".into(),
            ));
        }
        let rows = |r: &std::ops::Range<usize>| &emb[r.clone()];
        let new_matrix = || Matrix { dim, data: Vec::new() };

        let mut keywords = new_matrix();
        kw_ranges.iter().for_each(|r| keywords.push(&mean_unit(rows(r), dim)));
        let mut keyword_terms = new_matrix();
        kw_term_ranges.iter().for_each(|r| keyword_terms.push(&emb[r.start]));
        let mut emojis = new_matrix();
        emoji_ranges.iter().for_each(|r| emojis.push(&mean_unit(rows(r), dim)));
        let mut vibes = new_matrix();
        for (lr, tr) in vibe_label_ranges.iter().zip(&vibe_term_ranges) {
            let label = mean_unit(rows(lr), dim);
            // The first term is the bare label, the rest are the synonyms.
            let sr = tr.start + 1..tr.end;
            let w = if sr.is_empty() {
                0.0
            } else {
                params.synonym_weight.clamp(0.0, 1.0)
            };
            let syn = mean_unit(rows(&sr), dim);
            let mut proto: Vec<f32> = label.iter().zip(&syn).map(|(l, s)| (1.0 - w) * l + w * s).collect();
            l2_normalize(&mut proto);
            vibes.push(&proto);
        }
        let mut vibe_term_emb = new_matrix();
        for r in &vibe_term_ranges {
            rows(r).iter().for_each(|e| vibe_term_emb.push(e));
        }

        tracing::debug!(
            prompts = texts.len(),
            model = enricher.model_id(),
            "zero-shot vocabulary embedded"
        );
        Ok(ZeroShot {
            vocab,
            model_id: enricher.model_id().to_owned(),
            dim,
            params,
            keywords,
            keyword_terms,
            emojis,
            vibes,
            vibe_terms,
            vibe_term_emb,
        })
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn params(&self) -> &ZeroShotParams {
        &self.params
    }

    /// See [`Vocabulary::fingerprint`].
    pub fn vocab_fingerprint(&self) -> String {
        self.vocab.fingerprint()
    }

    pub fn suggest(&self, image_embedding: &[f32], max_keywords: usize, max_emojis: usize) -> Suggestions {
        if image_embedding.len() != self.dim {
            tracing::warn!(
                got = image_embedding.len(),
                want = self.dim,
                "embedding dimension mismatch"
            );
            return Suggestions::default();
        }
        let p = &self.params;
        let keywords = select(
            &self.keywords.scores(image_embedding),
            max_keywords,
            p.keyword_margin,
            p.keyword_min_similarity,
        )
        .into_iter()
        .map(|(i, s)| (self.vocab.keywords[i].clone(), s))
        .collect();
        let emojis = select(
            &self.emojis.scores(image_embedding),
            max_emojis,
            p.emoji_margin,
            p.emoji_min_similarity,
        )
        .into_iter()
        .map(|(i, s)| (self.vocab.emojis[i].emoji.clone(), s))
        .collect();
        let vibe = self.vibe_probabilities(image_embedding).and_then(|probs| {
            let (i, &pmax) = probs.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
            (pmax >= p.vibe_min_confidence).then(|| (self.vocab.vibes[i].label.clone(), pmax))
        });
        Suggestions { keywords, emojis, vibe }
    }

    /// Softmax over the vibe prototypes (vocabulary order), or None without vibes.
    pub fn vibe_probabilities(&self, image_embedding: &[f32]) -> Option<Vec<f32>> {
        if self.vibes.len() == 0 || image_embedding.len() != self.dim {
            return None;
        }
        let logits: Vec<f32> = self
            .vibes
            .scores(image_embedding)
            .iter()
            .map(|s| s * self.params.vibe_logit_scale)
            .collect();
        Some(softmax(&logits))
    }

    /// Vocabulary terms (keywords and vibe labels) closest to a query text
    /// embedding, for search query expansion.
    ///
    /// Text-to-text cosine against the bare keyword and against each vibe's
    /// label and synonyms (a vibe scores its best term), best first. CLIP
    /// text-text cosines are high even for unrelated words ("cat"~"truck"
    /// 0.79, "kitty"~"puppy" 0.91, "cat"~"kitten" 0.94; an exact synonym
    /// ~1.0), so use the top 1-3 or those close to the best (docs/ai.md).
    pub fn related_terms(&self, text_embedding: &[f32], k: usize) -> Vec<(String, f32)> {
        if text_embedding.len() != self.dim || k == 0 {
            return Vec::new();
        }
        let mut out: Vec<(String, f32)> = self
            .keyword_terms
            .scores(text_embedding)
            .into_iter()
            .enumerate()
            .map(|(i, s)| (self.vocab.keywords[i].clone(), s))
            .collect();
        let mut best = vec![f32::NEG_INFINITY; self.vocab.vibes.len()];
        for (row, s) in self.vibe_term_emb.scores(text_embedding).into_iter().enumerate() {
            let v = self.vibe_terms[row];
            best[v] = best[v].max(s);
        }
        for (v, s) in best.into_iter().enumerate() {
            let label = &self.vocab.vibes[v].label;
            match out.iter_mut().find(|(t, _)| t == label) {
                Some(entry) => entry.1 = entry.1.max(s),
                None => out.push((label.clone(), s)),
            }
        }
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out.truncate(k);
        out
    }
}

/// Indices of the best scores, at most `max`, within `margin` of the best and
/// at least `min`.
fn select(scores: &[f32], max: usize, margin: f32, min: f32) -> Vec<(usize, f32)> {
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    idx.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]));
    let Some(&top) = idx.first() else { return Vec::new() };
    let best = scores[top];
    idx.into_iter()
        .take(max)
        .take_while(|&i| scores[i] >= best - margin && scores[i] >= min)
        .map(|i| (i, scores[i]))
        .collect()
}

fn softmax(logits: &[f32]) -> Vec<f32> {
    let m = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp: Vec<f32> = logits.iter().map(|l| (l - m).exp()).collect();
    let sum: f32 = exp.iter().sum();
    exp.iter().map(|e| e / sum).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocab::{EmojiEntry, VibeEntry};
    use crate::MockEnricher;
    use image::{Rgba, RgbaImage};

    fn solid(c: [u8; 3]) -> RgbaImage {
        RgbaImage::from_pixel(48, 48, Rgba([c[0], c[1], c[2], 255]))
    }

    fn colour_vocab() -> Vocabulary {
        Vocabulary {
            keywords: vec![
                "red".into(),
                "blue".into(),
                "green".into(),
                "cat".into(),
                "red car".into(),
            ],
            emojis: vec![
                EmojiEntry {
                    emoji: "🔴".into(),
                    description: "red circle".into(),
                },
                EmojiEntry {
                    emoji: "🔵".into(),
                    description: "blue circle".into(),
                },
                EmojiEntry {
                    emoji: "🐱".into(),
                    description: "cat".into(),
                },
            ],
            vibes: vec![
                VibeEntry {
                    label: "angry".into(),
                    synonyms: vec!["red".into(), "furious".into()],
                },
                VibeEntry {
                    label: "sad".into(),
                    synonyms: vec!["blue".into(), "down".into()],
                },
                VibeEntry {
                    label: "calm".into(),
                    synonyms: vec!["green".into()],
                },
            ],
        }
    }

    fn zs() -> (MockEnricher, ZeroShot) {
        let m = MockEnricher::default();
        let params = ZeroShotParams {
            keyword_margin: 0.3,
            keyword_min_similarity: 0.1,
            emoji_margin: 0.3,
            emoji_min_similarity: 0.1,
            vibe_logit_scale: 10.0,
            vibe_min_confidence: 0.4,
            synonym_weight: 0.5,
        };
        let z = ZeroShot::with_params(&m, colour_vocab(), params).unwrap();
        (m, z)
    }

    #[test]
    fn suggests_matching_colour() {
        let (m, z) = zs();
        let red = m.embed_image(&solid([230, 20, 20])).unwrap();
        let s = z.suggest(&red, 5, 3);
        assert_eq!(s.keywords[0].0, "red");
        // "red car" is related (cos ~0.7), within the margin; blue/green/cat are not.
        let names: Vec<&str> = s.keywords.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, vec!["red", "red car"]);
        assert!(s.keywords.windows(2).all(|w| w[0].1 >= w[1].1));
        assert_eq!(s.emojis.first().map(|e| e.0.as_str()), Some("🔴"));
        let (vibe, p) = s.vibe.clone().unwrap();
        assert_eq!(vibe, "angry");
        assert!(p > 0.4 && p <= 1.0);

        let blue = m.embed_image(&solid([20, 40, 230])).unwrap();
        let s = z.suggest(&blue, 5, 3);
        assert_eq!(s.keywords[0].0, "blue");
        assert_eq!(s.emojis[0].0, "🔵");
        assert_eq!(s.vibe.unwrap().0, "sad");
    }

    #[test]
    fn limits_and_thresholds() {
        let (m, z) = zs();
        let red = m.embed_image(&solid([230, 20, 20])).unwrap();
        assert_eq!(z.suggest(&red, 1, 0).keywords.len(), 1);
        assert!(z.suggest(&red, 0, 0).keywords.is_empty());
        assert!(z.suggest(&red, 5, 0).emojis.is_empty());

        // A pink image matches nothing in this vocabulary: no keyword clears
        // the absolute minimum and the vibe softmax is flat -> no vibe.
        let pink = m.embed_image(&solid([245, 150, 200])).unwrap();
        let s = z.suggest(&pink, 5, 3);
        assert!(s.keywords.is_empty(), "{:?}", s.keywords);
        assert!(s.emojis.is_empty());
        assert_eq!(s.vibe, None);
        let probs = z.vibe_probabilities(&pink).unwrap();
        assert_eq!(probs.len(), 3);
        assert!((probs.iter().sum::<f32>() - 1.0).abs() < 1e-5);

        // Wrong dimension: nothing, no panic.
        assert_eq!(z.suggest(&[1.0, 0.0], 5, 3), Suggestions::default());
        assert!(z.related_terms(&[1.0], 3).is_empty());
    }

    #[test]
    fn related_terms_cover_keywords_and_vibes() {
        let (m, z) = zs();
        let q = m.embed_text("furious").unwrap();
        let r = z.related_terms(&q, 3);
        assert_eq!(r[0].0, "angry");
        assert!(r[0].1 > 0.99);
        let q = m.embed_text("cat").unwrap();
        assert_eq!(
            z.related_terms(&q, 1),
            vec![("cat".to_owned(), z.related_terms(&q, 1)[0].1)]
        );
        // "red" is both a keyword and a synonym of angry: both returned, sorted.
        let r = z.related_terms(&m.embed_text("red").unwrap(), 10);
        let names: Vec<&str> = r.iter().take(2).map(|(t, _)| t.as_str()).collect();
        assert!(names.contains(&"red") && names.contains(&"angry"), "{r:?}");
        assert!(r.windows(2).all(|w| w[0].1 >= w[1].1));
        assert_eq!(z.related_terms(&q, 0), vec![]);
    }

    #[test]
    fn builtin_vocabulary_with_mock() {
        let m = MockEnricher { dim: 128 };
        let z = ZeroShot::new(&m, Vocabulary::builtin()).unwrap();
        assert_eq!(z.model_id(), "mock-128");
        let s = z.suggest(&m.embed_image(&solid([230, 20, 20])).unwrap(), 5, 3);
        assert!(s.keywords.len() <= 5 && s.emojis.len() <= 3);
        assert_eq!(z.vocab_fingerprint(), Vocabulary::builtin().fingerprint());
        // Lexical vibe matching is independent of the model.
        assert_eq!(z.vocab.vibes_for_word("naahh"), vec!["dismissive"]);
    }

    #[test]
    fn empty_sections() {
        let m = MockEnricher::default();
        let v = Vocabulary {
            keywords: vec![],
            emojis: vec![],
            vibes: vec![],
        };
        let z = ZeroShot::new(&m, v).unwrap();
        let s = z.suggest(&m.embed_image(&solid([1, 2, 3])).unwrap(), 5, 5);
        assert_eq!(s, Suggestions::default());
        assert!(z.vibe_probabilities(&m.embed_text("x").unwrap()).is_none());
        assert!(z.related_terms(&m.embed_text("x").unwrap(), 5).is_empty());
    }

    #[test]
    fn select_and_softmax() {
        let s = select(&[0.1, 0.30, 0.28, 0.25, 0.29], 3, 0.03, 0.2);
        assert_eq!(s.iter().map(|x| x.0).collect::<Vec<_>>(), vec![1, 4, 2]);
        let s = select(&[0.1, 0.30, 0.28, 0.25, 0.29], 10, 0.015, 0.2);
        assert_eq!(s.iter().map(|x| x.0).collect::<Vec<_>>(), vec![1, 4]);
        assert!(select(&[0.1, 0.15], 3, 1.0, 0.2).is_empty());
        assert!(select(&[], 3, 1.0, 0.0).is_empty());
        let p = softmax(&[1.0, 1.0]);
        assert!((p[0] - 0.5).abs() < 1e-6);
        let p = softmax(&[1000.0, 0.0]);
        assert!((p[0] - 1.0).abs() < 1e-6 && p[1] >= 0.0);
    }
}
