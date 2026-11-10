//! Manual probe for the real models: timings, zero-shot suggestions per image,
//! and an optional JSON dump of embeddings for threshold calibration.
//!
//! ```sh
//! cargo run --release -p stickpile-ai --example probe -- \
//!     --models models --encoder clip [--threads 4] [--vocab vocab.toml] \
//!     [--texts prompts.txt] [--dump out.json] [--query "text"]... image.png...
//! ```

use std::path::PathBuf;
use std::time::Instant;

use stickpile_ai::clip::ClipEnricher;
use stickpile_ai::{ClipOptions, Enricher, TextEncoderKind, Vocabulary, ZeroShot};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut models = PathBuf::from("models");
    let mut encoder = TextEncoderKind::Clip;
    let mut threads = 4usize;
    let mut vocab_path: Option<PathBuf> = None;
    let mut texts_path: Option<PathBuf> = None;
    let mut dump: Option<PathBuf> = None;
    let mut queries: Vec<String> = Vec::new();
    let mut images: Vec<PathBuf> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--models" => models = val()?.into(),
            "--encoder" => {
                encoder = match val()?.as_str() {
                    "multilingual" => TextEncoderKind::Multilingual,
                    _ => TextEncoderKind::Clip,
                }
            }
            "--threads" => threads = val()?.parse()?,
            "--vocab" => vocab_path = Some(val()?.into()),
            "--texts" => texts_path = Some(val()?.into()),
            "--dump" => dump = Some(val()?.into()),
            "--query" => queries.push(val()?),
            _ => images.push(a.into()),
        }
    }

    let t = Instant::now();
    let clip = ClipEnricher::load(&ClipOptions {
        model_dir: models,
        text_encoder: encoder,
        onnxruntime_library: None,
        threads,
    })?;
    println!("load: {:?} ({})", t.elapsed(), clip.model_id());

    let vocab = match &vocab_path {
        Some(p) => Vocabulary::load(p)?,
        None => Vocabulary::builtin(),
    };
    let t = Instant::now();
    let zs = ZeroShot::new(&clip, vocab)?;
    println!("zero-shot vocabulary embedded: {:?}", t.elapsed());

    // Warm-up, then timing.
    let probe = image::RgbaImage::from_pixel(300, 300, image::Rgba([200, 100, 50, 255]));
    clip.embed_image(&probe)?;
    clip.embed_text("warm up")?;
    // Min and median of single calls: robust against a busy machine.
    let n = 30;
    let stats = |mut ms: Vec<f64>| {
        ms.sort_by(f64::total_cmp);
        format!("min {:.1} ms, median {:.1} ms", ms[0], ms[ms.len() / 2])
    };
    let mut ms = Vec::new();
    for _ in 0..n {
        let t = Instant::now();
        clip.embed_image(&probe)?;
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    println!("embed_image: {}", stats(ms));
    let mut ms = Vec::new();
    for i in 0..n {
        let t = Instant::now();
        clip.embed_text(&format!("a meme of a cat number {i}"))?;
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    println!("embed_text: {}", stats(ms));
    let batch: Vec<String> = (0..32).map(|i| format!("a sticker of a dog number {i}")).collect();
    let refs: Vec<&str> = batch.iter().map(String::as_str).collect();
    let mut ms = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        clip.embed_texts(&refs)?;
        ms.push(t.elapsed().as_secs_f64() * 1000.0 / 32.0);
    }
    println!("embed_texts (batch of 32), per text: {}", stats(ms));

    let mut out_images = Vec::new();
    for path in &images {
        let img = image::open(path)?.to_rgba8();
        let e = clip.embed_image(&img)?;
        let s = zs.suggest(&e, 5, 3);
        let probs = zs.vibe_probabilities(&e).unwrap_or_default();
        let mut vp: Vec<(usize, f32)> = probs.iter().copied().enumerate().collect();
        vp.sort_by(|a, b| b.1.total_cmp(&a.1));
        let top_vibes: Vec<String> = vp
            .iter()
            .take(3)
            .map(|(i, p)| format!("{}={:.2}", zs.vocab.vibes[*i].label, p))
            .collect();
        println!(
            "{}\n  keywords {:?}\n  emojis {:?}\n  vibe {:?} [{}]",
            path.display(),
            s.keywords
                .iter()
                .map(|(k, v)| format!("{k}={v:.3}"))
                .collect::<Vec<_>>(),
            s.emojis.iter().map(|(k, v)| format!("{k}={v:.3}")).collect::<Vec<_>>(),
            s.vibe,
            top_vibes.join(" ")
        );
        out_images.push((path.display().to_string(), e));
    }
    for q in &queries {
        let e = clip.embed_text(q)?;
        let r = zs.related_terms(&e, 6);
        println!(
            "query {q:?}: {:?}",
            r.iter().map(|(t, s)| format!("{t}={s:.3}")).collect::<Vec<_>>()
        );
        for (name, ie) in &out_images {
            println!("  {:.5} {name}", stickpile_ai::cosine(&e, ie));
        }
    }

    if let Some(dump) = dump {
        let mut texts: Vec<String> = Vec::new();
        if let Some(p) = &texts_path {
            texts = std::fs::read_to_string(p)?
                .lines()
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
        }
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let t = Instant::now();
        let temb = clip.embed_texts(&refs)?;
        println!("dumped {} texts in {:?}", texts.len(), t.elapsed());
        let json = serde_json::json!({
            "model": clip.model_id(),
            "images": out_images.iter().map(|(n, e)| serde_json::json!({"name": n, "emb": e})).collect::<Vec<_>>(),
            "texts": texts.iter().zip(&temb).map(|(t, e)| serde_json::json!({"text": t, "emb": e})).collect::<Vec<_>>(),
        });
        std::fs::write(dump, serde_json::to_vec(&json)?)?;
    }
    Ok(())
}
