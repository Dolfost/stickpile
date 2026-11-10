//! Text normalisation, tokenisation and string distances.

use unicode_normalization::char::{decompose_compatible, is_combining_mark};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// Lowercase, NFKD, strip combining marks. See [`crate::normalize`].
pub(crate) fn normalize(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(text.len());
    for c in text.nfc() {
        if keeps_marks(c) {
            out.extend(c.to_lowercase());
            continue;
        }
        decompose_compatible(c, |d| {
            if is_combining_mark(d) || is_variation_selector(d) {
                return;
            }
            for l in d.to_lowercase() {
                if !is_combining_mark(l) {
                    out.push(l);
                }
            }
        });
    }
    out
}

/// Letters whose mark makes a different letter rather than an accent:
/// Cyrillic й ї ў ќ ѓ (ё still folds to е), and kana voicing marks.
fn keeps_marks(c: char) -> bool {
    matches!(
        c,
        'й' | 'Й' | 'ї' | 'Ї' | 'ў' | 'Ў' | 'ќ' | 'Ќ' | 'ѓ' | 'Ѓ' | '\u{3040}'..='\u{30FF}'
    )
}

/// U+FE00..U+FE0F (VS1..VS16, includes the emoji/text presentation selectors).
pub(crate) fn is_variation_selector(c: char) -> bool {
    matches!(c, '\u{FE00}'..='\u{FE0F}')
}

/// Fitzpatrick skin tone modifiers.
pub(crate) fn is_skin_tone(c: char) -> bool {
    matches!(c, '\u{1F3FB}'..='\u{1F3FF}')
}

/// Rough `Extended_Pictographic` test: enough to tell emoji segments from punctuation.
pub(crate) fn is_pictographic(c: char) -> bool {
    matches!(
        c as u32,
        0x1F000..=0x1FAFF
            | 0x2600..=0x27BF
            | 0x2300..=0x23FF
            | 0x2B00..=0x2BFF
            | 0x2194..=0x2199
            | 0x21A9..=0x21AA
            | 0x25AA..=0x25FE
            | 0x2934..=0x2935
            | 0x3030
            | 0x303D
            | 0x3297
            | 0x3299
            | 0x00A9
            | 0x00AE
            | 0x203C
            | 0x2049
            | 0x2122
            | 0x2139
            | 0x24C2
    )
}

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '\u{02BC}' | '`')
}

/// Separators that UAX #29 keeps inside a word (`my_cats`, `file.png`, `a:b`).
fn is_inner_separator(c: char) -> bool {
    matches!(
        c,
        '_' | '.' | ':' | '\u{00B7}' | '\u{2027}' | '\u{FE13}' | '\u{FF0E}' | '\u{FF1A}'
    )
}

/// Collapse runs of the same letter. See [`crate::squash`].
pub(crate) fn squash(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut prev = None;
    for c in word.chars() {
        if prev == Some(c) && c.is_alphabetic() {
            continue;
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// Key of the squashed side index: [`squash`], then repeated letter pairs
/// collapsed too (`hahaha` -> `ha`, `lololol` -> `lol`), like the slang
/// matching of the vibe vocabulary in stickpile-ai.
pub(crate) fn squash_key(word: &str) -> String {
    let s: Vec<char> = squash(word).chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let n = out.len();
        let repeats = i + 1 < s.len()
            && n >= 2
            && (out[n - 2], out[n - 1]) == (s[i], s[i + 1])
            && s[i].is_alphabetic()
            && s[i + 1].is_alphabetic();
        if repeats {
            i += 2;
            continue;
        }
        out.push(s[i]);
        i += 1;
    }
    out.into_iter().collect()
}

/// One indexed or query token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub text: String,
    /// Contains letters or digits (eligible for prefix/squash/fuzzy/substring);
    /// false for emoji tokens, which only match exactly.
    pub word: bool,
}

/// Normalise and split into word and emoji tokens (UAX #29 word boundaries).
/// Emoji keep ZWJ sequences and flags whole; skin tones and variation
/// selectors are dropped. Apostrophes are removed (`can't` -> `cant`).
pub(crate) fn tokenize(text: &str) -> Vec<Token> {
    let norm = normalize(text);
    let mut out = Vec::new();
    for seg in norm.split_word_bounds() {
        if seg.chars().any(is_pictographic) {
            let e: String = seg
                .chars()
                .filter(|c| !is_skin_tone(*c) && !is_variation_selector(*c))
                .collect();
            if !e.is_empty() {
                out.push(Token { text: e, word: false });
            }
        } else if seg.chars().any(char::is_alphanumeric) {
            for part in seg.split(is_inner_separator) {
                let w: String = part.chars().filter(|c| !is_apostrophe(*c)).collect();
                if w.chars().any(char::is_alphanumeric) {
                    out.push(Token { text: w, word: true });
                }
            }
        }
    }
    out
}

/// Emoji comparison key for `e:` filters: variation selectors removed.
pub(crate) fn emoji_key(e: &str) -> String {
    e.trim().chars().filter(|c| !is_variation_selector(*c)).collect()
}

/// `emoji_key` with skin tones removed as well.
pub(crate) fn emoji_base(e: &str) -> String {
    e.chars()
        .filter(|c| !is_skin_tone(*c) && !is_variation_selector(*c))
        .collect()
}

const PAD: u32 = 0x1F_FFFF;

fn tri(a: u32, b: u32, c: u32) -> u64 {
    ((a as u64) << 42) | ((b as u64) << 21) | c as u64
}

/// True if a trigram key contains padding (it is not an interior trigram).
pub(crate) fn tri_is_padded(t: u64) -> bool {
    let m = (1u64 << 21) - 1;
    (t >> 42) & m == PAD as u64 || (t >> 21) & m == PAD as u64 || t & m == PAD as u64
}

/// Unique trigrams of `"$$" + word + "$$"` (sorted). A word of n chars has
/// n + 2 padded trigrams; one edit operation destroys at most 4 of them.
pub(crate) fn padded_trigrams(chars: &[char]) -> Vec<u64> {
    let mut v: Vec<u32> = Vec::with_capacity(chars.len() + 4);
    v.extend([PAD, PAD]);
    v.extend(chars.iter().map(|c| *c as u32));
    v.extend([PAD, PAD]);
    let mut out: Vec<u64> = v.windows(3).map(|w| tri(w[0], w[1], w[2])).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Optimal string alignment distance (Damerau-Levenshtein with adjacent
/// transpositions) if it is `<= max`, else `None`.
pub(crate) fn osa_within(a: &[char], b: &[char], max: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    const STACK: usize = 48;
    let m = b.len();
    if m < STACK {
        let mut rows = [[0usize; STACK]; 3];
        osa_rows(a, b, max, &mut rows)
    } else {
        let mut rows = [vec![0usize; m + 1], vec![0usize; m + 1], vec![0usize; m + 1]];
        osa_rows(a, b, max, &mut rows)
    }
}

/// Three DP rows (two back, one back, current), each at least `b.len() + 1` long.
fn osa_rows<R: AsMut<[usize]>>(a: &[char], b: &[char], max: usize, rows: &mut [R; 3]) -> Option<usize> {
    let m = b.len();
    let [r0, r1, r2] = rows;
    let (mut prev2, mut prev, mut cur) = (r0.as_mut(), r1.as_mut(), r2.as_mut());
    for (j, v) in prev.iter_mut().enumerate().take(m + 1) {
        *v = j;
    }
    for i in 1..=a.len() {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(prev2[j - 2] + 1);
            }
            cur[j] = v;
            row_min = row_min.min(v);
        }
        if row_min > max {
            return None;
        }
        (prev2, prev, cur) = (prev, cur, prev2);
    }
    Some(prev[m]).filter(|d| *d <= max)
}

/// Dot product with independent accumulators so it vectorises.
pub(crate) fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0f32; 8];
    let ca = a.chunks_exact(8);
    let cb = b.chunks_exact(8);
    let (ra, rb) = (ca.remainder(), cb.remainder());
    for (x, y) in ca.zip(cb) {
        for k in 0..8 {
            acc[k] += x[k] * y[k];
        }
    }
    let mut s: f32 = acc.iter().sum();
    for (x, y) in ra.iter().zip(rb) {
        s += x * y;
    }
    s
}
