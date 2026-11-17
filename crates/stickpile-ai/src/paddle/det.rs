//! Detection: the working picture, the detector input and the DB
//! post-processing, after RapidOCR 3.8.1 (`main.py` `preprocess_img`,
//! `utils/process_img.py`, `ch_ppocr_det/utils.py` `DetPreProcess` and
//! `DBPostProcess`, `ch_ppocr_det/main.py` `sorted_boxes`).

use image::RgbaImage;

use super::geometry::{
    box_score, contours, dilate2x2, dist, min_area_rect, mini_box, order_clockwise, resize_bilinear, unclip, Image3,
    Point,
};

/// RapidOCR `Global.max_side_len`: larger pictures are scaled down first.
pub const MAX_SIDE_LEN: usize = 2000;
/// RapidOCR `Global.min_side_len`: smaller pictures are scaled up first.
pub const MIN_SIDE_LEN: usize = 30;
/// RapidOCR `Global.min_height` and `width_height_ratio`: lower or wider
/// pictures are padded with black above and below.
pub const MIN_HEIGHT: usize = 30;
pub const WIDTH_HEIGHT_RATIO: f64 = 8.0;
/// RapidOCR `Det.limit_side_len` (`limit_type = min`): the detector input's
/// short side is scaled up to this.
pub const DET_LIMIT_SIDE: f64 = 736.0;
/// Our cap on the detector input's long side (docs/ai.md, OCR evaluation).
pub const DET_MAX_SIDE: f64 = 2000.0;
/// DB post-processing (`Det.thresh`, `box_thresh`, `max_candidates`,
/// `unclip_ratio`, `min_size`).
pub const DB_THRESH: f32 = 0.3;
pub const BOX_THRESH: f64 = 0.5;
pub const MAX_CANDIDATES: usize = 1000;
pub const UNCLIP_RATIO: f64 = 1.6;
const MIN_SIZE: f64 = 3.0;
/// `sorted_boxes`: boxes whose tops differ by less than this share a row.
const ROW_THRESHOLD: f32 = 10.0;
/// Our bound on the padded working picture: the scale-up of very thin
/// strips (side under 30 px) plus the padding of very wide ones would
/// otherwise grow without limit (a 2000 x 1 strip becomes 60000 x 15000).
pub const MAX_WORK_PIXELS: usize = 16 << 20;

/// The picture the detector and the crops work on: BGR, scaled within
/// [`MIN_SIDE_LEN`]..[`MAX_SIDE_LEN`] and padded, plus how to map back.
#[derive(Debug)]
pub struct Working {
    pub img: Image3,
    /// Rows of black padding above (and below) the picture.
    pub pad_top: usize,
    /// Original size divided by the scaled size, per axis.
    pub ratio_w: f64,
    pub ratio_h: f64,
    pub orig_w: usize,
    pub orig_h: usize,
}

impl Working {
    /// A point of the working picture in the original picture.
    pub fn to_original(&self, p: Point) -> Point {
        let x = (p[0] * self.ratio_w).clamp(0.0, self.orig_w as f64);
        let y = ((p[1] - self.pad_top as f64) * self.ratio_h).clamp(0.0, self.orig_h as f64);
        [x, y]
    }
}

/// `int(round(v / 32) * 32)` with Python's round (half to even).
fn round32(v: f64) -> i64 {
    ((v / 32.0).round_ties_even() * 32.0) as i64
}

/// One step of RapidOCR's `reduce_max_side` / `increase_min_side`: scale by
/// `ratio`, truncate, round to multiples of 32. `None` when a side becomes 0.
fn scaled32(w: usize, h: usize, ratio: f64) -> Option<(usize, usize)> {
    let rw = round32(((w as f64 * ratio) as i64) as f64);
    let rh = round32(((h as f64 * ratio) as i64) as f64);
    (rw > 0 && rh > 0).then_some((rw as usize, rh as usize))
}

/// Size of the picture after RapidOCR's `resize_image_within_bounds`.
pub fn bounded_size(w: usize, h: usize) -> Option<(usize, usize)> {
    let (mut w, mut h) = (w, h);
    if w.max(h) > MAX_SIDE_LEN {
        (w, h) = scaled32(w, h, MAX_SIDE_LEN as f64 / w.max(h) as f64)?;
    }
    if w.min(h) < MIN_SIDE_LEN {
        (w, h) = scaled32(w, h, MIN_SIDE_LEN as f64 / w.min(h) as f64)?;
    }
    Some((w, h))
}

/// Rows of padding RapidOCR's `apply_vertical_padding` adds above and below.
pub fn vertical_padding(w: usize, h: usize) -> usize {
    if h <= MIN_HEIGHT || w as f64 / h as f64 > WIDTH_HEIGHT_RATIO {
        let new_h = ((w as f64 / WIDTH_HEIGHT_RATIO) as usize).max(MIN_HEIGHT) * 2;
        new_h.abs_diff(h) / 2
    } else {
        0
    }
}

/// Detector input size for a working picture: short side scaled up to
/// [`DET_LIMIT_SIDE`], long side capped at [`DET_MAX_SIDE`], both rounded
/// to multiples of 32 (at least 32).
pub fn det_input_size(w: usize, h: usize) -> (usize, usize) {
    let (wf, hf) = (w as f64, h as f64);
    let mut ratio = 1.0;
    if wf.min(hf) < DET_LIMIT_SIDE {
        ratio = DET_LIMIT_SIDE / wf.min(hf);
    }
    if wf.max(hf) * ratio > DET_MAX_SIDE {
        ratio = DET_MAX_SIDE / wf.max(hf);
    }
    let side = |v: f64| round32(v * ratio).max(32) as usize;
    (side(wf), side(hf))
}

/// Background for transparent pixels (RapidOCR `cvt_four_to_three`): white
/// when the visible pixels are dark on average, else black.
fn background(rgba: &RgbaImage) -> f32 {
    let (mut sum, mut n) = (0f64, 0u64);
    for p in rgba.pixels().filter(|p| p[3] > 0) {
        sum += 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
        n += 1;
    }
    if n == 0 || sum / (n as f64) < 128.0 {
        255.0
    } else {
        0.0
    }
}

/// The picture flattened onto `bg` in BGR order, computed per pixel so a
/// large input is never copied whole.
fn flattened(rgba: &RgbaImage, bg: f32) -> impl Fn(usize, usize) -> [u8; 3] + '_ {
    move |x, y| {
        let p = rgba.get_pixel(x as u32, y as u32);
        if p[3] == 255 {
            return [p[2], p[1], p[0]];
        }
        let a = p[3] as f32 / 255.0;
        let c = |v: u8| (v as f32 * a + bg * (1.0 - a)).round_ties_even().clamp(0.0, 255.0) as u8;
        [c(p[2]), c(p[1]), c(p[0])]
    }
}

/// The working picture for `rgba`, or `None` when there is nothing to read
/// (an empty picture, or one so thin that the scaling or the padding would
/// exceed [`MAX_WORK_PIXELS`]).
pub fn working_picture(rgba: &RgbaImage) -> Option<Working> {
    let (ow, oh) = (rgba.width() as usize, rgba.height() as usize);
    if ow == 0 || oh == 0 {
        return None;
    }
    let (w, h) = bounded_size(ow, oh)?;
    let pad = vertical_padding(w, h);
    if w.checked_mul(h + 2 * pad)? > MAX_WORK_PIXELS {
        return None;
    }
    let sample = flattened(rgba, background(rgba));
    let scaled = if (w, h) == (ow, oh) {
        let mut img = Image3::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                img.data[i..i + 3].copy_from_slice(&sample(x, y));
            }
        }
        img
    } else {
        resize_bilinear(ow, oh, w, h, sample)
    };
    let img = if pad == 0 {
        scaled
    } else {
        let mut img = Image3::new(w, h + 2 * pad);
        img.data[pad * w * 3..(pad + h) * w * 3].copy_from_slice(&scaled.data);
        img
    };
    Some(Working {
        img,
        pad_top: pad,
        ratio_w: ow as f64 / w as f64,
        ratio_h: oh as f64 / h as f64,
        orig_w: ow,
        orig_h: oh,
    })
}

/// `(x / 255 - 0.5) / 0.5` per channel, planar (NCHW without N), `width`
/// columns with zero padding on the right of `img`.
pub fn normalise_into(img: &Image3, width: usize, out: &mut [f32]) {
    let plane = img.height * width;
    debug_assert!(out.len() >= 3 * plane && width >= img.width);
    for y in 0..img.height {
        for x in 0..img.width {
            let p = img.pixel(x, y);
            for c in 0..3 {
                out[c * plane + y * width + x] = ((p[c] as f64 / 255.0 - 0.5) / 0.5) as f32;
            }
        }
    }
}

/// The detector input tensor `[1, 3, h, w]` for a working picture.
pub fn det_input(img: &Image3) -> (usize, usize, Vec<f32>) {
    let (rw, rh) = det_input_size(img.width, img.height);
    let resized = resize_bilinear(img.width, img.height, rw, rh, |x, y| img.pixel(x, y));
    let mut data = vec![0f32; 3 * rw * rh];
    normalise_into(&resized, rw, &mut data);
    (rw, rh, data)
}

/// A detected text box: corners in working-picture pixels (top-left,
/// top-right, bottom-right, bottom-left) and the mean probability inside.
#[derive(Debug, Clone, PartialEq)]
pub struct DetBox {
    pub quad: [Point; 4],
    pub score: f64,
}

/// DB post-processing of the probability map `pred` (`mw x mh`) for a
/// working picture of `dest_w x dest_h`: binarise, dilate, contours, boxes,
/// filtered and in RapidOCR's reading order.
pub fn boxes_from_map(pred: &[f32], mw: usize, mh: usize, dest_w: usize, dest_h: usize) -> Vec<DetBox> {
    let bitmap: Vec<bool> = pred.iter().map(|&p| p > DB_THRESH).collect();
    let bitmap = dilate2x2(&bitmap, mw, mh);
    let mut boxes = Vec::new();
    for c in contours(&bitmap, mw, mh).iter().take(MAX_CANDIDATES) {
        let Some(rect) = min_area_rect(&c.points) else {
            continue;
        };
        let (pts, side) = mini_box(&rect);
        if side < MIN_SIZE {
            continue;
        }
        let score = box_score(pred, mw, mh, &pts);
        if BOX_THRESH > score {
            continue;
        }
        let Some(rect) = min_area_rect(&unclip(&pts, UNCLIP_RATIO)) else {
            continue;
        };
        let (b, side) = mini_box(&rect);
        if side < MIN_SIZE + 2.0 {
            continue;
        }
        // Scaled to the working picture in float32, as numpy does it.
        let scale = |v: f64, from: usize, to: usize| -> f64 {
            ((v as f32 / from as f32 * to as f32).round_ties_even()).clamp(0.0, to as f32) as f64
        };
        let quad = b.map(|p| [scale(p[0], mw, dest_w), scale(p[1], mh, dest_h)]);
        boxes.push(DetBox { quad, score });
    }
    // filter_det_res
    let mut kept: Vec<DetBox> = boxes
        .into_iter()
        .filter_map(|b| {
            let q = order_clockwise(&b.quad).map(|p| {
                [
                    p[0].clamp(0.0, dest_w as f64 - 1.0).trunc(),
                    p[1].clamp(0.0, dest_h as f64 - 1.0).trunc(),
                ]
            });
            let (w, h) = (dist(q[0], q[1]) as i64, dist(q[0], q[3]) as i64);
            (w > 3 && h > 3).then_some(DetBox {
                quad: q,
                score: b.score,
            })
        })
        .collect();
    sort_boxes(&mut kept);
    kept
}

/// RapidOCR `sorted_boxes`: by the top-left corner's y; boxes whose tops
/// are within 10 px of the previous one form a row, ordered by x.
pub fn sort_boxes(boxes: &mut [DetBox]) {
    boxes.sort_by(|a, b| (a.quad[0][1] as f32).total_cmp(&(b.quad[0][1] as f32)));
    let mut row = 0usize;
    let mut keyed: Vec<(usize, f32, DetBox)> = Vec::with_capacity(boxes.len());
    for (i, b) in boxes.iter().enumerate() {
        if i > 0 && (b.quad[0][1] - boxes[i - 1].quad[0][1]) as f32 >= ROW_THRESHOLD {
            row += 1;
        }
        keyed.push((row, b.quad[0][0] as f32, b.clone()));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    for (slot, (_, _, b)) in boxes.iter_mut().zip(keyed) {
        *slot = b;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn detector_input_is_capped_at_2000() {
        // Small pictures are scaled up to a 736 px short side.
        assert_eq!(det_input_size(512, 512), (736, 736));
        assert_eq!(det_input_size(600, 300), (1472, 736));
        // A long strip would become 736 x 11040; the cap keeps 2000 at most.
        let (w, h) = det_input_size(72, 1080);
        assert!(w.max(h) <= 2000, "{w}x{h}");
        assert_eq!((w, h), (128, 1984));
        assert_eq!(det_input_size(2000, 1500), (1984, 1504));
        assert_eq!(det_input_size(3, 2000), (32, 1984));
        for (w, h) in [(1, 1), (30, 9000), (2000, 2000), (5000, 40), (737, 2001)] {
            let (a, b) = det_input_size(w, h);
            assert!(a.max(b) <= 2000 && a.min(b) >= 32, "{w}x{h} -> {a}x{b}");
            assert!(a % 32 == 0 && b % 32 == 0);
        }
    }

    #[test]
    fn working_picture_bounds_and_padding() {
        assert_eq!(bounded_size(512, 512), Some((512, 512)));
        assert_eq!(bounded_size(4000, 3000), Some((1984, 1504)));
        assert_eq!(bounded_size(600, 20), Some((896, 32)));
        assert_eq!(vertical_padding(512, 512), 0);
        assert_eq!(vertical_padding(896, 32), 96);
        assert_eq!(vertical_padding(1080, 72), 99);
        // A 2000 x 1 strip would need 60000 x 15000 px: nothing is read.
        assert!(working_picture(&RgbaImage::new(2000, 1)).is_none());
        assert!(working_picture(&RgbaImage::new(0, 0)).is_none());

        let w = working_picture(&RgbaImage::from_pixel(600, 20, Rgba([10, 20, 30, 255]))).unwrap();
        assert_eq!((w.img.width, w.img.height, w.pad_top), (896, 224, 96));
        assert_eq!(w.img.pixel(0, 0), [0, 0, 0], "padding is black");
        assert_eq!(w.img.pixel(5, 100), [30, 20, 10], "BGR");
        let p = w.to_original([896.0, 96.0 + 32.0]);
        assert!((p[0] - 600.0).abs() < 1e-9 && (p[1] - 20.0).abs() < 1e-9);
    }

    #[test]
    fn transparency_is_flattened_for_contrast() {
        // Dark text on transparency goes on white, light text on black.
        let mut dark = RgbaImage::from_pixel(40, 40, Rgba([0, 0, 0, 0]));
        dark.put_pixel(1, 1, Rgba([20, 20, 20, 255]));
        dark.put_pixel(2, 1, Rgba([20, 20, 20, 128]));
        let w = working_picture(&dark).unwrap();
        assert_eq!(w.img.pixel(0, 0), [255, 255, 255]);
        assert_eq!(w.img.pixel(1, 1), [20, 20, 20]);
        assert_eq!(w.img.pixel(2, 1), [137, 137, 137]);
        let light = RgbaImage::from_pixel(40, 40, Rgba([250, 250, 250, 0]));
        let mut light = light;
        light.put_pixel(1, 1, Rgba([250, 250, 250, 255]));
        assert_eq!(working_picture(&light).unwrap().img.pixel(0, 0), [0, 0, 0]);
    }

    #[test]
    fn boxes_come_from_the_probability_map() {
        // Two text blobs on one row and one below, in a 64 x 64 map for a
        // 128 x 128 picture.
        let (mw, mh) = (64, 64);
        let mut pred = vec![0f32; mw * mh];
        let mut blob = |x0: usize, y0: usize, w: usize, h: usize| {
            for y in y0..y0 + h {
                for x in x0..x0 + w {
                    pred[y * mw + x] = 0.95;
                }
            }
        };
        blob(30, 10, 20, 6);
        blob(4, 11, 20, 6);
        blob(4, 40, 40, 8);
        // A speck too small to keep.
        blob(60, 60, 2, 2);
        let boxes = boxes_from_map(&pred, mw, mh, 128, 128);
        assert_eq!(boxes.len(), 3, "{boxes:?}");
        assert!(boxes[0].quad[0][0] < boxes[1].quad[0][0], "left box first in its row");
        assert!(boxes[2].quad[0][1] > 60.0, "lower box last");
        // Unclipped (grown) and scaled by 2: wider than the 40 px blob x 2.
        let w = dist(boxes[2].quad[0], boxes[2].quad[1]);
        assert!(w > 80.0 && w < 110.0, "{w}");
        // The dilated blob reaches one pixel past the probabilities.
        assert!(boxes.iter().all(|b| b.score > 0.7 && b.score < 0.95), "{boxes:?}");
    }

    #[test]
    fn rows_follow_rapidocr_sorted_boxes() {
        let b = |x: f64, y: f64| DetBox {
            quad: [[x, y], [x + 10.0, y], [x + 10.0, y + 5.0], [x, y + 5.0]],
            score: 1.0,
        };
        let mut v = vec![b(50.0, 3.0), b(0.0, 30.0), b(10.0, 0.0), b(0.0, 9.0)];
        sort_boxes(&mut v);
        let order: Vec<(f64, f64)> = v.iter().map(|d| (d.quad[0][0], d.quad[0][1])).collect();
        assert_eq!(order, vec![(0.0, 9.0), (10.0, 0.0), (50.0, 3.0), (0.0, 30.0)]);
    }
}
