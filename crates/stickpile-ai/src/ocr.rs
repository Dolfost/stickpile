//! Reading the words written in a picture (OCR) for the item text field
//! (docs/ai.md, "OCR and the text field").
//!
//! [`TextReader`] is the seam: a reader finds lines of text in one frame
//! with a confidence, and [`read_frames`] turns what it found in the frames
//! of a picture into the stored text with the rules every reader shares
//! ([`clean_lines`]): lines below [`MIN_CONFIDENCE`] dropped, reading order
//! (top to bottom, left to right), the pieces of one row joined with a
//! space, printable text only, a line seen in an earlier frame not
//! repeated, at most [`stickpile_api::text::MAX_TEXT_CHARS`] characters. Frames larger than [`MAX_READ_PIXELS`] are scaled down
//! first. [`MockTextReader`] stands in for a model in tests.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Mutex;

use image::RgbaImage;
use stickpile_api::text::{clean_text, truncate_text};

use crate::AiError;

/// Lines a reader is less sure of (confidence in 0..=1) are dropped.
pub const MIN_CONFIDENCE: f32 = 0.5;
/// Frames are scaled down to at most this many pixels (aspect kept) before
/// a reader sees them: text that needs more is too small to matter, and
/// readers' time grows with the pixels.
pub const MAX_READ_PIXELS: u64 = 2048 * 2048;

/// Where a line is in its frame, in pixels from the top left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl LineBox {
    fn centre_y(&self) -> f32 {
        self.y + self.height / 2.0
    }
}

/// One line of text a reader found.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLine {
    pub text: String,
    /// 0..=1.
    pub confidence: f32,
    /// Where it is, if the reader knows; without boxes the lines keep the
    /// order the reader gave them.
    pub bounds: Option<LineBox>,
}

impl TextLine {
    pub fn new(text: impl Into<String>, confidence: f32) -> TextLine {
        TextLine {
            text: text.into(),
            confidence,
            bounds: None,
        }
    }

    pub fn at(mut self, x: f32, y: f32, width: f32, height: f32) -> TextLine {
        self.bounds = Some(LineBox { x, y, width, height });
        self
    }
}

/// An OCR backend. Share one behind an `Arc`; calls may come from several
/// threads (the server runs them on its blocking pool).
pub trait TextReader: Send + Sync {
    /// Identifies the model (and its settings that change the output). It is
    /// stored next to every result (`media.ocr_model`), and media read by
    /// another model count as not read.
    fn model_id(&self) -> &str;
    /// The lines in one frame (at most [`MAX_READ_PIXELS`] pixels), in any
    /// order, each with its confidence. No text: an empty list.
    fn read(&self, frame: &RgbaImage) -> Result<Vec<TextLine>, AiError>;
}

/// `frame` scaled down (aspect kept) to at most `max_pixels` pixels; as it
/// is when it is not larger.
pub fn fit_for_reading(frame: &RgbaImage, max_pixels: u64) -> Cow<'_, RgbaImage> {
    let (w, h) = frame.dimensions();
    let pixels = u64::from(w) * u64::from(h);
    if pixels <= max_pixels || pixels == 0 {
        return Cow::Borrowed(frame);
    }
    let scale = (max_pixels as f64 / pixels as f64).sqrt();
    let nw = ((f64::from(w) * scale).floor() as u32).max(1);
    let nh = ((f64::from(h) * scale).floor() as u32).max(1);
    Cow::Owned(image::imageops::resize(
        frame,
        nw,
        nh,
        image::imageops::FilterType::Triangle,
    ))
}

/// Read every frame (the picture, or the first, middle and last frames of
/// an animation, in that order) and clean what was found into one text.
/// `Ok("")`: nothing confident.
pub fn read_frames(reader: &dyn TextReader, frames: &[RgbaImage]) -> Result<String, AiError> {
    let mut found = Vec::with_capacity(frames.len());
    for frame in frames {
        found.push(reader.read(&fit_for_reading(frame, MAX_READ_PIXELS))?);
    }
    Ok(clean_lines(found, MIN_CONFIDENCE))
}

/// The rules every reader's output goes through (see the module docs):
/// `frames` holds the lines of each frame, in frame order.
pub fn clean_lines(frames: Vec<Vec<TextLine>>, min_confidence: f32) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for lines in frames {
        let kept: Vec<TextLine> = lines
            .into_iter()
            .filter(|l| l.confidence >= min_confidence)
            .filter_map(|mut l| {
                l.text = printable_line(&l.text)?;
                Some(l)
            })
            .collect();
        // A line already read in an earlier frame is not repeated; the same
        // line twice in one frame is (`HA` / `HA`).
        let mut this_frame = Vec::new();
        for line in rows(kept) {
            let key = dedup_key(&line);
            if !seen.contains(&key) {
                this_frame.push(key);
                out.push(line);
            }
        }
        seen.extend(this_frame);
    }
    truncate_text(&out.join("\n"))
}

/// One line as it is stored: [`clean_text`], private-use and replacement
/// characters removed, whitespace runs as one space; `None` when no letter
/// or digit is left and the line is no [`punctuation_line`]
/// (frames, borders and specks read as `|`, `_`, `~`, `•`, `-`).
fn printable_line(text: &str) -> Option<String> {
    let cleaned: String = clean_text(text)
        .chars()
        .filter(|c| !matches!(c, '\u{FFFD}' | '\u{E000}'..='\u{F8FF}'))
        .collect();
    let line = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    (line.chars().any(char::is_alphanumeric) || punctuation_line(&line)).then_some(line)
}

/// A line without a letter or digit that is still writing: a question or
/// an exclamation (`???`, `?!`, `!!!`, full-width `？` too) or an ellipsis
/// (`...`, `…`), which a sticker may say alone. Specks are not read as
/// those (docs/ai.md, "OCR and the text field").
fn punctuation_line(line: &str) -> bool {
    line.contains("...")
        || line.chars().any(|c| {
            matches!(
                c,
                '?' | '!' | '\u{00A1}' | '\u{00BF}' | '\u{2026}' | '\u{203C}' | '\u{203D}' | '\u{2047}'
                    ..='\u{2049}' | '\u{FE56}' | '\u{FE57}' | '\u{FF01}' | '\u{FF1F}'
            )
        })
}

/// Lines compare equal across frames when they differ only in case and spacing.
fn dedup_key(line: &str) -> String {
    line.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text of each row, top to bottom; the pieces of a row (a reader may
/// find a line as several boxes, word by word) left to right, joined with a
/// space. A piece joins the row when its vertical centre is less than half
/// the smaller height away from the centre of the row's first piece (the
/// rule the OCR evaluation in docs/ai.md scored). Without boxes every piece
/// is a row, in the reader's order.
fn rows(lines: Vec<TextLine>) -> Vec<String> {
    if lines.iter().any(|l| l.bounds.is_none()) {
        return lines.into_iter().map(|l| l.text).collect();
    }
    let mut items: Vec<(LineBox, String)> = lines
        .into_iter()
        .map(|l| (l.bounds.expect("checked"), l.text))
        .collect();
    items.sort_by(|p, q| p.0.centre_y().total_cmp(&q.0.centre_y()).then(p.0.x.total_cmp(&q.0.x)));
    let mut rows: Vec<(LineBox, Vec<(LineBox, String)>)> = Vec::new();
    for (b, text) in items {
        match rows.last_mut() {
            Some((first, row)) if (b.centre_y() - first.centre_y()).abs() < first.height.min(b.height) / 2.0 => {
                row.push((b, text));
            }
            _ => rows.push((b, vec![(b, text)])),
        }
    }
    rows.into_iter()
        .map(|(_, mut row)| {
            row.sort_by(|p, q| p.0.x.total_cmp(&q.0.x));
            row.into_iter().map(|(_, t)| t).collect::<Vec<_>>().join(" ")
        })
        .collect()
}

/// Model-free [`TextReader`] for tests: returns the same lines for every
/// frame and records the size of each frame it was given.
pub struct MockTextReader {
    model_id: String,
    lines: Vec<TextLine>,
    frames: Mutex<Vec<(u32, u32)>>,
}

impl MockTextReader {
    /// Model id of [`MockTextReader::new`].
    pub const MODEL_ID: &'static str = "mock-ocr";

    /// Reads `lines` (confidence 0.9) in every frame.
    pub fn new(lines: &[&str]) -> MockTextReader {
        MockTextReader::with_lines(Self::MODEL_ID, lines.iter().map(|l| TextLine::new(*l, 0.9)).collect())
    }

    pub fn with_lines(model_id: &str, lines: Vec<TextLine>) -> MockTextReader {
        MockTextReader {
            model_id: model_id.into(),
            lines,
            frames: Mutex::new(Vec::new()),
        }
    }

    /// (width, height) of every frame read so far, in order.
    pub fn frames(&self) -> Vec<(u32, u32)> {
        self.frames.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// How many frames were read so far.
    pub fn reads(&self) -> usize {
        self.frames.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

impl TextReader for MockTextReader {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn read(&self, frame: &RgbaImage) -> Result<Vec<TextLine>, AiError> {
        self.frames
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(frame.dimensions());
        Ok(self.lines.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, conf: f32, x: f32, y: f32) -> TextLine {
        TextLine::new(text, conf).at(x, y, 100.0, 20.0)
    }

    #[test]
    fn drops_unsure_and_unprintable_lines() {
        let got = clean_lines(
            vec![vec![
                TextLine::new("no thoughts", 0.9),
                TextLine::new("maybe", 0.3),
                TextLine::new("  |  ~ ", 0.99),
                TextLine::new("head\u{7}\tempty\u{FFFD}\u{E001}", 0.8),
            ]],
            MIN_CONFIDENCE,
        );
        assert_eq!(got, "no thoughts\nhead empty");
        assert_eq!(clean_lines(vec![vec![], vec![TextLine::new("x", 0.1)]], 0.5), "");
    }

    /// A sticker may say only "???" or "?!": such a line is writing.
    #[test]
    fn punctuation_only_lines_are_kept() {
        for line in [
            "???",
            "?!",
            "!?",
            "!!!",
            "!",
            "?",
            "...",
            "....",
            "\u{2026}",
            "? !",
            "\u{FF1F}\u{FF01}",
            "\u{00BF}?",
        ] {
            assert_eq!(printable_line(line).as_deref(), Some(line), "{line}");
        }
        assert_eq!(printable_line("  ?   ! ").as_deref(), Some("? !"));
        let got = clean_lines(
            vec![vec![
                line("???", 0.99, 0.0, 0.0),
                line("|", 0.99, 0.0, 100.0),
                line("?!", 0.3, 0.0, 200.0),
            ]],
            MIN_CONFIDENCE,
        );
        assert_eq!(got, "???", "an unsure one still goes");
    }

    /// Frames, borders, lines and specks, as the reader read them in the
    /// rendered pictures of the punctuation round (`•` at 0.99, `—`, `–`,
    /// `_`, `□□`) and as it may (`|`, `~`, `.`, `..`).
    #[test]
    fn speck_lines_are_dropped() {
        for speck in [
            "|",
            "_",
            "~",
            "\u{2022}",
            "\u{2022} \u{2022} \u{2022}",
            "-",
            "\u{2013}",
            "\u{2014}",
            "\u{2212}",
            ".",
            "..",
            ". . .",
            ",",
            "'",
            "`",
            "\u{25A1}\u{25A1}",
            "*",
            "/",
            ":",
            ";",
            "\u{00B0}",
            "^",
            "=",
        ] {
            assert_eq!(printable_line(speck), None, "{speck}");
        }
        let got = clean_lines(
            vec![vec![
                line("\u{2022}", 0.99, 0.0, 0.0),
                line("\u{2013}", 0.67, 0.0, 100.0),
                line("\u{2022}", 0.99, 0.0, 200.0),
            ]],
            MIN_CONFIDENCE,
        );
        assert_eq!(got, "");
    }

    #[test]
    fn reading_order_is_rows_then_columns() {
        let got = clean_lines(
            vec![vec![
                line("bottom", 0.9, 0.0, 200.0),
                line("right", 0.9, 300.0, 4.0),
                line("left", 0.9, 10.0, 0.0),
                line("middle", 0.9, 50.0, 100.0),
            ]],
            0.5,
        );
        assert_eq!(got, "left right\nmiddle\nbottom");
        // Word boxes of one caption make one line; a smaller box whose
        // centre is outside half its height starts its own row.
        let got = clean_lines(
            vec![vec![
                TextLine::new("UKRAINE", 0.9).at(220.0, 12.0, 90.0, 40.0),
                TextLine::new("GLORY", 0.9).at(0.0, 10.0, 90.0, 40.0),
                TextLine::new("TO", 0.9).at(110.0, 14.0, 40.0, 36.0),
                TextLine::new("small print", 0.9).at(0.0, 42.0, 60.0, 10.0),
            ]],
            0.5,
        );
        assert_eq!(got, "GLORY TO UKRAINE\nsmall print");
        // Without boxes the reader's order stays.
        let got = clean_lines(vec![vec![TextLine::new("b", 0.9), TextLine::new("a", 0.9)]], 0.5);
        assert_eq!(got, "b\na");
    }

    #[test]
    fn frames_merge_without_repeating_lines() {
        let frame = |lines: &[&str]| lines.iter().map(|l| TextLine::new(*l, 0.9)).collect::<Vec<_>>();
        let got = clean_lines(
            vec![
                frame(&["WHEN THE", "code works"]),
                frame(&["when  the", "HA", "HA"]),
                frame(&["code works", "first try"]),
            ],
            0.5,
        );
        assert_eq!(got, "WHEN THE\ncode works\nHA\nHA\nfirst try");
    }

    #[test]
    fn caps_the_text() {
        let long = "w".repeat(300);
        let lines: Vec<TextLine> = (0..10).map(|i| TextLine::new(format!("{i}{long}"), 0.9)).collect();
        let got = clean_lines(vec![lines], 0.5);
        assert_eq!(got.chars().count(), stickpile_api::text::MAX_TEXT_CHARS);
    }

    #[test]
    fn large_frames_are_scaled_down() {
        let big = RgbaImage::new(4000, 1000);
        let fit = fit_for_reading(&big, MAX_READ_PIXELS);
        let (w, h) = fit.dimensions();
        assert!(u64::from(w) * u64::from(h) <= MAX_READ_PIXELS);
        assert_eq!((w as f32 / h as f32).round(), 4.0, "aspect kept: {w}x{h}");
        let small = RgbaImage::new(300, 200);
        assert!(matches!(fit_for_reading(&small, MAX_READ_PIXELS), Cow::Borrowed(_)));
    }

    #[test]
    fn mock_reads_every_frame_once() {
        let mock = MockTextReader::new(&["Кіт", "Кіт"]);
        let frames = [
            RgbaImage::new(10, 10),
            RgbaImage::new(3000, 3000),
            RgbaImage::new(10, 10),
        ];
        let text = read_frames(&mock, &frames).unwrap();
        assert_eq!(
            text, "Кіт\nКіт",
            "a line twice in one frame stays, later frames add nothing"
        );
        assert_eq!(mock.reads(), 3);
        assert_eq!(mock.frames()[1], (2048, 2048), "bounded");
        assert_eq!(mock.model_id(), MockTextReader::MODEL_ID);
    }
}
