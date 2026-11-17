//! Tests against the real OCR models. They only run when
//! `STICKPILE_TEST_MODEL_DIR` points at a model dir holding `ocr/det.onnx`
//! and `ocr/rec.onnx` (docs/ai.md layout), e.g.
//!
//! ```sh
//! scripts/fetch-models.sh --english models
//! STICKPILE_TEST_MODEL_DIR=$PWD/models cargo test -p stickpile-ai --release --test ocr_model
//! ```
//!
//! Without the variable (or without the OCR files) every test returns
//! immediately (Nix sandbox, CI).
#![cfg(feature = "ocr")]

use std::path::PathBuf;
use std::sync::OnceLock;

use image::{Rgba, RgbaImage};
use stickpile_ai::ocr::{clean_lines, read_frames, MIN_CONFIDENCE};
use stickpile_ai::paddle::{PaddleOcr, MODEL_ID};
use stickpile_ai::TextReader;

fn reader() -> Option<&'static PaddleOcr> {
    static R: OnceLock<Option<PaddleOcr>> = OnceLock::new();
    R.get_or_init(|| {
        let dir = PathBuf::from(std::env::var_os("STICKPILE_TEST_MODEL_DIR")?);
        if !stickpile_ai::missing_ocr_files(&dir).is_empty() {
            eprintln!("no OCR models in {}; skipping", dir.display());
            return None;
        }
        Some(PaddleOcr::load(&dir, 2).unwrap_or_else(|e| panic!("loading the OCR models: {e}")))
    })
    .as_ref()
}

fn fixture() -> RgbaImage {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocr-en-uk.png");
    image::open(&p)
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        .to_rgba8()
}

#[test]
fn reads_english_and_ukrainian() {
    let Some(r) = reader() else { return };
    assert_eq!(r.model_id(), MODEL_ID);
    // Dark text on transparency (flattened onto white), 300 x 110 px.
    let img = fixture();
    let lines = r.read(&img).unwrap();
    for l in &lines {
        eprintln!("{:.3} {:?} {:?}", l.confidence, l.text, l.bounds);
    }
    assert_eq!(
        clean_lines(vec![lines.clone()], MIN_CONFIDENCE),
        "Hello, world\nПривіт, світе"
    );
    // The detector finds words; bounds are in the picture's pixels, the
    // English words above the Ukrainian ones.
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["Hello,", "world", "Привіт,", "світе"]);
    let b: Vec<_> = lines.iter().map(|l| l.bounds.expect("bounds")).collect();
    assert!(b[0].x < b[1].x && b[2].x < b[3].x, "{b:?}");
    assert!(b[0].y + b[0].height < b[2].y + b[2].height / 2.0, "{b:?}");
    assert!(b
        .iter()
        .all(|b| b.x >= 0.0 && b.x + b.width <= 300.0 && b.y + b.height <= 110.0));
    assert!(lines.iter().all(|l| l.confidence > 0.9));
}

#[test]
fn reads_through_the_server_path() {
    let Some(r) = reader() else { return };
    // The same picture twice as frames, and once scaled up past the frame
    // bound (read_frames scales it down first): one text, no repeats.
    let img = fixture();
    let big = image::imageops::resize(&img, 3000, 1100, image::imageops::FilterType::Triangle);
    let text = read_frames(r, &[img.clone(), img, big]).unwrap();
    assert_eq!(text, "Hello, world\nПривіт, світе");
}

#[test]
fn pictures_without_text_give_no_lines() {
    let Some(r) = reader() else { return };
    let blank = RgbaImage::from_pixel(400, 300, Rgba([255, 255, 255, 255]));
    assert!(r.read(&blank).unwrap().is_empty());
    let mut stripes = RgbaImage::from_pixel(300, 300, Rgba([240, 240, 240, 255]));
    for (x, y, p) in stripes.enumerate_pixels_mut() {
        if (x / 7 + y / 11) % 2 == 0 {
            *p = Rgba([90, 120, 30, 255]);
        }
    }
    assert_eq!(clean_lines(vec![r.read(&stripes).unwrap()], MIN_CONFIDENCE), "");
    // Degenerate sizes are fine.
    assert!(r.read(&RgbaImage::new(1, 1)).unwrap().is_empty());
    assert!(r.read(&RgbaImage::new(3000, 1)).unwrap().is_empty());
}

#[test]
fn concurrent_reads_agree() {
    let Some(r) = reader() else { return };
    let img = fixture();
    let one = r.read(&img).unwrap();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..3).map(|_| s.spawn(|| r.read(&img).unwrap())).collect();
        for h in handles {
            assert_eq!(h.join().unwrap(), one);
        }
    });
}
