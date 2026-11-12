//! Detection tests: synthetic facts for every kind, and real images through
//! the media pipeline (ingest -> hashes and thumbnail -> signature) for the
//! near-duplicate thresholds.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use image::imageops::{self, FilterType};
use image::{Rgba, RgbaImage};
use stickpile_api::{JobStatus, MediaKind, ProblemKind, ProblemSeverity, Variant};
use stickpile_media::signature::Signature;

use super::detect::*;

const ENV: Env = Env {
    telegram: true,
    ffmpeg: true,
};

fn facts(id: &str, n: u64) -> ItemFacts {
    ItemFacts {
        id: id.into(),
        name: id.into(),
        position: n as i64,
        created_at: 0,
        media_id: format!("m-{id}"),
        sha256: format!("sha-{id}"),
        original_sha256: format!("orig-{id}"),
        kind: MediaKind::Still,
        width: 512,
        height: 512,
        duration_ms: None,
        quality: Some(80),
        // Far apart by default: every item gets its own 16-bit pattern.
        phash: (n.wrapping_mul(0x9E37_79B9_7F4A_7C15)) ^ 0x5555_0000_ffff_0000,
        dhash: n.wrapping_mul(0xC2B2_AE3D_27D4_EB4F),
        emojis: vec!["😀".into()],
        ai_fields: Vec::new(),
        variants: vec![Variant::Original, Variant::Thumb, Variant::TgSticker],
        variants_done: true,
        variants_job: None,
        embedding: None,
        earlier_in: Vec::new(),
    }
}

fn set(items: Vec<ItemFacts>) -> SetFacts {
    SetFacts {
        id: "set".into(),
        version: 3,
        updated_at: 100,
        items,
        ..SetFacts::default()
    }
}

fn run(f: &SetFacts, env: &Env) -> Vec<stickpile_api::Problem> {
    run_with(f, env, &HashMap::new())
}

fn run_with(f: &SetFacts, env: &Env, sigs: &HashMap<String, Arc<Signature>>) -> Vec<stickpile_api::Problem> {
    let c = candidates(&f.items);
    detect(f, &c, sigs, env)
}

fn kinds(ps: &[stickpile_api::Problem]) -> Vec<ProblemKind> {
    ps.iter().map(|p| p.kind).collect()
}

fn of_kind(ps: &[stickpile_api::Problem], k: ProblemKind) -> Vec<&stickpile_api::Problem> {
    ps.iter().filter(|p| p.kind == k).collect()
}

#[test]
fn a_clean_set_has_no_problems_and_an_empty_one_says_so() {
    let f = set(vec![facts("a", 1), facts("b", 2), facts("c", 3)]);
    assert!(run(&f, &ENV).is_empty(), "{:?}", run(&f, &ENV));
    let empty = run(&set(Vec::new()), &ENV);
    assert_eq!(kinds(&empty), [ProblemKind::EmptySet]);
    assert_eq!(empty[0].severity, ProblemSeverity::Info);
}

#[test]
fn exact_duplicates_are_grouped_by_stored_file() {
    let mut items: Vec<ItemFacts> = (0..4).map(|n| facts(&format!("i{n}"), n)).collect();
    // i0..i2 store the same sanitised original (different uploads).
    for it in &mut items[..3] {
        it.original_sha256 = "same-original".into();
        it.phash = 7;
        it.dhash = 7;
    }
    let f = set(items);
    let ps = run(&f, &ENV);
    let dups = of_kind(&ps, ProblemKind::ExactDuplicate);
    assert_eq!(dups.len(), 1);
    assert_eq!(dups[0].items.len(), 3);
    assert_eq!(dups[0].severity, ProblemSeverity::Warning);
    // Exact duplicates are not compared again as pairs.
    assert!(
        candidates(&f.items).iter().all(|c| c.b == 3),
        "{:?}",
        candidates(&f.items)
    );
}

#[test]
fn similar_needs_both_embeddings_and_a_high_cosine() {
    let unit = |v: &[f32]| -> Arc<[f32]> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect::<Vec<_>>().into()
    };
    let mut a = facts("a", 1);
    let mut b = facts("b", 2);
    let mut c = facts("c", 3);
    let d = facts("d", 4);
    a.embedding = Some(unit(&[1.0, 0.0, 0.0]));
    b.embedding = Some(unit(&[1.0, 0.25, 0.0])); // cos 0.970
    c.embedding = Some(unit(&[1.0, 0.0, 0.5])); // cos 0.894 to a
    let ps = run(&set(vec![a, b, c, d]), &ENV);
    let sim = of_kind(&ps, ProblemKind::Similar);
    assert_eq!(sim.len(), 1, "{ps:?}");
    assert_eq!(sim[0].severity, ProblemSeverity::Info);
    let ids: Vec<&str> = sim[0].items.iter().map(|i| i.item_id.as_str()).collect();
    assert_eq!(ids, ["a", "b"]);
    assert!(sim[0].score.unwrap() > 0.96 && sim[0].score.unwrap() < 0.98);
}

#[test]
fn look_alike_pairs_are_capped() {
    let e: Arc<[f32]> = vec![1.0, 0.0].into();
    let items: Vec<ItemFacts> = (0..20)
        .map(|n| {
            let mut it = facts(&format!("i{n:02}"), n);
            it.embedding = Some(e.clone());
            it
        })
        .collect();
    let ps = run(&set(items), &ENV);
    assert_eq!(of_kind(&ps, ProblemKind::Similar).len(), MAX_SIMILAR_PAIRS);
}

#[test]
fn hash_candidates_need_the_pixel_check() {
    let mut a = facts("a", 1);
    let mut b = facts("b", 2);
    b.phash = a.phash ^ 0b111; // distance 3
    b.dhash = a.dhash;
    a.kind = MediaKind::Still;
    let f = set(vec![a.clone(), b.clone()]);
    let c = candidates(&f.items);
    assert_eq!(c.len(), 1);
    assert!(c[0].near);
    // Without signatures (no thumbnails) nothing is claimed.
    assert!(of_kind(&run(&f, &ENV), ProblemKind::NearDuplicate).is_empty());
    // Same picture: a near-duplicate pair, worst first in set order.
    let pic = Arc::new(Signature::of(&face(Face::default())));
    let sigs: HashMap<String, Arc<Signature>> = [(a.media_id.clone(), pic.clone()), (b.media_id.clone(), pic)]
        .into_iter()
        .collect();
    let ps = run_with(&f, &ENV, &sigs);
    let near = of_kind(&ps, ProblemKind::NearDuplicate);
    assert_eq!(near.len(), 1);
    assert_eq!(near[0].score, Some(3.0));
    assert_eq!(near[0].items[0].item_id, "a");
    // A still and an animation, or different shapes, are never copies.
    let mut moving = b.clone();
    moving.kind = MediaKind::Animation;
    assert!(candidates(&[a.clone(), moving]).is_empty());
    let mut wide = b;
    wide.width = 1024;
    assert!(candidates(&[a, wide]).is_empty());
}

#[test]
fn emoji_checks_follow_the_telegram_export() {
    let mut a = facts("a", 1);
    a.emojis = vec!["cat".into(), "😺".into()];
    let mut b = facts("b", 2);
    b.emojis = Vec::new();
    let mut f = set(vec![a, b]);
    // Not published: words in the emoji field are a hint, no emoji is fine.
    let ps = run(&f, &ENV);
    assert_eq!(kinds(&ps), [ProblemKind::NotEmoji]);
    assert_eq!(ps[0].severity, ProblemSeverity::Info);
    assert_eq!(ps[0].items[0].detail.as_deref(), Some("cat"));
    // Published: both matter.
    f.telegram_pack = Some("cats_by_bot".into());
    f.telegram_mapping = Some(
        f.items
            .iter()
            .map(|it| (it.id.clone(), (it.sha256.clone(), format!("fu-{}", it.id))))
            .collect(),
    );
    let ps = run(&f, &ENV);
    assert_eq!(kinds(&ps), [ProblemKind::NoEmoji, ProblemKind::NotEmoji]);
    assert!(ps.iter().all(|p| p.severity == ProblemSeverity::Warning));
    // Without the Telegram integration a binding means nothing.
    let off = Env {
        telegram: false,
        ffmpeg: true,
    };
    assert_eq!(kinds(&run(&f, &off)), [ProblemKind::NotEmoji]);
}

#[test]
fn problem_ids_follow_the_content() {
    let mut a = facts("a", 1);
    a.emojis = vec!["lol".into()];
    let f = set(vec![a.clone()]);
    let id1 = run(&f, &ENV)[0].id.clone();
    assert_eq!(run(&f, &ENV)[0].id, id1, "stable");
    assert!(valid_problem_id(&id1), "{id1}");
    assert!(id1.starts_with("not_emoji-"));
    a.emojis = vec!["lol".into(), "omg".into()];
    let id2 = run(&set(vec![a]), &ENV)[0].id.clone();
    assert_ne!(id1, id2, "a changed item makes a new problem");
    for bad in [
        "",
        "not_emoji",
        "nope-0123456789abcdef",
        "not_emoji-0123",
        "not_emoji-0123456789ABCDEF",
    ] {
        assert!(!valid_problem_id(bad), "{bad}");
    }
}

#[test]
fn telegram_blockers_of_a_published_set() {
    let mut items: Vec<ItemFacts> = (0..123).map(|n| facts(&format!("i{n:03}"), n)).collect();
    // A long video is skipped, a 5 s animation is trimmed.
    items[0].kind = MediaKind::Video;
    items[0].duration_ms = Some(15_000);
    items[0].variants = vec![Variant::Original, Variant::Thumb];
    items[1].kind = MediaKind::Animation;
    items[1].duration_ms = Some(5_000);
    items[1].variants = vec![Variant::Original, Variant::Thumb, Variant::TgVideoSticker];
    let mut f = set(items);
    f.telegram_pack = Some("big_by_bot".into());
    f.telegram_mapping = Some(Default::default());
    let ps = run(&f, &ENV);
    let too_many = of_kind(&ps, ProblemKind::TelegramTooMany);
    assert_eq!(too_many.len(), 1);
    assert_eq!(too_many[0].severity, ProblemSeverity::Error);
    assert!(too_many[0].message.contains("122 stickers"), "{}", too_many[0].message);
    let skipped = of_kind(&ps, ProblemKind::TelegramUnsupported);
    assert_eq!(skipped[0].items.len(), 1);
    assert!(skipped[0].items[0]
        .detail
        .as_deref()
        .unwrap()
        .contains("longer than 10 s"));
    let trimmed = of_kind(&ps, ProblemKind::TelegramTrimmed);
    assert_eq!(trimmed[0].items[0].item_id, "i001");
    assert_eq!(trimmed[0].items[0].detail.as_deref(), Some("5 s"));
    let outdated = of_kind(&ps, ProblemKind::TelegramOutdated);
    assert!(
        outdated[0].message.contains("122 items not on Telegram yet"),
        "{}",
        outdated[0].message
    );
    // Errors first.
    assert_eq!(ps[0].kind, ProblemKind::TelegramTooMany);
    // Animations without ffmpeg cannot become stickers at all.
    let no_ffmpeg = Env {
        telegram: true,
        ffmpeg: false,
    };
    let ps = run(&f, &no_ffmpeg);
    assert_eq!(of_kind(&ps, ProblemKind::TelegramUnsupported)[0].items.len(), 2);
}

#[test]
fn telegram_outdated_and_failed() {
    let items: Vec<ItemFacts> = (0..3).map(|n| facts(&format!("i{n}"), n)).collect();
    let mut mapping: stickpile_telegram::PackMapping = items
        .iter()
        .map(|it| (it.id.clone(), (it.sha256.clone(), format!("fu-{}", it.id))))
        .collect();
    let mut f = set(items);
    f.telegram_pack = Some("p_by_bot".into());
    f.telegram_mapping = Some(mapping.clone());
    f.last_export_done_at = Some(200);
    assert!(run(&f, &ENV).is_empty(), "in sync");
    // Edited after the last export.
    f.updated_at = 300;
    let ps = run(&f, &ENV);
    assert_eq!(kinds(&ps), [ProblemKind::TelegramOutdated]);
    assert_eq!(ps[0].severity, ProblemSeverity::Info);
    // A sticker whose item is gone, and a replaced file.
    mapping.insert("gone".into(), ("x".into(), "fu-x".into()));
    mapping.get_mut("i1").unwrap().0 = "old-sha".into();
    f.telegram_mapping = Some(mapping);
    let ps = run(&f, &ENV);
    assert_eq!(ps[0].severity, ProblemSeverity::Warning);
    assert!(ps[0].message.contains("1 item with a new file"), "{}", ps[0].message);
    assert!(ps[0].message.contains("1 sticker removed here"), "{}", ps[0].message);
    // The newest export failed (also before any binding).
    f.telegram_pack = None;
    f.last_export = Some(JobFact {
        id: "job1".into(),
        status: JobStatus::Failed,
        error: Some("STICKERSET_INVALID".into()),
        finished_at: Some(400),
    });
    let ps = run(&f, &ENV);
    assert_eq!(kinds(&ps), [ProblemKind::TelegramFailed]);
    assert!(ps[0].message.contains("STICKERSET_INVALID"));
    f.last_export.as_mut().unwrap().status = JobStatus::Queued;
    assert!(run(&f, &ENV).is_empty(), "a queued retry is not a problem");
}

#[test]
fn media_and_metadata_checks() {
    let mut a = facts("a", 1);
    a.width = 100;
    a.height = 60;
    let mut b = facts("b", 2);
    b.variants_done = false;
    b.variants_job = Some(JobFact {
        id: "j".into(),
        status: JobStatus::Failed,
        error: Some("ffmpeg failed".into()),
        finished_at: Some(1),
    });
    let mut c = facts("c", 3);
    c.variants_done = false;
    c.variants_job = Some(JobFact {
        id: "k".into(),
        status: JobStatus::Queued,
        error: None,
        finished_at: None,
    });
    let mut d = facts("d", 4);
    d.variants = vec![Variant::Original];
    d.ai_fields = vec!["keywords", "vibe"];
    let mut e = facts("e", 5);
    e.earlier_in = vec![("s2".into(), "Older set".into())];
    let mut f = set(vec![a, b, c, d, e]);
    f.git_error = Some("could not resolve host".into());
    let ps = run(&f, &ENV);
    assert_eq!(
        kinds(&ps),
        [
            ProblemKind::MissingFiles,
            ProblemKind::GitFailed,
            ProblemKind::ProcessingFailed,
            ProblemKind::LowResolution,
            ProblemKind::InOtherSet,
            ProblemKind::AiUnreviewed,
        ]
    );
    let get = |k| of_kind(&ps, k)[0];
    assert_eq!(
        get(ProblemKind::MissingFiles).items[0].detail.as_deref(),
        Some("no thumbnail")
    );
    assert_eq!(get(ProblemKind::ProcessingFailed).items.len(), 1, "queued is fine");
    assert_eq!(
        get(ProblemKind::ProcessingFailed).items[0].detail.as_deref(),
        Some("ffmpeg failed")
    );
    assert_eq!(
        get(ProblemKind::LowResolution).items[0].detail.as_deref(),
        Some("100×60 px")
    );
    assert_eq!(get(ProblemKind::LowResolution).severity, ProblemSeverity::Info);
    assert_eq!(
        get(ProblemKind::AiUnreviewed).items[0].detail.as_deref(),
        Some("keywords, vibe")
    );
    assert_eq!(
        get(ProblemKind::InOtherSet).items[0].detail.as_deref(),
        Some("also in Older set")
    );
    assert!(get(ProblemKind::GitFailed).message.contains("could not resolve host"));
}

// ---------------------------------------------------------------------------
// Real images through the media pipeline
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Eyes {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mouth {
    Smile,
    Frown,
}

#[derive(Debug, Clone, Copy)]
struct Face {
    eyes: Eyes,
    mouth: Mouth,
    color: [u8; 3],
    ears: bool,
}

impl Default for Face {
    fn default() -> Face {
        Face {
            eyes: Eyes::Open,
            mouth: Mouth::Smile,
            color: [250, 210, 60],
            ears: false,
        }
    }
}

/// A flat cartoon face with a black outline on a transparent 512² canvas:
/// the kind of sticker whose variations the hashes confuse.
fn face(f: Face) -> RgbaImage {
    let (cx, cy, r) = (256.0f32, 270.0f32, 180.0f32);
    let fill = Rgba([f.color[0], f.color[1], f.color[2], 255]);
    let black = Rgba([10, 10, 10, 255]);
    let white = Rgba([255, 255, 255, 255]);
    let ring = |x: f32, y: f32, ox: f32, oy: f32, rr: f32, w: f32| ((x - ox).hypot(y - oy) - rr).abs() < w / 2.0;
    let arc = |x: f32, y: f32, ox: f32, oy: f32, rr: f32, w: f32, upper: bool| {
        ring(x, y, ox, oy, rr, w) && if upper { y < oy - rr * 0.4 } else { y > oy + rr * 0.4 }
    };
    RgbaImage::from_fn(512, 512, |x, y| {
        let (x, y) = (x as f32, y as f32);
        let d = (x - cx).hypot(y - cy);
        let ear = f.ears && (y < cy - 60.0) && (y > cy - 230.0) && {
            let t = (cy - 60.0 - y) / 170.0;
            ((x - (cx - 120.0)).abs() < 50.0 * (1.0 - t)) || ((x - (cx + 120.0)).abs() < 50.0 * (1.0 - t))
        };
        let mut px = if d < r - 5.0 || ear {
            fill
        } else if d < r + 5.0 {
            black
        } else {
            Rgba([0, 0, 0, 0])
        };
        if d < r {
            for ex in [cx - 65.0, cx + 65.0] {
                match f.eyes {
                    Eyes::Open => {
                        let de = (x - ex).hypot(y - (cy - 42.0));
                        if de < 12.0 {
                            px = black;
                        } else if de < 25.0 {
                            px = white;
                        } else if de < 29.0 {
                            px = black;
                        }
                    }
                    Eyes::Closed => {
                        if arc(x, y, ex, cy - 25.0, 30.0, 8.0, true) {
                            px = black;
                        }
                    }
                }
            }
            let mouth = match f.mouth {
                Mouth::Smile => arc(x, y, cx, cy + 10.0, 80.0, 10.0, false),
                Mouth::Frown => arc(x, y, cx, cy + 130.0, 80.0, 10.0, true),
            };
            if mouth {
                px = black;
            }
        }
        px
    })
}

/// A full-bleed picture (gradient, discs and texture) for crops into the image.
fn scene(seed: u32) -> RgbaImage {
    let s = seed as f32;
    RgbaImage::from_fn(480, 480, |x, y| {
        let (fx, fy) = (x as f32 / 480.0, y as f32 / 480.0);
        let mut c = [
            60.0 + 150.0 * fx,
            80.0 + 120.0 * (fy * (2.0 + s)).sin().abs(),
            200.0 - 120.0 * (fx + fy) / 2.0,
        ];
        for (k, (ox, oy, rr)) in [(0.3, 0.35, 0.12), (0.7, 0.6, 0.15), (0.45, 0.8, 0.08)]
            .iter()
            .enumerate()
        {
            let (ox, oy) = ((ox + 0.1 * s) % 1.0, (oy + 0.07 * s * k as f32) % 1.0);
            if (fx - ox).hypot(fy - oy) < *rr {
                c = [230.0 - 60.0 * k as f32, 60.0 * k as f32, 40.0 + 80.0 * k as f32];
            }
        }
        let t = 12.0 * ((fx * 41.0).sin() * (fy * 29.0).cos());
        Rgba([
            (c[0] + t).clamp(0.0, 255.0) as u8,
            (c[1] + t).clamp(0.0, 255.0) as u8,
            (c[2] + t).clamp(0.0, 255.0) as u8,
            255,
        ])
    })
}

fn png(img: &RgbaImage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// JPEG at `quality`, flattened on white (as a screenshot or a re-save would).
fn jpeg(img: &RgbaImage, quality: u8) -> Vec<u8> {
    let rgb = image::RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y).0;
        let a = f32::from(p[3]) / 255.0;
        image::Rgb([0, 1, 2].map(|i| (f32::from(p[i]) * a + 255.0 * (1.0 - a)).round() as u8))
    });
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(&rgb)
        .unwrap();
    out
}

fn resized(img: &RgbaImage, side: u32) -> RgbaImage {
    imageops::resize(img, side, side, FilterType::Lanczos3)
}

/// Cut `frac` off every side and scale back to the original size.
fn cropped(img: &RgbaImage, frac: f32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let (dx, dy) = ((w as f32 * frac) as u32, (h as f32 * frac) as u32);
    let c = imageops::crop_imm(img, dx, dy, w - 2 * dx, h - 2 * dy).to_image();
    imageops::resize(&c, w, h, FilterType::Lanczos3)
}

/// Ingest `bytes` like an upload and build the facts plus the signature of
/// its grid thumbnail.
fn ingest(name: &str, n: u64, bytes: &[u8]) -> (ItemFacts, Arc<Signature>) {
    let tmp = tempfile::tempdir().unwrap();
    let tools = stickpile_media::Tools::from_env(tmp.path().join("tmp"), Duration::from_secs(30));
    let opts = stickpile_media::IngestOptions {
        limits: stickpile_media::Limits::default(),
        thumb_px: 256,
        recommended_px: 512,
    };
    let ing = stickpile_media::ingest(bytes, &opts, &tools).unwrap();
    let sig = Signature::of_thumbnail(&ing.thumb.bytes, &stickpile_media::Limits::default()).unwrap();
    let mut it = facts(name, n);
    it.sha256 = ing.sha256.clone();
    it.original_sha256 = crate::ids::sha256_hex(&ing.original);
    it.phash = ing.phash;
    it.dhash = ing.dhash;
    it.width = ing.probe.width;
    it.height = ing.probe.height;
    (it, Arc::new(sig))
}

/// Near-duplicate pairs (by item id) the full detection finds among `files`.
fn near_pairs(files: &[(&str, Vec<u8>)]) -> Vec<(String, String)> {
    let mut items = Vec::new();
    let mut sigs = HashMap::new();
    for (n, (name, bytes)) in files.iter().enumerate() {
        let (it, sig) = ingest(name, n as u64, bytes);
        sigs.insert(it.media_id.clone(), sig);
        items.push(it);
    }
    let ps = run_with(&set(items), &ENV, &sigs);
    of_kind(&ps, ProblemKind::NearDuplicate)
        .iter()
        .map(|p| (p.items[0].item_id.clone(), p.items[1].item_id.clone()))
        .collect()
}

#[test]
fn copies_of_a_sticker_are_near_duplicates() {
    let base = face(Face::default());
    let files = [
        ("base", png(&base)),
        ("small", png(&resized(&base, 300))),
        ("jpeg", jpeg(&base, 60)),
        ("small-jpeg", jpeg(&resized(&base, 380), 70)),
        ("crop3", png(&cropped(&base, 0.03))),
        ("crop6", png(&cropped(&base, 0.06))),
    ];
    let pairs = near_pairs(&files);
    for other in ["small", "jpeg", "small-jpeg", "crop3", "crop6"] {
        assert!(
            pairs.iter().any(|p| p == &("base".to_string(), other.to_string())),
            "base ~ {other} missing: {pairs:?}"
        );
    }
}

#[test]
fn variations_of_one_character_are_not_near_duplicates() {
    let base = Face::default();
    let files = [
        ("smile", png(&face(base))),
        (
            "closed",
            png(&face(Face {
                eyes: Eyes::Closed,
                ..base
            })),
        ),
        (
            "frown",
            png(&face(Face {
                mouth: Mouth::Frown,
                ..base
            })),
        ),
        (
            "cat",
            png(&face(Face {
                ears: true,
                color: [240, 160, 70],
                ..base
            })),
        ),
        (
            "frog",
            png(&face(Face {
                color: [90, 190, 90],
                eyes: Eyes::Closed,
                ..base
            })),
        ),
        // The same face in pink: about the same grey, other colours.
        (
            "pink",
            png(&face(Face {
                color: [255, 175, 200],
                ..base
            })),
        ),
    ];
    // The hashes alone would call most of these copies...
    let items: Vec<ItemFacts> = files
        .iter()
        .enumerate()
        .map(|(n, (name, b))| ingest(name, n as u64, b).0)
        .collect();
    assert!(
        candidates(&items).iter().filter(|c| c.near).count() >= 3,
        "expected hash candidates among variations"
    );
    // ...the pixel check does not.
    assert_eq!(near_pairs(&files), Vec::<(String, String)>::new());
}

#[test]
fn crops_into_a_full_picture_are_near_duplicates_other_pictures_are_not() {
    let a = scene(1);
    let files = [
        ("scene", png(&a)),
        ("crop3", jpeg(&cropped(&a, 0.03), 80)),
        ("small", png(&resized(&a, 200))),
        ("other", png(&scene(3))),
    ];
    let pairs = near_pairs(&files);
    assert!(pairs.contains(&("scene".into(), "crop3".into())), "{pairs:?}");
    assert!(pairs.contains(&("scene".into(), "small".into())), "{pairs:?}");
    assert!(!pairs.iter().any(|(x, y)| x == "other" || y == "other"), "{pairs:?}");
}

#[test]
fn a_two_hundred_item_set_is_checked_quickly() {
    // Worst case for the pair pass: every pair is a hash candidate and needs
    // the pixel check (one character, many variations).
    let sig = |n: u64| {
        Arc::new(Signature::of(&face(Face {
            color: [(n * 37 % 200) as u8 + 40, (n * 91 % 200) as u8 + 40, 60],
            eyes: if n.is_multiple_of(2) { Eyes::Open } else { Eyes::Closed },
            ..Face::default()
        })))
    };
    let mut items = Vec::new();
    let mut sigs = HashMap::new();
    for n in 0..200u64 {
        let mut it = facts(&format!("i{n:03}"), n);
        it.phash = 0xF0F0_F0F0_0F0F_0F0F ^ (1 << (n % 64));
        it.dhash = 0x1234_5678_9ABC_DEF0;
        it.embedding = Some(vec![1.0, (n as f32) / 1000.0].into());
        sigs.insert(it.media_id.clone(), sig(n % 8));
        items.push(it);
    }
    let f = set(items);
    let t = std::time::Instant::now();
    let c = candidates(&f.items);
    let ps = detect(&f, &c, &sigs, &ENV);
    let took = t.elapsed();
    assert_eq!(c.len(), 200 * 199 / 2);
    assert!(!ps.is_empty());
    // Generous: debug builds on a busy machine.
    assert!(took < Duration::from_secs(20), "{took:?}");
    eprintln!("200 items, {} candidate pairs: {took:?}", c.len());
}
