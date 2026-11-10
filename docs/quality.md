# Image quality score

Every media item gets a no-reference quality score from 0 to 100 at ingest
(`stickpile_media::quality::assess`), stored with a per-component breakdown
(`QualityReport { score, components }`). It drives the quality badge, the
`q>70` search filter and "find a better version" (same perceptual hash,
higher score).

The score answers a narrow question: *how much has this copy been degraded
compared with a good copy of the same picture?* It does not judge
aesthetics or content. All components are heuristics on the decoded pixels
(plus the JPEG quantisation tables); none needs a reference image or a
model.

## Total

`score = round(Σ weightᵢ · scoreᵢ / Σ weightᵢ)` over the components that
apply to the item. A component that cannot be measured (for example ringing
in an image without strong edges) is left out rather than guessed.

| Component | Weight | Applies to |
|---|---|---|
| `resolution` | 2.0 | all |
| `jpeg_quality` | 2.0 | JPEG |
| `recompression` | 1.0 | JPEG; PNG, GIF and lossless WebP (not lossy WebP or video) |
| `blockiness` | 1.0 | frames ≥ 32×32 |
| `ringing` | 0.5 | frames with strong edges |
| `sharpness` | 1.5 | frames with enough edge content |
| `alpha` | 0.5 | only when the frame has real transparency (bonus) |
| `frame_rate` | 1.0 | animations and videos |
| `palette` | 0.5 | animations |

JPEG analysis (quantisation, grid, recompression) runs on the pixel grid as
stored, before EXIF orientation is applied. Frames larger than 2048 px are
analysed for blockiness, ringing and recompression on a central 2048×2048
crop whose offset keeps the 8×8 grid phase. Grey values are BT.601 luma of
the frame composited on white.

## Components

### `resolution`

Longest side compared with the set's recommended size (`recommended_px`,
512 when the set has none): `100 · min(1, effective / recommended)`.

"Effective" detects upscaled copies, which have many pixels but little
detail. The frame is cut into 32×32 tiles (up to 400, evenly sampled) and a
2-D DCT measures the AC energy in four octaves of spatial frequency
(DCT index 16–31, 8–15, 4–7, 2–3). An octave counts as empty when its
energy is below 0.30 / 0.25 / 0.15 of the next coarser octave (natural
images measure about 0.7–0.9, 0.5–0.65 and 0.35–0.45; content upscaled by
a smooth filter 0.05–0.2). Each leading empty octave halves the effective
resolution. Octaves finer than the recommended size needs are skipped first
(box downsampling), which keeps large images cheap and cannot change the
score.

Limits: the granularity is one octave (a 1.33× upscale reads as 2×); blur
and upscaling look the same (both remove detail, so both lower the score);
nearest-neighbour upscales and noise added after upscaling hide the
upscale.

### `jpeg_quality`

The luminance quantisation table (the table used by the first frame
component) is compared with the IJG/libjpeg table for every quality 1–100,
scaled with the libjpeg formula (`scale = q < 50 ? 5000/q : 200 − 2q`,
`Q = clamp((std·scale + 50)/100, 1, 255)`). The best match by mean relative
deviation wins. DQT tables are stored in zig-zag order and converted to
natural order; a table whose stored order fits better (encoders that forgot
the zig-zag) is accepted as well. Tables that do not match any IJG scaling
(cameras, Photoshop) still get the closest IJG quality, marked
"non-standard tables" in the detail.

Score: linear from 0 at q ≤ 10 to 100 at q ≥ 90. `jpeg_quality_estimate()`
exposes the estimate; for IJG-style encoders it is exact.

### `recompression`

Detects an earlier lossy generation. For JPEG the strongest of three
signals is used, score `100 · (1 − evidence)`:

1. **Double quantisation.** The 8×8 DCT is recomputed from the decoded
   luma on the aligned grid (blocks touching 0 or 255 are skipped), and the
   first 20 zig-zag AC coefficients are divided by the file's quantisation
   steps Q2 into histograms. A single compression gives smooth histograms.
   Compressing with Q1 > Q2 first leaves only the values `round(Q1·k/Q2)`
   populated, with periodic empty bins. For each candidate earlier IJG
   quality q1 whose lattice is sparse, we compare the share of non-zero
   coefficient mass on the lattice with the share a locally smoothed
   histogram puts there (chance). Evidence ramps from 0.1 to 0.5 of
   "better than chance". The best q1 is reported ("coefficients fit an
   earlier JPEG at q≈50 (now q=90)"). On the test fixtures single
   compressions measure ≤ 0.002 and q50→q90, q30→q85, q60→q80 and q70→q95
   0.3–0.97.
2. **Shifted grid.** Seam strength (below) per phase of the 8-pixel grid on
   each axis. If the strongest phase is not the file's own grid (the image
   was cropped after an earlier compression), evidence ramps with the seam
   ratio 1.25–1.75 and the seam step 0.5–1.5 grey levels.
3. **Seams too strong for the quality.** At q ≥ 85 the file's own grid
   should be nearly invisible; seam ratio 2.2–3.2 and step 1.6–2.6 grey
   levels ramp the evidence (a low-quality copy re-saved at high quality).

For PNG, GIF and lossless WebP any visible 8×8 grid (ratio 1.3–2.0, step
0.5–1.5) means the pixels went through JPEG at some point ("saved from a
JPEG"), which is a generation loss even though the container is lossless.

Limits: a re-save at a *lower* or nearly equal quality (q90→q50, q75→q80)
leaves almost no trace and is not detected; non-IJG first compressions are
only found through the grid signals; heavy resizing between generations
erases the evidence (the resolution and sharpness components then carry
the loss).

### `blockiness`

For every column boundary and row boundary the mean absolute neighbour
difference is collected by phase modulo 8 (differences clipped at 24 grey
levels so real edges do not dominate). The seam *ratio* is the boundary
phase against the mean of the other seven, the seam *step* the difference
in grey levels. Blocking is visible only when both are high:
`level = min(ramp(ratio, 1.15, 2.5), ramp(step, 0.3, 2.5))`, score
`100 · (1 − level)`, using the aligned grid or the strongest shifted one,
whichever is worse.

Limits: content with an 8-pixel period (pixel art, some patterns) reads as
blocking; video codecs with other block sizes are only partly captured.

### `ringing`

Oscillation next to strong edges (mosquito noise, Gibbs ringing). Strong
edges are Sobel magnitude > 160 (a step of about 40 grey levels). For
flat-ish pixels (Sobel < 60) 2–4 px from a strong edge we take the 3×3
mean of |Laplacian| minus |mean Laplacian|, which is zero on ramps and
smooth curvature (a blurred edge) and positive where the signal wiggles.
The same measure on flat pixels more than 8 px from any edge is subtracted
as a texture/grain baseline. Score: `100 · (1 − ramp(index, 1, 8))`.

Limits: fine texture right next to edges (hair, foliage) counts as
ringing; images without strong edges get no ringing component.

### `sharpness`

Variance of the 4-neighbour Laplacian on the frame scaled to fit 512 px
(downscale only, so the measure does not depend on the pixel count),
restricted to pixels with Sobel magnitude > 24 so flat sticker backgrounds
do not drown the edges. Score: `100 · ramp(log10(variance), 2.0, 3.6)`
(variance 100 → 0, ≈4000 → 100).

Limits: noise and compression artefacts raise the variance, so a noisy or
blocky image can look "sharp"; the blockiness and ringing components
counter that. Deliberately soft art scores low.

### `alpha`

A bonus for stickers with real transparency: present (score 100) when
between 0.5 % and 99.5 % of the pixels have alpha < 250, absent otherwise,
so opaque images are not penalised.

### `frame_rate`

Animations and videos: `100 · min(1, fps/24)^0.7` (24 fps or more → 100,
12 fps → 62, 5 fps → 33). The animation rate is frames / total duration
with browser delay rules (≤ 10 ms counts as 100 ms).

### `palette`

Animations: distinct colours in the first frame (fully transparent pixels
count as one colour, counting stops at 4096):
`100 · min(1, log2(colours)/8)`; 256 colours or more → 100, 16 → 50.

Limits: a flat cartoon with few colours scores low even when it is a
perfect copy; the weight is small for that reason.

## Reading the badge

| Score | Meaning |
|---|---|
| 85–100 | Good copy: native resolution, no visible compression damage. |
| 65–84 | Usable; one weakness (smaller than recommended, moderate JPEG, soft). |
| 40–64 | Visibly degraded: low JPEG quality, blocking, upscaled or re-compressed. |
| 0–39 | Poor: several problems at once; worth looking for a better version. |

The breakdown explains *why*; "find a better version" compares items with
the same pHash and prefers the higher total. Scores of different pictures
are only roughly comparable: a flat cartoon and a photo stress different
components.

## Tests

`crates/stickpile-media/src/quality/tests.rs` covers: the estimate within ±5
of the encoder quality for q = 5…100 and for natural-order tables; q=90
scoring above q=20 (total, `jpeg_quality`, `blockiness`); ringing growing
with compression and absent on clean or blurred edges; blurred < sharp;
upscaled-from-small < native on `resolution` for bilinear, Catmull-Rom and
Lanczos upscales; detection of double compression, shifted grids and
lossless copies of JPEGs, with single compressions left alone; EXIF
orientation not shifting the grid; alpha, frame rate and palette
behaviour; tiny and flat frames; the weighted mean. An ignored
`calibrate` test prints the raw measurements for retuning.

## Future: learned models

A no-reference model (BRISQUE features with an SVR, or NIMA on top of the
CLIP image tower that `stickpile-ai` already runs) can be added as one
more component, for example `perceptual`, without changing the API: the
report is a list of named, weighted components and clients display
whatever names they get. The model would run in the enrich job (it needs
the ONNX runtime), update the stored report and re-rank "find a better
version". The heuristics stay as the explainable part of the breakdown.
