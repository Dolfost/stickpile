//! Stills and animations: decode under limits, sanitise, iterate frames.

use std::io::Cursor;
use std::num::NonZeroU64;

use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, RgbaImage};
use stickpile_api::{MediaFormat, MediaKind};

use crate::{jpeg, webp, Limits, MediaError, Probe};

/// Total decoded pixels (frames x frame area) allowed for one animation, as
/// a multiple of `max_pixels`: bounds CPU time for LZW/VP8 bombs.
pub(crate) const FRAME_PIXEL_BUDGET: u64 = 100;

/// Browsers show frames with a delay of <= 10 ms for 100 ms; do the same.
pub(crate) fn normalise_delay(ms: u32) -> u32 {
    if ms <= 10 {
        100
    } else {
        ms
    }
}

pub(crate) fn invalid(fmt: MediaFormat, e: impl std::fmt::Display) -> MediaError {
    MediaError::Invalid(format!("{}: {e}", fmt.extension()))
}

pub(crate) fn check_pixels(w: u32, h: u32, limits: &Limits) -> Result<(), MediaError> {
    if w == 0 || h == 0 {
        return Err(MediaError::Invalid("zero-sized image".into()));
    }
    let px = u64::from(w) * u64::from(h);
    if px > limits.max_pixels {
        return Err(MediaError::TooLarge(format!(
            "{w}x{h} exceeds {} pixels",
            limits.max_pixels
        )));
    }
    Ok(())
}

fn image_limits(limits: &Limits) -> image::Limits {
    let mut l = image::Limits::default();
    let side = u32::try_from(limits.max_pixels).unwrap_or(u32::MAX);
    l.max_image_width = Some(side);
    l.max_image_height = Some(side);
    // RGBA16 output plus decoder working memory.
    l.max_alloc = Some(limits.max_pixels.saturating_mul(8).saturating_add(64 << 20));
    l
}

fn image_format(fmt: MediaFormat) -> Option<ImageFormat> {
    match fmt {
        MediaFormat::Png => Some(ImageFormat::Png),
        MediaFormat::Jpeg => Some(ImageFormat::Jpeg),
        MediaFormat::Gif => Some(ImageFormat::Gif),
        MediaFormat::Webp => Some(ImageFormat::WebP),
        MediaFormat::Mp4 | MediaFormat::Webm => None,
    }
}

/// Decode a still (first frame of animations) with the dimensions checked
/// from the header before any pixel buffer is allocated.
pub(crate) fn decode_still(bytes: &[u8], fmt: MediaFormat, limits: &Limits) -> Result<DynamicImage, MediaError> {
    let format = image_format(fmt).ok_or(MediaError::Unsupported)?;
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(image_limits(limits));
    let decoder = reader.into_decoder().map_err(|e| invalid(fmt, e))?;
    let (w, h) = decoder.dimensions();
    check_pixels(w, h, limits)?;
    DynamicImage::from_decoder(decoder).map_err(|e| invalid(fmt, e))
}

/// One decoded, composited animation frame.
pub(crate) struct AnimFrame {
    pub image: RgbaImage,
    pub delay_ms: u32,
}

/// Iterate the composited frames of a GIF or WebP animation. `f` returns
/// `false` to stop early.
pub(crate) fn for_each_frame(
    bytes: &[u8],
    fmt: MediaFormat,
    limits: &Limits,
    mut f: impl FnMut(AnimFrame) -> Result<bool, MediaError>,
) -> Result<(), MediaError> {
    let mut lim = image_limits(limits);
    let frames = match fmt {
        MediaFormat::Gif => {
            let mut d = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).map_err(|e| invalid(fmt, e))?;
            let (w, h) = d.dimensions();
            check_pixels(w, h, limits)?;
            d.set_limits(lim.clone()).map_err(|e| invalid(fmt, e))?;
            d.into_frames()
        }
        MediaFormat::Webp => {
            let mut d = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).map_err(|e| invalid(fmt, e))?;
            let (w, h) = d.dimensions();
            check_pixels(w, h, limits)?;
            lim.max_alloc = None; // WebP frames are allocated per frame; dimensions checked above.
            d.set_limits(lim).map_err(|e| invalid(fmt, e))?;
            d.into_frames()
        }
        _ => return Err(MediaError::Unsupported),
    };
    let budget = limits.max_pixels.saturating_mul(FRAME_PIXEL_BUDGET);
    let mut used = 0u64;
    let mut count = 0u32;
    for frame in frames {
        let frame = frame.map_err(|e| invalid(fmt, e))?;
        count += 1;
        if count > limits.max_frames {
            return Err(MediaError::TooLarge(format!("more than {} frames", limits.max_frames)));
        }
        let (n, d) = frame.delay().numer_denom_ms();
        let delay_ms = normalise_delay(n.checked_div(d).unwrap_or(0));
        let image = frame.into_buffer();
        used = used.saturating_add(u64::from(image.width()) * u64::from(image.height()));
        if used > budget {
            return Err(MediaError::TooLarge("animation decodes to too many pixels".into()));
        }
        if !f(AnimFrame { image, delay_ms })? {
            break;
        }
    }
    if count == 0 {
        return Err(invalid(fmt, "no frames"));
    }
    Ok(())
}

/// The still image, or the first composited frame of an animation.
pub(crate) fn first_frame(bytes: &[u8], fmt: MediaFormat, limits: &Limits) -> Result<RgbaImage, MediaError> {
    let animated = match fmt {
        MediaFormat::Gif => true,
        MediaFormat::Webp => image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
            .map_err(|e| invalid(fmt, e))?
            .has_animation(),
        _ => false,
    };
    if !animated {
        return Ok(decode_still(bytes, fmt, limits)?.to_rgba8());
    }
    let mut first = None;
    for_each_frame(bytes, fmt, limits, |f| {
        first = Some(f.image);
        Ok(false)
    })?;
    first.ok_or_else(|| invalid(fmt, "no frames"))
}

/// Frame delays (normalised, ms) read from the container without decoding
/// pixels.
pub(crate) fn frame_delays(bytes: &[u8], fmt: MediaFormat) -> Result<Vec<u32>, MediaError> {
    match fmt {
        MediaFormat::Gif => {
            let mut opts = gif::DecodeOptions::new();
            opts.skip_frame_decoding(true);
            let mut d = opts.read_info(Cursor::new(bytes)).map_err(|e| invalid(fmt, e))?;
            let mut out = Vec::new();
            while let Some(frame) = d.read_next_frame().map_err(|e| invalid(fmt, e))? {
                out.push(normalise_delay(u32::from(frame.delay) * 10));
            }
            Ok(out)
        }
        MediaFormat::Webp => {
            let (_, layout) = webp::rebuild(bytes)?;
            if layout.durations.is_empty() {
                Ok(vec![100])
            } else {
                Ok(layout.durations.into_iter().map(normalise_delay).collect())
            }
        }
        _ => Err(MediaError::Unsupported),
    }
}

/// Whether any pixel is not fully opaque.
pub(crate) fn has_transparency(img: &RgbaImage) -> bool {
    img.pixels().any(|p| p[3] < 255)
}

/// Result of sanitising a still or animation.
pub(crate) struct Sanitised {
    pub original: Vec<u8>,
    pub probe: Probe,
    /// Representative frame: the image, or the first frame of an animation.
    pub frame: RgbaImage,
}

pub(crate) fn sanitise(bytes: &[u8], fmt: MediaFormat, limits: &Limits) -> Result<Sanitised, MediaError> {
    match fmt {
        MediaFormat::Png => sanitise_png(bytes, limits),
        MediaFormat::Jpeg => sanitise_jpeg(bytes, limits),
        MediaFormat::Gif => sanitise_gif(bytes, limits),
        MediaFormat::Webp => sanitise_webp(bytes, limits),
        MediaFormat::Mp4 | MediaFormat::Webm => Err(MediaError::Unsupported),
    }
}

fn still_probe(fmt: MediaFormat, w: u32, h: u32, has_alpha: bool) -> Probe {
    Probe {
        format: fmt,
        kind: MediaKind::Still,
        width: w,
        height: h,
        frame_count: None,
        duration_ms: None,
        fps: None,
        has_alpha,
    }
}

/// Decode with the png crate (no transformations) and re-encode the same
/// colour type, bit depth, palette and tRNS. Every other chunk (text, time,
/// ICC, EXIF, APNG frames, private chunks) is dropped.
fn sanitise_png(bytes: &[u8], limits: &Limits) -> Result<Sanitised, MediaError> {
    let fmt = MediaFormat::Png;
    let byte_limit = usize::try_from(limits.max_pixels.saturating_mul(8).saturating_add(1 << 20)).unwrap_or(usize::MAX);
    let mut dec = png::Decoder::new_with_limits(Cursor::new(bytes), png::Limits { bytes: byte_limit });
    dec.set_transformations(png::Transformations::IDENTITY);
    dec.set_ignore_text_chunk(true);
    dec.set_ignore_iccp_chunk(true);
    let mut reader = dec.read_info().map_err(|e| invalid(fmt, e))?;
    let info = reader.info();
    let (w, h) = (info.width, info.height);
    check_pixels(w, h, limits)?;
    let color = info.color_type;
    let depth = info.bit_depth;
    let palette = info.palette.as_ref().map(|p| p.to_vec());
    let trns = info.trns.as_ref().map(|t| t.to_vec());
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| invalid(fmt, "image too large"))?;
    let mut buf = vec![0u8; size];
    let out_info = reader.next_frame(&mut buf).map_err(|e| invalid(fmt, e))?;
    buf.truncate(out_info.buffer_size());

    let mut out = Vec::with_capacity(bytes.len());
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(depth);
        if let Some(p) = palette {
            enc.set_palette(p);
        }
        if let Some(t) = trns.clone() {
            enc.set_trns(t);
        }
        let mut writer = enc.write_header().map_err(|e| invalid(fmt, e))?;
        writer.write_image_data(&buf).map_err(|e| invalid(fmt, e))?;
        writer.finish().map_err(|e| invalid(fmt, e))?;
    }
    let frame = decode_still(&out, fmt, limits)?.to_rgba8();
    let has_alpha =
        matches!(color, png::ColorType::Rgba | png::ColorType::GrayscaleAlpha) || trns.is_some_and(|t| !t.is_empty());
    Ok(Sanitised {
        original: out,
        probe: still_probe(fmt, w, h, has_alpha),
        frame,
    })
}

/// Rebuild from whitelisted segments, then fully decode the rebuilt file.
fn sanitise_jpeg(bytes: &[u8], limits: &Limits) -> Result<Sanitised, MediaError> {
    let fmt = MediaFormat::Jpeg;
    let (out, orientation) = jpeg::rebuild(bytes)?;
    let mut img = decode_still(&out, fmt, limits)?;
    if let Some(o) = image::metadata::Orientation::from_exif(orientation) {
        img.apply_orientation(o);
    }
    let frame = img.to_rgba8();
    let probe = still_probe(fmt, frame.width(), frame.height(), false);
    Ok(Sanitised {
        original: out,
        probe,
        frame,
    })
}

/// Facts collected while re-encoding a GIF.
struct GifFacts {
    frames: u32,
    total_ms: u64,
    transparent: bool,
}

/// Re-encode a GIF frame by frame with the gif crate: palettes, disposal,
/// delays, transparency, offsets and the NETSCAPE loop count are kept, the
/// LZW data is decoded and re-compressed (lossless). Comments, XMP and other
/// application extensions and any trailing data are dropped.
fn reencode_gif(bytes: &[u8], limits: &Limits) -> Result<(Vec<u8>, GifFacts), MediaError> {
    let fmt = MediaFormat::Gif;
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::Indexed);
    let frame_limit = NonZeroU64::new(limits.max_pixels.max(1)).expect("non-zero");
    opts.set_memory_limit(gif::MemoryLimit::Bytes(frame_limit));
    let mut dec = opts.read_info(Cursor::new(bytes)).map_err(|e| invalid(fmt, e))?;
    let (w, h) = (dec.width(), dec.height());
    check_pixels(u32::from(w), u32::from(h), limits)?;
    let global = dec.global_palette().map(<[u8]>::to_vec).unwrap_or_default();
    let budget = limits.max_pixels.saturating_mul(FRAME_PIXEL_BUDGET);
    let mut used = 0u64;
    let mut facts = GifFacts {
        frames: 0,
        total_ms: 0,
        transparent: false,
    };
    let mut enc: Option<gif::Encoder<Vec<u8>>> = None;
    while let Some(f) = dec.read_next_frame().map_err(|e| invalid(fmt, e))? {
        let mut frame = f.clone();
        facts.frames += 1;
        if facts.frames > limits.max_frames {
            return Err(MediaError::TooLarge(format!("more than {} frames", limits.max_frames)));
        }
        used = used.saturating_add(u64::from(frame.width) * u64::from(frame.height));
        if used > budget {
            return Err(MediaError::TooLarge("animation decodes to too many pixels".into()));
        }
        facts.total_ms += u64::from(normalise_delay(u32::from(frame.delay) * 10));
        facts.transparent |= frame.transparent.is_some();
        if enc.is_none() {
            // The loop count extension precedes the first frame.
            let repeat = dec.repeat();
            let sink = Vec::with_capacity(bytes.len());
            let mut e = gif::Encoder::new(sink, w, h, &global).map_err(|e| invalid(fmt, e))?;
            e.set_repeat(repeat).map_err(|e| invalid(fmt, e))?;
            enc = Some(e);
        }
        // The decoder de-interlaces; write rows sequentially.
        frame.interlaced = false;
        enc.as_mut()
            .expect("set above")
            .write_frame(&frame)
            .map_err(|e| invalid(fmt, e))?;
    }
    let enc = enc.ok_or_else(|| invalid(fmt, "no frames"))?;
    let out = enc.into_inner().map_err(|e| invalid(fmt, e))?;
    Ok((out, facts))
}

fn sanitise_gif(bytes: &[u8], limits: &Limits) -> Result<Sanitised, MediaError> {
    let fmt = MediaFormat::Gif;
    let (out, facts) = reencode_gif(bytes, limits)?;
    let frame = first_frame(&out, fmt, limits)?;
    let (w, h) = frame.dimensions();
    let mut probe = still_probe(fmt, w, h, facts.transparent);
    if facts.frames > 1 {
        probe.kind = MediaKind::Animation;
        probe.frame_count = Some(facts.frames);
        probe.duration_ms = Some(facts.total_ms);
        probe.fps = Some(fps(facts.frames, facts.total_ms));
    }
    Ok(Sanitised {
        original: out,
        probe,
        frame,
    })
}

fn fps(frames: u32, total_ms: u64) -> f32 {
    if total_ms == 0 {
        0.0
    } else {
        (f64::from(frames) * 1000.0 / total_ms as f64) as f32
    }
}

/// Rebuild from whitelisted chunks, then fully decode every frame of the
/// rebuilt file.
fn sanitise_webp(bytes: &[u8], limits: &Limits) -> Result<Sanitised, MediaError> {
    let fmt = MediaFormat::Webp;
    let (out, _) = webp::rebuild(bytes)?;
    let d = image::codecs::webp::WebPDecoder::new(Cursor::new(&out[..])).map_err(|e| invalid(fmt, e))?;
    let (w, h) = d.dimensions();
    check_pixels(w, h, limits)?;
    let has_alpha = d.color_type().has_alpha();
    if !d.has_animation() {
        drop(d);
        let frame = decode_still(&out, fmt, limits)?.to_rgba8();
        return Ok(Sanitised {
            original: out,
            probe: still_probe(fmt, w, h, has_alpha),
            frame,
        });
    }
    drop(d);
    let mut first = None;
    let mut frames = 0u32;
    let mut total_ms = 0u64;
    for_each_frame(&out, fmt, limits, |f| {
        frames += 1;
        total_ms += u64::from(f.delay_ms);
        if first.is_none() {
            first = Some(f.image);
        }
        Ok(true)
    })?;
    let frame = first.ok_or_else(|| invalid(fmt, "no frames"))?;
    let mut probe = still_probe(fmt, w, h, has_alpha);
    if frames > 1 {
        probe.kind = MediaKind::Animation;
        probe.frame_count = Some(frames);
        probe.duration_ms = Some(total_ms);
        probe.fps = Some(fps(frames, total_ms));
    }
    Ok(Sanitised {
        original: out,
        probe,
        frame,
    })
}
