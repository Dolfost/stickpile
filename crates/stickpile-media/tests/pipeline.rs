//! End-to-end tests of the public API: sniff, ingest, generate_variants and
//! decode_frame on fixtures generated at runtime (image/gif/webp crates and
//! ffmpeg's lavfi `testsrc`). Needs `ffmpeg`/`ffprobe` on PATH (or
//! STICKPILE_FFMPEG / STICKPILE_FFPROBE).

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use image::{AnimationDecoder, Rgba, RgbaImage};
use stickpile_media::*;

const ZIP_PAYLOAD: &[u8] = b"PK\x03\x04\x14\0\0\0\0\0SECRET-ZIP-PAYLOAD/evil.sh";
const HTML_PAYLOAD: &[u8] = b"<html><script>alert('SECRET')</script></html>";

struct Env {
    _dir: tempfile::TempDir,
    tools: Tools,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let tools = Tools::from_env(dir.path().join("tmp"), Duration::from_secs(120));
    Env { _dir: dir, tools }
}

fn opts() -> IngestOptions {
    IngestOptions {
        limits: Limits::default(),
        thumb_px: 256,
        recommended_px: 512,
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn assert_clean(bytes: &[u8]) {
    for needle in [&b"SECRET"[..], b"PK\x03\x04", b"<html", b"<script"] {
        assert!(
            !contains(bytes, needle),
            "{:?} survived sanitising",
            String::from_utf8_lossy(needle)
        );
    }
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(b))
}

/// Sticker-like still: disc on transparent background with a gradient.
fn sticker(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        let (dx, dy) = (x as f32 - w as f32 / 2.0, y as f32 - h as f32 / 2.0);
        if dx * dx + dy * dy > (w.min(h) as f32 / 2.2).powi(2) {
            Rgba([0, 0, 0, 0])
        } else {
            Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, 160, 255])
        }
    })
}

fn opaque(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        Rgba([(x * 3) as u8, (y * 5) as u8, ((x + y) * 2) as u8, 255])
    })
}

fn png_with_text(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width(), img.height());
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.add_text_chunk("Comment".into(), "SECRET-TEXT".into()).unwrap();
        let mut w = enc.write_header().unwrap();
        w.write_chunk(png::chunk::ChunkType(*b"eXIf"), b"MM\0*SECRET-EXIF")
            .unwrap();
        w.write_image_data(img.as_raw()).unwrap();
    }
    out
}

fn jpeg(img: &RgbaImage, q: u8) -> Vec<u8> {
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q)
        .encode_image(&rgb)
        .unwrap();
    out
}

fn with_jpeg_metadata(jpeg: &[u8]) -> Vec<u8> {
    let seg = |m: u8, p: &[u8]| {
        let mut v = vec![0xFF, m];
        v.extend_from_slice(&((p.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(p);
        v
    };
    let mut out = jpeg[..2].to_vec();
    out.extend(seg(0xE1, b"Exif\0\0II*\0\x08\0\0\0\0\0SECRET-GPS"));
    out.extend(seg(
        0xE1,
        b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>SECRET</x:xmpmeta>",
    ));
    out.extend(seg(0xFE, b"SECRET-COMMENT"));
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// Three-frame GIF: local palettes, transparency, disposal, delays, loop
/// count 3, plus a comment extension.
fn animated_gif(w: u16, h: u16) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let global = [0u8, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0, 255];
        let mut enc = gif::Encoder::new(&mut out, w, h, &global).unwrap();
        enc.set_repeat(gif::Repeat::Finite(3)).unwrap();
        enc.write_raw_extension(gif::AnyExtension(0xFE), &[b"SECRET-COMMENT"])
            .unwrap();
        for i in 0..3u16 {
            let pixels: Vec<u8> = (0..u32::from(w) * u32::from(h))
                .map(|p| {
                    let (x, y) = (p % u32::from(w), p / u32::from(w));
                    if (x + y + u32::from(i) * 7) % 13 == 0 {
                        0
                    } else {
                        ((x / 8 + y / 8 + u32::from(i)) % 3 + 1) as u8
                    }
                })
                .collect();
            let mut frame = gif::Frame::from_indexed_pixels(w, h, pixels, Some(0));
            frame.delay = 10 * (i + 1);
            frame.dispose = gif::DisposalMethod::Background;
            if i == 1 {
                frame.palette = Some(vec![0, 0, 0, 0, 200, 0, 0, 0, 200, 200, 200, 0]);
            }
            enc.write_frame(&frame).unwrap();
        }
    }
    out
}

/// Photo-like GIF that LZW cannot compress: 256-colour noise.
fn noisy_gif(w: u16, h: u16, frames: u16) -> Vec<u8> {
    let palette: Vec<u8> = (0..=255u8).flat_map(|i| [i, 255 - i, i / 2]).collect();
    let mut out = Vec::new();
    {
        let mut enc = gif::Encoder::new(&mut out, w, h, &palette).unwrap();
        enc.set_repeat(gif::Repeat::Infinite).unwrap();
        let mut state = 0x9E37_79B9u32;
        for _ in 0..frames {
            let pixels: Vec<u8> = (0..u32::from(w) * u32::from(h))
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state as u8
                })
                .collect();
            let mut frame = gif::Frame::from_indexed_pixels(w, h, pixels, None);
            frame.delay = 8;
            enc.write_frame(&frame).unwrap();
        }
    }
    out
}

fn animated_webp(w: u32, h: u32, frames: u32) -> Vec<u8> {
    let config = webp::WebPConfig::new().unwrap();
    let images: Vec<Vec<u8>> = (0..frames)
        .map(|i| {
            RgbaImage::from_fn(w, h, |x, y| {
                let on = (x + i * 4) % 24 < 12;
                Rgba([
                    if on { 230 } else { 20 },
                    (y * 255 / h) as u8,
                    90,
                    if x < 4 { 0 } else { 255 },
                ])
            })
            .into_raw()
        })
        .collect();
    let mut enc = webp::AnimEncoder::new(w, h, &config);
    enc.set_loop_count(0);
    for (i, img) in images.iter().enumerate() {
        enc.add_frame(webp::AnimFrame::from_rgba(img, w, h, i as i32 * 80));
    }
    enc.encode().to_vec()
}

/// Wrap a WebP's chunks into a VP8X container with metadata chunks.
fn webp_with_metadata(simple: &[u8]) -> Vec<u8> {
    let push = |out: &mut Vec<u8>, fourcc: &[u8], payload: &[u8]| {
        out.extend_from_slice(fourcc);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            out.push(0);
        }
    };
    let img = image::load_from_memory(simple).unwrap();
    let (w, h) = (img.width() - 1, img.height() - 1);
    let mut body = Vec::new();
    let mut vp8x = vec![0x20 | 0x10 | 0x08 | 0x04, 0, 0, 0];
    vp8x.extend_from_slice(&w.to_le_bytes()[..3]);
    vp8x.extend_from_slice(&h.to_le_bytes()[..3]);
    push(&mut body, b"VP8X", &vp8x);
    push(&mut body, b"ICCP", b"SECRET-ICC");
    let mut pos = 12;
    while pos + 8 <= simple.len() {
        let size = u32::from_le_bytes(simple[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if &simple[pos..pos + 4] != b"VP8X" {
            push(&mut body, &simple[pos..pos + 4], &simple[pos + 8..pos + 8 + size]);
        }
        pos += 8 + size + (size & 1);
    }
    push(&mut body, b"EXIF", b"SECRET-EXIF");
    push(&mut body, b"XMP ", b"SECRET-XMP");
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    out
}

fn ffmpeg_bin() -> PathBuf {
    std::env::var_os("STICKPILE_FFMPEG")
        .map(PathBuf::from)
        .unwrap_or_else(|| "ffmpeg".into())
}

fn ffprobe_bin() -> PathBuf {
    std::env::var_os("STICKPILE_FFPROBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| "ffprobe".into())
}

/// Generate a test video with ffmpeg's lavfi sources.
fn make_video(dir: &Path, name: &str, extra: &[&str]) -> Vec<u8> {
    let out = dir.join(name);
    let status = Command::new(ffmpeg_bin())
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
        .args(extra)
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success(), "fixture generation failed: {extra:?}");
    std::fs::read(out).unwrap()
}

fn mp4_fixture(dir: &Path, seconds: u32) -> Vec<u8> {
    let d = seconds.to_string();
    make_video(
        dir,
        &format!("fixture-{seconds}.mp4"),
        &[
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc=size=160x120:rate=15:duration={d}"),
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency=440:duration={d}"),
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-metadata",
            "title=SECRET-TITLE",
            "-metadata",
            "comment=SECRET-COMMENT",
            "-metadata:s:v:0",
            "handler_name=SECRET-HANDLER",
        ],
    )
}

fn webm_fixture(dir: &Path, alpha: bool) -> Vec<u8> {
    let src = if alpha {
        "testsrc=size=128x96:rate=20:duration=2,format=yuva420p,geq=lum='lum(X,Y)':cb='cb(X,Y)':cr='cr(X,Y)':a='if(lt(X,64),0,255)'"
    } else {
        "testsrc=size=128x96:rate=20:duration=2"
    };
    make_video(
        dir,
        if alpha { "alpha.webm" } else { "plain.webm" },
        &[
            "-f",
            "lavfi",
            "-i",
            src,
            "-c:v",
            "libvpx-vp9",
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
            "-pix_fmt",
            if alpha { "yuva420p" } else { "yuv420p" },
            "-metadata",
            "title=SECRET-TITLE",
        ],
    )
}

#[derive(Debug, serde::Deserialize)]
struct ProbeOut {
    streams: Vec<serde_json::Value>,
    format: serde_json::Value,
}

fn ffprobe_bytes(bytes: &[u8], ext: &str) -> ProbeOut {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("x.{ext}"));
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new(ffprobe_bin())
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn gif_frames(bytes: &[u8]) -> Vec<(RgbaImage, u32)> {
    image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
        .unwrap()
        .into_frames()
        .map(|f| {
            let f = f.unwrap();
            let (n, d) = f.delay().numer_denom_ms();
            (f.into_buffer(), n / d)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Sniffing and limits
// ---------------------------------------------------------------------------

#[test]
fn rejects_unknown_and_lookalike_formats() {
    let e = env();
    for bytes in [
        &b"hello world, not an image"[..],
        b"\0\0\0\x18ftypheic\0\0\0\0mif1heic",
        b"\x1a\x45\xdf\xa3\x88\x42\x82\x88matroska",
        b"%PDF-1.7",
        ZIP_PAYLOAD,
        HTML_PAYLOAD,
    ] {
        assert!(sniff(bytes).is_none());
        assert!(matches!(ingest(bytes, &opts(), &e.tools), Err(MediaError::Unsupported)));
    }
}

#[test]
fn enforces_byte_pixel_and_frame_limits() {
    let e = env();
    let png = png_with_text(&opaque(64, 64));
    let mut o = opts();
    o.limits.max_bytes = 100;
    assert!(matches!(ingest(&png, &o, &e.tools), Err(MediaError::TooLarge(_))));

    // A 6000x6000 palette PNG is a few KB on disk; the header check must
    // reject it before a pixel buffer is allocated.
    let mut bomb = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut bomb, 6000, 6000);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::One);
        enc.set_palette(vec![0, 0, 0, 255, 255, 255]);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&vec![0u8; 750 * 6000]).unwrap();
    }
    let mut o = opts();
    o.limits.max_pixels = 1_000_000;
    assert!(matches!(ingest(&bomb, &o, &e.tools), Err(MediaError::TooLarge(_))));
    assert!(matches!(decode_frame(&bomb, &o.limits), Err(MediaError::TooLarge(_))));

    let gif = animated_gif(32, 32);
    let mut o = opts();
    o.limits.max_frames = 2;
    assert!(matches!(ingest(&gif, &o, &e.tools), Err(MediaError::TooLarge(_))));
    let webp = animated_webp(32, 32, 4);
    assert!(matches!(ingest(&webp, &o, &e.tools), Err(MediaError::TooLarge(_))));
}

#[test]
fn rejects_corrupt_files() {
    let e = env();
    let png = png_with_text(&opaque(64, 64));
    assert!(matches!(
        ingest(&png[..png.len() / 2], &opts(), &e.tools),
        Err(MediaError::Invalid(_))
    ));
    let gif = animated_gif(32, 32);
    assert!(matches!(
        ingest(&gif[..40], &opts(), &e.tools),
        Err(MediaError::Invalid(_))
    ));
    let webp = webp::Encoder::from_rgba(opaque(16, 16).as_raw(), 16, 16)
        .encode(80.0)
        .to_vec();
    assert!(matches!(
        ingest(&webp[..webp.len() - 20], &opts(), &e.tools),
        Err(MediaError::Invalid(_))
    ));
    let mut bad = webp.clone();
    bad[20..30].fill(0xAA); // VP8 frame header
    assert!(matches!(ingest(&bad, &opts(), &e.tools), Err(MediaError::Invalid(_))));
    let jpeg = jpeg(&opaque(64, 64), 80);
    assert!(matches!(
        ingest(&jpeg[..200], &opts(), &e.tools),
        Err(MediaError::Invalid(_))
    ));
}

// ---------------------------------------------------------------------------
// Stills
// ---------------------------------------------------------------------------

#[test]
fn png_is_reencoded_losslessly_without_metadata() {
    let e = env();
    let img = sticker(300, 200);
    let mut upload = png_with_text(&img);
    upload.extend_from_slice(ZIP_PAYLOAD);
    let ing = ingest(&upload, &opts(), &e.tools).unwrap();
    assert_eq!(ing.sha256, sha256_hex(&upload));
    assert_eq!(ing.probe.format, MediaFormat::Png);
    assert_eq!(ing.probe.kind, MediaKind::Still);
    assert_eq!((ing.probe.width, ing.probe.height), (300, 200));
    assert!(ing.probe.has_alpha);
    assert_clean(&ing.original);
    assert!(!contains(&ing.original, b"tEXt") && !contains(&ing.original, b"eXIf"));
    assert_eq!(image::load_from_memory(&ing.original).unwrap().to_rgba8(), img);
    assert_eq!(ing.frame, img);
    assert_eq!(ing.thumb.variant, Variant::Thumb);
    assert_eq!(ing.thumb.mime, "image/webp");
    assert_eq!((ing.thumb.width, ing.thumb.height), (256, 171));
    let thumb = image::load_from_memory(&ing.thumb.bytes).unwrap().to_rgba8();
    assert_eq!(thumb.get_pixel(0, 0)[3], 0, "thumb keeps alpha");
    assert!(ing.quality.components.iter().any(|c| c.name == "alpha"));
    assert_eq!(ing.phash, hash::phash(&img));
    // Idempotent: ingesting the sanitised original gives the same bytes.
    assert_eq!(ingest(&ing.original, &opts(), &e.tools).unwrap().original, ing.original);
}

#[test]
fn palette_png_stays_a_palette_png() {
    let e = env();
    let mut upload = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut upload, 40, 30);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Four);
        enc.set_palette(vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 9, 9, 9]);
        enc.set_trns(vec![0, 255, 255, 128]);
        let mut w = enc.write_header().unwrap();
        let data: Vec<u8> = (0..30 * 20).map(|i| ((i % 4) << 4 | ((i + 1) % 4)) as u8).collect();
        w.write_image_data(&data).unwrap();
    }
    let ing = ingest(&upload, &opts(), &e.tools).unwrap();
    let dec = png::Decoder::new(Cursor::new(&ing.original));
    let reader = dec.read_info().unwrap();
    assert_eq!(reader.info().color_type, png::ColorType::Indexed);
    assert_eq!(reader.info().bit_depth, png::BitDepth::Four);
    assert!(reader.info().trns.is_some());
    assert_eq!(
        image::load_from_memory(&ing.original).unwrap().to_rgba8(),
        image::load_from_memory(&upload).unwrap().to_rgba8()
    );
    assert!(ing.probe.has_alpha);
}

#[test]
fn jpeg_is_rebuilt_without_metadata_or_polyglot() {
    let e = env();
    let base = jpeg(&opaque(200, 120), 85);
    let mut upload = with_jpeg_metadata(&base);
    upload.extend_from_slice(HTML_PAYLOAD);
    upload.extend_from_slice(ZIP_PAYLOAD);
    let ing = ingest(&upload, &opts(), &e.tools).unwrap();
    assert_eq!(ing.probe.format, MediaFormat::Jpeg);
    assert_clean(&ing.original);
    assert!(!contains(&ing.original, b"Exif") && !contains(&ing.original, b"xmpmeta"));
    assert!(ing.original.ends_with(&[0xFF, 0xD9]));
    // Entropy-coded data untouched: identical pixels, no new generation.
    assert_eq!(
        image::load_from_memory(&ing.original).unwrap().to_rgb8(),
        image::load_from_memory(&base).unwrap().to_rgb8()
    );
    assert_eq!(quality::jpeg_quality_estimate(&ing.original), Some(85));
    let q = ing
        .quality
        .components
        .iter()
        .find(|c| c.name == "jpeg_quality")
        .unwrap();
    assert!(q.detail.contains("q=85"), "{q:?}");
}

#[test]
fn webp_still_drops_metadata_chunks() {
    let e = env();
    let simple = webp::Encoder::from_rgba(sticker(64, 48).as_raw(), 64, 48)
        .encode(85.0)
        .to_vec();
    let mut upload = webp_with_metadata(&simple);
    upload.extend_from_slice(ZIP_PAYLOAD);
    let ing = ingest(&upload, &opts(), &e.tools).unwrap();
    assert_eq!(ing.probe.format, MediaFormat::Webp);
    assert_eq!(ing.probe.kind, MediaKind::Still);
    assert!(ing.probe.has_alpha);
    assert_clean(&ing.original);
    for chunk in [b"ICCP", b"EXIF", b"XMP "] {
        assert!(!contains(&ing.original, chunk));
    }
    assert_eq!(
        image::load_from_memory(&ing.original).unwrap().to_rgba8(),
        image::load_from_memory(&simple).unwrap().to_rgba8()
    );
}

#[test]
fn still_gif_is_a_still() {
    let e = env();
    let mut out = Vec::new();
    {
        let mut enc = gif::Encoder::new(&mut out, 20, 10, &[0, 0, 0, 255, 255, 255]).unwrap();
        let frame = gif::Frame::from_indexed_pixels(20, 10, (0..200).map(|i| (i % 2) as u8).collect::<Vec<_>>(), None);
        enc.write_frame(&frame).unwrap();
    }
    let ing = ingest(&out, &opts(), &e.tools).unwrap();
    assert_eq!(ing.probe.kind, MediaKind::Still);
    assert_eq!(ing.probe.frame_count, None);
}

#[test]
fn still_variants() {
    let e = env();
    let dir = tempfile::tempdir().unwrap();
    let vopts = VariantOptions {
        discord_gif_max_bytes: 10 << 20,
    };

    let ing = ingest(&jpeg(&opaque(300, 600), 90), &opts(), &e.tools).unwrap();
    let path = dir.path().join("orig.jpg");
    std::fs::write(&path, &ing.original).unwrap();
    let v = generate_variants(&path, &ing.probe, &vopts, &e.tools).unwrap();
    let kinds: Vec<Variant> = v.iter().map(|r| r.variant).collect();
    assert_eq!(kinds, vec![Variant::Png, Variant::TgSticker]);
    assert_eq!(v[0].mime, "image/png");
    assert_eq!(
        image::load_from_memory(&v[0].bytes).unwrap().dimensions_tuple(),
        (300, 600)
    );
    let tg = &v[1];
    assert_eq!((tg.width, tg.height), (256, 512));
    assert_eq!(tg.mime, "image/webp");
    assert!(tg.bytes.len() <= 512 * 1024);
    assert_eq!(sniff(&tg.bytes), Some(MediaFormat::Webp));

    // PNG original: no Png variant; small stickers are upscaled to 512.
    let ing = ingest(&png_with_text(&sticker(100, 80)), &opts(), &e.tools).unwrap();
    let path = dir.path().join("orig.png");
    std::fs::write(&path, &ing.original).unwrap();
    let v = generate_variants(&path, &ing.probe, &vopts, &e.tools).unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].variant, Variant::TgSticker);
    assert_eq!((v[0].width, v[0].height), (512, 410));
    let decoded = image::load_from_memory(&v[0].bytes).unwrap().to_rgba8();
    assert_eq!(decoded.get_pixel(0, 0)[3], 0, "sticker keeps alpha");
}

/// `(w, h)` of a decoded image.
trait Dims {
    fn dimensions_tuple(&self) -> (u32, u32);
}
impl Dims for image::DynamicImage {
    fn dimensions_tuple(&self) -> (u32, u32) {
        (self.width(), self.height())
    }
}

#[test]
fn big_noisy_sticker_is_squeezed_under_the_limit() {
    let e = env();
    let dir = tempfile::tempdir().unwrap();
    // White noise is the worst case for WebP.
    let mut state = 0x1234_5678u32;
    let noise = RgbaImage::from_fn(1024, 1024, |_, _| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let b = state.to_le_bytes();
        Rgba([b[0], b[1], b[2], 255])
    });
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(noise)
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let ing = ingest(&png, &opts(), &e.tools).unwrap();
    let path = dir.path().join("noise.png");
    std::fs::write(&path, &ing.original).unwrap();
    let v = generate_variants(
        &path,
        &ing.probe,
        &VariantOptions {
            discord_gif_max_bytes: 1 << 20,
        },
        &e.tools,
    )
    .unwrap();
    let tg = v.iter().find(|r| r.variant == Variant::TgSticker).unwrap();
    assert!(tg.bytes.len() <= 512 * 1024, "{}", tg.bytes.len());
    assert_eq!((tg.width, tg.height), (512, 512));
}

// ---------------------------------------------------------------------------
// Animations
// ---------------------------------------------------------------------------

#[test]
fn animated_gif_is_reencoded_frame_by_frame() {
    let e = env();
    let src = animated_gif(48, 32);
    let mut upload = src.clone();
    upload.extend_from_slice(HTML_PAYLOAD);
    let ing = ingest(&upload, &opts(), &e.tools).unwrap();
    assert_eq!(ing.probe.kind, MediaKind::Animation);
    assert_eq!(ing.probe.frame_count, Some(3));
    assert_eq!(ing.probe.duration_ms, Some(100 + 200 + 300));
    assert!((ing.probe.fps.unwrap() - 5.0).abs() < 0.01);
    assert!(ing.probe.has_alpha);
    assert_clean(&ing.original);
    assert!(contains(&ing.original, b"NETSCAPE2.0"), "loop count kept");
    let dec = gif::DecodeOptions::new().read_info(Cursor::new(&ing.original)).unwrap();
    let mut dec = dec;
    while dec.read_next_frame().unwrap().is_some() {}
    assert_eq!(dec.repeat(), gif::Repeat::Finite(3));
    // Lossless: every composited frame and delay identical.
    assert_eq!(gif_frames(&ing.original), gif_frames(&src));
    assert_eq!(ing.frame, gif_frames(&src)[0].0);
    let names: Vec<&str> = ing.quality.components.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"frame_rate") && names.contains(&"palette"), "{names:?}");
}

#[test]
fn animated_webp_is_probed_and_cleaned() {
    let e = env();
    let src = animated_webp(64, 48, 5);
    let mut upload = src.clone();
    upload.extend_from_slice(ZIP_PAYLOAD);
    let ing = ingest(&upload, &opts(), &e.tools).unwrap();
    assert_eq!(ing.probe.format, MediaFormat::Webp);
    assert_eq!(ing.probe.kind, MediaKind::Animation);
    assert_eq!(ing.probe.frame_count, Some(5));
    assert!(ing.probe.has_alpha);
    assert_clean(&ing.original);
    let frames = |b: &[u8]| -> Vec<RgbaImage> {
        image::codecs::webp::WebPDecoder::new(Cursor::new(b))
            .unwrap()
            .into_frames()
            .map(|f| f.unwrap().into_buffer())
            .collect()
    };
    assert_eq!(frames(&ing.original), frames(&src));
    let first = decode_frame(&ing.original, &Limits::default()).unwrap();
    assert_eq!(first, frames(&src)[0]);
}

#[test]
fn animation_variants() {
    let e = env();
    let dir = tempfile::tempdir().unwrap();

    // Animated WebP: MP4, Discord GIF and Telegram video sticker.
    let ing = ingest(&animated_webp(96, 64, 30), &opts(), &e.tools).unwrap();
    let path = dir.path().join("anim.webp");
    std::fs::write(&path, &ing.original).unwrap();
    let v = generate_variants(
        &path,
        &ing.probe,
        &VariantOptions {
            discord_gif_max_bytes: 10 << 20,
        },
        &e.tools,
    )
    .unwrap();
    let kinds: Vec<Variant> = v.iter().map(|r| r.variant).collect();
    assert_eq!(kinds, vec![Variant::TgVideoSticker, Variant::DiscordGif, Variant::Mp4]);

    let tg = &v[0];
    assert_eq!(tg.mime, "video/webm");
    assert_eq!((tg.width, tg.height), (512, 340));
    assert!(tg.bytes.len() <= 256 * 1024);
    let p = ffprobe_bytes(&tg.bytes, "webm");
    assert_eq!(p.streams.len(), 1, "no audio");
    assert_eq!(p.streams[0]["codec_name"], "vp9");
    assert_eq!(p.streams[0]["width"], 512);
    assert_eq!(p.streams[0]["tags"]["alpha_mode"], "1", "{:?}", p.streams[0]);
    let dur: f64 = p.format["duration"].as_str().unwrap().parse().unwrap();
    assert!(dur <= 3.0 && dur > 2.0, "{dur}");

    let gif = &v[1];
    assert_eq!(gif.mime, "image/gif");
    assert_eq!(sniff(&gif.bytes), Some(MediaFormat::Gif));
    let frames = gif_frames(&gif.bytes);
    assert!(frames.len() >= 20, "{}", frames.len());
    assert_eq!(frames[0].0.get_pixel(0, 0)[3], 0, "transparency kept");

    let mp4 = &v[2];
    assert_eq!(mp4.mime, "video/mp4");
    assert_eq!((mp4.width, mp4.height), (96, 64));
    let p = ffprobe_bytes(&mp4.bytes, "mp4");
    assert_eq!(p.streams.len(), 1);
    assert_eq!(p.streams[0]["codec_name"], "h264");
    assert_eq!(p.streams[0]["pix_fmt"], "yuv420p");
    // faststart: moov before mdat.
    let moov = mp4.bytes.windows(4).position(|w| w == b"moov").unwrap();
    let mdat = mp4.bytes.windows(4).position(|w| w == b"mdat").unwrap();
    assert!(moov < mdat);

    // Small GIF original: no Discord GIF (the original already fits), odd
    // dimensions become even for H.264.
    let ing = ingest(&animated_gif(49, 33), &opts(), &e.tools).unwrap();
    let path = dir.path().join("anim.gif");
    std::fs::write(&path, &ing.original).unwrap();
    let v = generate_variants(
        &path,
        &ing.probe,
        &VariantOptions {
            discord_gif_max_bytes: 10 << 20,
        },
        &e.tools,
    )
    .unwrap();
    let kinds: Vec<Variant> = v.iter().map(|r| r.variant).collect();
    assert_eq!(kinds, vec![Variant::TgVideoSticker, Variant::Mp4]);
    assert_eq!((v[1].width, v[1].height), (48, 32));

    // Oversized GIF: shrunk until it fits; impossible limit: skipped.
    let ing = ingest(&noisy_gif(400, 300, 4), &opts(), &e.tools).unwrap();
    let path = dir.path().join("big.gif");
    std::fs::write(&path, &ing.original).unwrap();
    let limit = ing.original.len() as u64 / 4;
    let v = generate_variants(
        &path,
        &ing.probe,
        &VariantOptions {
            discord_gif_max_bytes: limit,
        },
        &e.tools,
    )
    .unwrap();
    let gif = v.iter().find(|r| r.variant == Variant::DiscordGif).expect("shrunk gif");
    assert!(gif.bytes.len() as u64 <= limit);
    assert!(gif.width < 400 && gif.height < 300, "{}x{}", gif.width, gif.height);
    assert_eq!(gif_frames(&gif.bytes)[0].0.dimensions(), (gif.width, gif.height));
    let v = generate_variants(
        &path,
        &ing.probe,
        &VariantOptions {
            discord_gif_max_bytes: 50,
        },
        &e.tools,
    )
    .unwrap();
    assert!(v.iter().all(|r| r.variant != Variant::DiscordGif));
}

// ---------------------------------------------------------------------------
// Videos
// ---------------------------------------------------------------------------

#[test]
fn mp4_is_remuxed_without_metadata() {
    let e = env();
    let dir = tempfile::tempdir().unwrap();
    let mut upload = mp4_fixture(dir.path(), 2);
    assert!(contains(&upload, b"SECRET-TITLE"));
    upload.extend_from_slice(ZIP_PAYLOAD);
    let ing = ingest(&upload, &opts(), &e.tools).unwrap();
    assert_eq!(ing.probe.format, MediaFormat::Mp4);
    assert_eq!(ing.probe.kind, MediaKind::Video);
    assert_eq!((ing.probe.width, ing.probe.height), (160, 120));
    assert!(
        (ing.probe.duration_ms.unwrap() as i64 - 2000).abs() < 150,
        "{:?}",
        ing.probe.duration_ms
    );
    assert!((ing.probe.fps.unwrap() - 15.0).abs() < 0.1);
    assert!(!ing.probe.has_alpha);
    assert_clean(&ing.original);
    let p = ffprobe_bytes(&ing.original, "mp4");
    let types: Vec<&str> = p.streams.iter().map(|s| s["codec_type"].as_str().unwrap()).collect();
    assert_eq!(types, vec!["video", "audio"], "first audio stream kept");
    assert_eq!(ing.frame.dimensions(), (160, 120));
    assert_eq!(ing.thumb.mime, "image/webp");
    // Nothing left behind in the scratch directory.
    assert_eq!(std::fs::read_dir(&e.tools.tmp_dir).unwrap().count(), 0);

    let mut o = opts();
    o.limits.max_video_ms = 1000;
    assert!(matches!(ingest(&upload, &o, &e.tools), Err(MediaError::TooLarge(_))));
    let mut o = opts();
    o.limits.max_pixels = 100 * 100;
    assert!(matches!(ingest(&upload, &o, &e.tools), Err(MediaError::TooLarge(_))));
}

#[test]
fn webm_with_alpha_is_detected() {
    let e = env();
    let dir = tempfile::tempdir().unwrap();
    let upload = webm_fixture(dir.path(), true);
    let ing = ingest(&upload, &opts(), &e.tools).unwrap();
    assert_eq!(ing.probe.format, MediaFormat::Webm);
    assert!(ing.probe.has_alpha);
    assert_clean(&ing.original);
    assert_eq!(
        ing.frame.get_pixel(5, 50)[3],
        0,
        "libvpx decode keeps alpha in the frame"
    );
    assert_eq!(ing.frame.get_pixel(100, 50)[3], 255);
}

#[test]
fn video_variants() {
    let e = env();
    let dir = tempfile::tempdir().unwrap();
    let vopts = VariantOptions {
        discord_gif_max_bytes: 10 << 20,
    };

    // H.264 MP4: already fine as MP4, so only GIF and sticker.
    let ing = ingest(&mp4_fixture(dir.path(), 2), &opts(), &e.tools).unwrap();
    let path = dir.path().join("orig.mp4");
    std::fs::write(&path, &ing.original).unwrap();
    let v = generate_variants(&path, &ing.probe, &vopts, &e.tools).unwrap();
    let kinds: Vec<Variant> = v.iter().map(|r| r.variant).collect();
    assert_eq!(kinds, vec![Variant::TgVideoSticker, Variant::DiscordGif]);
    let p = ffprobe_bytes(&v[0].bytes, "webm");
    assert_eq!(p.streams.len(), 1, "sticker has no audio");
    assert_eq!((v[0].width, v[0].height), (512, 384));

    // WebM (alpha): MP4 is added, flattened.
    let ing = ingest(&webm_fixture(dir.path(), true), &opts(), &e.tools).unwrap();
    let path = dir.path().join("orig.webm");
    std::fs::write(&path, &ing.original).unwrap();
    let v = generate_variants(&path, &ing.probe, &vopts, &e.tools).unwrap();
    let kinds: Vec<Variant> = v.iter().map(|r| r.variant).collect();
    assert_eq!(kinds, vec![Variant::TgVideoSticker, Variant::DiscordGif, Variant::Mp4]);
    let sticker = ffprobe_bytes(&v[0].bytes, "webm");
    assert_eq!(
        sticker.streams[0]["tags"]["alpha_mode"], "1",
        "alpha kept in the sticker"
    );
    let mp4 = ffprobe_bytes(&v[2].bytes, "mp4");
    assert_eq!(mp4.streams.len(), 1);
    assert_eq!(mp4.streams[0]["codec_name"], "h264");

    // Long video: no GIF, no sticker.
    let ing = ingest(&mp4_fixture(dir.path(), 11), &opts(), &e.tools).unwrap();
    let path = dir.path().join("long.mp4");
    std::fs::write(&path, &ing.original).unwrap();
    assert!(generate_variants(&path, &ing.probe, &vopts, &e.tools)
        .unwrap()
        .is_empty());
}

#[test]
fn ffmpeg_timeout_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let upload = mp4_fixture(dir.path(), 2);
    let tools = Tools::from_env(dir.path().join("tmp"), Duration::from_millis(1));
    match ingest(&upload, &opts(), &tools) {
        Err(MediaError::Ffmpeg(m)) => assert!(m.contains("timed out"), "{m}"),
        other => panic!("{other:?}"),
    }
    let missing = Tools {
        ffprobe: "/nonexistent/ffprobe".into(),
        ..Tools::from_env(dir.path(), Duration::from_secs(5))
    };
    assert!(matches!(ingest(&upload, &opts(), &missing), Err(MediaError::Ffmpeg(_))));
}

#[test]
fn decode_frame_handles_every_raster_format() {
    let img = sticker(40, 30);
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img.clone())
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    assert_eq!(decode_frame(&png, &Limits::default()).unwrap(), img);
    let lossless = webp::Encoder::from_rgba(img.as_raw(), 40, 30)
        .encode_lossless()
        .to_vec();
    // libwebp may rewrite the colour of fully transparent pixels.
    let visible = |i: RgbaImage| -> Vec<[u8; 4]> { i.pixels().map(|p| if p[3] == 0 { [0; 4] } else { p.0 }).collect() };
    assert_eq!(
        visible(decode_frame(&lossless, &Limits::default()).unwrap()),
        visible(img.clone())
    );
    assert_eq!(
        decode_frame(&jpeg(&img, 90), &Limits::default()).unwrap().dimensions(),
        (40, 30)
    );
    assert_eq!(
        decode_frame(&animated_gif(10, 10), &Limits::default())
            .unwrap()
            .dimensions(),
        (10, 10)
    );
    assert!(matches!(
        decode_frame(b"\0\0\0\x14ftypisom\0\0\0\0isom", &Limits::default()),
        Err(MediaError::Unsupported)
    ));
    let tight = Limits {
        max_pixels: 100,
        ..Limits::default()
    };
    assert!(matches!(decode_frame(&png, &tight), Err(MediaError::TooLarge(_))));
}
