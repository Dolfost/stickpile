//! CLIP ViT-B/32 via ONNX Runtime (`ort`, dynamically loaded).
//!
//! * Image tower: `clip-vit-b32/vision.onnx` (`pixel_values` -> `image_embeds`).
//! * Text, `clip`: `clip-vit-b32/text.onnx` (`input_ids` -> `text_embeds`),
//!   byte-level BPE, truncated to 77 tokens (BOS/EOS kept).
//! * Text, `multilingual`: DistilBERT `multilingual/model.onnx`
//!   (`input_ids`, `attention_mask` -> `last_hidden_state`), mean pooling over
//!   the attention mask, then `dense.safetensors` (768->512, no bias, identity
//!   activation), truncated to 128 tokens like sentence-transformers.
//!
//! All outputs are L2-normalised.
//!
//! # Threads
//!
//! `ort` 2.0 runs a session through `&mut Session`, so each tower sits behind
//! its own `Mutex`: concurrent `embed_image` calls queue up, while an image and
//! a text can be embedded at the same time. Every run uses `threads` intra-op
//! threads (0 = ONNX Runtime default, one per core). Intra-op spinning is
//! disabled so an idle server does not burn CPU after a request.
//!
//! # Runtime library
//!
//! ONNX Runtime is loaded once per process (`OnceLock`) from
//! `onnxruntime_library`, else `ORT_DYLIB_PATH`, else `libonnxruntime.so` via
//! the dynamic loader. The first real load attempt fixes the outcome for the
//! process (see [`init_runtime`]); later [`ClipEnricher::load`]s reuse it.
//! Failures, including the panics `ort` raises for an unloadable library,
//! surface as [`AiError::Model`].

use std::borrow::Cow;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;
use tokenizers::{Encoding, Tokenizer, TruncationDirection, TruncationParams, TruncationStrategy};

use crate::{
    l2_normalize, layout, missing_model_files, preprocess, AiError, ClipOptions, Enricher, TextEncoderKind, CLIP_DIM,
    IMAGE_MODEL_ID,
};

/// CLIP context length (including BOS and EOS).
pub const CLIP_MAX_TOKENS: usize = 77;
/// sentence-transformers `max_seq_length` of clip-ViT-B-32-multilingual-v1.
pub const MCLIP_MAX_TOKENS: usize = 128;
/// `<|endoftext|>`: CLIP's EOS, also used for padding (as Hugging Face does).
const CLIP_PAD_ID: i64 = 49407;
/// Texts per ONNX run in [`Enricher::embed_texts`].
const TEXT_BATCH: usize = 32;

pub struct ClipEnricher {
    model_id: String,
    vision: Tower,
    text: TextModel,
}

/// One ONNX session with the names of its input and output tensors.
struct Tower {
    session: Mutex<Session>,
    output: String,
}

struct TextModel {
    tower: Tower,
    tokenizer: Tokenizer,
    pad_id: i64,
    /// Name of the attention-mask input, if the graph has one.
    mask_input: Option<String>,
    /// DistilBERT exports sometimes carry `token_type_ids` (all zeros).
    type_ids_input: Option<String>,
    head: TextHead,
}

enum TextHead {
    /// The graph already outputs projected embeddings `[N, 512]`.
    Projected,
    /// `[N, L, H]` hidden states -> masked mean pooling -> dense `[512, H]`.
    MeanPoolDense(Dense),
}

impl std::fmt::Debug for ClipEnricher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipEnricher")
            .field("model_id", &self.model_id)
            .finish_non_exhaustive()
    }
}

static RUNTIME: OnceLock<Result<String, String>> = OnceLock::new();

/// Loads ONNX Runtime and creates the global environment (once per process).
/// Returns the library that was used.
///
/// A path that does not exist is reported without touching ONNX Runtime, so
/// a later call with a corrected path still works. The outcome of an actual
/// load attempt (success or failure) is final for the process: ONNX Runtime
/// cannot be unloaded or swapped, so fixing a broken library needs a restart.
pub fn init_runtime(library: Option<&Path>) -> Result<&'static str, AiError> {
    let path: Option<PathBuf> = library
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("ORT_DYLIB_PATH").map(PathBuf::from));
    if RUNTIME.get().is_none() {
        if let Some(p) = &path {
            if !p.is_file() {
                return Err(AiError::Model(format!(
                    "ONNX Runtime library {} does not exist",
                    p.display()
                )));
            }
        }
    }
    let res = RUNTIME.get_or_init(|| {
        let shown = path
            .as_ref()
            .map_or_else(|| "libonnxruntime.so (system)".to_owned(), |p| p.display().to_string());
        let committed = catch_panic(|| {
            let builder = match &path {
                Some(p) => ort::init_from(p.to_string_lossy()),
                None => ort::init(),
            };
            builder.with_name("stickpile").with_telemetry(false).commit()
        });
        match committed {
            Ok(Ok(_)) => {
                tracing::info!(library = %shown, "ONNX Runtime loaded");
                Ok(shown)
            }
            Ok(Err(e)) => Err(format!("ONNX Runtime ({shown}): {e}")),
            Err(panic) => Err(format!("cannot load ONNX Runtime ({shown}): {panic}")),
        }
    });
    match res {
        Ok(lib) => {
            if let Some(wanted) = library {
                if wanted.display().to_string() != *lib {
                    tracing::warn!(wanted = %wanted.display(), loaded = %lib, "ONNX Runtime already loaded; keeping it");
                }
            }
            Ok(lib.as_str())
        }
        Err(e) => Err(AiError::Model(e.clone())),
    }
}

/// `ort` panics instead of returning errors in a few places (e.g. when the
/// dynamic library cannot be opened); a server must not go down for that.
fn catch_panic<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        p.downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "panic".to_owned())
    })
}

fn inference<E: std::fmt::Display>(e: E) -> AiError {
    AiError::Inference(e.to_string())
}

/// Opens a session with full graph optimisation. Some exports (e.g. the fp16
/// CLIP graphs) trip an ONNX Runtime fusion bug at level 3; those are retried
/// at level 1 so other ONNX exports can be dropped into the model dir.
fn open_session(path: &Path, threads: usize) -> Result<Session, AiError> {
    let build = |level: GraphOptimizationLevel| {
        catch_panic(|| -> ort::Result<Session> {
            let mut b = Session::builder()?
                .with_optimization_level(level)?
                .with_intra_op_spinning(false)?
                .with_inter_threads(1)?;
            if threads > 0 {
                b = b.with_intra_threads(threads)?;
            }
            b.commit_from_file(path)
        })
    };
    let first = match build(GraphOptimizationLevel::Level3) {
        Ok(Ok(s)) => return Ok(s),
        Ok(Err(e)) => e.to_string(),
        Err(p) => p,
    };
    match build(GraphOptimizationLevel::Level1) {
        Ok(Ok(s)) => {
            tracing::warn!(model = %path.display(), error = %first, "full graph optimisation failed; using basic");
            Ok(s)
        }
        _ => Err(AiError::Model(format!("{}: {first}", path.display()))),
    }
}

fn input_names(s: &Session) -> Vec<&str> {
    s.inputs.iter().map(|i| i.name.as_str()).collect()
}

fn output_names(s: &Session) -> Vec<&str> {
    s.outputs.iter().map(|o| o.name.as_str()).collect()
}

/// Picks the first of `wanted` among the graph outputs.
fn pick_output(s: &Session, wanted: &[&str], path: &Path) -> Result<String, AiError> {
    let names = output_names(s);
    wanted
        .iter()
        .find(|w| names.contains(w))
        .map(|w| (*w).to_owned())
        .ok_or_else(|| {
            AiError::Model(format!(
                "{}: expected output {wanted:?}, graph has {names:?}",
                path.display()
            ))
        })
}

fn require_input(s: &Session, name: &str, path: &Path) -> Result<(), AiError> {
    let names = input_names(s);
    if names.contains(&name) {
        Ok(())
    } else {
        Err(AiError::Model(format!(
            "{}: expected input `{name}`, graph has {names:?}",
            path.display()
        )))
    }
}

fn optional_input(s: &Session, name: &str) -> Option<String> {
    input_names(s).contains(&name).then(|| name.to_owned())
}

/// Loads a `tokenizer.json`, with truncation to `max_len` (special tokens
/// included) and padding disabled (we pad ourselves, see [`pad_batch`]).
pub fn load_tokenizer(path: &Path, max_len: usize) -> Result<Tokenizer, AiError> {
    let mut tok = Tokenizer::from_file(path).map_err(|e| AiError::Model(format!("{}: {e}", path.display())))?;
    configure_tokenizer(&mut tok, max_len)?;
    Ok(tok)
}

fn configure_tokenizer(tok: &mut Tokenizer, max_len: usize) -> Result<(), AiError> {
    tok.with_padding(None);
    tok.with_truncation(Some(TruncationParams {
        max_length: max_len,
        strategy: TruncationStrategy::LongestFirst,
        stride: 0,
        direction: TruncationDirection::Right,
    }))
    .map_err(|e| AiError::Model(format!("tokenizer truncation: {e}")))?;
    Ok(())
}

/// A right-padded batch of token ids and attention masks, row-major `[batch, seq]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenBatch {
    pub ids: Vec<i64>,
    pub mask: Vec<i64>,
    pub batch: usize,
    pub seq: usize,
}

/// Pads encodings to the longest one with `pad_id` (mask 0 on padding).
/// Empty encodings are not expected (CLIP and BERT both add special tokens),
/// but a batch of them still yields `seq >= 1`.
pub fn pad_batch(encodings: &[Encoding], pad_id: i64) -> TokenBatch {
    let seq = encodings.iter().map(|e| e.get_ids().len()).max().unwrap_or(0).max(1);
    let mut ids = Vec::with_capacity(encodings.len() * seq);
    let mut mask = Vec::with_capacity(encodings.len() * seq);
    for e in encodings {
        let n = e.get_ids().len();
        ids.extend(e.get_ids().iter().map(|&i| i as i64));
        ids.extend(std::iter::repeat_n(pad_id, seq - n));
        mask.extend(std::iter::repeat_n(1, n));
        mask.extend(std::iter::repeat_n(0, seq - n));
    }
    TokenBatch {
        ids,
        mask,
        batch: encodings.len(),
        seq,
    }
}

/// Mean of `hidden` (`[batch, seq, h]`) over positions where `mask` is 1, as
/// sentence-transformers `Pooling(mean_tokens)` does.
pub fn mean_pool(hidden: &[f32], mask: &[i64], batch: usize, seq: usize, h: usize) -> Vec<Vec<f32>> {
    (0..batch)
        .map(|b| {
            let mut acc = vec![0f32; h];
            let mut count = 0f32;
            for t in 0..seq {
                if mask[b * seq + t] != 0 {
                    count += 1.0;
                    let row = &hidden[(b * seq + t) * h..(b * seq + t + 1) * h];
                    acc.iter_mut().zip(row).for_each(|(a, x)| *a += x);
                }
            }
            let count = count.max(1e-9);
            acc.iter_mut().for_each(|a| *a /= count);
            acc
        })
        .collect()
}

/// sentence-transformers `Dense` layer: `y = W x (+ b)`, identity activation.
#[derive(Debug, Clone)]
pub struct Dense {
    /// Row-major `[out, in]` (torch `Linear.weight`).
    weight: Vec<f32>,
    bias: Option<Vec<f32>>,
    pub in_features: usize,
    pub out_features: usize,
}

impl Dense {
    /// Reads `linear.weight` (and optional `linear.bias`), f32 or f16.
    pub fn from_safetensors(bytes: &[u8]) -> Result<Dense, AiError> {
        use safetensors::SafeTensors;
        let st = SafeTensors::deserialize(bytes).map_err(|e| AiError::Model(format!("dense: {e}")))?;
        let w = st
            .tensor("linear.weight")
            .map_err(|e| AiError::Model(format!("dense: linear.weight: {e}")))?;
        let shape = w.shape();
        if shape.len() != 2 {
            return Err(AiError::Model(format!("dense: linear.weight has shape {shape:?}")));
        }
        let (out_features, in_features) = (shape[0], shape[1]);
        let weight = tensor_f32(&w)?;
        let bias = match st.tensor("linear.bias") {
            Ok(b) if b.shape() == [out_features] => Some(tensor_f32(&b)?),
            Ok(b) => return Err(AiError::Model(format!("dense: linear.bias has shape {:?}", b.shape()))),
            Err(_) => None,
        };
        Ok(Dense {
            weight,
            bias,
            in_features,
            out_features,
        })
    }

    pub fn apply(&self, x: &[f32]) -> Vec<f32> {
        debug_assert_eq!(x.len(), self.in_features);
        (0..self.out_features)
            .map(|o| {
                let row = &self.weight[o * self.in_features..(o + 1) * self.in_features];
                let y: f32 = row.iter().zip(x).map(|(w, v)| w * v).sum();
                y + self.bias.as_ref().map_or(0.0, |b| b[o])
            })
            .collect()
    }
}

fn tensor_f32(t: &safetensors::tensor::TensorView<'_>) -> Result<Vec<f32>, AiError> {
    use safetensors::Dtype;
    let d = t.data();
    match t.dtype() {
        Dtype::F32 => Ok(d
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()),
        Dtype::F16 => Ok(d
            .chunks_exact(2)
            .map(|c| f16_to_f32(u16::from_le_bytes([c[0], c[1]])))
            .collect()),
        other => Err(AiError::Model(format!("dense: unsupported dtype {other:?}"))),
    }
}

fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1f) as u32;
    let frac = (h & 0x3ff) as u32;
    let bits = match (exp, frac) {
        (0, 0) => sign,
        (0, f) => {
            // subnormal: renormalise
            let mut e = 127 - 15 + 1;
            let mut f = f;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((f & 0x3ff) << 13)
        }
        (0x1f, f) => sign | 0x7f80_0000 | (f << 13),
        (e, f) => sign | ((e + 127 - 15) << 23) | (f << 13),
    };
    f32::from_bits(bits)
}

impl ClipEnricher {
    pub fn load(opts: &ClipOptions) -> Result<ClipEnricher, AiError> {
        let missing = missing_model_files(&opts.model_dir, opts.text_encoder);
        if !missing.is_empty() {
            let list: Vec<String> = missing.iter().map(|p| p.display().to_string()).collect();
            return Err(AiError::Model(format!(
                "missing {} (fetch with scripts/fetch-models.sh or nix/models.nix, see docs/ai.md)",
                list.join(", ")
            )));
        }
        init_runtime(opts.onnxruntime_library.as_deref())?;
        let dir = &opts.model_dir;

        let vpath = dir.join(layout::VISION);
        let vs = open_session(&vpath, opts.threads)?;
        require_input(&vs, "pixel_values", &vpath)?;
        let output = pick_output(&vs, &["image_embeds"], &vpath)?;
        let vision = Tower {
            session: Mutex::new(vs),
            output,
        };

        let text = match opts.text_encoder {
            TextEncoderKind::Clip => {
                let path = dir.join(layout::CLIP_TEXT);
                let s = open_session(&path, opts.threads)?;
                require_input(&s, "input_ids", &path)?;
                let output = pick_output(&s, &["text_embeds"], &path)?;
                TextModel {
                    tokenizer: load_tokenizer(&dir.join(layout::CLIP_TOKENIZER), CLIP_MAX_TOKENS)?,
                    pad_id: CLIP_PAD_ID,
                    mask_input: optional_input(&s, "attention_mask"),
                    type_ids_input: None,
                    head: TextHead::Projected,
                    tower: Tower {
                        session: Mutex::new(s),
                        output,
                    },
                }
            }
            TextEncoderKind::Multilingual => {
                let path = dir.join(layout::MCLIP_MODEL);
                let s = open_session(&path, opts.threads)?;
                require_input(&s, "input_ids", &path)?;
                require_input(&s, "attention_mask", &path)?;
                let output = pick_output(&s, &["last_hidden_state", "token_embeddings"], &path)?;
                let dpath = dir.join(layout::MCLIP_DENSE);
                let bytes = std::fs::read(&dpath).map_err(|e| AiError::Model(format!("{}: {e}", dpath.display())))?;
                let dense = Dense::from_safetensors(&bytes)?;
                if dense.out_features != CLIP_DIM {
                    return Err(AiError::Model(format!(
                        "{}: projects to {} dims, expected {CLIP_DIM}",
                        dpath.display(),
                        dense.out_features
                    )));
                }
                let tokenizer = load_tokenizer(&dir.join(layout::MCLIP_TOKENIZER), MCLIP_MAX_TOKENS)?;
                let pad_id = tokenizer.token_to_id("[PAD]").map_or(0, i64::from);
                TextModel {
                    tokenizer,
                    pad_id,
                    mask_input: Some("attention_mask".into()),
                    type_ids_input: optional_input(&s, "token_type_ids"),
                    head: TextHead::MeanPoolDense(dense),
                    tower: Tower {
                        session: Mutex::new(s),
                        output,
                    },
                }
            }
        };
        let model_id = opts.text_encoder.model_id().to_owned();
        tracing::info!(%model_id, dir = %dir.display(), threads = opts.threads, "CLIP models loaded");
        Ok(ClipEnricher { model_id, vision, text })
    }

    /// Embeds up to [`TEXT_BATCH`] texts in one run.
    fn embed_text_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AiError> {
        let t = &self.text;
        let encodings = t.tokenizer.encode_batch(texts.to_vec(), true).map_err(inference)?;
        let tb = pad_batch(&encodings, t.pad_id);
        let shape = [tb.batch, tb.seq];
        let mut inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = Vec::with_capacity(3);
        inputs.push((
            "input_ids".into(),
            Tensor::from_array((shape, tb.ids.clone())).map_err(inference)?.into(),
        ));
        if let Some(name) = &t.mask_input {
            inputs.push((
                name.as_str().into(),
                Tensor::from_array((shape, tb.mask.clone())).map_err(inference)?.into(),
            ));
        }
        if let Some(name) = &t.type_ids_input {
            let zeros = vec![0i64; tb.batch * tb.seq];
            inputs.push((
                name.as_str().into(),
                Tensor::from_array((shape, zeros)).map_err(inference)?.into(),
            ));
        }
        let mut session = t.tower.session.lock().unwrap_or_else(|e| e.into_inner());
        let outputs = session.run(inputs).map_err(inference)?;
        let (oshape, data) = outputs[t.tower.output.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(inference)?;
        let mut out = match &t.head {
            TextHead::Projected => {
                if oshape.len() != 2 || oshape[0] as usize != tb.batch || oshape[1] as usize != CLIP_DIM {
                    return Err(AiError::Inference(format!("text output has shape {oshape:?}")));
                }
                data.chunks_exact(CLIP_DIM).map(<[f32]>::to_vec).collect::<Vec<_>>()
            }
            TextHead::MeanPoolDense(dense) => {
                if oshape.len() != 3
                    || oshape[0] as usize != tb.batch
                    || oshape[1] as usize != tb.seq
                    || oshape[2] as usize != dense.in_features
                {
                    return Err(AiError::Inference(format!("text output has shape {oshape:?}")));
                }
                mean_pool(data, &tb.mask, tb.batch, tb.seq, dense.in_features)
                    .iter()
                    .map(|p| dense.apply(p))
                    .collect()
            }
        };
        out.iter_mut().for_each(|v| l2_normalize(v));
        Ok(out)
    }
}

impl Enricher for ClipEnricher {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn image_model_id(&self) -> &str {
        IMAGE_MODEL_ID
    }
    fn dim(&self) -> usize {
        CLIP_DIM
    }
    fn embed_image(&self, rgba: &image::RgbaImage) -> Result<Vec<f32>, AiError> {
        let pixels = preprocess::preprocess(rgba)?;
        let n = preprocess::INPUT_SIZE as usize;
        let input = Tensor::from_array(([1, 3, n, n], pixels)).map_err(inference)?;
        let mut session = self.vision.session.lock().unwrap_or_else(|e| e.into_inner());
        let outputs = session.run(ort::inputs!["pixel_values" => input]).map_err(inference)?;
        let (shape, data) = outputs[self.vision.output.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(inference)?;
        if data.len() != CLIP_DIM {
            return Err(AiError::Inference(format!("image output has shape {shape:?}")));
        }
        let mut v = data.to_vec();
        l2_normalize(&mut v);
        Ok(v)
    }
    fn embed_text(&self, text: &str) -> Result<Vec<f32>, AiError> {
        Ok(self.embed_text_batch(&[text])?.remove(0))
    }
    fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AiError> {
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(TEXT_BATCH) {
            out.extend(self.embed_text_batch(chunk)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenizers::models::wordlevel::WordLevel;
    use tokenizers::pre_tokenizers::whitespace::Whitespace;
    use tokenizers::processors::template::TemplateProcessing;

    /// A tiny BERT-like tokenizer: [CLS] words [SEP], no model files needed.
    fn tiny_tokenizer(max_len: usize) -> Tokenizer {
        let vocab = [
            ("[PAD]", 0),
            ("[UNK]", 1),
            ("[CLS]", 2),
            ("[SEP]", 3),
            ("a", 4),
            ("cat", 5),
            ("sat", 6),
            ("down", 7),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let model = WordLevel::builder()
            .vocab(vocab)
            .unk_token("[UNK]".into())
            .build()
            .unwrap();
        let mut tok = Tokenizer::new(model);
        tok.with_pre_tokenizer(Some(Whitespace {}));
        tok.with_post_processor(Some(
            TemplateProcessing::builder()
                .try_single("[CLS] $A [SEP]")
                .unwrap()
                .special_tokens(vec![("[CLS]", 2), ("[SEP]", 3)])
                .build()
                .unwrap(),
        ));
        configure_tokenizer(&mut tok, max_len).unwrap();
        tok
    }

    #[test]
    fn padding_is_right_aligned_with_mask() {
        let tok = tiny_tokenizer(16);
        let enc = tok
            .encode_batch(vec!["a cat", "a cat sat down", "zebra"], true)
            .unwrap();
        let b = pad_batch(&enc, 0);
        assert_eq!((b.batch, b.seq), (3, 6));
        assert_eq!(b.ids, vec![2, 4, 5, 3, 0, 0, 2, 4, 5, 6, 7, 3, 2, 1, 3, 0, 0, 0]);
        assert_eq!(b.mask, vec![1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0]);
        // CLIP-style pad id.
        let b = pad_batch(&enc[..1], CLIP_PAD_ID);
        assert_eq!(b.ids, vec![2, 4, 5, 3]);
        assert_eq!(pad_batch(&[], 0).seq, 1);
    }

    #[test]
    fn truncation_keeps_special_tokens() {
        let tok = tiny_tokenizer(4);
        let enc = tok.encode("a cat sat down a cat", true).unwrap();
        assert_eq!(enc.get_ids(), &[2, 4, 5, 3]);
        // Padding configured in a tokenizer.json is ignored; we pad per batch.
        let mut tok = tiny_tokenizer(77);
        tok.with_padding(Some(tokenizers::PaddingParams::default()));
        configure_tokenizer(&mut tok, 77).unwrap();
        assert_eq!(tok.encode("a cat", true).unwrap().get_ids().len(), 4);
    }

    #[test]
    fn mean_pooling_ignores_padding() {
        // batch 2, seq 3, hidden 2
        let hidden = [1.0, 2.0, 3.0, 4.0, 100.0, 100.0, 5.0, 5.0, 7.0, 9.0, 9.0, 9.0];
        let mask = [1, 1, 0, 1, 1, 1];
        let p = mean_pool(&hidden, &mask, 2, 3, 2);
        assert_eq!(p[0], vec![2.0, 3.0]);
        assert_eq!(p[1], vec![7.0, 23.0 / 3.0]);
        // All-zero mask does not divide by zero.
        let p = mean_pool(&hidden[..6], &[0, 0, 0], 1, 3, 2);
        assert_eq!(p[0], vec![0.0, 0.0]);
    }

    fn safetensors_bytes(tensors: &[(&str, Vec<usize>, &str, Vec<u8>)]) -> Vec<u8> {
        // Minimal writer for the safetensors format (8-byte header length + JSON + data).
        let mut header = String::from("{");
        let mut data = Vec::new();
        for (i, (name, shape, dtype, bytes)) in tensors.iter().enumerate() {
            if i > 0 {
                header.push(',');
            }
            let start = data.len();
            data.extend_from_slice(bytes);
            header.push_str(&format!(
                "\"{name}\":{{\"dtype\":\"{dtype}\",\"shape\":{shape:?},\"data_offsets\":[{start},{}]}}",
                data.len()
            ));
        }
        header.push('}');
        while header.len() % 8 != 0 {
            header.push(' ');
        }
        let mut out = (header.len() as u64).to_le_bytes().to_vec();
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(&data);
        out
    }

    #[test]
    fn dense_projection() {
        // W = [[1, 2, 3], [0, -1, 0]], x = [1, 1, 2]
        let w: Vec<u8> = [1f32, 2.0, 3.0, 0.0, -1.0, 0.0]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let d =
            Dense::from_safetensors(&safetensors_bytes(&[("linear.weight", vec![2, 3], "F32", w.clone())])).unwrap();
        assert_eq!((d.out_features, d.in_features), (2, 3));
        assert_eq!(d.apply(&[1.0, 1.0, 2.0]), vec![9.0, -1.0]);

        let b: Vec<u8> = [0.5f32, 1.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        let d = Dense::from_safetensors(&safetensors_bytes(&[
            ("linear.bias", vec![2], "F32", b),
            ("linear.weight", vec![2, 3], "F32", w),
        ]))
        .unwrap();
        assert_eq!(d.apply(&[1.0, 1.0, 2.0]), vec![9.5, 0.0]);

        // f16 weights: 1.0 = 0x3c00, -2.0 = 0xc000
        let h: Vec<u8> = [0x3c00u16, 0xc000].iter().flat_map(|x| x.to_le_bytes()).collect();
        let d = Dense::from_safetensors(&safetensors_bytes(&[("linear.weight", vec![1, 2], "F16", h)])).unwrap();
        assert_eq!(d.apply(&[3.0, 1.0]), vec![1.0]);

        assert!(Dense::from_safetensors(b"garbage").is_err());
        assert!(Dense::from_safetensors(&safetensors_bytes(&[("other", vec![1], "F32", vec![0; 4])])).is_err());
    }

    #[test]
    fn f16_conversion() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0x3555), 0.333_251_95);
        assert_eq!(f16_to_f32(0x7bff), 65504.0);
        assert_eq!(f16_to_f32(0x0001), 5.960_464_5e-8);
        assert!(f16_to_f32(0x7c00).is_infinite());
    }

    #[test]
    fn load_reports_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let err = ClipEnricher::load(&ClipOptions {
            model_dir: dir.path().into(),
            text_encoder: TextEncoderKind::Multilingual,
            onnxruntime_library: None,
            threads: 1,
        })
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("multilingual/dense.safetensors"), "{msg}");
        assert!(msg.contains("clip-vit-b32/vision.onnx"), "{msg}");
    }
}
