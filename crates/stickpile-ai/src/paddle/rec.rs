//! Recognition: line crops to the recogniser's input and its output back to
//! text, after RapidOCR 3.8.1 (`ch_ppocr_rec/main.py` `TextRecognizer`,
//! `resize_norm_img`; `ch_ppocr_rec/utils.py` `CTCLabelDecode`).

use super::det::normalise_into;
use super::geometry::{resize_bilinear, Image3};

/// Recogniser input height and minimum width (`Rec.rec_img_shape`).
pub const REC_HEIGHT: usize = 48;
pub const REC_MIN_WIDTH: usize = 320;
/// Crops per recogniser run (`Rec.rec_batch_num`).
pub const REC_BATCH: usize = 6;
/// Our bound on a crop's width / height for the recogniser (input width
/// 48 x 160 = 7680 px); wider crops are squeezed to it. Real lines stay far
/// below; this only bounds the tensor for degenerate boxes.
pub const MAX_REC_RATIO: f64 = 160.0;

/// Batches of crop indices in RapidOCR's order: sorted by width / height,
/// [`REC_BATCH`] at a time.
pub fn batches(crops: &[Image3]) -> Vec<Vec<usize>> {
    let ratio = |i: usize| crops[i].width as f64 / crops[i].height as f64;
    let mut idx: Vec<usize> = (0..crops.len()).collect();
    idx.sort_by(|&a, &b| ratio(a).total_cmp(&ratio(b)));
    idx.chunks(REC_BATCH).map(<[usize]>::to_vec).collect()
}

/// Input width of a batch: the widest crop at height 48, at least 320.
pub fn batch_width(crops: &[&Image3]) -> usize {
    let max_ratio = crops
        .iter()
        .map(|c| (c.width as f64 / c.height as f64).min(MAX_REC_RATIO))
        .fold(REC_MIN_WIDTH as f64 / REC_HEIGHT as f64, f64::max);
    (REC_HEIGHT as f64 * max_ratio) as usize
}

/// The recogniser input `[n, 3, 48, width]` for a batch: each crop scaled to
/// height 48 (aspect kept, at most `width`), normalised, zero-padded on the
/// right. Returns the width and the data.
pub fn rec_input(crops: &[&Image3]) -> (usize, Vec<f32>) {
    let width = batch_width(crops);
    let item = 3 * REC_HEIGHT * width;
    let mut data = vec![0f32; crops.len() * item];
    for (k, c) in crops.iter().enumerate() {
        let ratio = c.width as f64 / c.height as f64;
        let w = ((REC_HEIGHT as f64 * ratio).ceil() as usize).clamp(1, width);
        let resized = resize_bilinear(c.width, c.height, w, REC_HEIGHT, |x, y| c.pixel(x, y));
        normalise_into(&resized, width, &mut data[k * item..(k + 1) * item]);
    }
    (width, data)
}

/// The recogniser's dictionary: the model's `character` metadata, one
/// entry per line (Python `splitlines`), then a space. Output class `i` is
/// entry `i - 1`; class 0 is the CTC blank.
pub fn dictionary(metadata: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = metadata.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(std::mem::take(&mut cur));
            }
            '\n' | '\u{b}' | '\u{c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                out.push(std::mem::take(&mut cur));
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.push(" ".to_owned());
    out
}

/// `round(x, 5)`.
fn round5(x: f64) -> f64 {
    (x * 1e5).round_ties_even() / 1e5
}

/// Greedy CTC decoding of one line's probabilities (`steps x classes`,
/// row-major): the most likely class per step, repeats collapsed, blanks
/// (class 0) dropped. The score is the mean probability of the kept steps
/// (0 when none), rounded to 5 decimals like RapidOCR's.
pub fn ctc_decode(probs: &[f32], classes: usize, dict: &[String]) -> (String, f32) {
    let mut text = String::new();
    let mut confs: Vec<f64> = Vec::new();
    let mut prev: Option<usize> = None;
    for row in probs.chunks_exact(classes) {
        let (idx, p) = row.iter().enumerate().fold(
            (0usize, f32::NEG_INFINITY),
            |m, (i, &v)| if v > m.1 { (i, v) } else { m },
        );
        if idx != 0 && prev != Some(idx) {
            if let Some(s) = dict.get(idx - 1) {
                text.push_str(s);
            }
            confs.push(round5(p as f64));
        }
        prev = Some(idx);
    }
    let score = if confs.is_empty() {
        0.0
    } else {
        round5(confs.iter().sum::<f64>() / confs.len() as f64)
    };
    (text, score as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Vec<String> {
        dictionary("к\nі\nт\nK")
    }

    /// One-hot-ish rows: class `c` with probability `p`, the rest share 1 - p.
    fn rows(seq: &[(usize, f32)], classes: usize) -> Vec<f32> {
        seq.iter()
            .flat_map(|&(c, p)| (0..classes).map(move |i| if i == c { p } else { (1.0 - p) / (classes - 1) as f32 }))
            .collect()
    }

    #[test]
    fn dictionary_appends_a_space() {
        assert_eq!(dict(), vec!["к", "і", "т", "K", " "]);
        assert_eq!(dictionary("a\r\nb\n"), vec!["a", "b", " "]);
        assert_eq!(dictionary(""), vec![" "]);
    }

    #[test]
    fn ctc_collapses_repeats_and_drops_blanks() {
        let d = dict();
        let classes = d.len() + 1;
        // к к _ і т т _ т  ->  кітт
        let p = rows(
            &[
                (1, 0.9),
                (1, 0.8),
                (0, 0.99),
                (2, 0.7),
                (3, 0.6),
                (3, 0.9),
                (0, 0.9),
                (3, 1.0),
            ],
            classes,
        );
        let (text, score) = ctc_decode(&p, classes, &d);
        assert_eq!(text, "кітт");
        // Kept steps: к 0.9, і 0.7, т 0.6, т 1.0.
        assert!((score - 0.8).abs() < 1e-6, "{score}");
        // The last class is the space.
        let p = rows(&[(4, 0.9), (5, 0.9), (4, 0.9)], classes);
        assert_eq!(ctc_decode(&p, classes, &d).0, "K K");
        // Only blanks: empty, score 0.
        let p = rows(&[(0, 0.9), (0, 0.9)], classes);
        assert_eq!(ctc_decode(&p, classes, &d), (String::new(), 0.0));
    }

    #[test]
    fn batches_sort_by_aspect_and_pad_to_the_widest() {
        let crop = |w, h| Image3::new(w, h);
        let crops: Vec<Image3> = [(100, 10), (10, 10), (50, 10), (30, 10), (20, 10), (40, 10), (60, 10)]
            .iter()
            .map(|&(w, h)| crop(w, h))
            .collect();
        let b = batches(&crops);
        assert_eq!(b, vec![vec![1, 4, 3, 5, 2, 6], vec![0]]);
        // At least 320 wide; a 10:1 crop needs 480.
        let refs: Vec<&Image3> = b[0].iter().map(|&i| &crops[i]).collect();
        assert_eq!(batch_width(&refs), 320);
        assert_eq!(batch_width(&[&crops[0]]), 480);
        // A degenerate 10000:1 box is bounded.
        let thin = crop(10000, 1);
        assert_eq!(batch_width(&[&thin]), 7680);

        let mut white = Image3::new(20, 10);
        white.data.fill(255);
        let (w, data) = rec_input(&[&white]);
        assert_eq!((w, data.len()), (320, 3 * 48 * 320));
        // 20 x 10 scales to 96 x 48: white (1.0) there, zero padding after.
        assert_eq!(data[95], 1.0);
        assert_eq!(data[96], 0.0);
    }
}
