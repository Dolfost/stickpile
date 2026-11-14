//! No-reference image quality score, 0..=100 with a per-component breakdown
//! (docs/quality.md): resolution vs recommended size, JPEG quantisation
//! estimate, recompression (generation) detection, blockiness, ringing,
//! sharpness (Laplacian variance), alpha presence, frame rate and palette size
//! for animations.
//!
//! The total is the weighted mean of the components that apply to the item.
//! All measurements are heuristics on the decoded pixels (plus the JPEG
//! quantisation tables); none of them needs a reference image or a model.

use std::borrow::Cow;
use std::collections::HashSet;

use image::RgbaImage;
use stickpile_api::{MediaFormat, MediaKind, QualityComponent, QualityReport};

use crate::grey::{dct2, dct_matrix, downsample2, luma_on_white, resize, Grey};
use crate::jpeg::{self, QuantTable, ZIGZAG_TO_NATURAL};
use crate::Probe;

/// Component weights (docs/quality.md).
pub const W_RESOLUTION: f32 = 2.0;
pub const W_JPEG_QUALITY: f32 = 2.0;
pub const W_RECOMPRESSION: f32 = 1.0;
pub const W_BLOCKINESS: f32 = 1.0;
pub const W_RINGING: f32 = 0.5;
pub const W_SHARPNESS: f32 = 1.5;
pub const W_ALPHA: f32 = 0.5;
pub const W_FRAME_RATE: f32 = 1.0;
pub const W_PALETTE: f32 = 0.5;

/// Used when the set has no recommended size.
pub const DEFAULT_RECOMMENDED_PX: u32 = 512;

/// Larger frames are analysed on a central crop of this size (blockiness,
/// ringing, recompression); the crop offset keeps the 8x8 grid phase.
const ANALYSIS_SIDE: u32 = 2048;

/// `input` is the file as uploaded (needed for JPEG quantisation tables);
/// `frame` is the representative decoded frame.
pub fn assess(input: &[u8], probe: &Probe, frame: &image::RgbaImage, recommended_px: u32) -> QualityReport {
    let rec = if recommended_px == 0 {
        DEFAULT_RECOMMENDED_PX
    } else {
        recommended_px
    };
    if frame.width() == 0 || frame.height() == 0 {
        return QualityReport {
            score: 0,
            components: Vec::new(),
        };
    }
    let is_jpeg = probe.format == MediaFormat::Jpeg;
    let table = if is_jpeg { jpeg::luma_table(input) } else { None };
    // JPEG analysis runs on the stored pixel grid, before EXIF orientation.
    let luma = if is_jpeg {
        luma_on_white(&unorient(frame, input))
    } else {
        luma_on_white(frame)
    };
    let crop = analysis_crop(&luma);
    let grid = GridProfile::measure(&crop);

    let mut c = Vec::new();
    c.push(resolution(&luma, rec));
    if let Some(t) = &table {
        c.push(jpeg_quality(t));
    }
    c.extend(recompression(probe, input, &crop, table.as_ref(), grid.as_ref()));
    c.extend(grid.as_ref().map(blockiness));
    c.extend(ringing(&crop));
    c.extend(sharpness(&luma));
    c.extend(alpha(frame));
    if matches!(probe.kind, MediaKind::Animation | MediaKind::Video) {
        c.extend(probe.fps.map(frame_rate));
    }
    if probe.kind == MediaKind::Animation {
        c.push(palette(frame));
    }
    QualityReport {
        score: total(&c),
        components: c,
    }
}

/// Weighted mean of the component scores, rounded.
pub fn total(components: &[QualityComponent]) -> u8 {
    let (sum, w) = components.iter().fold((0f64, 0f64), |(s, w), c| {
        (s + f64::from(c.score) * f64::from(c.weight), w + f64::from(c.weight))
    });
    if w <= 0.0 {
        0
    } else {
        (sum / w).round().clamp(0.0, 100.0) as u8
    }
}

/// Estimated IJG quality (1..=100) from a JPEG's luminance quantisation table.
pub fn jpeg_quality_estimate(jpeg: &[u8]) -> Option<u8> {
    jpeg::luma_table(jpeg).map(|t| estimate_ijg(&t).quality)
}

const UPSCALED: &str = "{long} px longest side, looks upscaled from about {eff} px (recommended {rec} px)";
const RESOLUTION: &str = "{long} px longest side (recommended {rec} px)";
const JPEG_GUESS: &str = "estimated JPEG q≈{q} (non-standard tables, closest IJG match)";
const JPEG: &str = "estimated JPEG q={q}";
const NO_SEAMS: &str = "no visible 8x8 block seams";
const SEAMS_SHIFTED: &str = "8x8 block seams of {level} grey levels (grid offset {x},{y})";
const SEAMS: &str = "8x8 block seams of {level} grey levels";
const EARLIER_JPEG: &str = "coefficients fit an earlier JPEG at q≈{first} (now q={q})";
const GRID_SHIFTED: &str = "block grid shifted by {phase} px in {dir} (cropped after an earlier compression)";
const SEAMS_DESPITE: &str = "{level} grey-level block seams despite q={q}";
const RECOMPRESSED: &str = "likely recompressed: {why}";
const NOT_RECOMPRESSED: &str = "no sign of an earlier JPEG generation";
const LOSSLESS_FROM_JPEG: &str = "lossless file with an 8x8 JPEG grid (offset {x},{y}): saved from a JPEG";
const RINGING: &str = "oscillation index {r} next to strong edges";
const SHARPNESS: &str = "Laplacian variance {v} on edges at ≤{side} px";
const ALPHA: &str = "{pct}% transparent (cut-out sticker)";
const MANY_COLOURS: &str = "more than 4096 colours in the first frame";
const COLOURS: &str = "{n} colours in the first frame";
const FRAME_RATE: &str = "{fps} fps";

/// Every [`QualityComponent::detail`], English with `{name}` placeholders,
/// in the order to match them: the web UI translates the stored text with
/// `stickpile_i18n::t_template`.
pub const DETAILS: &[&str] = &[
    UPSCALED,
    RESOLUTION,
    JPEG_GUESS,
    JPEG,
    NO_SEAMS,
    SEAMS_SHIFTED,
    SEAMS,
    EARLIER_JPEG,
    GRID_SHIFTED,
    SEAMS_DESPITE,
    RECOMPRESSED,
    NOT_RECOMPRESSED,
    LOSSLESS_FROM_JPEG,
    RINGING,
    SHARPNESS,
    ALPHA,
    MANY_COLOURS,
    COLOURS,
    FRAME_RATE,
];

/// `template` (one of [`DETAILS`]) with its placeholders filled.
fn detail(template: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    args.iter().fold(template.to_string(), |s, (name, value)| {
        s.replace(&format!("{{{name}}}"), &value.to_string())
    })
}

fn component(name: &str, score: f64, weight: f32, detail: String) -> QualityComponent {
    QualityComponent {
        name: name.into(),
        score: score.round().clamp(0.0, 100.0) as u8,
        weight,
        detail,
    }
}

/// Linear ramp: 0 at `lo`, 1 at `hi` (clamped).
fn ramp(v: f64, lo: f64, hi: f64) -> f64 {
    ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
}

fn unorient<'a>(frame: &'a RgbaImage, input: &[u8]) -> Cow<'a, RgbaImage> {
    use image::metadata::Orientation::*;
    let o = jpeg::parse(input)
        .ok()
        .and_then(|p| jpeg::exif_orientation(&p))
        .unwrap_or(1);
    let Some(o) = image::metadata::Orientation::from_exif(o) else {
        return Cow::Borrowed(frame);
    };
    let inverse = match o {
        NoTransforms => return Cow::Borrowed(frame),
        Rotate90 => Rotate270,
        Rotate270 => Rotate90,
        other => other,
    };
    let mut img = image::DynamicImage::ImageRgba8(frame.clone());
    img.apply_orientation(inverse);
    Cow::Owned(img.to_rgba8())
}

fn analysis_crop(g: &Grey) -> Cow<'_, Grey> {
    let (w, h) = g.dimensions();
    if w <= ANALYSIS_SIDE && h <= ANALYSIS_SIDE {
        return Cow::Borrowed(g);
    }
    let cw = w.min(ANALYSIS_SIDE);
    let ch = h.min(ANALYSIS_SIDE);
    let x0 = ((w - cw) / 2) & !7;
    let y0 = ((h - ch) / 2) & !7;
    Cow::Owned(image::imageops::crop_imm(g, x0, y0, cw, ch).to_image())
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Energy ratio (octave / next coarser octave) below which an octave counts
/// as empty, per octave from the finest. Natural images measure about
/// 0.7-0.9, 0.5-0.65 and 0.35-0.45; content upscaled by a smooth filter
/// 0.05-0.2.
const EMPTY_OCTAVE_RATIO: [f64; 3] = [0.3, 0.25, 0.15];
/// Mean band energy per tile below which an octave has no meaningful content.
const FLAT_ENERGY: f64 = 50.0;
const TILE: usize = 32;
const MAX_TILES: usize = 400;

/// AC energy per 32x32 DCT tile in four octaves of spatial frequency:
/// `[0]` = finest (index 16..32, i.e. periods of 2..4 px), `[1]` = 8..16,
/// `[2]` = 4..8, `[3]` = 2..4.
pub(crate) fn octave_energies(g: &Grey) -> Option<[f64; 4]> {
    let (w, h) = (g.width() as usize, g.height() as usize);
    let (tw, th) = (w / TILE, h / TILE);
    if tw * th == 0 {
        return None;
    }
    let m = dct_matrix::<TILE>();
    let stride = ((tw * th) / MAX_TILES).max(1);
    let px = g.as_raw();
    let mut e = [0f64; 4];
    let mut n = 0u32;
    for (i, (tx, ty)) in (0..th).flat_map(|y| (0..tw).map(move |x| (x, y))).enumerate() {
        if i % stride != 0 {
            continue;
        }
        let mut block = [[0f32; TILE]; TILE];
        for (y, row) in block.iter_mut().enumerate() {
            let off = (ty * TILE + y) * w + tx * TILE;
            row.copy_from_slice(&px[off..off + TILE]);
        }
        let c = dct2(&m, &block);
        for (v, row) in c.iter().enumerate() {
            for (u, coef) in row.iter().enumerate() {
                let f = u.max(v);
                let band = match f {
                    16.. => 0,
                    8..=15 => 1,
                    4..=7 => 2,
                    2..=3 => 3,
                    _ => continue,
                };
                e[band] += f64::from(*coef) * f64::from(*coef);
            }
        }
        n += 1;
    }
    Some(e.map(|v| v / f64::from(n)))
}

/// Longest side, and the effective resolution: the longest side divided by
/// 2^(number of leading near-empty octaves). An image upscaled by 2^k has
/// (almost) no energy in its k finest octaves.
pub(crate) fn effective_resolution(luma: &Grey, rec: u32) -> (u32, u32, bool) {
    let long = luma.width().max(luma.height());
    // Octaves finer than what the recommended size needs cannot change the
    // score; drop them first (keeps big images cheap).
    let mut level = Cow::Borrowed(luma);
    let mut skipped = 0u32;
    while (long >> (skipped + 1)) >= rec && level.width().min(level.height()) >= 64 {
        level = Cow::Owned(downsample2(&level));
        skipped += 1;
    }
    let Some(e) = octave_energies(&level) else {
        return (long, long, false);
    };
    let mut empty = 0usize;
    while empty < 3 && e[empty + 1] > FLAT_ENERGY && e[empty] < EMPTY_OCTAVE_RATIO[empty] * e[empty + 1] {
        empty += 1;
    }
    if empty == 0 {
        (long, long, false)
    } else {
        (long, (long >> (skipped as usize + empty)).max(1), true)
    }
}

fn resolution(luma: &Grey, rec: u32) -> QualityComponent {
    let (long, eff, upscaled) = effective_resolution(luma, rec);
    let score = 100.0 * (f64::from(eff) / f64::from(rec)).min(1.0);
    let detail = if upscaled {
        detail(UPSCALED, &[("long", &long), ("eff", &eff), ("rec", &rec)])
    } else {
        detail(RESOLUTION, &[("long", &long), ("rec", &rec)])
    };
    component("resolution", score, W_RESOLUTION, detail)
}

// ---------------------------------------------------------------------------
// JPEG quantisation
// ---------------------------------------------------------------------------

/// Annex K.1 luminance table, natural order.
const STD_LUMA: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51,
    87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101,
    72, 92, 95, 98, 112, 100, 103, 99,
];

/// The luminance table libjpeg produces for quality `q` (baseline clamp).
pub(crate) fn ijg_table(q: u32) -> [u16; 64] {
    let q = q.clamp(1, 100);
    let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };
    STD_LUMA.map(|v| ((u32::from(v) * scale + 50) / 100).clamp(1, 255) as u16)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct IjgEstimate {
    pub quality: u8,
    /// Mean relative deviation from the best-matching IJG table (0 = exact).
    pub misfit: f64,
}

/// Best-fitting IJG quality. Tables are compared in natural order; a table
/// whose stored order fits better (encoders that forget the zig-zag) is
/// accepted too.
pub(crate) fn estimate_ijg(t: &QuantTable) -> IjgEstimate {
    let mut best = IjgEstimate {
        quality: 50,
        misfit: f64::INFINITY,
    };
    for cand in [&t.natural, &t.stored] {
        for q in 1..=100u32 {
            let std = ijg_table(q);
            let (diff, sum) = cand.iter().zip(std.iter()).fold((0f64, 0f64), |(d, s), (a, b)| {
                (d + (f64::from(*a.min(&255)) - f64::from(*b)).abs(), s + f64::from(*b))
            });
            let misfit = diff / sum;
            if misfit <= best.misfit {
                best = IjgEstimate {
                    quality: q as u8,
                    misfit,
                };
            }
        }
    }
    best
}

fn jpeg_quality(t: &QuantTable) -> QualityComponent {
    let est = estimate_ijg(t);
    let score = 100.0 * ramp(f64::from(est.quality), 10.0, 90.0);
    let detail = if est.misfit > 0.1 {
        detail(JPEG_GUESS, &[("q", &est.quality)])
    } else {
        detail(JPEG, &[("q", &est.quality)])
    };
    component("jpeg_quality", score, W_JPEG_QUALITY, detail)
}

// ---------------------------------------------------------------------------
// 8x8 grid profile (blockiness, misaligned grids)
// ---------------------------------------------------------------------------

/// Differences above this are treated as image edges, not block seams.
const SEAM_CLIP: f32 = 24.0;

/// Mean clipped neighbour difference across column (`cols[m]`, boundary at
/// x = 8k + m) and row boundaries, by phase modulo 8.
#[derive(Debug, Clone)]
pub(crate) struct GridProfile {
    cols: [f64; 8],
    rows: [f64; 8],
}

impl GridProfile {
    pub(crate) fn measure(g: &Grey) -> Option<GridProfile> {
        let (w, h) = (g.width() as usize, g.height() as usize);
        if w < 32 || h < 32 {
            return None;
        }
        let px = g.as_raw();
        let mut cols = [0f64; 8];
        let mut rows = [0f64; 8];
        let mut ncols = [0u64; 8];
        let mut nrows = [0u64; 8];
        for y in 0..h {
            let row = &px[y * w..(y + 1) * w];
            for x in 1..w {
                cols[x % 8] += f64::from((row[x] - row[x - 1]).abs().min(SEAM_CLIP));
                ncols[x % 8] += 1;
            }
            if y > 0 {
                let prev = &px[(y - 1) * w..y * w];
                let s: f64 = row
                    .iter()
                    .zip(prev)
                    .map(|(a, b)| f64::from((a - b).abs().min(SEAM_CLIP)))
                    .sum();
                rows[y % 8] += s;
                nrows[y % 8] += w as u64;
            }
        }
        for m in 0..8 {
            cols[m] /= ncols[m].max(1) as f64;
            rows[m] /= nrows[m].max(1) as f64;
        }
        Some(GridProfile { cols, rows })
    }

    /// Seam strength at phase (mx, my) relative to the other phases
    /// (1.0 = no grid).
    pub(crate) fn ratio(&self, mx: usize, my: usize) -> f64 {
        let other = |a: &[f64; 8], m: usize| (a.iter().sum::<f64>() - a[m]) / 7.0;
        (self.cols[mx] + self.rows[my]) / (other(&self.cols, mx) + other(&self.rows, my) + 0.25)
    }

    /// Mean seam step above the other phases, in grey levels.
    pub(crate) fn excess(&self, mx: usize, my: usize) -> f64 {
        let other = |a: &[f64; 8], m: usize| (a.iter().sum::<f64>() - a[m]) / 7.0;
        ((self.cols[mx] - other(&self.cols, mx)) + (self.rows[my] - other(&self.rows, my))) / 2.0
    }

    /// The strongest seam phase on one axis that is not the aligned one:
    /// `(axis, phase, ratio, excess)` with axis 0 = columns, 1 = rows.
    pub(crate) fn misaligned(&self) -> Option<(usize, usize, f64, f64)> {
        let mut best: Option<(usize, usize, f64, f64)> = None;
        for (axis, a) in [&self.cols, &self.rows].into_iter().enumerate() {
            let m = (0..8).max_by(|&i, &j| a[i].total_cmp(&a[j])).unwrap_or(0);
            if m == 0 || a[m] <= a[0] * 1.1 {
                continue;
            }
            let other = (a.iter().sum::<f64>() - a[m]) / 7.0;
            let cand = (axis, m, a[m] / (other + 0.125), a[m] - other);
            if best.is_none_or(|b| cand.2 > b.2) {
                best = Some(cand);
            }
        }
        best
    }

    /// The strongest phase and its ratio.
    pub(crate) fn strongest(&self) -> ((usize, usize), f64) {
        let argmax = |a: &[f64; 8]| (0..8).max_by(|&i, &j| a[i].total_cmp(&a[j])).unwrap_or(0);
        let p = (argmax(&self.cols), argmax(&self.rows));
        (p, self.ratio(p.0, p.1))
    }
}

/// Visible blocking (0..1): the seams must stand out relative to the texture
/// (ratio) *and* in absolute grey levels (excess), so neither flat
/// backgrounds (tiny but relatively large seams) nor busy textures count.
pub(crate) fn block_level(ratio: f64, excess: f64) -> f64 {
    ramp(ratio, 1.15, 2.5).min(ramp(excess, 0.3, 2.5))
}

fn blockiness(grid: &GridProfile) -> QualityComponent {
    let aligned = (grid.ratio(0, 0), grid.excess(0, 0));
    let (phase, best) = grid.strongest();
    let shifted = (best, grid.excess(phase.0, phase.1));
    let (a, s) = (block_level(aligned.0, aligned.1), block_level(shifted.0, shifted.1));
    let b = a.max(s);
    let detail = if b <= 0.0 {
        NO_SEAMS.to_string()
    } else if s > a {
        detail(
            SEAMS_SHIFTED,
            &[
                ("level", &format!("{:.1}", shifted.1)),
                ("x", &phase.0),
                ("y", &phase.1),
            ],
        )
    } else {
        detail(SEAMS, &[("level", &format!("{:.1}", aligned.1))])
    };
    component("blockiness", 100.0 * (1.0 - b), W_BLOCKINESS, detail)
}

// ---------------------------------------------------------------------------
// Recompression
// ---------------------------------------------------------------------------

/// Number of zig-zag AC positions examined for double quantisation.
const DQ_POSITIONS: usize = 20;
const DQ_MAX_BLOCKS: usize = 20_000;
const DQ_BINS: i32 = 40;

/// Histograms of the quantised DCT coefficients `round(F / Q2)` of the
/// first `DQ_POSITIONS` zig-zag AC positions, recomputed from the decoded
/// luma on the aligned 8x8 grid. Blocks touching 0/255 are skipped (clipping
/// distorts their coefficients). Index `DQ_BINS` is k = 0.
fn coefficient_histograms(g: &Grey, q2: &[u16; 64]) -> Option<Vec<Vec<u32>>> {
    let (bw, bh) = (g.width() as usize / 8, g.height() as usize / 8);
    if bw * bh < 64 {
        return None;
    }
    let m = dct_matrix::<8>();
    let stride = ((bw * bh) / DQ_MAX_BLOCKS).max(1);
    let size = (2 * DQ_BINS + 1) as usize;
    let mut hist = vec![vec![0u32; size]; DQ_POSITIONS];
    let px = g.as_raw();
    let w = g.width() as usize;
    for (i, (bx, by)) in (0..bh).flat_map(|y| (0..bw).map(move |x| (x, y))).enumerate() {
        if i % stride != 0 {
            continue;
        }
        let mut block = [[0f32; 8]; 8];
        let mut clipped = false;
        for (y, row) in block.iter_mut().enumerate() {
            for (x, v) in row.iter_mut().enumerate() {
                let p = px[(by * 8 + y) * w + bx * 8 + x];
                clipped |= !(1.5..=253.5).contains(&p);
                *v = p - 128.0;
            }
        }
        if clipped {
            continue;
        }
        let c = dct2(&m, &block);
        for (pos, h) in hist.iter_mut().enumerate() {
            let nat = ZIGZAG_TO_NATURAL[pos + 1];
            let q = f32::from(q2[nat].max(1));
            let k = (c[nat / 8][nat % 8] / q).round() as i32;
            if k.abs() <= DQ_BINS {
                h[(k + DQ_BINS) as usize] += 1;
            }
        }
    }
    Some(hist)
}

/// Result of the double-quantisation test.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DoubleQuantisation {
    /// 0..1: how much better than chance the earlier-quantiser lattice
    /// explains the coefficient histograms.
    pub evidence: f64,
    /// Best-fitting IJG quality of the earlier compression.
    pub first_quality: u8,
}

/// Double-quantisation test.
///
/// Quantising with step Q1, decoding and quantising again with Q2 leaves
/// only the values `round(Q1 * k / Q2)` populated (plus/minus decoder noise)
/// when Q1 > Q2: periodic gaps in the histogram. For every candidate earlier
/// IJG quality q1 whose lattice is sparse, we measure the share of the
/// non-zero coefficient mass that lands on the lattice and compare it with
/// the share a locally smooth histogram would put there (chance). Content
/// with a few distinct edge contrasts makes histograms bumpy, but it does
/// not put nearly all of the mass on one regular lattice, and it keeps the
/// +-1 bins (the most populated in any single compression) filled.
pub(crate) fn double_quantisation(g: &Grey, q2: &[u16; 64]) -> Option<DoubleQuantisation> {
    let hist = coefficient_histograms(g, q2)?;
    let c = DQ_BINS as usize;
    // Folded |k| histograms (k >= 1) and their smoothed versions.
    let folded: Vec<Vec<f64>> = hist
        .iter()
        .map(|h| (1..=c).map(|k| f64::from(h[c + k]) + f64::from(h[c - k])).collect())
        .collect();
    let total: f64 = folded.iter().flatten().sum();
    if total < 200.0 {
        return None;
    }
    let smooth: Vec<Vec<f64>> = folded
        .iter()
        .map(|f| {
            (0..f.len())
                .map(|i| {
                    let lo = i.saturating_sub(3);
                    let hi = (i + 4).min(f.len());
                    f[lo..hi].iter().sum::<f64>() / (hi - lo) as f64
                })
                .collect()
        })
        .collect();
    let mut best = DoubleQuantisation {
        evidence: 0.0,
        first_quality: 0,
    };
    for q1 in 1..=100u32 {
        let t1 = ijg_table(q1);
        let (mut on, mut chance, mut chance_total) = (0f64, 0f64, 0f64);
        for (pos, (f, sm)) in folded.iter().zip(&smooth).enumerate() {
            let nat = ZIGZAG_TO_NATURAL[pos + 1];
            let (a, b) = (f64::from(t1[nat]), f64::from(q2[nat].max(1)));
            let tol = if b <= 2.0 { 1 } else { 0 };
            let mut allowed = vec![a / b < 1.5; f.len()];
            if a / b >= 1.5 {
                let mut k1 = 1.0;
                loop {
                    let centre = (a * k1 / b).round() as i64;
                    if centre - tol > c as i64 {
                        break;
                    }
                    for k in (centre - tol).max(1)..=(centre + tol).min(c as i64) {
                        allowed[k as usize - 1] = true;
                    }
                    k1 += 1.0;
                }
            }
            for (i, ok) in allowed.iter().enumerate() {
                if *ok {
                    on += f[i];
                    chance += sm[i];
                }
                chance_total += sm[i];
            }
        }
        let chance = chance / chance_total.max(1e-9);
        if chance > 0.7 {
            continue;
        }
        let evidence = ((on / total) - chance) / (1.0 - chance);
        if evidence > best.evidence {
            best = DoubleQuantisation {
                evidence,
                first_quality: q1 as u8,
            };
        }
    }
    Some(best)
}

/// Lossless containers (PNG, GIF, lossless WebP) that show an 8x8 grid were
/// JPEG-compressed at some point.
fn lossless_container(probe: &Probe, input: &[u8]) -> bool {
    match probe.format {
        MediaFormat::Png | MediaFormat::Gif => true,
        MediaFormat::Webp => crate::webp::layout(input).is_some_and(|l| !l.lossy),
        _ => false,
    }
}

fn recompression(
    probe: &Probe,
    input: &[u8],
    g: &Grey,
    table: Option<&QuantTable>,
    grid: Option<&GridProfile>,
) -> Option<QualityComponent> {
    if let Some(t) = table {
        let est = estimate_ijg(t);
        let mut findings: Vec<(f64, String)> = Vec::new();
        if let Some(dq) = double_quantisation(g, &t.natural) {
            findings.push((
                ramp(dq.evidence, 0.1, 0.5),
                detail(EARLIER_JPEG, &[("first", &dq.first_quality), ("q", &est.quality)]),
            ));
        }
        if let Some(grid) = grid {
            let aligned = grid.ratio(0, 0);
            if let Some((axis, phase, ratio, excess)) = grid.misaligned() {
                let dir = if axis == 0 { "x" } else { "y" };
                findings.push((
                    ramp(ratio, 1.25, 1.75).min(ramp(excess, 0.5, 1.5)),
                    detail(GRID_SHIFTED, &[("phase", &phase), ("dir", &dir)]),
                ));
            }
            // A high-quality encode does not produce strong seams by itself.
            if est.quality >= 85 {
                let excess = grid.excess(0, 0);
                findings.push((
                    ramp(aligned, 2.2, 3.2).min(ramp(excess, 1.6, 2.6)),
                    detail(
                        SEAMS_DESPITE,
                        &[("level", &format!("{excess:.1}")), ("q", &est.quality)],
                    ),
                ));
            }
        }
        // Nothing measurable (tiny or flat image): no component.
        let (evidence, why) = findings.into_iter().max_by(|a, b| a.0.total_cmp(&b.0))?;
        let detail = if evidence > 0.0 {
            detail(RECOMPRESSED, &[("why", &why)])
        } else {
            NOT_RECOMPRESSED.into()
        };
        return Some(component(
            "recompression",
            100.0 * (1.0 - evidence),
            W_RECOMPRESSION,
            detail,
        ));
    }
    if !lossless_container(probe, input) {
        return None;
    }
    let grid = grid?;
    let (phase, best) = grid.strongest();
    let evidence = ramp(best, 1.3, 2.0).min(ramp(grid.excess(phase.0, phase.1), 0.5, 1.5));
    let detail = if evidence > 0.0 {
        detail(LOSSLESS_FROM_JPEG, &[("x", &phase.0), ("y", &phase.1)])
    } else {
        NOT_RECOMPRESSED.into()
    };
    Some(component(
        "recompression",
        100.0 * (1.0 - evidence),
        W_RECOMPRESSION,
        detail,
    ))
}

// ---------------------------------------------------------------------------
// Ringing
// ---------------------------------------------------------------------------

const EDGE_STRONG: f32 = 160.0;
const EDGE_WEAK: f32 = 60.0;

fn sobel(g: &Grey) -> Vec<f32> {
    let (w, h) = (g.width() as usize, g.height() as usize);
    let p = g.as_raw();
    let mut out = vec![0f32; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let at = |dx: isize, dy: isize| p[(y as isize + dy) as usize * w + (x as isize + dx) as usize];
            let gx = at(1, -1) + 2.0 * at(1, 0) + at(1, 1) - at(-1, -1) - 2.0 * at(-1, 0) - at(-1, 1);
            let gy = at(-1, 1) + 2.0 * at(0, 1) + at(1, 1) - at(-1, -1) - 2.0 * at(0, -1) - at(1, -1);
            out[y * w + x] = (gx * gx + gy * gy).sqrt();
        }
    }
    out
}

fn laplacian(g: &Grey) -> Vec<f32> {
    let (w, h) = (g.width() as usize, g.height() as usize);
    let p = g.as_raw();
    let mut out = vec![0f32; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            out[i] = 4.0 * p[i] - p[i - 1] - p[i + 1] - p[i - w] - p[i + w];
        }
    }
    out
}

/// Oscillation in a 3x3 neighbourhood: mean |L| - |mean L| of the
/// Laplacian. Zero on ramps and smooth curvature (L keeps its sign), positive
/// where the signal wiggles (ringing, mosquito noise, grain).
fn oscillation(lap: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0f32; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let (mut abs, mut sum) = (0f32, 0f32);
            for dy in 0..3 {
                let row = (y + dy - 1) * w + x - 1;
                for v in &lap[row..row + 3] {
                    abs += v.abs();
                    sum += v;
                }
            }
            out[y * w + x] = (abs - sum.abs()) / 9.0;
        }
    }
    out
}

/// Chebyshev dilation of a mask by `r` (separable running max).
fn dilate(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let mut tmp = vec![false; w * h];
    for y in 0..h {
        let row = &mask[y * w..(y + 1) * w];
        let mut last: Option<usize> = None;
        // Distance to the nearest set pixel on the left, then the right.
        for x in 0..w {
            if row[x] {
                last = Some(x);
            }
            tmp[y * w + x] = last.is_some_and(|l| x - l <= r);
        }
        let mut next: Option<usize> = None;
        for x in (0..w).rev() {
            if row[x] {
                next = Some(x);
            }
            tmp[y * w + x] |= next.is_some_and(|n| n - x <= r);
        }
    }
    let mut out = vec![false; w * h];
    for x in 0..w {
        let mut last: Option<usize> = None;
        for y in 0..h {
            if tmp[y * w + x] {
                last = Some(y);
            }
            out[y * w + x] = last.is_some_and(|l| y - l <= r);
        }
        let mut next: Option<usize> = None;
        for y in (0..h).rev() {
            if tmp[y * w + x] {
                next = Some(y);
            }
            out[y * w + x] |= next.is_some_and(|n| n - y <= r);
        }
    }
    out
}

/// Ringing level: mean oscillation in flat pixels 2..4 px from strong edges,
/// minus the same in flat pixels far from edges (texture/grain baseline).
pub(crate) fn ringing_level(g: &Grey) -> Option<f64> {
    let (w, h) = (g.width() as usize, g.height() as usize);
    if w < 16 || h < 16 {
        return None;
    }
    let grad = sobel(g);
    let lap = oscillation(&laplacian(g), w, h);
    let strong: Vec<bool> = grad.iter().map(|v| *v > EDGE_STRONG).collect();
    let core = dilate(&strong, w, h, 1);
    let near = dilate(&strong, w, h, 4);
    let wide = dilate(&strong, w, h, 8);
    let (mut band, mut nb, mut far, mut nf) = (0f64, 0u64, 0f64, 0u64);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            if grad[i] >= EDGE_WEAK {
                continue;
            }
            if near[i] && !core[i] {
                band += f64::from(lap[i]);
                nb += 1;
            } else if !wide[i] {
                far += f64::from(lap[i]);
                nf += 1;
            }
        }
    }
    if nb < 200 {
        return None;
    }
    let baseline = if nf >= 200 { far / nf as f64 } else { 0.0 };
    Some((band / nb as f64 - baseline).max(0.0))
}

fn ringing(g: &Grey) -> Option<QualityComponent> {
    let r = ringing_level(g)?;
    let score = 100.0 * (1.0 - ramp(r, 1.0, 8.0));
    Some(component(
        "ringing",
        score,
        W_RINGING,
        detail(RINGING, &[("r", &format!("{r:.1}"))]),
    ))
}

// ---------------------------------------------------------------------------
// Sharpness
// ---------------------------------------------------------------------------

/// Working size for the Laplacian (downscale only), so the measure does not
/// depend on the pixel count.
const SHARPNESS_SIDE: u32 = 512;
const ACTIVE_GRADIENT: f32 = 24.0;

/// Variance of the Laplacian over pixels with some gradient (flat
/// backgrounds of stickers are ignored), on the frame scaled to fit 512 px.
pub(crate) fn laplacian_variance(luma: &Grey) -> Option<f64> {
    let (w, h) = crate::variants::fit_within(luma.width(), luma.height(), SHARPNESS_SIDE);
    let g = resize(luma, w, h);
    let (w, h) = (w as usize, h as usize);
    if w < 8 || h < 8 {
        return None;
    }
    let grad = sobel(&g);
    let p = g.as_raw();
    let mut vals = Vec::new();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            if grad[i] > ACTIVE_GRADIENT {
                vals.push(f64::from(4.0 * p[i] - p[i - 1] - p[i + 1] - p[i - w] - p[i + w]));
            }
        }
    }
    if vals.len() < (w * h / 100).max(32) {
        return None;
    }
    let n = vals.len() as f64;
    let mean = vals.iter().sum::<f64>() / n;
    Some(vals.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n)
}

fn sharpness(luma: &Grey) -> Option<QualityComponent> {
    let v = laplacian_variance(luma)?;
    let score = 100.0 * ramp(v.max(1.0).log10(), 2.0, 3.6);
    Some(component(
        "sharpness",
        score,
        W_SHARPNESS,
        detail(SHARPNESS, &[("v", &format!("{v:.0}")), ("side", &SHARPNESS_SIDE)]),
    ))
}

// ---------------------------------------------------------------------------
// Alpha, frame rate, palette
// ---------------------------------------------------------------------------

/// Bonus only: present when the frame has real transparency (between 0.5 %
/// and 99.5 % of the pixels not opaque), absent otherwise.
fn alpha(frame: &RgbaImage) -> Option<QualityComponent> {
    let n = u64::from(frame.width()) * u64::from(frame.height());
    let clear = frame.pixels().filter(|p| p[3] < 250).count() as u64;
    let frac = clear as f64 / n as f64;
    (0.005..0.995).contains(&frac).then(|| {
        component(
            "alpha",
            100.0,
            W_ALPHA,
            detail(ALPHA, &[("pct", &format!("{:.0}", frac * 100.0))]),
        )
    })
}

fn frame_rate(fps: f32) -> QualityComponent {
    let score = 100.0 * (f64::from(fps) / 24.0).clamp(0.0, 1.0).powf(0.7);
    component(
        "frame_rate",
        score,
        W_FRAME_RATE,
        detail(FRAME_RATE, &[("fps", &format!("{fps:.1}"))]),
    )
}

fn palette(frame: &RgbaImage) -> QualityComponent {
    let mut seen = HashSet::new();
    for p in frame.pixels() {
        let key = if p[3] == 0 { 0 } else { u32::from_le_bytes(p.0) };
        seen.insert(key);
        if seen.len() > 4096 {
            break;
        }
    }
    let n = seen.len();
    let score = 100.0 * ((n.max(1) as f64).log2() / 8.0).clamp(0.0, 1.0);
    let detail = if n > 4096 {
        MANY_COLOURS.to_string()
    } else {
        detail(COLOURS, &[("n", &n)])
    };
    component("palette", score, W_PALETTE, detail)
}

#[cfg(test)]
mod tests;
