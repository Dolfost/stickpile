//! OCR with PaddleOCR PP-OCRv5 through ONNX Runtime (feature `ocr`).
//!
//! * Detector: `ocr/det.onnx`, PP-OCRv5 mobile DB (`x` `[1,3,H,W]` ->
//!   probability map `[1,1,H,W]`).
//! * Recogniser: `ocr/rec.onnx`, PP-OCRv5 mobile Cyrillic CTC (`x`
//!   `[N,3,48,W]` -> `[N,W/8,852]`): Latin and Cyrillic, all Ukrainian
//!   letters; its dictionary is the model metadata `character`.
//!
//! Pre- and post-processing follow RapidOCR 3.8.1, which the evaluation in
//! docs/ai.md measured, in pure Rust ([`det`], [`rec`], [`geometry`]), with
//! three changes from that evaluation: the detector input's long side is
//! capped at 2000 px, look-alike letters are repaired ([`text`]), and the
//! sessions run without ONNX Runtime's CPU memory arena and memory pattern
//! (input sizes vary; the arena would keep the largest activations).
//!
//! [`PaddleOcr`] implements [`TextReader`]: it returns every recognised box
//! with its score and position; [`crate::ocr::clean_lines`] applies the
//! confidence threshold, the letter-or-digit rule and the reading order.
//!
//! Work per picture is bounded: the working picture is at most 2000 px on
//! the long side (very thin strips aside, which [`det::MAX_WORK_PIXELS`]
//! bounds), the detector input at most 2000 px, at most
//! [`det::MAX_CANDIDATES`] contours and [`MAX_LINES`] recognised boxes. It
//! runs synchronously on the calling thread.

pub mod det;
pub mod geometry;
pub mod rec;
pub mod text;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use image::RgbaImage;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use sha2::{Digest, Sha256};

use crate::ocr::{LineBox, TextLine, TextReader};
use crate::runtime::{catch_panic, inference, runtime};
use crate::{layout, AiError, MODEL_FILES};

pub use text::fix_lookalikes;

/// At most this many boxes are recognised per picture (in reading order);
/// the stored text is capped at 2000 characters anyway.
pub const MAX_LINES: usize = 300;

/// [`TextReader::model_id`] with the shipped files. It names the reading
/// settings too (the 2000 px cap), so changing them re-reads media.
pub const MODEL_ID: &str = "ppocrv5-mobile-det+cyrillic-rec@2000";

pub struct PaddleOcr {
    model_id: String,
    det: Model,
    rec: Model,
    /// Output class `i` (1-based) is `dict[i - 1]`; class 0 is the CTC blank.
    dict: Vec<String>,
}

struct Model {
    session: Mutex<Session>,
    input: String,
}

impl std::fmt::Debug for PaddleOcr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaddleOcr")
            .field("model_id", &self.model_id)
            .field("dictionary", &self.dict.len())
            .finish_non_exhaustive()
    }
}

/// An OCR session: full graph optimisation, no spinning, and no CPU memory
/// arena or memory pattern (both keep buffers for the largest input seen).
fn open_session(env: &ort::environment::Environment, path: &Path, threads: u32) -> Result<Session, AiError> {
    let built = catch_panic(|| -> ort::Result<Session> {
        let mut b = Session::builder(env)?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_intra_op_spinning(false)?
            .with_inter_threads(1)?
            .with_memory_pattern(false)?
            .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])?;
        if threads > 0 {
            b = b.with_intra_threads(threads as usize)?;
        }
        b.commit_from_file(path)
    });
    match built {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => Err(AiError::Model(format!("{}: {e}", path.display()))),
        Err(p) => Err(AiError::Model(format!("{}: {p}", path.display()))),
    }
}

fn first_input(s: &Session, path: &Path) -> Result<String, AiError> {
    s.inputs()
        .first()
        .map(|i| i.name().to_owned())
        .ok_or_else(|| AiError::Model(format!("{}: the graph has no input", path.display())))
}

fn sha256_hex(path: &Path) -> Result<String, AiError> {
    let mut f = std::fs::File::open(path).map_err(|e| AiError::Model(format!("{}: {e}", path.display())))?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).map_err(|e| AiError::Model(format!("{}: {e}", path.display())))?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// [`MODEL_ID`] for the shipped files; other files get an id from their
/// hashes, so swapping models re-reads media.
pub fn model_id_for(det_sha256: &str, rec_sha256: &str) -> String {
    let shipped = |path: &str| MODEL_FILES.iter().find(|f| f.path == path).map(|f| f.sha256);
    if shipped(layout::OCR_DET) == Some(det_sha256) && shipped(layout::OCR_REC) == Some(rec_sha256) {
        MODEL_ID.to_owned()
    } else {
        format!(
            "ppocr-det-{}+rec-{}@2000",
            &det_sha256[..8.min(det_sha256.len())],
            &rec_sha256[..8.min(rec_sha256.len())]
        )
    }
}

impl PaddleOcr {
    /// Loads `ocr/det.onnx` and `ocr/rec.onnx` below `model_dir` (docs/ai.md
    /// layout), with `threads` intra-op threads per run (0 = one per core).
    ///
    /// Uses the process's ONNX Runtime: the one [`crate::init_runtime`] (or a
    /// CLIP load) chose, else `ORT_DYLIB_PATH`, else the system library.
    pub fn load(model_dir: &Path, threads: u32) -> Result<PaddleOcr, AiError> {
        let missing = crate::missing_ocr_files(model_dir);
        if !missing.is_empty() {
            let list: Vec<String> = missing.iter().map(|p| p.display().to_string()).collect();
            return Err(AiError::Model(format!(
                "missing {} (fetch with scripts/fetch-models.sh or nix/models.nix, see docs/ai.md)",
                list.join(", ")
            )));
        }
        let env = &runtime(None)?.env;
        let det_path: PathBuf = model_dir.join(layout::OCR_DET);
        let rec_path: PathBuf = model_dir.join(layout::OCR_REC);
        let model_id = model_id_for(&sha256_hex(&det_path)?, &sha256_hex(&rec_path)?);

        let det = open_session(env, &det_path, threads)?;
        let rec = open_session(env, &rec_path, threads)?;
        let chars = rec.metadata().ok().and_then(|m| m.custom("character")).ok_or_else(|| {
            AiError::Model(format!(
                "{}: no `character` dictionary in the model metadata",
                rec_path.display()
            ))
        })?;
        let dict = rec::dictionary(&chars);
        let det = Model {
            input: first_input(&det, &det_path)?,
            session: Mutex::new(det),
        };
        let rec = Model {
            input: first_input(&rec, &rec_path)?,
            session: Mutex::new(rec),
        };
        tracing::info!(%model_id, dir = %model_dir.display(), threads, dictionary = dict.len(), "OCR models loaded");
        Ok(PaddleOcr {
            model_id,
            det,
            rec,
            dict,
        })
    }

    /// The probability map for a working picture: `(width, height, map)`.
    fn detect(&self, img: &geometry::Image3) -> Result<(usize, usize, Vec<f32>), AiError> {
        let (w, h, data) = det::det_input(img);
        let input = Tensor::from_array(([1usize, 3, h, w], data)).map_err(inference)?;
        let mut session = self.det.session.lock().unwrap_or_else(|e| e.into_inner());
        let outputs = session
            .run(ort::inputs![self.det.input.as_str() => input])
            .map_err(inference)?;
        let (shape, map) = outputs[0].try_extract_tensor::<f32>().map_err(inference)?;
        if shape.len() != 4 || shape[2] as usize != h || shape[3] as usize != w {
            return Err(AiError::Inference(format!(
                "detector output has shape {shape:?} for input {w}x{h}"
            )));
        }
        Ok((w, h, map[..w * h].to_vec()))
    }

    /// Text and score of each crop, in input order.
    fn recognise(&self, crops: &[geometry::Image3]) -> Result<Vec<(String, f32)>, AiError> {
        let mut out = vec![(String::new(), 0.0); crops.len()];
        let classes = self.dict.len() + 1;
        for batch in rec::batches(crops) {
            let refs: Vec<&geometry::Image3> = batch.iter().map(|&i| &crops[i]).collect();
            let (w, data) = rec::rec_input(&refs);
            let input = Tensor::from_array(([refs.len(), 3, rec::REC_HEIGHT, w], data)).map_err(inference)?;
            let mut session = self.rec.session.lock().unwrap_or_else(|e| e.into_inner());
            let outputs = session
                .run(ort::inputs![self.rec.input.as_str() => input])
                .map_err(inference)?;
            let (shape, probs) = outputs[0].try_extract_tensor::<f32>().map_err(inference)?;
            if shape.len() != 3 || shape[0] as usize != refs.len() || shape[2] as usize != classes {
                return Err(AiError::Inference(format!(
                    "recogniser output has shape {shape:?}, expected [{}, T, {classes}] (dictionary)",
                    refs.len()
                )));
            }
            let per = shape[1] as usize * classes;
            for (k, &i) in batch.iter().enumerate() {
                out[i] = rec::ctc_decode(&probs[k * per..(k + 1) * per], classes, &self.dict);
            }
        }
        Ok(out)
    }
}

impl TextReader for PaddleOcr {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Every box with text, in RapidOCR's order, as read (look-alikes
    /// repaired), with its recogniser score and its axis-aligned bounds in
    /// `frame` pixels. Low scores are left for the caller's threshold.
    fn read(&self, frame: &RgbaImage) -> Result<Vec<TextLine>, AiError> {
        let Some(work) = det::working_picture(frame) else {
            return Ok(Vec::new());
        };
        let (mw, mh, map) = self.detect(&work.img)?;
        let mut boxes = det::boxes_from_map(&map, mw, mh, work.img.width, work.img.height);
        drop(map);
        if boxes.len() > MAX_LINES {
            tracing::debug!(
                boxes = boxes.len(),
                "OCR: too many text boxes; reading the first {MAX_LINES}"
            );
            boxes.truncate(MAX_LINES);
        }
        let (crops, quads): (Vec<_>, Vec<_>) = boxes
            .iter()
            .filter_map(|b| geometry::crop_quad(&work.img, &b.quad).map(|c| (c, b.quad)))
            .unzip();
        let read = self.recognise(&crops)?;
        Ok(read
            .into_iter()
            .zip(quads)
            .filter(|((t, _), _)| !t.trim().is_empty())
            .map(|((t, score), quad)| {
                let pts = quad.map(|p| work.to_original(p));
                let (x0, x1) = pts
                    .iter()
                    .fold((f64::MAX, f64::MIN), |m, p| (m.0.min(p[0]), m.1.max(p[0])));
                let (y0, y1) = pts
                    .iter()
                    .fold((f64::MAX, f64::MIN), |m, p| (m.0.min(p[1]), m.1.max(p[1])));
                TextLine {
                    text: fix_lookalikes(&t),
                    confidence: score,
                    bounds: Some(LineBox {
                        x: x0 as f32,
                        y: y0 as f32,
                        width: (x1 - x0) as f32,
                        height: (y1 - y0) as f32,
                    }),
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_id_names_the_shipped_files() {
        let sha = |p: &str| MODEL_FILES.iter().find(|f| f.path == p).unwrap().sha256;
        assert_eq!(model_id_for(sha(layout::OCR_DET), sha(layout::OCR_REC)), MODEL_ID);
        assert_eq!(
            model_id_for(sha(layout::OCR_DET), &"ab".repeat(32)),
            "ppocr-det-4d97c44a+rec-abababab@2000"
        );
    }

    #[test]
    fn load_reports_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let err = PaddleOcr::load(dir.path(), 1).unwrap_err().to_string();
        assert!(err.contains("ocr/det.onnx") && err.contains("ocr/rec.onnx"), "{err}");
        std::fs::create_dir_all(dir.path().join("ocr")).unwrap();
        std::fs::write(dir.path().join(layout::OCR_DET), b"x").unwrap();
        let err = PaddleOcr::load(dir.path(), 1).unwrap_err().to_string();
        assert!(err.contains("ocr/rec.onnx") && !err.contains("ocr/det.onnx"), "{err}");
    }
}
