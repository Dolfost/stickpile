use super::*;
use crate::hash::fixtures::{jpeg_round_trip, scene, stripes};
use image::imageops::{resize as img_resize, FilterType};
use image::Rgba;

/// Photo-like fixture: scene plus deterministic grain and sharp text-like bars.
pub(crate) fn photo(w: u32, h: u32) -> RgbaImage {
    let base = scene(w, h);
    let bars = stripes(w, h);
    RgbaImage::from_fn(w, h, |x, y| {
        let mut p = *base.get_pixel(x, y);
        let n = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)).wrapping_mul(2_654_435_761) >> 27;
        let grain = n as i32 - 16;
        if y > h * 3 / 4 && bars.get_pixel(x, y)[0] < 128 {
            p = Rgba([30, 25, 20, 255]);
        }
        for c in 0..3 {
            p[c] = (i32::from(p[c]) + grain / 3).clamp(20, 235) as u8;
        }
        p
    })
}

fn jpeg_bytes(img: &RgbaImage, q: u8) -> Vec<u8> {
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q)
        .encode_image(&rgb)
        .unwrap();
    out
}

fn png_bytes(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img.clone())
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn decode(bytes: &[u8]) -> RgbaImage {
    image::load_from_memory(bytes).unwrap().to_rgba8()
}

fn probe(format: MediaFormat, kind: MediaKind, img: &RgbaImage) -> Probe {
    Probe {
        format,
        kind,
        width: img.width(),
        height: img.height(),
        frame_count: None,
        duration_ms: None,
        fps: None,
        has_alpha: false,
    }
}

fn comp<'a>(r: &'a QualityReport, name: &str) -> &'a QualityComponent {
    r.components
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no {name} in {r:#?}"))
}

fn assess_jpeg(bytes: &[u8]) -> QualityReport {
    let frame = decode(bytes);
    assess(bytes, &probe(MediaFormat::Jpeg, MediaKind::Still, &frame), &frame, 512)
}

fn assess_png(img: &RgbaImage, rec: u32) -> QualityReport {
    let bytes = png_bytes(img);
    assess(&bytes, &probe(MediaFormat::Png, MediaKind::Still, img), img, rec)
}

const NAMES: [&str; 9] = [
    "resolution",
    "jpeg_quality",
    "recompression",
    "blockiness",
    "ringing",
    "sharpness",
    "alpha",
    "frame_rate",
    "palette",
];

fn check_shape(r: &QualityReport) {
    assert!(r.score <= 100);
    for c in &r.components {
        assert!(NAMES.contains(&c.name.as_str()), "unknown component {}", c.name);
        assert!(c.score <= 100 && c.weight > 0.0 && !c.detail.is_empty(), "{c:?}");
    }
    assert_eq!(r.score, total(&r.components));
}

#[test]
fn jpeg_quality_estimate_matches_encoder() {
    for img in [photo(256, 192), scene(200, 120)] {
        for q in [5u8, 10, 20, 35, 50, 62, 75, 85, 90, 95, 100] {
            let est = jpeg_quality_estimate(&jpeg_bytes(&img, q)).unwrap();
            assert!((i32::from(est) - i32::from(q)).abs() <= 5, "q={q} estimated {est}");
        }
    }
    assert_eq!(jpeg_quality_estimate(b"not a jpeg"), None);
}

#[test]
fn jpeg_quality_estimate_handles_natural_order_tables() {
    // An encoder that wrote its table in natural instead of zig-zag order:
    // the stored sequence equals the natural IJG table.
    let jpeg = jpeg_bytes(&scene(64, 64), 70);
    let t = jpeg::luma_table(&jpeg).unwrap();
    let mut broken = jpeg.clone();
    let pos = broken.windows(2).position(|w| w == [0xFF, 0xDB]).unwrap();
    // FF DB, length (2 bytes), Pq/Tq, 64 values.
    for (i, v) in t.natural.iter().enumerate() {
        broken[pos + 5 + i] = *v as u8;
    }
    assert_eq!(jpeg_quality_estimate(&broken), Some(70));
    // Tables that are not IJG-scaled still get a nearby estimate.
    let custom = QuantTable {
        id: 0,
        natural: ijg_table(80).map(|v| v + 1),
        stored: ijg_table(80).map(|v| v + 2),
    };
    let est = estimate_ijg(&custom);
    assert!((75..=80).contains(&est.quality), "{est:?}");
}

#[test]
fn jpeg_q90_scores_higher_than_q20() {
    for img in [photo(384, 384), scene(384, 320)] {
        let hi = assess_jpeg(&jpeg_bytes(&img, 90));
        let lo = assess_jpeg(&jpeg_bytes(&img, 20));
        check_shape(&hi);
        check_shape(&lo);
        assert!(hi.score > lo.score + 10, "q90 {} vs q20 {}", hi.score, lo.score);
        assert!(comp(&hi, "jpeg_quality").score > comp(&lo, "jpeg_quality").score);
        assert!(comp(&hi, "blockiness").score > comp(&lo, "blockiness").score);
        assert!(comp(&lo, "jpeg_quality").detail.contains("q=20"));
    }
}

#[test]
fn ringing_grows_with_compression() {
    let img = scene(384, 384);
    let q95 = assess_jpeg(&jpeg_bytes(&img, 95));
    let q40 = assess_jpeg(&jpeg_bytes(&img, 40));
    assert!(
        comp(&q95, "ringing").score > comp(&q40, "ringing").score,
        "{q95:#?} {q40:#?}"
    );
    // A clean synthetic edge has none.
    assert_eq!(comp(&assess_png(&img, 384), "ringing").score, 100);
}

#[test]
fn blurred_is_less_sharp() {
    let img = photo(400, 300);
    let blurred = image::imageops::blur(&img, 1.5);
    let sharp = assess_png(&img, 400);
    let soft = assess_png(&blurred, 400);
    assert!(
        comp(&sharp, "sharpness").score > comp(&soft, "sharpness").score + 20,
        "{:?} vs {:?}",
        comp(&sharp, "sharpness"),
        comp(&soft, "sharpness")
    );
    // Blur must not read as ringing.
    assert_eq!(comp(&soft, "ringing").score, 100);
}

#[test]
fn upscaled_small_scores_lower_on_resolution() {
    let native = photo(512, 512);
    let native_score = comp(&assess_png(&native, 512), "resolution").score;
    assert_eq!(native_score, 100);
    for (small, filter) in [
        (128, FilterType::Triangle),
        (128, FilterType::Lanczos3),
        (256, FilterType::CatmullRom),
    ] {
        let up = img_resize(
            &img_resize(&native, small, small, FilterType::Lanczos3),
            512,
            512,
            filter,
        );
        let r = assess_png(&up, 512);
        let res = comp(&r, "resolution");
        assert!(res.score <= 50, "{small} {filter:?}: {res:?}");
        assert!(res.detail.contains("upscaled"), "{res:?}");
    }
    // Smaller than recommended: proportional.
    let small = photo(256, 128);
    assert_eq!(comp(&assess_png(&small, 512), "resolution").score, 50);
    // Larger than recommended is capped at 100 and cheap.
    let big = photo(1600, 1200);
    assert_eq!(comp(&assess_png(&big, 512), "resolution").score, 100);
}

#[test]
fn double_compression_is_detected() {
    for img in [photo(384, 384), scene(384, 384)] {
        let single = assess_jpeg(&jpeg_bytes(&img, 90));
        assert_eq!(
            comp(&single, "recompression").score,
            100,
            "{:?}",
            comp(&single, "recompression")
        );
        for (q1, q2) in [(50u8, 90u8), (30, 85), (60, 80)] {
            let first = decode(&jpeg_bytes(&img, q1));
            let double = assess_jpeg(&jpeg_bytes(&first, q2));
            let rc = comp(&double, "recompression");
            assert!(rc.score <= 50, "{q1}->{q2}: {rc:?}");
            assert!(rc.detail.contains("earlier JPEG"), "{rc:?}");
        }
    }
}

#[test]
fn shifted_grid_is_detected() {
    let first = decode(&jpeg_bytes(&photo(400, 400), 40));
    let cropped = image::imageops::crop_imm(&first, 3, 5, 384, 384).to_image();
    let r = assess_jpeg(&jpeg_bytes(&cropped, 92));
    let rc = comp(&r, "recompression");
    assert!(rc.score <= 50, "{rc:?}");
}

#[test]
fn lossless_copy_of_a_jpeg_is_flagged() {
    let img = photo(320, 320);
    let clean = assess_png(&img, 320);
    assert_eq!(comp(&clean, "recompression").score, 100);
    let from_jpeg = jpeg_round_trip(&img, 25);
    let dirty = assess_png(&from_jpeg, 320);
    let rc = comp(&dirty, "recompression");
    assert!(rc.score < 50, "{rc:?}");
    assert!(dirty.score < clean.score);
}

#[test]
fn alpha_is_a_bonus_only_when_transparent() {
    let mut sticker = scene(128, 128);
    for (x, y, p) in sticker.enumerate_pixels_mut() {
        let (dx, dy) = (x as i32 - 64, y as i32 - 64);
        if dx * dx + dy * dy > 50 * 50 {
            *p = Rgba([0, 0, 0, 0]);
        }
    }
    let r = assess_png(&sticker, 128);
    check_shape(&r);
    assert_eq!(comp(&r, "alpha").score, 100);
    let opaque = assess_png(&scene(128, 128), 128);
    assert!(opaque.components.iter().all(|c| c.name != "alpha"));
}

#[test]
fn animation_components() {
    let frame = scene(160, 120);
    let mut p = probe(MediaFormat::Gif, MediaKind::Animation, &frame);
    p.fps = Some(25.0);
    let smooth = assess(&png_bytes(&frame), &p, &frame, 160);
    check_shape(&smooth);
    assert_eq!(comp(&smooth, "frame_rate").score, 100);
    p.fps = Some(5.0);
    let choppy = assess(&png_bytes(&frame), &p, &frame, 160);
    assert!(comp(&choppy, "frame_rate").score < 50);
    assert!(choppy.score < smooth.score);
    let rich = comp(&smooth, "palette").score;
    let two = RgbaImage::from_fn(160, 120, |x, _| {
        if x < 80 {
            Rgba([0, 0, 0, 255])
        } else {
            Rgba([255, 255, 255, 255])
        }
    });
    let poor = assess(&png_bytes(&two), &p, &two, 160);
    assert!(comp(&poor, "palette").score < rich);
    assert!(comp(&poor, "palette").detail.starts_with("2 colours"));
    // Stills have neither.
    let still = assess_png(&frame, 160);
    assert!(still
        .components
        .iter()
        .all(|c| c.name != "frame_rate" && c.name != "palette"));
    // Videos have a frame rate but no palette.
    let mut v = probe(MediaFormat::Mp4, MediaKind::Video, &frame);
    v.fps = Some(30.0);
    let video = assess(b"", &v, &frame, 160);
    assert_eq!(comp(&video, "frame_rate").score, 100);
    assert!(video
        .components
        .iter()
        .all(|c| c.name != "palette" && c.name != "jpeg_quality"));
}

#[test]
fn tiny_and_flat_images_do_not_panic() {
    for (w, h) in [(1, 1), (3, 200), (8, 8), (33, 31)] {
        let img = RgbaImage::from_pixel(w, h, Rgba([120, 130, 140, 255]));
        let r = assess_png(&img, 512);
        check_shape(&r);
        assert!(r.components.iter().any(|c| c.name == "resolution"));
        let jpeg = jpeg_bytes(&img, 75);
        check_shape(&assess_jpeg(&jpeg));
    }
    let empty = assess(
        b"",
        &probe(MediaFormat::Png, MediaKind::Still, &RgbaImage::new(0, 0)),
        &RgbaImage::new(0, 0),
        0,
    );
    assert_eq!(empty.score, 0);
}

#[test]
fn total_is_the_weighted_mean() {
    let c = |score, weight| QualityComponent {
        name: "x".into(),
        score,
        weight,
        detail: "d".into(),
    };
    assert_eq!(total(&[c(100, 1.0), c(0, 3.0)]), 25);
    assert_eq!(total(&[c(80, 2.0)]), 80);
    assert_eq!(total(&[]), 0);
}

#[test]
fn exif_orientation_does_not_shift_the_grid() {
    // A rotated JPEG whose width is not a multiple of 8 must not look
    // "shifted" after the frame is turned upright.
    let img = photo(203, 150);
    let jpeg = jpeg_bytes(&img, 90);
    let mut exif = b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0".to_vec();
    let mut seg = vec![0xFF, 0xE1];
    seg.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    seg.append(&mut exif);
    let mut rotated = jpeg[..2].to_vec();
    rotated.extend_from_slice(&seg);
    rotated.extend_from_slice(&jpeg[2..]);
    let frame = crate::decode_frame(&rotated, &crate::Limits::default()).unwrap();
    assert_eq!(frame.dimensions(), (150, 203));
    let r = assess(
        &rotated,
        &probe(MediaFormat::Jpeg, MediaKind::Still, &frame),
        &frame,
        512,
    );
    assert_eq!(comp(&r, "recompression").score, 100, "{:?}", comp(&r, "recompression"));
}

#[test]
#[ignore = "calibration printout: cargo test -p stickpile-media --release calibrate -- --ignored --nocapture"]
fn calibrate() {
    let img = photo(512, 512);
    for q in [10u8, 20, 40, 60, 75, 90, 95] {
        let b = jpeg_bytes(&img, q);
        let g = luma_on_white(&decode(&b));
        let grid = GridProfile::measure(&g).unwrap();
        println!(
            "q={q}: octaves={:?} seams={:.2}/{:.2} ring={:?} vol={:?} dq={:?} score={}",
            octave_energies(&g),
            grid.ratio(0, 0),
            grid.excess(0, 0),
            ringing_level(&g),
            laplacian_variance(&g),
            double_quantisation(&g, &jpeg::luma_table(&b).unwrap().natural),
            assess_jpeg(&b).score
        );
    }
}
