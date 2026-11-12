//! Pixel signatures: the check that turns a near-duplicate *candidate* into
//! a near-duplicate (the server's problems report).
//!
//! The 64-bit perceptual hashes ([`crate::hash`]) cannot tell a resized copy
//! of a sticker from the same character with another expression: flat
//! drawings on a transparent background all reduce to "a blob in the middle"
//! (calibration on synthetic stickers: resized, recompressed and blurred
//! copies have a pHash distance of 0 to 10, the same face with closed eyes
//! or a frown 2 to 4, two different faces 6 to 8). Candidates are therefore
//! compared on small grey renderings (48×48, of the grid thumbnail): the
//! worst of the 8×8 blocks decides, so a changed mouth or eye counts even
//! when the rest of the picture is identical, while recompression noise
//! spreads thinly over every block. Slight crops are matched by also
//! comparing the content boxes (crops of a transparent margin) and centre
//! crops of 3, 6 and 10 % (crops into the picture). Grey hides colour, so
//! each view also keeps the mean chroma of a 4×4 grid: the same drawing in
//! another colour is not a copy.
//!
//! A signature is about 12 KB and depends only on the picture, so callers
//! can cache it by media id.

use image::RgbaImage;

/// Side of the compared grey images.
const SIDE: usize = 48;
/// The grey images are compared in 8×8 blocks of this many pixels a side.
const BLOCK: usize = 6;
const BLOCKS: usize = SIDE / BLOCK;
/// Centre crops tried for slightly cropped copies (fraction cut per side).
const CROPS: [f32; 3] = [0.03, 0.06, 0.10];
/// Luma below this (after flattening on white) is content, not background.
const CONTENT_LUMA: u8 = 240;

/// Colour is compared on this many cells a side of each view.
const CHROMA_CELLS: usize = 4;

/// Views of one picture (see [`View`]).
#[derive(Debug, Clone)]
pub struct Signature {
    full: View,
    /// The content box (non-background pixels), scaled to the full side.
    content: View,
    /// Centre crops by [`CROPS`].
    crops: [View; 3],
}

/// One view: [`SIDE`]² grey pixels, the sum of each block (two blocks whose
/// sums differ by more than the limit cannot be within it, so most pairs
/// are rejected after 64 subtractions), and the mean (Cb, Cr) of
/// [`CHROMA_CELLS`]² cells: grey alone cannot tell a red sticker from the
/// same one in green.
#[derive(Debug, Clone)]
struct View {
    px: Vec<u8>,
    sums: Vec<u16>,
    chroma: Vec<(u8, u8)>,
}

impl View {
    fn of(g: &Grey, rect: Rect) -> View {
        let px = view(g, rect);
        let mut sums = vec![0u16; BLOCKS * BLOCKS];
        for (i, v) in px.iter().enumerate() {
            let (x, y) = (i % SIDE, i / SIDE);
            sums[(y / BLOCK) * BLOCKS + x / BLOCK] += u16::from(*v);
        }
        View {
            px,
            sums,
            chroma: chroma_cells(g, rect),
        }
    }

    /// The largest |ΔCb| + |ΔCr| (0..=510) over the colour cells.
    fn colour_distance(&self, other: &View) -> u32 {
        self.chroma
            .iter()
            .zip(&other.chroma)
            .map(|(a, b)| u32::from(a.0.abs_diff(b.0)) + u32::from(a.1.abs_diff(b.1)))
            .max()
            .unwrap_or(0)
    }
}

/// A picture flattened on white: row-major luma, and chroma (Cb, Cr).
struct Grey {
    px: Vec<u8>,
    chroma: Vec<(u8, u8)>,
    w: usize,
    h: usize,
}

/// A rectangle of a [`Grey`] picture: x, y, width, height.
type Rect = (usize, usize, usize, usize);

impl Signature {
    /// Signature of an encoded grid thumbnail. Still WebP (what the server
    /// stores) is decoded by libwebp, which is optimised even in debug
    /// builds (the pure-Rust decoder is generic and compiled into this crate
    /// unoptimised there); anything else goes through
    /// [`crate::decode_frame`] under `limits`.
    pub fn of_thumbnail(bytes: &[u8], limits: &crate::Limits) -> Result<Signature, crate::MediaError> {
        if crate::sniff(bytes) == Some(crate::MediaFormat::Webp) {
            let small = webp::BitstreamFeatures::new(bytes).is_some_and(|f| {
                !f.has_animation() && u64::from(f.width()) * u64::from(f.height()) <= limits.max_pixels
            });
            if let Some(img) = small.then(|| webp::Decoder::new(bytes).decode()).flatten() {
                let (w, h) = (img.width(), img.height());
                let rgba = if img.is_alpha() {
                    img.to_vec()
                } else {
                    img.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()
                };
                if let Some(frame) = RgbaImage::from_raw(w, h, rgba) {
                    return Ok(Signature::of(&frame));
                }
            }
        }
        Ok(Signature::of(&crate::decode_frame(bytes, limits)?))
    }

    /// Signature of a decoded frame (the grid thumbnail, at most 256 px).
    /// Plain loops instead of the image crate's generic filters: a server
    /// computes this for every thumbnail of a set, and it must stay cheap in
    /// unoptimised builds too (about 15 ms a thumbnail there).
    pub fn of(frame: &RgbaImage) -> Signature {
        let grey = luma_on_white(frame);
        let (w, h) = (grey.w, grey.h);
        let content = content_box(&grey).unwrap_or((0, 0, w, h));
        let crops = CROPS.map(|f| {
            let (dx, dy) = ((w as f32 * f).round() as usize, (h as f32 * f).round() as usize);
            if 2 * dx >= w || 2 * dy >= h {
                (0, 0, w, h)
            } else {
                (dx, dy, w - 2 * dx, h - 2 * dy)
            }
        });
        Signature {
            full: View::of(&grey, (0, 0, w, h)),
            content: View::of(&grey, content),
            crops: crops.map(|r| View::of(&grey, r)),
        }
    }

    /// The smallest worst-block difference (mean absolute luma difference,
    /// 0..=255) between the two pictures over the compared views, if one is
    /// at most `limit` and the colours of those views are at most
    /// `colour_limit` apart (the largest |ΔCb| + |ΔCr| of a colour cell,
    /// 0..=510: recompression and resizing keep it small, the same drawing
    /// in another colour does not). Views: whole pictures, content boxes, and
    /// each centre crop of one against the whole other (both ways).
    pub fn distance(&self, other: &Signature, limit: f32, colour_limit: u32) -> Option<f32> {
        let mut pairs: Vec<(&View, &View)> = vec![(&self.full, &other.full), (&self.content, &other.content)];
        for c in &self.crops {
            pairs.push((c, &other.full));
        }
        for c in &other.crops {
            pairs.push((&self.full, c));
        }
        let mut best: Option<f32> = None;
        for (a, b) in pairs {
            let bound = best.unwrap_or(limit);
            if a.colour_distance(b) > colour_limit {
                continue;
            }
            if let Some(d) = worst_block(a, b, bound) {
                best = Some(d);
                if d == 0.0 {
                    break;
                }
            }
        }
        best
    }
}

/// Mean absolute difference of the worst block, or `None` as soon as a block
/// exceeds `limit` (most pairs are rejected by the block sums alone).
fn worst_block(va: &View, vb: &View, limit: f32) -> Option<f32> {
    let area = (BLOCK * BLOCK) as f32;
    if va
        .sums
        .iter()
        .zip(&vb.sums)
        .any(|(x, y)| f32::from(x.abs_diff(*y)) / area > limit)
    {
        return None;
    }
    let (a, b) = (&va.px, &vb.px);
    let mut worst = 0f32;
    for by in 0..BLOCKS {
        for bx in 0..BLOCKS {
            let mut sum = 0u32;
            for y in by * BLOCK..(by + 1) * BLOCK {
                let row = y * SIDE;
                for x in bx * BLOCK..(bx + 1) * BLOCK {
                    sum += u32::from(a[row + x].abs_diff(b[row + x]));
                }
            }
            let mean = sum as f32 / area;
            if mean > limit {
                return None;
            }
            worst = worst.max(mean);
        }
    }
    Some(worst)
}

/// The frame composited on white (like the perceptual hashes) as BT.601
/// luma and chroma.
fn luma_on_white(img: &RgbaImage) -> Grey {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 {
        return Grey {
            px: vec![255],
            chroma: vec![(128, 128)],
            w: 1,
            h: 1,
        };
    }
    // Integer arithmetic (coefficients scaled by 2^16): cheap in debug builds.
    let (px, chroma) = img
        .as_raw()
        .chunks_exact(4)
        .map(|p| {
            let a = i32::from(p[3]);
            let [r, g, b] = [p[0], p[1], p[2]].map(|c| (i32::from(c) * a + 255 * (255 - a) + 127) / 255);
            let q = |v: i32| ((v + 32_768) >> 16).clamp(0, 255) as u8;
            let y = q(19_595 * r + 38_470 * g + 7_471 * b);
            let cb = q(-11_059 * r - 21_709 * g + 32_768 * b + (128 << 16));
            let cr = q(32_768 * r - 27_439 * g - 5_329 * b + (128 << 16));
            (y, (cb, cr))
        })
        .unzip();
    Grey { px, chroma, w, h }
}

/// Mean chroma of [`CHROMA_CELLS`]² cells of `rect`.
fn chroma_cells(g: &Grey, (x0, y0, cw, ch): Rect) -> Vec<(u8, u8)> {
    let n = CHROMA_CELLS;
    let mut sums = vec![(0u32, 0u32, 0u32); n * n];
    for y in 0..ch {
        let cy = y * n / ch;
        for x in 0..cw {
            let (cb, cr) = g.chroma[(y0 + y) * g.w + x0 + x];
            let s = &mut sums[cy * n + x * n / cw];
            s.0 += u32::from(cb);
            s.1 += u32::from(cr);
            s.2 += 1;
        }
    }
    sums.into_iter()
        .map(|(cb, cr, k)| match k {
            0 => (128, 128),
            k => (((cb + k / 2) / k) as u8, ((cr + k / 2) / k) as u8),
        })
        .collect()
}

/// For each of `out` target cells over `len` source pixels: the first
/// source pixel and the share of the cell each following pixel covers
/// (exact area averaging). Cells smaller than a pixel (enlarging) take the
/// pixel under their centre.
fn area_weights(len: usize, out: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = len as f32 / out as f32;
    (0..out)
        .map(|o| {
            let (a, b) = (o as f32 * scale, (o + 1) as f32 * scale);
            if scale <= 1.0 {
                return ((((a + b) / 2.0) as usize).min(len - 1), vec![1.0]);
            }
            let first = a.floor() as usize;
            let last = (b.ceil() as usize).min(len);
            let w = (first..last)
                .map(|i| ((i as f32 + 1.0).min(b) - (i as f32).max(a)).max(0.0) / scale)
                .collect();
            (first, w)
        })
        .collect()
}

/// `rect` of `g` scaled to [`SIDE`]² by area averaging, in two passes.
fn view(g: &Grey, (x0, y0, cw, ch): Rect) -> Vec<u8> {
    let (wx, wy) = (area_weights(cw, SIDE), area_weights(ch, SIDE));
    // Rows of the rectangle, each reduced to SIDE columns. Plain index
    // loops: iterator adaptors are slow in unoptimised builds.
    let mut rows = vec![0f32; ch * SIDE];
    for y in 0..ch {
        let src = &g.px[(y0 + y) * g.w + x0..(y0 + y) * g.w + x0 + cw];
        for (ox, (first, w)) in wx.iter().enumerate() {
            let mut acc = 0f32;
            for (k, wk) in w.iter().enumerate() {
                acc += wk * f32::from(src[first + k]);
            }
            rows[y * SIDE + ox] = acc;
        }
    }
    let mut out = vec![0u8; SIDE * SIDE];
    for (oy, (first, w)) in wy.iter().enumerate() {
        for ox in 0..SIDE {
            let mut acc = 0f32;
            for (k, wk) in w.iter().enumerate() {
                acc += wk * rows[(first + k) * SIDE + ox];
            }
            out[oy * SIDE + ox] = acc.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// The pixels darker than the background, unless that is (nearly) the
/// whole picture or nothing.
fn content_box(g: &Grey) -> Option<Rect> {
    let (mut x0, mut y0, mut x1, mut y1) = (g.w, g.h, 0, 0);
    for y in 0..g.h {
        let row = &g.px[y * g.w..(y + 1) * g.w];
        let Some(first) = row.iter().position(|&v| v < CONTENT_LUMA) else {
            continue;
        };
        let last = row.iter().rposition(|&v| v < CONTENT_LUMA).unwrap_or(first);
        x0 = x0.min(first);
        x1 = x1.max(last);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    if x0 > x1 || y0 > y1 {
        return None;
    }
    let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
    if bw * 100 >= g.w * 98 && bh * 100 >= g.h * 98 {
        return None;
    }
    Some((x0, y0, bw, bh))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::imageops::{self, FilterType};
    use image::Rgba;

    fn disc(w: u32, h: u32, r: f32, shade: u8) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let (dx, dy) = (x as f32 - w as f32 / 2.0, y as f32 - h as f32 / 2.0);
            if (dx * dx + dy * dy).sqrt() < r {
                Rgba([shade, shade / 2, 40, 255])
            } else {
                Rgba([0, 0, 0, 0])
            }
        })
    }

    #[test]
    fn identical_and_rescaled_pictures_are_close() {
        let a = Signature::of(&disc(256, 256, 90.0, 200));
        assert_eq!(a.distance(&a, 20.0, 10), Some(0.0));
        let small = imageops::resize(&disc(256, 256, 90.0, 200), 100, 100, FilterType::Lanczos3);
        let d = a.distance(&Signature::of(&small), 20.0, 10).expect("rescaled copy");
        assert!(d < 10.0, "{d}");
        // Tiny pictures are scaled up.
        let tiny = imageops::resize(&disc(256, 256, 90.0, 200), 24, 24, FilterType::Lanczos3);
        assert!(a.distance(&Signature::of(&tiny), 20.0, 10).is_some());
    }

    #[test]
    fn content_box_ignores_transparent_margins() {
        // The same disc with a much smaller margin (a crop of the margin).
        let a = Signature::of(&disc(256, 256, 60.0, 200));
        let b = Signature::of(&imageops::crop_imm(&disc(256, 256, 60.0, 200), 60, 60, 136, 136).to_image());
        assert!(a.distance(&b, 20.0, 10).is_some());
    }

    #[test]
    fn different_pictures_are_far() {
        let a = Signature::of(&disc(256, 256, 90.0, 200));
        let b = Signature::of(&disc(256, 256, 40.0, 60));
        assert_eq!(a.distance(&b, 20.0, 10), None);
    }

    #[test]
    fn thumbnails_decode_like_frames() {
        let frame = disc(200, 160, 60.0, 220);
        let thumb = crate::variants::thumb(&frame, 256).unwrap();
        let via_libwebp = Signature::of_thumbnail(&thumb.bytes, &crate::Limits::default()).unwrap();
        let decoded = crate::decode_frame(&thumb.bytes, &crate::Limits::default()).unwrap();
        let d = via_libwebp.distance(&Signature::of(&decoded), 1.0, 2);
        assert!(d.is_some(), "{d:?}");
        // Opaque thumbnails (RGB WebP) and other formats work too.
        let opaque = RgbaImage::from_pixel(64, 48, Rgba([10, 200, 30, 255]));
        let t = crate::variants::thumb(&opaque, 256).unwrap();
        assert!(Signature::of_thumbnail(&t.bytes, &crate::Limits::default()).is_ok());
        let mut png = std::io::Cursor::new(Vec::new());
        opaque.write_to(&mut png, image::ImageFormat::Png).unwrap();
        assert!(Signature::of_thumbnail(png.get_ref(), &crate::Limits::default()).is_ok());
        assert!(Signature::of_thumbnail(b"not an image", &crate::Limits::default()).is_err());
    }

    #[test]
    fn another_colour_with_the_same_grey_is_not_a_copy() {
        // Red and green of about the same luma: the grey views agree.
        let red = RgbaImage::from_pixel(64, 64, Rgba([220, 60, 60, 255]));
        let green = RgbaImage::from_pixel(64, 64, Rgba([40, 145, 40, 255]));
        let (a, b) = (Signature::of(&red), Signature::of(&green));
        assert!(a.distance(&b, 20.0, 510).is_some(), "same grey");
        assert_eq!(a.distance(&b, 20.0, 12), None);
        assert!(a.distance(&a, 20.0, 0).is_some());
    }
}
