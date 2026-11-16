//! Square sticker renditions for the sticker packs of Discord and Signal
//! (docs/discord.md and docs/signal.md, "Renditions"): one call makes a
//! sticker of a given side and byte budget from a stored original.
//!
//! * Stills, and animations whose frames all look the same: the image
//!   scaled so its longer side is the target side and centred on a
//!   transparent square, as PNG (lossless, else 256 colours) or WebP
//!   (lossless, else lossy at the highest quality that fits).
//! * Animations (decoded here) and videos (decoded by ffmpeg): frames at a
//!   constant rate of at most 20 per second, at most `max_animation_ms`
//!   long (longer ones are cut), looping forever, as APNG (one palette of
//!   256 colours with alpha for every frame; frames after the first cover
//!   only the area that changed) or GIF (a palette per frame, transparency
//!   on or off per pixel). Too large: fewer frames per second, then a
//!   smaller square (down to a quarter of the side), until it fits.
//!
//! The platforms' rules are not repeated here: the caller passes them as a
//! [`PackTarget`] (the server takes them from stickpile-discord and
//! stickpile-signal) and checks the result with those crates.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use image::imageops::FilterType;
use image::RgbaImage;
use stickpile_api::MediaKind;

use crate::tools::{self, file_arg};
use crate::variants::{encode_png, fit_exact, squeeze_webp, trusted_limits, webp_lossless};
use crate::video::{input_args, needs_libvpx};
use crate::whatsapp::{first_frame, pad_square};
use crate::{raster, MediaError, Probe, Tools};

/// How still stickers are encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StillFormat {
    /// Lossless RGBA PNG, else a PNG of 256 colours.
    Png,
    /// Lossless WebP, else lossy WebP.
    Webp,
}

/// How animated stickers are encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationFormat {
    Apng,
    Gif,
}

/// What a platform takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackTarget {
    /// Width and height of a sticker; animations may shrink to a quarter
    /// of it to fit `max_bytes`.
    pub side: u32,
    pub max_bytes: u64,
    /// Longest animation; longer ones are cut.
    pub max_animation_ms: u64,
    pub still: StillFormat,
    pub animation: AnimationFormat,
}

/// A sticker that fits a [`PackTarget`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackSticker {
    pub bytes: Vec<u8>,
    pub animated: bool,
    /// Width and height.
    pub side: u32,
    /// Frames of an animation (1 for a still).
    pub frames: u32,
    /// Length of an animation (0 for a still).
    pub duration_ms: u64,
    /// The source was longer than `max_animation_ms`: only its beginning
    /// is in the sticker.
    pub cut: bool,
}

/// Highest frame rate of animated stickers.
pub const MAX_FPS: u32 = 20;

/// (side in sixteenths of the target side, frame rate cap), tried in order
/// until an animation fits.
const LADDER: [(u32, u32); 11] = [
    (16, 20),
    (16, 15),
    (16, 12),
    (14, 12),
    (12, 10),
    (10, 10),
    (8, 8),
    (7, 8),
    (6, 6),
    (5, 5),
    (4, 5),
];

/// A rung predicted (from the last attempt) to be this many times over the
/// budget is not tried; the last one always is.
const SKIP_FACTOR: f64 = 1.15;

/// Colours a NeuQuant palette is trained on, at most.
const TRAINING_PIXELS: usize = 1 << 20;

/// A sticker for `target` from a stored original (`probe` describes it).
/// `None`: it does not fit `max_bytes`, even at the smallest size and
/// lowest frame rate. Videos need ffmpeg.
pub fn sticker(
    original: &Path,
    probe: &Probe,
    tools: &Tools,
    target: &PackTarget,
) -> Result<Option<PackSticker>, MediaError> {
    let clip = match probe.kind {
        MediaKind::Still => {
            let frame = first_frame(original, probe, tools)?;
            return still_sticker(&frame, target);
        }
        MediaKind::Animation => clip_from_animation(original, probe, target)?,
        MediaKind::Video => clip_from_video(original, probe, tools, target)?,
    };
    encode_clip(&clip, target)
}

/// A still sticker for `target` from a decoded frame (a cover image, the
/// first frame of an animation). `None`: it does not fit.
pub fn still(frame: &RgbaImage, target: &PackTarget) -> Result<Option<Vec<u8>>, MediaError> {
    let img = pad_square(frame, target.side);
    let fits = |b: &Vec<u8>| b.len() as u64 <= target.max_bytes;
    match target.still {
        StillFormat::Webp => {
            let lossless = webp_lossless(&img)?;
            if fits(&lossless) {
                return Ok(Some(lossless));
            }
            squeeze_webp(&img, target.max_bytes)
        }
        StillFormat::Png => {
            let full = encode_png(&img)?;
            if fits(&full) {
                return Ok(Some(full));
            }
            let q = Quantizer::new(std::slice::from_ref(&img));
            let indexed = q.index(&img);
            let one = encode_apng(&q, &[(&indexed, 0)], img.width())?;
            Ok(Some(one).filter(fits))
        }
    }
}

fn still_sticker(frame: &RgbaImage, target: &PackTarget) -> Result<Option<PackSticker>, MediaError> {
    Ok(still(frame, target)?.map(|bytes| PackSticker {
        bytes,
        animated: false,
        side: target.side,
        frames: 1,
        duration_ms: 0,
        cut: false,
    }))
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// An animation as squares of the target side at a constant rate.
struct Clip {
    /// The distinct pictures.
    images: Vec<RgbaImage>,
    /// One entry per frame at `step` ms: an index into `images`.
    timeline: Vec<usize>,
    step: f64,
    cut: bool,
}

impl Clip {
    fn duration(&self) -> f64 {
        self.timeline.len() as f64 * self.step
    }
}

/// Frames at `fps` for a source of `total_ms`: how many (at most
/// `max_ms` long in total) and the step.
fn frame_plan(fps: f64, total_ms: f64, max_ms: u64) -> (usize, f64) {
    let step = 1000.0 / fps;
    let max = (max_ms as f64 / step + 1e-9).floor() as usize;
    let wanted = (total_ms / step - 1e-9).ceil() as usize;
    (wanted.clamp(1, max.max(1)), step)
}

/// Duration of frame `k` at `step` ms: whole milliseconds that add up to
/// the frame timestamps.
fn frame_ms(k: usize, step: f64) -> u32 {
    let at = |i: usize| (i as f64 * step).round() as u64;
    (at(k + 1) - at(k)).max(1) as u32
}

fn clip_from_animation(original: &Path, probe: &Probe, target: &PackTarget) -> Result<Clip, MediaError> {
    let bytes = std::fs::read(original)?;
    let limits = trusted_limits(probe);
    let fmt = probe.format;
    let delays = raster::frame_delays(&bytes, fmt)?;
    let total: u64 = delays.iter().map(|d| u64::from(*d)).sum();
    let shortest = delays.iter().copied().min().unwrap_or(100).max(1);
    let fps = (1000.0 / f64::from(shortest)).min(f64::from(MAX_FPS));
    let (count, step) = frame_plan(fps, total as f64, target.max_animation_ms);
    let mut clip = Clip {
        images: Vec::new(),
        timeline: Vec::with_capacity(count),
        step,
        cut: total > target.max_animation_ms,
    };
    let mut src_end = 0f64;
    let mut last: Option<(RgbaImage, bool)> = None;
    raster::for_each_frame(&bytes, fmt, &limits, |f| {
        src_end += f64::from(f.delay_ms);
        let mut added = false;
        while clip.timeline.len() < count && (clip.timeline.len() as f64) * step < src_end {
            if !added {
                clip.images.push(pad_square(&f.image, target.side));
                added = true;
            }
            clip.timeline.push(clip.images.len() - 1);
        }
        last = Some((f.image, added));
        Ok(clip.timeline.len() < count)
    })?;
    // Rounding can leave the tail a frame short: it shows the last frame.
    if clip.timeline.len() < count {
        match last {
            Some((img, false)) => clip.images.push(pad_square(&img, target.side)),
            Some((_, true)) => {}
            None => return Err(MediaError::Invalid("animation has no frames".into())),
        }
        while clip.timeline.len() < count {
            clip.timeline.push(clip.images.len() - 1);
        }
    }
    if clip.images.is_empty() {
        return Err(MediaError::Invalid("animation has no frames".into()));
    }
    Ok(clip)
}

fn clip_from_video(original: &Path, probe: &Probe, tools: &Tools, target: &PackTarget) -> Result<Clip, MediaError> {
    let dir = tools::scratch(tools)?;
    let input = dir.path().join(format!("input.{}", probe.format.extension()));
    std::fs::copy(original, &input)?;
    let info = tools::ffprobe(tools, dir.path(), probe.format, &input)?;
    let v = info
        .video()
        .ok_or_else(|| MediaError::Invalid("no video stream".into()))?;
    let libvpx = needs_libvpx(probe.format, &info);
    let (w, h) = (v.width.unwrap_or(probe.width), v.height.unwrap_or(probe.height));
    let (sw, sh) = fit_exact(w.max(1), h.max(1), target.side);
    let duration_ms = probe.duration_ms.or_else(|| info.duration_ms()).unwrap_or(0);
    let src_fps = probe.fps.or_else(|| v.fps()).map(f64::from).filter(|f| *f > 0.0);
    let fps = src_fps.map_or(f64::from(MAX_FPS), |f| f.min(f64::from(MAX_FPS)));
    let raw = dir.path().join("frames.rgba");
    let limit = format!("{:.3}", target.max_animation_ms as f64 / 1000.0);
    let vf = format!("fps={fps:.4},scale={sw}:{sh}:flags=lanczos,format=rgba");
    let mut a = input_args(probe.format, &input, libvpx);
    for s in [
        "-map",
        "0:V:0",
        "-t",
        &limit,
        "-an",
        "-sn",
        "-dn",
        "-vf",
        &vf,
        "-pix_fmt",
        "rgba",
        "-map_metadata",
        "-1",
        "-fflags",
        "+bitexact",
        "-f",
        "rawvideo",
        "-y",
    ] {
        a.push(s.into());
    }
    a.push(file_arg(&raw));
    tools::ffmpeg(tools, dir.path(), a, None)?;

    let frame_bytes = u64::from(sw) * u64::from(sh) * 4;
    let available = std::fs::metadata(&raw)?.len() / frame_bytes;
    if available == 0 {
        return Err(MediaError::Invalid("video has no decodable frame".into()));
    }
    let (count, step) = frame_plan(fps, available as f64 * 1000.0 / fps, target.max_animation_ms);
    let count = count.min(available as usize);
    let mut file = std::io::BufReader::new(std::fs::File::open(&raw)?);
    file.seek(SeekFrom::Start(0))?;
    let mut clip = Clip {
        images: Vec::with_capacity(count),
        timeline: Vec::with_capacity(count),
        step,
        cut: duration_ms > target.max_animation_ms,
    };
    let mut buf = vec![0u8; frame_bytes as usize];
    for k in 0..count {
        file.read_exact(&mut buf)?;
        let img =
            RgbaImage::from_raw(sw, sh, buf.clone()).ok_or_else(|| MediaError::Invalid("bad raw frame".into()))?;
        clip.images.push(pad_square(&img, target.side));
        clip.timeline.push(k);
    }
    Ok(clip)
}

// ---------------------------------------------------------------------------
// The size ladder
// ---------------------------------------------------------------------------

/// The pictures of a clip at one side, ready to be written.
enum Prepared {
    /// One palette for all; an index buffer per picture.
    Apng { q: Box<Quantizer>, indexed: Vec<Vec<u8>> },
    /// A GIF frame per picture.
    Gif(Vec<GifPicture>),
}

fn prepare(clip: &Clip, side: u32, format: AnimationFormat) -> Prepared {
    let scaled: Vec<RgbaImage> = if side == clip.images[0].width() {
        clip.images.clone()
    } else {
        clip.images
            .iter()
            // A smaller rung of the ladder: a cheaper filter does for
            // shrinking by less than half.
            .map(|img| image::imageops::resize(img, side, side, FilterType::Triangle))
            .collect()
    };
    match format {
        AnimationFormat::Apng => {
            let q = Box::new(Quantizer::new(&scaled));
            let indexed = scaled.iter().map(|img| q.index(img)).collect();
            Prepared::Apng { q, indexed }
        }
        AnimationFormat::Gif => Prepared::Gif(scaled.iter().map(GifPicture::new).collect()),
    }
}

impl Prepared {
    fn same(&self, a: usize, b: usize) -> bool {
        a == b
            || match self {
                Prepared::Apng { indexed, .. } => indexed[a] == indexed[b],
                Prepared::Gif(p) => p[a].indexed == p[b].indexed && p[a].palette == p[b].palette,
            }
    }
}

fn encode_clip(clip: &Clip, target: &PackTarget) -> Result<Option<PackSticker>, MediaError> {
    let master_fps = 1000.0 / clip.step;
    let total = clip.duration();
    // The pictures at the side of the current rung (the ladder only goes
    // down, so earlier sides are dropped).
    let mut prepared: Option<(u32, Prepared)> = None;
    let mut tried: Vec<(u32, usize)> = Vec::new();
    // The last attempt that was too large: (bytes, side, frames).
    let mut measured: Option<(u64, u32, usize)> = None;
    let min_side = (target.side / 4).max(1);
    for (rung, (sixteenths, cap)) in LADDER.into_iter().enumerate() {
        let last = rung == LADDER.len() - 1;
        let side = (target.side * sixteenths / 16).max(min_side);
        let fps = master_fps.min(f64::from(cap));
        let (count, step) = frame_plan(fps, total, target.max_animation_ms);
        if tried.contains(&(side, count)) {
            continue;
        }
        if let Some((size, side0, count0)) = measured {
            // The size goes with the number of frames and the area.
            let scale = f64::from(side) / f64::from(side0);
            let predicted = size as f64 * (count as f64 / count0 as f64) * scale * scale;
            if !last && predicted > SKIP_FACTOR * target.max_bytes as f64 {
                continue;
            }
        }
        tried.push((side, count));
        if prepared.as_ref().is_none_or(|(s, _)| *s != side) {
            prepared = Some((side, prepare(clip, side, target.animation)));
        }
        let pics = &prepared.as_ref().expect("prepared above").1;
        // Output frame k shows the clip's picture at its start time; equal
        // neighbours become one longer frame.
        let mut frames: Vec<(usize, u32)> = Vec::with_capacity(count);
        for k in 0..count {
            let at = ((k as f64 * step) / clip.step + 1e-9).floor() as usize;
            let pic = clip.timeline[at.min(clip.timeline.len() - 1)];
            let ms = frame_ms(k, step);
            match frames.last_mut() {
                Some((prev, d)) if pics.same(*prev, pic) => *d += ms,
                _ => frames.push((pic, ms)),
            }
        }
        if frames.len() <= 1 {
            // Every frame looks the same: a still sticker.
            let pic = frames.first().map_or(clip.timeline[0], |f| f.0);
            return still_sticker(&clip.images[pic], target);
        }
        let duration_ms: u64 = frames.iter().map(|(_, d)| u64::from(*d)).sum();
        let bytes = match pics {
            Prepared::Apng { q, indexed } => {
                let list: Vec<(&Vec<u8>, u32)> = frames.iter().map(|(p, d)| (&indexed[*p], *d)).collect();
                encode_apng(q, &list, side)?
            }
            Prepared::Gif(p) => {
                let list: Vec<(&GifPicture, u32)> = frames.iter().map(|(i, d)| (&p[*i], *d)).collect();
                encode_gif(&list, side)?
            }
        };
        if bytes.len() as u64 <= target.max_bytes {
            return Ok(Some(PackSticker {
                bytes,
                animated: true,
                side,
                frames: frames.len() as u32,
                duration_ms,
                cut: clip.cut,
            }));
        }
        measured = Some((bytes.len() as u64, side, count));
    }
    tracing::warn!(
        "animated sticker does not fit {} bytes at any size or frame rate",
        target.max_bytes
    );
    Ok(None)
}

// ---------------------------------------------------------------------------
// Palettes
// ---------------------------------------------------------------------------

/// Maps RGBA pixels to at most 256 palette entries; fully transparent
/// pixels all go to one transparent entry.
struct Quantizer {
    /// RGBA entries.
    palette: Vec<[u8; 4]>,
    transparent: Option<u8>,
    exact: Option<HashMap<[u8; 4], u8>>,
    neu: Option<color_quant::NeuQuant>,
}

impl Quantizer {
    /// A palette for every pixel of `images`: their own colours when there
    /// are few enough, else NeuQuant's.
    fn new(images: &[RgbaImage]) -> Quantizer {
        let has_transparent = images.iter().any(|img| img.pixels().any(|p| p[3] == 0));
        let room = if has_transparent { 255 } else { 256 };
        let mut colours: HashMap<[u8; 4], u8> = HashMap::new();
        let mut exact = true;
        'scan: for img in images {
            for p in img.pixels() {
                if p[3] == 0 {
                    continue;
                }
                if !colours.contains_key(&p.0) {
                    if colours.len() == room {
                        exact = false;
                        break 'scan;
                    }
                    let n = colours.len() as u8;
                    colours.insert(p.0, n);
                }
            }
        }
        if exact {
            let mut palette = vec![[0u8; 4]; colours.len()];
            for (c, i) in &colours {
                palette[usize::from(*i)] = *c;
            }
            let transparent = has_transparent.then(|| {
                palette.push([0, 0, 0, 0]);
                (palette.len() - 1) as u8
            });
            return Quantizer {
                palette,
                transparent,
                exact: Some(colours),
                neu: None,
            };
        }
        // Train on the visible pixels, evenly thinned out.
        let pixels: usize = images.iter().map(|img| img.as_raw().len() / 4).sum();
        let every = pixels.div_ceil(TRAINING_PIXELS).max(1);
        let mut sample = Vec::with_capacity((pixels / every + 1) * 4);
        for img in images {
            for p in img.as_raw().chunks_exact(4).step_by(every) {
                if p[3] > 0 {
                    sample.extend_from_slice(p);
                }
            }
        }
        let nq = color_quant::NeuQuant::new(10, room, &sample);
        let mut palette: Vec<[u8; 4]> = nq
            .color_map_rgba()
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        let transparent = has_transparent.then(|| {
            palette.push([0, 0, 0, 0]);
            (palette.len() - 1) as u8
        });
        Quantizer {
            palette,
            transparent,
            exact: None,
            neu: Some(nq),
        }
    }

    fn index_of(&self, p: &[u8; 4]) -> u8 {
        if p[3] == 0 {
            if let Some(t) = self.transparent {
                return t;
            }
        }
        if let Some(map) = &self.exact {
            return map.get(p).copied().unwrap_or(0);
        }
        self.neu.as_ref().map_or(0, |nq| nq.index_of(p) as u8)
    }

    fn index(&self, img: &RgbaImage) -> Vec<u8> {
        // Neighbours are often the same colour: look each run up once.
        let mut out = Vec::with_capacity(img.as_raw().len() / 4);
        let mut last: Option<([u8; 4], u8)> = None;
        for p in img.pixels() {
            let i = match last {
                Some((c, i)) if c == p.0 => i,
                _ => {
                    let i = self.index_of(&p.0);
                    last = Some((p.0, i));
                    i
                }
            };
            out.push(i);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// APNG
// ---------------------------------------------------------------------------

/// The smallest rectangle (x, y, w, h) where `a` and `b` (square, `side`
/// wide) differ, or `None` when they are equal.
fn changed_area(a: &[u8], b: &[u8], side: u32) -> Option<(u32, u32, u32, u32)> {
    let side_us = side as usize;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (y, (ra, rb)) in a.chunks_exact(side_us).zip(b.chunks_exact(side_us)).enumerate() {
        if ra == rb {
            continue;
        }
        let first = ra.iter().zip(rb).position(|(p, q)| p != q).unwrap_or(0);
        let last = side_us
            - 1
            - ra.iter()
                .rev()
                .zip(rb.iter().rev())
                .position(|(p, q)| p != q)
                .unwrap_or(0);
        x0 = x0.min(first);
        x1 = x1.max(last);
        y0 = y0.min(y);
        y1 = y;
    }
    (x0 != usize::MAX).then(|| (x0 as u32, y0 as u32, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32))
}

fn png_err(e: png::EncodingError) -> MediaError {
    MediaError::Invalid(format!("png encode: {e}"))
}

/// An 8-bit palette PNG of `side` x `side`: one frame (`frames[0]`, the
/// duration ignored) as a plain PNG, several as an APNG looping forever.
/// Frames after the first are written as the rectangle that changed,
/// replacing what was there (blend "source", dispose "none").
fn encode_apng(q: &Quantizer, frames: &[(&Vec<u8>, u32)], side: u32) -> Result<Vec<u8>, MediaError> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, side, side);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        let rgb: Vec<u8> = q.palette.iter().flat_map(|c| [c[0], c[1], c[2]]).collect();
        let mut alpha: Vec<u8> = q.palette.iter().map(|c| c[3]).collect();
        while alpha.last() == Some(&255) {
            alpha.pop();
        }
        enc.set_palette(rgb);
        if !alpha.is_empty() {
            enc.set_trns(alpha);
        }
        let animated = frames.len() > 1;
        if animated {
            enc.set_animated(frames.len() as u32, 0).map_err(png_err)?;
        }
        let mut w = enc.write_header().map_err(png_err)?;
        let mut prev: Option<&Vec<u8>> = None;
        for (data, ms) in frames {
            if animated {
                w.set_frame_delay(u16::try_from(*ms).unwrap_or(u16::MAX), 1000)
                    .map_err(png_err)?;
                w.set_blend_op(png::BlendOp::Source).map_err(png_err)?;
                w.set_dispose_op(png::DisposeOp::None).map_err(png_err)?;
            }
            match prev.and_then(|p| changed_area(p, data, side)) {
                Some((x, y, cw, ch)) if (cw, ch) != (side, side) => {
                    let mut crop = Vec::with_capacity((cw * ch) as usize);
                    for row in data.chunks_exact(side as usize).skip(y as usize).take(ch as usize) {
                        crop.extend_from_slice(&row[x as usize..(x + cw) as usize]);
                    }
                    w.set_frame_position(0, 0).map_err(png_err)?;
                    w.set_frame_dimension(cw, ch).map_err(png_err)?;
                    w.set_frame_position(x, y).map_err(png_err)?;
                    w.write_image_data(&crop).map_err(png_err)?;
                }
                _ => {
                    if animated {
                        w.set_frame_position(0, 0).map_err(png_err)?;
                        w.set_frame_dimension(side, side).map_err(png_err)?;
                    }
                    w.write_image_data(data).map_err(png_err)?;
                }
            }
            prev = Some(data);
        }
        w.finish().map_err(png_err)?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// GIF
// ---------------------------------------------------------------------------

/// One GIF frame: its own palette (256 entries, the last transparent when
/// any pixel is) and index buffer.
struct GifPicture {
    palette: Vec<u8>,
    indexed: Vec<u8>,
    transparent: Option<u8>,
}

impl GifPicture {
    /// GIF transparency is on or off: pixels at least half opaque are
    /// drawn, the others are transparent.
    fn new(img: &RgbaImage) -> GifPicture {
        let mut solid = img.clone();
        for p in solid.pixels_mut() {
            p[3] = if p[3] >= 128 { 255 } else { 0 };
            if p[3] == 0 {
                p.0 = [0, 0, 0, 0];
            }
        }
        let q = Quantizer::new(std::slice::from_ref(&solid));
        let mut palette: Vec<u8> = q.palette.iter().flat_map(|c| [c[0], c[1], c[2]]).collect();
        palette.resize(256 * 3, 0);
        GifPicture {
            indexed: q.index(&solid),
            palette,
            transparent: q.transparent,
        }
    }
}

/// A GIF looping forever; every frame covers the whole canvas and is
/// cleared before the next one, so transparency stays right.
fn encode_gif(frames: &[(&GifPicture, u32)], side: u32) -> Result<Vec<u8>, MediaError> {
    let side16 = u16::try_from(side).map_err(|_| MediaError::Invalid("gif: too large".into()))?;
    let gif_err = |e: gif::EncodingError| MediaError::Invalid(format!("gif encode: {e}"));
    let mut out = Vec::new();
    {
        let mut enc = gif::Encoder::new(&mut out, side16, side16, &[]).map_err(gif_err)?;
        enc.set_repeat(gif::Repeat::Infinite).map_err(gif_err)?;
        // Delays are in centiseconds: keep the rounding error from adding up.
        let mut at_ms = 0u64;
        for (pic, ms) in frames {
            let start = at_ms.div_ceil(10);
            at_ms += u64::from(*ms);
            let delay = (at_ms.div_ceil(10) - start).clamp(2, u64::from(u16::MAX)) as u16;
            let mut f = gif::Frame::from_palette_pixels(
                side16,
                side16,
                pic.indexed.clone(),
                pic.palette.clone(),
                pic.transparent,
            );
            f.delay = delay;
            f.dispose = gif::DisposalMethod::Background;
            enc.write_frame(&f).map_err(gif_err)?;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(still: StillFormat, animation: AnimationFormat) -> PackTarget {
        PackTarget {
            side: 64,
            max_bytes: 64 * 1024,
            max_animation_ms: 1000,
            still,
            animation,
        }
    }

    fn noise(w: u32, h: u32, seed: u32) -> RgbaImage {
        let mut s = seed | 1;
        RgbaImage::from_fn(w, h, |_, _| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let b = s.to_le_bytes();
            image::Rgba([b[0], b[1], b[2], 255])
        })
    }

    /// A disc that moves `k` pixels to the right, on transparency.
    fn disc(side: u32, k: u32) -> RgbaImage {
        RgbaImage::from_fn(side, side, |x, y| {
            let (dx, dy) = (x as i64 - (side / 3 + k) as i64, y as i64 - (side / 2) as i64);
            if dx * dx + dy * dy < i64::from(side * side / 16) {
                image::Rgba([200, (x * 3) as u8, 40, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            }
        })
    }

    fn clip(images: Vec<RgbaImage>, step: f64) -> Clip {
        let timeline = (0..images.len()).collect();
        Clip {
            images,
            timeline,
            step,
            cut: false,
        }
    }

    #[test]
    fn plans_frames_within_the_limit() {
        let (n, step) = frame_plan(20.0, 300.0, 3000);
        assert_eq!((n, step), (6, 50.0));
        let (n, step) = frame_plan(15.0, 10_000.0, 3000);
        assert_eq!(n, 45);
        let total: u64 = (0..n).map(|k| u64::from(frame_ms(k, step))).sum();
        assert_eq!(total, 3000);
        let (n, step) = frame_plan(7.0, 10_000.0, 3000);
        let total: u64 = (0..n).map(|k| u64::from(frame_ms(k, step))).sum();
        assert!(total <= 3000, "{total}");
        assert_eq!(frame_plan(5.0, 10.0, 3000).0, 1);
    }

    #[test]
    fn finds_the_changed_rectangle() {
        let a = vec![0u8; 16];
        let mut b = a.clone();
        assert_eq!(changed_area(&a, &b, 4), None);
        b[4 + 1] = 1; // (1, 1)
        b[2 * 4 + 2] = 1; // (2, 2)
        assert_eq!(changed_area(&a, &b, 4), Some((1, 1, 2, 2)));
    }

    #[test]
    fn exact_palettes_keep_colours_and_transparency() {
        let img = disc(32, 0);
        let q = Quantizer::new(std::slice::from_ref(&img));
        assert!(q.exact.is_some());
        let t = q.transparent.unwrap();
        assert_eq!(q.palette[usize::from(t)], [0, 0, 0, 0]);
        let idx = q.index(&img);
        for (p, i) in img.pixels().zip(&idx) {
            if p[3] == 0 {
                assert_eq!(*i, t);
            } else {
                assert_eq!(q.palette[usize::from(*i)], p.0);
            }
        }
        // Many colours: NeuQuant, still at most 256 entries.
        let q = Quantizer::new(&[noise(64, 64, 3), disc(64, 0)]);
        assert!(q.exact.is_none());
        assert!(q.palette.len() <= 256);
        assert_eq!(q.palette[usize::from(q.transparent.unwrap())][3], 0);
    }

    #[test]
    fn apng_frames_loop_and_keep_their_timing() {
        let t = target(StillFormat::Png, AnimationFormat::Apng);
        let c = clip((0..6).map(|k| disc(64, k * 2)).collect(), 50.0);
        let s = encode_clip(&c, &t).unwrap().unwrap();
        assert!(s.animated);
        assert_eq!((s.side, s.frames, s.duration_ms), (64, 6, 300));
        let dec = image::codecs::png::PngDecoder::new(std::io::Cursor::new(&s.bytes)).unwrap();
        let apng = dec.apng().unwrap();
        let frames: Vec<image::Frame> = image::AnimationDecoder::into_frames(apng)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(frames.len(), 6);
        // The composited frames are the clip's pictures.
        for (k, f) in frames.iter().enumerate() {
            let want = disc(64, k as u32 * 2);
            assert_eq!(f.buffer().get_pixel(10, 10)[3], 0);
            assert_eq!(
                f.buffer().get_pixel(32 + k as u32 * 2, 32),
                want.get_pixel(32 + k as u32 * 2, 32)
            );
            assert_eq!(f.delay().numer_denom_ms(), (50, 1));
        }
        assert!(s.bytes.windows(4).any(|w| w == b"acTL"));
    }

    #[test]
    fn equal_frames_merge_and_one_picture_is_a_still() {
        let t = target(StillFormat::Png, AnimationFormat::Apng);
        let mut c = clip(vec![disc(64, 0), disc(64, 0), disc(64, 6)], 100.0);
        let s = encode_clip(&c, &t).unwrap().unwrap();
        assert_eq!((s.frames, s.duration_ms), (2, 300));
        c.images = vec![disc(64, 0), disc(64, 0)];
        c.timeline = vec![0, 1];
        let s = encode_clip(&c, &t).unwrap().unwrap();
        assert!(!s.animated);
        assert!(s.bytes.starts_with(b"\x89PNG"));
        assert!(!s.bytes.windows(4).any(|w| w == b"acTL"));
    }

    #[test]
    fn gif_frames_loop_with_transparency() {
        let t = target(StillFormat::Png, AnimationFormat::Gif);
        let c = clip((0..4).map(|k| disc(64, k * 3)).collect(), 100.0);
        let s = encode_clip(&c, &t).unwrap().unwrap();
        assert!(s.bytes.starts_with(b"GIF89a"));
        assert!(s.bytes.windows(11).any(|w| w == b"NETSCAPE2.0"));
        let dec = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&s.bytes)).unwrap();
        let frames: Vec<image::Frame> = image::AnimationDecoder::into_frames(dec)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(frames.len(), 4);
        assert_eq!(frames[0].buffer().get_pixel(2, 2)[3], 0);
        assert_eq!(frames[3].delay().numer_denom_ms(), (100, 1));
    }

    #[test]
    fn large_animations_shrink_until_they_fit() {
        let mut t = target(StillFormat::Png, AnimationFormat::Apng);
        t.side = 128;
        t.max_bytes = 40 * 1024;
        let c = clip((0..10).map(|k| noise(128, 128, k + 1)).collect(), 50.0);
        let s = encode_clip(&c, &t).unwrap().unwrap();
        assert!(s.bytes.len() as u64 <= t.max_bytes, "{}", s.bytes.len());
        assert!(s.side < 128 || s.frames < 10, "{s:?}");
        // Nothing fits a tiny budget.
        t.max_bytes = 2 * 1024;
        assert!(encode_clip(&c, &t).unwrap().is_none());
    }

    #[test]
    fn stills_are_padded_squares_within_the_budget() {
        let wide = noise(200, 100, 9);
        let t = PackTarget {
            side: 128,
            max_bytes: 30 * 1024,
            ..target(StillFormat::Webp, AnimationFormat::Apng)
        };
        let webp = still(&wide, &t).unwrap().unwrap();
        assert!(webp.len() as u64 <= t.max_bytes);
        let img = image::load_from_memory(&webp).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (128, 128));
        assert_eq!(img.get_pixel(64, 5)[3], 0, "padded above");
        let t = PackTarget {
            still: StillFormat::Png,
            ..t
        };
        // Noise does not fit as RGBA PNG: 256 colours it is.
        let png = still(&wide, &t).unwrap().unwrap();
        assert!(png.len() as u64 <= t.max_bytes, "{}", png.len());
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (128, 128));
        assert_eq!(img.get_pixel(64, 5)[3], 0);
        assert!(still(&wide, &PackTarget { max_bytes: 100, ..t }).unwrap().is_none());
    }
}
