//! Manual probe for the OCR reader: the lines read in each picture, the
//! text the server would store, timings and peak memory.
//!
//! ```sh
//! cargo run --release -p stickpile-ai --example ocr -- \
//!     --models models [--threads 2] [--repeat 3] [--max-side 512] [--json] image.png...
//! ```
//!
//! `--max-side` first scales each picture down to that long side (as a
//! client sending a smaller rendering would).
//!
//! `--json` prints one object per picture (`file`, `lines` with `box`,
//! `text` and `score`, the stored `text`, `times_ms`) and a last one with
//! `summary: true` (load time, VmHWM).

use std::path::PathBuf;
use std::time::Instant;

use stickpile_ai::ocr::{clean_lines, fit_for_reading, MAX_READ_PIXELS, MIN_CONFIDENCE};
use stickpile_ai::paddle::PaddleOcr;
use stickpile_ai::TextReader;

fn vm_hwm_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
        })
        .map_or(0, |kb| kb / 1024)
}

fn decode(path: &PathBuf) -> Result<image::RgbaImage, Box<dyn std::error::Error>> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::open(path)?.with_guessed_format()?.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img.to_rgba8())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut models = PathBuf::from("models");
    let mut threads = 2u32;
    let mut repeat = 1usize;
    let mut json = false;
    let mut max_side: Option<u32> = None;
    let mut images: Vec<PathBuf> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--models" => models = val()?.into(),
            "--threads" => threads = val()?.parse()?,
            "--repeat" => repeat = val()?.parse::<usize>()?.max(1),
            "--json" => json = true,
            "--max-side" => max_side = Some(val()?.parse()?),
            _ if a.starts_with("--") => return Err(format!("unknown option {a}").into()),
            _ => images.push(a.into()),
        }
    }

    let t = Instant::now();
    let reader = PaddleOcr::load(&models, threads)?;
    let load_ms = t.elapsed().as_secs_f64() * 1000.0;
    if !json {
        println!(
            "model {} loaded in {load_ms:.0} ms, VmHWM {} MB",
            reader.model_id(),
            vm_hwm_mb()
        );
    }
    let mut medians = Vec::new();
    for path in &images {
        let mut img = decode(path)?;
        if let Some(m) = max_side.filter(|&m| img.width().max(img.height()) > m) {
            let s = m as f64 / img.width().max(img.height()) as f64;
            let (w, h) = (
                ((img.width() as f64 * s).round() as u32).max(1),
                ((img.height() as f64 * s).round() as u32).max(1),
            );
            img = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
        }
        // The server scales frames to MAX_READ_PIXELS before reading.
        let frame = fit_for_reading(&img, MAX_READ_PIXELS);
        let mut times = Vec::new();
        let mut lines = Vec::new();
        for _ in 0..repeat {
            let t = Instant::now();
            lines = reader.read(&frame)?;
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        let stored = clean_lines(vec![lines.clone()], MIN_CONFIDENCE);
        let best = times.iter().copied().fold(f64::MAX, f64::min);
        medians.push(best);
        if json {
            // Bounds are in `frame` pixels; scale them back to the file's.
            let s = img.width() as f32 / frame.width() as f32;
            let out: Vec<serde_json::Value> = lines
                .iter()
                .map(|l| {
                    let b = l.bounds.expect("the reader gives bounds");
                    let (x0, y0, x1, y1) = (b.x * s, b.y * s, (b.x + b.width) * s, (b.y + b.height) * s);
                    serde_json::json!({
                        "box": [[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
                        "text": l.text,
                        "score": l.confidence,
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::json!({
                    "file": path.display().to_string(),
                    "hw": [img.height(), img.width()],
                    "lines": out,
                    "text": stored,
                    "times_ms": times,
                })
            );
        } else {
            println!(
                "{}: {}x{}, {best:.0} ms, VmHWM {} MB",
                path.display(),
                img.width(),
                img.height(),
                vm_hwm_mb()
            );
            for l in &lines {
                println!("  {:.3}  {}", l.confidence, l.text);
            }
            println!("  stored: {stored:?}");
        }
    }
    medians.sort_by(f64::total_cmp);
    if json {
        println!(
            "{}",
            serde_json::json!({"summary": true, "load_ms": load_ms, "threads": threads, "hwm_end_mb": vm_hwm_mb()})
        );
    } else if !medians.is_empty() {
        println!(
            "{} pictures, fastest run per picture: median {:.0} ms, max {:.0} ms; VmHWM {} MB",
            medians.len(),
            medians[medians.len() / 2],
            medians[medians.len() - 1],
            vm_hwm_mb()
        );
    }
    Ok(())
}
