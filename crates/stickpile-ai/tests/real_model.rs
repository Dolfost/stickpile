//! Tests against the real CLIP models. They only run when
//! `STICKPILE_TEST_MODEL_DIR` points at a model dir (docs/ai.md layout), e.g.
//!
//! ```sh
//! scripts/fetch-models.sh --all models
//! STICKPILE_TEST_MODEL_DIR=$PWD/models cargo test -p stickpile-ai --release -- --nocapture
//! ```
//!
//! Without the variable every test returns immediately (Nix sandbox, CI).
#![cfg(feature = "clip")]

use std::path::PathBuf;
use std::sync::OnceLock;

use image::{Rgba, RgbaImage};
use stickpile_ai::clip::ClipEnricher;
use stickpile_ai::{cosine, ClipOptions, Enricher, TextEncoderKind, Vocabulary, ZeroShot};

fn model_dir() -> Option<PathBuf> {
    std::env::var_os("STICKPILE_TEST_MODEL_DIR").map(PathBuf::from)
}

fn load(kind: TextEncoderKind) -> ClipEnricher {
    let dir = model_dir().expect("checked by caller");
    ClipEnricher::load(&ClipOptions {
        model_dir: dir,
        text_encoder: kind,
        onnxruntime_library: None,
        threads: 4,
    })
    .unwrap_or_else(|e| panic!("loading {kind:?} models: {e}"))
}

/// Shared across tests (each load maps several hundred MB).
fn english() -> Option<&'static ClipEnricher> {
    static E: OnceLock<ClipEnricher> = OnceLock::new();
    model_dir()?;
    Some(E.get_or_init(|| load(TextEncoderKind::Clip)))
}

fn multilingual() -> Option<&'static ClipEnricher> {
    static M: OnceLock<ClipEnricher> = OnceLock::new();
    model_dir()?;
    Some(M.get_or_init(|| load(TextEncoderKind::Multilingual)))
}

fn both() -> Vec<&'static ClipEnricher> {
    english().into_iter().chain(multilingual()).collect()
}

fn english_zeroshot() -> Option<&'static ZeroShot> {
    static Z: OnceLock<ZeroShot> = OnceLock::new();
    let e = english()?;
    Some(Z.get_or_init(|| ZeroShot::new(e, Vocabulary::builtin()).unwrap()))
}

fn multilingual_zeroshot() -> Option<&'static ZeroShot> {
    static Z: OnceLock<ZeroShot> = OnceLock::new();
    let e = multilingual()?;
    Some(Z.get_or_init(|| ZeroShot::new(e, Vocabulary::builtin()).unwrap()))
}

// ---- synthetic images ----------------------------------------------------

const CLEAR: Rgba<u8> = Rgba([0, 0, 0, 0]);

fn canvas(size: u32) -> RgbaImage {
    RgbaImage::from_pixel(size, size, CLEAR)
}

/// Paints pixels whose centre (in unit coordinates, y down) satisfies `inside`.
fn paint(img: &mut RgbaImage, colour: [u8; 4], inside: impl Fn(f32, f32) -> bool) {
    let (w, h) = img.dimensions();
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            if inside(u, v) {
                img.put_pixel(x, y, Rgba(colour));
            }
        }
    }
}

fn disc(cx: f32, cy: f32, r: f32) -> impl Fn(f32, f32) -> bool {
    move |u, v| (u - cx).powi(2) + (v - cy).powi(2) <= r * r
}

fn square(colour: [u8; 3]) -> RgbaImage {
    let mut img = canvas(256);
    paint(&mut img, [colour[0], colour[1], colour[2], 255], |u, v| {
        (0.1..0.9).contains(&u) && (0.1..0.9).contains(&v)
    });
    img
}

/// Yellow emoji-style face on a transparent background; `smile` < 0 frowns.
fn face(smile: f32) -> RgbaImage {
    let mut img = canvas(384);
    let ink = [60, 40, 10, 255];
    paint(&mut img, ink, disc(0.5, 0.5, 0.46));
    paint(&mut img, [255, 204, 77, 255], disc(0.5, 0.5, 0.44));
    paint(&mut img, ink, disc(0.35, 0.38, 0.06));
    paint(&mut img, ink, disc(0.65, 0.38, 0.06));
    // Mouth: a thick arc of a circle below (smile) or above (frown) the mouth line.
    let (cy, lower) = if smile >= 0.0 { (0.50, true) } else { (0.85, false) };
    paint(&mut img, ink, move |u, v| {
        let d = ((u - 0.5).powi(2) + (v - cy).powi(2)).sqrt();
        (0.20..0.25).contains(&d) && if lower { v > cy + 0.08 } else { v < cy - 0.08 }
    });
    img
}

/// Red heart: (x^2 + y^2 - 1)^3 - x^2 y^3 <= 0.
fn heart() -> RgbaImage {
    let mut img = canvas(320);
    paint(&mut img, [220, 30, 40, 255], |u, v| {
        let (x, y) = ((u - 0.5) * 2.6, (0.45 - v) * 2.6);
        (x * x + y * y - 1.0).powi(3) - x * x * y.powi(3) <= 0.0
    });
    img
}

fn names(list: &[(String, f32)]) -> Vec<&str> {
    list.iter().map(|(n, _)| n.as_str()).collect()
}

// ---- tests ------------------------------------------------------------------

#[test]
fn embeddings_are_normalised_with_model_ids() {
    for e in both() {
        assert_eq!(e.dim(), 512);
        assert_eq!(e.image_model_id(), "clip-vit-b32");
        let v = e.embed_image(&square([200, 30, 30])).unwrap();
        let t = e.embed_text("a red square").unwrap();
        for x in [&v, &t] {
            assert_eq!(x.len(), 512);
            assert!((x.iter().map(|a| a * a).sum::<f32>().sqrt() - 1.0).abs() < 1e-4);
        }
    }
    if let (Some(en), Some(ml)) = (english(), multilingual()) {
        assert_eq!(en.model_id(), "clip-vit-b32+clip-text");
        assert_eq!(ml.model_id(), "clip-vit-b32+mclip-v1");
        // Same image tower in both configurations.
        let img = face(1.0);
        assert!(cosine(&en.embed_image(&img).unwrap(), &ml.embed_image(&img).unwrap()) > 0.9999);
    }
}

#[test]
fn red_square_prefers_red_prompt() {
    for e in both() {
        let red = e.embed_image(&square([220, 20, 20])).unwrap();
        let blue = e.embed_image(&square([20, 40, 220])).unwrap();
        let t_red = e.embed_text("a red square").unwrap();
        let t_blue = e.embed_text("a blue square").unwrap();
        let (rr, rb) = (cosine(&red, &t_red), cosine(&red, &t_blue));
        let (br, bb) = (cosine(&blue, &t_red), cosine(&blue, &t_blue));
        println!(
            "{}: red img: red {rr:.3} blue {rb:.3}; blue img: red {br:.3} blue {bb:.3}",
            e.model_id()
        );
        assert!(rr > rb, "{rr} vs {rb}");
        assert!(bb > br, "{bb} vs {br}");
    }
    if let Some(ml) = multilingual() {
        let red = ml.embed_image(&square([220, 20, 20])).unwrap();
        let (uk_red, uk_blue) = (
            ml.embed_text("червоний квадрат").unwrap(),
            ml.embed_text("синій квадрат").unwrap(),
        );
        println!(
            "uk: red img: червоний {:.3} синій {:.3}",
            cosine(&red, &uk_red),
            cosine(&red, &uk_blue)
        );
        assert!(cosine(&red, &uk_red) > cosine(&red, &uk_blue) + 0.01);
    }
}

#[test]
fn text_text_sanity() {
    for e in both() {
        let cat = e.embed_text("cat").unwrap();
        let kitten = e.embed_text("kitten").unwrap();
        let truck = e.embed_text("truck").unwrap();
        let (ck, ct) = (cosine(&cat, &kitten), cosine(&cat, &truck));
        println!("{}: cat~kitten {ck:.3} cat~truck {ct:.3}", e.model_id());
        assert!(ck > ct + 0.05, "{ck} vs {ct}");
    }
}

#[test]
fn multilingual_aligns_with_english() {
    let Some(ml) = multilingual() else { return };
    let kit = ml.embed_text("кіт").unwrap();
    let cat = ml.embed_text("cat").unwrap();
    let truck = ml.embed_text("truck").unwrap();
    println!(
        "mclip: кіт~cat {:.3} кіт~truck {:.3}",
        cosine(&kit, &cat),
        cosine(&kit, &truck)
    );
    assert!(cosine(&kit, &cat) > 0.9);
    assert!(cosine(&kit, &cat) > cosine(&kit, &truck) + 0.05);
    // The student model was distilled onto the CLIP text tower: the same
    // English sentence lands close to CLIP's own embedding, and so does the
    // Ukrainian translation.
    if let Some(en) = english() {
        let clip_cat = en.embed_text("a photo of a cat").unwrap();
        let m_en = ml.embed_text("a photo of a cat").unwrap();
        let m_uk = ml.embed_text("фото кота").unwrap();
        println!(
            "clip(cat photo)~mclip(en) {:.3} ~mclip(uk) {:.3}",
            cosine(&clip_cat, &m_en),
            cosine(&clip_cat, &m_uk)
        );
        assert!(cosine(&clip_cat, &m_en) > 0.9);
        assert!(cosine(&clip_cat, &m_uk) > 0.85);
    }
    // And it finds the matching image: a smiling face is closer to "усмішка"
    // (smile) than to "вантажівка" (truck).
    let img = ml.embed_image(&face(1.0)).unwrap();
    let smile = ml.embed_text("усміхнене обличчя").unwrap();
    let truck_uk = ml.embed_text("вантажівка").unwrap();
    println!(
        "smiley: усміхнене обличчя {:.3} вантажівка {:.3}",
        cosine(&img, &smile),
        cosine(&img, &truck_uk)
    );
    assert!(cosine(&img, &smile) > cosine(&img, &truck_uk) + 0.03);
}

#[test]
fn batching_matches_single_texts() {
    let texts = [
        "a",
        "a cat sitting on a very long sofa next to a window at night",
        "ні",
        "😂 lol",
    ];
    for e in both() {
        let batch = e.embed_texts(&texts).unwrap();
        assert_eq!(batch.len(), texts.len());
        for (t, b) in texts.iter().zip(&batch) {
            let single = e.embed_text(t).unwrap();
            let c = cosine(&single, b);
            // fp32 CLIP: identical up to float noise. The multilingual model is
            // dynamically quantised, and its activation scales are computed over
            // the whole batch, so batch composition moves results by ~1e-3
            // (with the fp32 export of the same model this is > 0.9999 too).
            let tol = if e.model_id().contains("mclip") { 0.995 } else { 0.9999 };
            assert!(c > tol, "{}: {t:?} batch vs single {c}", e.model_id());
        }
        assert!(e.embed_texts(&[]).unwrap().is_empty());
    }
}

#[test]
fn long_and_odd_texts_are_truncated_not_rejected() {
    let long = "cat ".repeat(500);
    for e in both() {
        for t in [long.as_str(), "", " ", "🐸🐸🐸", "\u{0}"] {
            let v = e.embed_text(t).unwrap_or_else(|err| panic!("{t:?}: {err}"));
            assert_eq!(v.len(), 512);
            assert!(v.iter().all(|x| x.is_finite()));
        }
    }
}

#[test]
fn transparency_is_white() {
    let Some(e) = english() else { return };
    let sticker = face(1.0);
    let mut on_white = RgbaImage::from_pixel(sticker.width(), sticker.height(), Rgba([255, 255, 255, 255]));
    image::imageops::overlay(&mut on_white, &sticker, 0, 0);
    let a = e.embed_image(&sticker).unwrap();
    let b = e.embed_image(&on_white).unwrap();
    assert!(cosine(&a, &b) > 0.9999, "{}", cosine(&a, &b));
}

#[test]
fn concurrent_use_is_safe_and_deterministic() {
    let Some(e) = english() else { return };
    let img = heart();
    let reference = e.embed_image(&img).unwrap();
    let text_ref = e.embed_text("a red heart").unwrap();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                s.spawn(|| {
                    for _ in 0..3 {
                        let v = e.embed_image(&img).unwrap();
                        assert!(cosine(&v, &reference) > 0.99999);
                        let t = e.embed_text("a red heart").unwrap();
                        assert!(cosine(&t, &text_ref) > 0.99999);
                    }
                })
            })
            .collect();
        handles.into_iter().for_each(|h| h.join().unwrap());
    });
}

#[test]
fn zero_shot_on_synthetic_stickers() {
    for (zs, e) in [
        (english_zeroshot(), english()),
        (multilingual_zeroshot(), multilingual()),
    ] {
        let (Some(zs), Some(e)) = (zs, e) else { continue };
        let report = |name: &str, img: &RgbaImage| {
            let emb = e.embed_image(img).unwrap();
            let s = zs.suggest(&emb, 5, 3);
            let probs = zs.vibe_probabilities(&emb).unwrap();
            let mut top: Vec<(usize, f32)> = probs.iter().copied().enumerate().collect();
            top.sort_by(|a, b| b.1.total_cmp(&a.1));
            let top: Vec<String> = top
                .iter()
                .take(3)
                .map(|(i, p)| format!("{}={p:.2}", zs.vocab.vibes[*i].label))
                .collect();
            println!(
                "{} {name}: keywords {:?} emojis {:?} vibe {:?} [{}]",
                e.model_id(),
                s.keywords,
                s.emojis,
                s.vibe,
                top.join(" ")
            );
            (s, probs)
        };
        let vibe_rank = |probs: &[f32], label: &str| {
            let i = zs.vocab.vibes.iter().position(|v| v.label == label).unwrap();
            probs.iter().filter(|&&p| p > probs[i]).count()
        };

        let (s, probs) = report("smiley", &face(1.0));
        assert!(!s.keywords.is_empty() && s.keywords.len() <= 5);
        assert!(s.keywords.windows(2).all(|w| w[0].1 >= w[1].1));
        assert!(
            names(&s.keywords)
                .iter()
                .any(|k| ["smiley face", "happy face", "smiling", "laughing face"].contains(k)),
            "{:?}",
            s.keywords
        );
        assert!(
            names(&s.emojis).iter().any(|k| ["😀", "😊", "😇", "🙃"].contains(k)),
            "{:?}",
            s.emojis
        );
        assert!(vibe_rank(&probs, "happy") <= 1, "happy should lead for a smiley");

        let (s, probs) = report("frowny", &face(-1.0));
        assert!(
            names(&s.keywords).iter().any(|k| k.contains("face")),
            "{:?}",
            s.keywords
        );
        assert!(
            vibe_rank(&probs, "sad") < vibe_rank(&probs, "happy"),
            "sad should beat happy for a frown"
        );

        let (s, _) = report("heart", &heart());
        assert!(names(&s.keywords).contains(&"heart"), "{:?}", s.keywords);
        assert!(names(&s.emojis).contains(&"❤️"), "{:?}", s.emojis);

        let (s, _) = report("red square", &square([220, 20, 20]));
        assert!(s.keywords.len() <= 5 && s.emojis.len() <= 3);
    }
}

#[test]
fn related_terms_expand_queries() {
    for (zs, e) in [
        (english_zeroshot(), english()),
        (multilingual_zeroshot(), multilingual()),
    ] {
        let (Some(zs), Some(e)) = (zs, e) else { continue };
        for q in ["kitty", "nope", "lol", "exhausted", "сумно", "кіт"] {
            let r = zs.related_terms(&e.embed_text(q).unwrap(), 5);
            println!("{} {q:?}: {r:?}", e.model_id());
            assert_eq!(r.len(), 5);
        }
        let top = |q: &str| zs.related_terms(&e.embed_text(q).unwrap(), 3);
        assert!(
            names(&top("kitty")).iter().any(|t| ["cat", "kitten"].contains(t)),
            "{:?}",
            top("kitty")
        );
        assert!(names(&top("exhausted")).contains(&"tired"), "{:?}", top("exhausted"));
        assert!(names(&top("nope")).contains(&"dismissive"), "{:?}", top("nope"));
        if e.model_id().contains("mclip") {
            assert!(
                names(&top("кіт")).iter().any(|t| ["cat", "kitten"].contains(t)),
                "{:?}",
                top("кіт")
            );
            assert!(names(&top("сумно")).contains(&"sad"), "{:?}", top("сумно"));
        }
    }
}
