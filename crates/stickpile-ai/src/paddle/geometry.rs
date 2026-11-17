//! Plane geometry for the DB post-processing, in pure Rust. Each function
//! names the OpenCV or Clipper routine it stands in for in RapidOCR's
//! pipeline; where the original rounds or truncates, so does this code.

use std::f64::consts::PI;

/// A point in pixel coordinates (x right, y down).
pub type Point = [f64; 2];
/// An integer point (a pixel centre).
pub type IPoint = [i64; 2];

/// A rotated rectangle: four corners in order around the rectangle (either
/// direction) and its side lengths.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RotatedRect {
    pub corners: [Point; 4],
    /// Length of the side from `corners[0]` to `corners[1]`.
    pub width: f64,
    /// Length of the side from `corners[1]` to `corners[2]`.
    pub height: f64,
}

impl RotatedRect {
    pub fn short_side(&self) -> f64 {
        self.width.min(self.height)
    }
}

fn cross(o: IPoint, a: IPoint, b: IPoint) -> i64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Convex hull (Andrew's monotone chain), without collinear points. Duplicates
/// are fine. One or two distinct points come back as they are.
pub fn convex_hull(points: &[IPoint]) -> Vec<IPoint> {
    let mut p: Vec<IPoint> = points.to_vec();
    p.sort_unstable();
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let mut hull: Vec<IPoint> = Vec::with_capacity(p.len() * 2);
    for &pt in &p {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], pt) <= 0 {
            hull.pop();
        }
        hull.push(pt);
    }
    let lower = hull.len() + 1;
    for &pt in p.iter().rev().skip(1) {
        while hull.len() >= lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], pt) <= 0 {
            hull.pop();
        }
        hull.push(pt);
    }
    hull.pop();
    hull
}

/// Minimum-area enclosing rectangle (`cv2.minAreaRect` + `cv2.boxPoints`):
/// one side lies on a hull edge, so every edge is tried. `None` for no points.
pub fn min_area_rect(points: &[IPoint]) -> Option<RotatedRect> {
    let hull = convex_hull(points);
    let f = |p: IPoint| [p[0] as f64, p[1] as f64];
    match hull.len() {
        0 => None,
        1 => {
            let p = f(hull[0]);
            Some(RotatedRect {
                corners: [p; 4],
                width: 0.0,
                height: 0.0,
            })
        }
        2 => {
            let (a, b) = (f(hull[0]), f(hull[1]));
            Some(RotatedRect {
                corners: [a, b, b, a],
                width: dist(a, b),
                height: 0.0,
            })
        }
        n => {
            let mut best: Option<(f64, RotatedRect)> = None;
            for i in 0..n {
                let o = f(hull[i]);
                let e = f(hull[(i + 1) % n]);
                let len = dist(o, e);
                let u = [(e[0] - o[0]) / len, (e[1] - o[1]) / len];
                let v = [-u[1], u[0]];
                let (mut u0, mut u1, mut v0, mut v1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
                for &q in &hull {
                    let d = [q[0] as f64 - o[0], q[1] as f64 - o[1]];
                    let pu = d[0] * u[0] + d[1] * u[1];
                    let pv = d[0] * v[0] + d[1] * v[1];
                    u0 = u0.min(pu);
                    u1 = u1.max(pu);
                    v0 = v0.min(pv);
                    v1 = v1.max(pv);
                }
                let area = (u1 - u0) * (v1 - v0);
                if best.as_ref().is_none_or(|(a, _)| area < *a) {
                    let at = |a: f64, b: f64| [o[0] + u[0] * a + v[0] * b, o[1] + u[1] * a + v[1] * b];
                    best = Some((
                        area,
                        RotatedRect {
                            corners: [at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1)],
                            width: u1 - u0,
                            height: v1 - v0,
                        },
                    ));
                }
            }
            best.map(|(_, r)| r)
        }
    }
}

pub fn dist(a: Point, b: Point) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// RapidOCR `get_mini_boxes`: the corners sorted by x, then the left pair and
/// the right pair each ordered by y, giving top-left, top-right,
/// bottom-right, bottom-left for upright boxes. Returns the box and the
/// rectangle's short side.
pub fn mini_box(rect: &RotatedRect) -> ([Point; 4], f64) {
    let mut p = rect.corners;
    p.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let (i1, i4) = if p[1][1] > p[0][1] { (0, 1) } else { (1, 0) };
    let (i2, i3) = if p[3][1] > p[2][1] { (2, 3) } else { (3, 2) };
    ([p[i1], p[i2], p[i3], p[i4]], rect.short_side())
}

/// Shoelace area (absolute) and perimeter of a polygon, as shapely's
/// `Polygon.area` and `Polygon.length`.
pub fn area_perimeter(poly: &[Point]) -> (f64, f64) {
    let n = poly.len();
    let (mut a, mut l) = (0.0, 0.0);
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        a += p[0] * q[1] - q[0] * p[1];
        l += dist(p, q);
    }
    ((a / 2.0).abs(), l)
}

/// Clipper's `Round`: half away from zero.
fn clipper_round(v: f64) -> i64 {
    if v < 0.0 {
        (v - 0.5) as i64
    } else {
        (v + 0.5) as i64
    }
}

/// Clipper's signed area (positive = the orientation Clipper offsets outward).
fn clipper_area(poly: &[IPoint]) -> f64 {
    let n = poly.len();
    let mut a = 0.0;
    let mut j = n - 1;
    for i in 0..n {
        a += (poly[j][0] as f64 + poly[i][0] as f64) * (poly[j][1] as f64 - poly[i][1] as f64);
        j = i;
    }
    -a * 0.5
}

fn unit_normal(a: IPoint, b: IPoint) -> Point {
    let (dx, dy) = ((b[0] - a[0]) as f64, (b[1] - a[1]) as f64);
    if dx == 0.0 && dy == 0.0 {
        return [0.0, 0.0];
    }
    let f = 1.0 / (dx * dx + dy * dy).sqrt();
    [dy * f, -dx * f]
}

/// Outward offset of a closed polygon with round joins, as pyclipper's
/// `PyclipperOffset` (Clipper 6.4, `JT_ROUND`, `ET_CLOSEDPOLYGON`, arc
/// tolerance 0.25) produces it before its final union. pyclipper truncates
/// float input to integers, which the caller does. For the convex polygons
/// this is used on, the final union only drops collinear and duplicate
/// points, which changes neither the hull nor the minimum-area rectangle.
pub fn offset_round(path: &[IPoint], delta: f64) -> Vec<IPoint> {
    // ClipperOffset::AddPath: drop repeated points (and a closing duplicate).
    let mut src: Vec<IPoint> = Vec::with_capacity(path.len());
    let mut end = path.len();
    while end > 1 && path[0] == path[end - 1] {
        end -= 1;
    }
    for &p in &path[..end] {
        if src.last() != Some(&p) {
            src.push(p);
        }
    }
    if src.len() < 3 || delta <= 0.0 {
        return src;
    }
    // FixOrientations: Clipper offsets outward for positive orientation.
    if clipper_area(&src) < 0.0 {
        src.reverse();
    }
    let len = src.len();
    let normals: Vec<Point> = (0..len).map(|j| unit_normal(src[j], src[(j + 1) % len])).collect();

    let ad = delta.abs();
    let y = if 0.25 > ad * 0.25 { ad * 0.25 } else { 0.25 };
    let mut steps = PI / (1.0 - y / ad).acos();
    if steps > ad * PI {
        steps = ad * PI;
    }
    let (sin, cos) = (2.0 * PI / steps).sin_cos();
    let steps_per_rad = steps / (2.0 * PI);

    let at = |p: IPoint, n: Point| {
        [
            clipper_round(p[0] as f64 + n[0] * delta),
            clipper_round(p[1] as f64 + n[1] * delta),
        ]
    };
    let mut out = Vec::with_capacity(len * 8);
    let mut k = len - 1;
    for j in 0..len {
        let (nk, nj) = (normals[k], normals[j]);
        let mut sin_a = nk[0] * nj[1] - nj[0] * nk[1];
        if (sin_a * delta).abs() < 1.0 {
            let cos_a = nk[0] * nj[0] + nk[1] * nj[1];
            if cos_a > 0.0 {
                // Nearly straight: one point, and k stays (as in Clipper).
                out.push(at(src[j], nk));
                continue;
            }
        } else {
            sin_a = sin_a.clamp(-1.0, 1.0);
        }
        if sin_a * delta < 0.0 {
            out.push(at(src[j], nk));
            out.push(src[j]);
            out.push(at(src[j], nj));
        } else {
            // DoRound
            let a = sin_a.atan2(nk[0] * nj[0] + nk[1] * nj[1]);
            let n = (clipper_round(steps_per_rad * a.abs()) as i32).max(1);
            let (mut x, mut yy) = (nk[0], nk[1]);
            for _ in 0..n {
                out.push(at(src[j], [x, yy]));
                let x2 = x;
                x = x * cos - sin * yy;
                yy = x2 * sin + yy * cos;
            }
            out.push(at(src[j], nj));
        }
        k = j;
    }
    out
}

/// RapidOCR `DBPostProcess.unclip` for a box from [`mini_box`]: offset by
/// `area * ratio / perimeter` with round joins.
pub fn unclip(b: &[Point; 4], ratio: f64) -> Vec<IPoint> {
    let (area, perimeter) = area_perimeter(b);
    if perimeter <= 0.0 {
        return Vec::new();
    }
    let distance = area * ratio / perimeter;
    let ints: Vec<IPoint> = b.iter().map(|p| [p[0] as i64, p[1] as i64]).collect();
    offset_round(&ints, distance)
}

/// RapidOCR `order_points_clockwise`: top-left, top-right, bottom-right,
/// bottom-left (the two leftmost points by y, then the two rightmost by y).
pub fn order_clockwise(pts: &[Point; 4]) -> [Point; 4] {
    let mut idx = [0usize, 1, 2, 3];
    idx.sort_by(|&a, &b| pts[a][0].total_cmp(&pts[b][0]));
    let mut left = [pts[idx[0]], pts[idx[1]]];
    let mut right = [pts[idx[2]], pts[idx[3]]];
    left.sort_by(|a, b| a[1].total_cmp(&b[1]));
    right.sort_by(|a, b| a[1].total_cmp(&b[1]));
    [left[0], right[0], right[1], left[1]]
}

/// 2 x 2 dilation with the anchor at the kernel's lower right (`cv2.dilate`
/// with `np.ones((2, 2))`): a pixel is set when it or its left, upper or
/// upper-left neighbour is.
pub fn dilate2x2(src: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut out = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            out[i] = src[i] || (x > 0 && src[i - 1]) || (y > 0 && (src[i - w] || (x > 0 && src[i - w - 1])));
        }
    }
    out
}

/// The point set of one contour that matters for its minimum-area rectangle
/// (the contour's convex hull is the hull of these points).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contour {
    /// Raster index (`y * width + x`) of the pixel where OpenCV's border
    /// following starts this contour; contours come out in this order.
    pub start: usize,
    pub points: Vec<IPoint>,
}

/// The contours `cv2.findContours(bitmap, RETR_LIST, CHAIN_APPROX_SIMPLE)`
/// returns, reduced to what the DB post-processing uses: for each
/// 8-connected foreground component its outer border, and for each hole (a
/// 4-connected background component that does not reach the edge of the
/// picture) the hole's border, i.e. the foreground pixels 4-adjacent to it.
/// Each comes as the leftmost and rightmost pixel of every row, which spans
/// the same convex hull. Sorted by where OpenCV's raster scan starts them.
pub fn contours(bitmap: &[bool], w: usize, h: usize) -> Vec<Contour> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    const NONE: u32 = u32::MAX;
    let mut out = Vec::new();
    let mut label = vec![NONE; w * h];
    let mut stack: Vec<usize> = Vec::new();

    // Foreground components (8-connected), labelled in raster order of their
    // first pixel, which is where the outer border starts.
    let mut fg_starts = Vec::new();
    for start in 0..w * h {
        if !bitmap[start] || label[start] != NONE {
            continue;
        }
        let id = fg_starts.len() as u32;
        fg_starts.push(start);
        label[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let j = ny * w + nx;
                    if bitmap[j] && label[j] == NONE {
                        label[j] = id;
                        stack.push(j);
                    }
                }
            }
        }
    }
    let n_fg = fg_starts.len();

    // Background components (4-connected). Those touching the edge are
    // outside (OpenCV pads the picture with zeros); the others are holes.
    let mut bg_starts = Vec::new();
    let mut bg_outside = Vec::new();
    for start in 0..w * h {
        if bitmap[start] || label[start] != NONE {
            continue;
        }
        let id = (n_fg + bg_starts.len()) as u32;
        bg_starts.push(start);
        let mut outside = false;
        label[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                outside = true;
            }
            let mut visit = |j: usize| {
                if !bitmap[j] && label[j] == NONE {
                    label[j] = id;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        bg_outside.push(outside);
    }

    // Row extremes per component, in one raster pass.
    let n = n_fg + bg_starts.len();
    let mut rows: Vec<Vec<(i64, i64, i64)>> = vec![Vec::new(); n];
    for y in 0..h {
        for x in 0..w {
            let id = label[y * w + x] as usize;
            if id >= n_fg && bg_outside[id - n_fg] {
                continue;
            }
            let r = &mut rows[id];
            match r.last_mut() {
                Some(last) if last.0 == y as i64 => last.2 = x as i64,
                _ => r.push((y as i64, x as i64, x as i64)),
            }
        }
    }

    for (id, &start) in fg_starts.iter().enumerate() {
        let points = rows[id].iter().flat_map(|&(y, x0, x1)| [[x0, y], [x1, y]]).collect();
        out.push(Contour { start, points });
    }
    for (b, &start) in bg_starts.iter().enumerate() {
        if bg_outside[b] {
            continue;
        }
        // The hole border: grow the hole by one pixel in the four directions
        // (those pixels are border or hole, and the border encloses the hole).
        let points = rows[n_fg + b]
            .iter()
            .flat_map(|&(y, x0, x1)| {
                [
                    [x0 - 1, y],
                    [x1 + 1, y],
                    [x0, y - 1],
                    [x1, y - 1],
                    [x0, y + 1],
                    [x1, y + 1],
                ]
            })
            .collect();
        // The scan starts a hole border at the pixel left of the hole's first pixel.
        out.push(Contour {
            start: start - 1,
            points,
        });
    }
    // Stable: an outer border and a hole border starting at the same pixel
    // keep that order, as OpenCV checks for an outer border first.
    out.sort_by_key(|c| c.start);
    out
}

/// Draws a line with 8-connected Bresenham steps (OpenCV `line`, LINE_8).
fn draw_line(mask: &mut [bool], w: usize, h: usize, a: IPoint, b: IPoint) {
    let (mut x, mut y) = (a[0], a[1]);
    let (dx, dy) = ((b[0] - a[0]).abs(), -(b[1] - a[1]).abs());
    let (sx, sy) = (if a[0] < b[0] { 1 } else { -1 }, if a[1] < b[1] { 1 } else { -1 });
    let mut err = dx + dy;
    loop {
        if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
            mask[y as usize * w + x as usize] = true;
        }
        if x == b[0] && y == b[1] {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// `cv2.fillPoly` of one polygon with integer vertices: the outline (8-connected
/// lines) plus every pixel whose centre lies inside, row by row.
pub fn fill_poly(mask: &mut [bool], w: usize, h: usize, poly: &[IPoint]) {
    let n = poly.len();
    if n == 0 {
        return;
    }
    for i in 0..n {
        draw_line(mask, w, h, poly[(i + n - 1) % n], poly[i]);
    }
    let ymin = poly.iter().map(|p| p[1]).min().unwrap_or(0).max(0);
    let ymax = poly.iter().map(|p| p[1]).max().unwrap_or(-1).min(h as i64 - 1);
    let mut xs: Vec<f64> = Vec::with_capacity(n);
    for y in ymin..=ymax {
        xs.clear();
        for i in 0..n {
            let (p, q) = (poly[(i + n - 1) % n], poly[i]);
            if p[1] == q[1] {
                continue;
            }
            let (lo, hi) = if p[1] < q[1] { (p, q) } else { (q, p) };
            if y >= lo[1] && y < hi[1] {
                xs.push(lo[0] as f64 + (y - lo[1]) as f64 * (hi[0] - lo[0]) as f64 / (hi[1] - lo[1]) as f64);
            }
        }
        if xs.len() < 2 {
            continue;
        }
        xs.sort_by(f64::total_cmp);
        for pair in xs.chunks_exact(2) {
            let x0 = (pair[0].ceil() as i64).max(0);
            let x1 = (pair[1].floor() as i64).min(w as i64 - 1);
            for x in x0..=x1 {
                mask[y as usize * w + x as usize] = true;
            }
        }
    }
}

/// RapidOCR `box_score_fast`: the mean of `pred` (row-major `w x h`) inside the
/// quadrilateral `b`, rasterised with [`fill_poly`] after truncating the
/// corners to integers.
pub fn box_score(pred: &[f32], w: usize, h: usize, b: &[Point; 4]) -> f64 {
    let clampi = |v: f64, hi: usize| (v as i64).clamp(0, hi as i64 - 1) as usize;
    let xmin = clampi(b.iter().map(|p| p[0]).fold(f64::MAX, f64::min).floor(), w);
    let xmax = clampi(b.iter().map(|p| p[0]).fold(f64::MIN, f64::max).ceil(), w);
    let ymin = clampi(b.iter().map(|p| p[1]).fold(f64::MAX, f64::min).floor(), h);
    let ymax = clampi(b.iter().map(|p| p[1]).fold(f64::MIN, f64::max).ceil(), h);
    let (mw, mh) = (xmax - xmin + 1, ymax - ymin + 1);
    let poly: Vec<IPoint> = b
        .iter()
        .map(|p| [(p[0] - xmin as f64) as i64, (p[1] - ymin as f64) as i64])
        .collect();
    let mut mask = vec![false; mw * mh];
    fill_poly(&mut mask, mw, mh, &poly);
    let (mut sum, mut count) = (0.0f64, 0usize);
    for y in 0..mh {
        for x in 0..mw {
            if mask[y * mw + x] {
                sum += pred[(ymin + y) * w + xmin + x] as f64;
                count += 1;
            }
        }
    }
    if count == 0 {
        0.0
    } else {
        sum / count as f64
    }
}

/// Solves `a x = b` (n x n, row-major) by Gaussian elimination with partial
/// pivoting. `None` when singular.
fn solve(mut a: Vec<f64>, mut b: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i * n + col].abs().total_cmp(&a[j * n + col].abs()))?;
        if a[piv * n + col].abs() < 1e-12 {
            return None;
        }
        if piv != col {
            for k in 0..n {
                a.swap(piv * n + k, col * n + k);
            }
            b.swap(piv, col);
        }
        for row in col + 1..n {
            let f = a[row * n + col] / a[col * n + col];
            if f != 0.0 {
                for k in col..n {
                    a[row * n + k] -= f * a[col * n + k];
                }
                b[row] -= f * b[col];
            }
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|k| a[row * n + k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row * n + row];
    }
    Some(x)
}

/// The homography mapping `from[i]` to `to[i]` (`cv2.getPerspectiveTransform`),
/// row-major 3 x 3 with the last entry 1. `None` for degenerate input.
pub fn perspective_transform(from: &[Point; 4], to: &[Point; 4]) -> Option<[f64; 9]> {
    let mut a = vec![0.0; 64];
    let mut b = vec![0.0; 8];
    for i in 0..4 {
        let ([u, v], [x, y]) = (from[i], to[i]);
        a[i * 8..i * 8 + 8].copy_from_slice(&[u, v, 1.0, 0.0, 0.0, 0.0, -u * x, -v * x]);
        a[(i + 4) * 8..(i + 4) * 8 + 8].copy_from_slice(&[0.0, 0.0, 0.0, u, v, 1.0, -u * y, -v * y]);
        b[i] = x;
        b[i + 4] = y;
    }
    let h = solve(a, b, 8)?;
    Some([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0])
}

/// OpenCV's cubic interpolation weights (`interpolateCubic`, A = -0.75).
fn cubic_weights(x: f32) -> [f32; 4] {
    const A: f32 = -0.75;
    let c0 = ((A * (x + 1.0) - 5.0 * A) * (x + 1.0) + 8.0 * A) * (x + 1.0) - 4.0 * A;
    let c1 = ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0;
    let c2 = ((A + 2.0) * (1.0 - x) - (A + 3.0)) * (1.0 - x) * (1.0 - x) + 1.0;
    [c0, c1, c2, 1.0 - c0 - c1 - c2]
}

/// An interleaved 3-channel 8-bit picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image3 {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Image3 {
    pub fn new(width: usize, height: usize) -> Image3 {
        Image3 {
            width,
            height,
            data: vec![0; width * height * 3],
        }
    }

    pub fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        let i = (y * self.width + x) * 3;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// `np.rot90`: a quarter turn counter-clockwise.
    pub fn rot90(&self) -> Image3 {
        let mut out = Image3::new(self.height, self.width);
        for y in 0..out.height {
            for x in 0..out.width {
                let i = (y * out.width + x) * 3;
                let p = self.pixel(self.width - 1 - y, x);
                out.data[i..i + 3].copy_from_slice(&p);
            }
        }
        out
    }
}

/// Bilinear resize with half-pixel centres and edge clamping, as `cv2.resize`
/// with `INTER_LINEAR` (no antialiasing when shrinking). `sample(x, y)` reads
/// a source pixel, so the source can be computed on the fly.
pub fn resize_bilinear(
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    sample: impl Fn(usize, usize) -> [u8; 3],
) -> Image3 {
    let taps = |dst: usize, src: usize| -> Vec<(usize, usize, f32)> {
        let scale = src as f64 / dst as f64;
        (0..dst)
            .map(|d| {
                let f = ((d as f64 + 0.5) * scale - 0.5) as f32;
                let mut s = f.floor() as i64;
                let mut frac = f - s as f32;
                if s < 0 {
                    s = 0;
                    frac = 0.0;
                }
                if s >= src as i64 - 1 {
                    s = src as i64 - 1;
                    frac = 0.0;
                }
                let s = s as usize;
                (s, (s + 1).min(src - 1), frac)
            })
            .collect()
    };
    let xs = taps(dst_w, src_w);
    let ys = taps(dst_h, src_h);
    let mut out = Image3::new(dst_w, dst_h);
    let mut row0 = vec![[0f32; 3]; dst_w];
    let mut row1 = vec![[0f32; 3]; dst_w];
    let hrow = |y: usize, row: &mut [[f32; 3]]| {
        for (r, &(x0, x1, fx)) in row.iter_mut().zip(&xs) {
            let (a, b) = (sample(x0, y), sample(x1, y));
            for c in 0..3 {
                r[c] = a[c] as f32 * (1.0 - fx) + b[c] as f32 * fx;
            }
        }
    };
    let mut cached: (usize, usize) = (usize::MAX, usize::MAX);
    for (dy, &(y0, y1, fy)) in ys.iter().enumerate() {
        if cached != (y0, y1) {
            if cached.1 == y0 {
                std::mem::swap(&mut row0, &mut row1);
            } else {
                hrow(y0, &mut row0);
            }
            hrow(y1, &mut row1);
            cached = (y0, y1);
        }
        let o = &mut out.data[dy * dst_w * 3..(dy + 1) * dst_w * 3];
        for x in 0..dst_w {
            for c in 0..3 {
                let v = row0[x][c] * (1.0 - fy) + row1[x][c] * fy;
                o[x * 3 + c] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// RapidOCR `get_rotate_crop_image`: warps the quadrilateral `quad`
/// (top-left, top-right, bottom-right, bottom-left) of `img` to an upright
/// rectangle as wide and high as its longer opposite edges (bicubic,
/// replicated border, `cv2.warpPerspective`), then turns it a quarter
/// counter-clockwise when it is at least 1.5 times as high as wide. `None`
/// for a degenerate quadrilateral.
pub fn crop_quad(img: &Image3, quad: &[Point; 4]) -> Option<Image3> {
    let w = dist(quad[0], quad[1]).max(dist(quad[2], quad[3])) as usize;
    let h = dist(quad[0], quad[3]).max(dist(quad[1], quad[2])) as usize;
    if w == 0 || h == 0 || img.width == 0 || img.height == 0 {
        return None;
    }
    let rect = [[0.0, 0.0], [w as f64, 0.0], [w as f64, h as f64], [0.0, h as f64]];
    // Destination -> source, which is what warpPerspective evaluates.
    let m = perspective_transform(&rect, quad)?;
    let mut out = Image3::new(w, h);
    let (iw, ih) = (img.width as i64, img.height as i64);
    for y in 0..h {
        for x in 0..w {
            let (xf, yf) = (x as f64, y as f64);
            let wv = m[6] * xf + m[7] * yf + m[8];
            let wv = if wv != 0.0 { 32.0 / wv } else { 0.0 };
            // Source position in 1/32 pixel steps (INTER_BITS = 5).
            let sx = ((m[0] * xf + m[1] * yf + m[2]) * wv).round_ties_even() as i64;
            let sy = ((m[3] * xf + m[4] * yf + m[5]) * wv).round_ties_even() as i64;
            let (ix, iy) = (sx >> 5, sy >> 5);
            let wx = cubic_weights((sx & 31) as f32 / 32.0);
            let wy = cubic_weights((sy & 31) as f32 / 32.0);
            let mut acc = [0f32; 3];
            for (j, wyj) in wy.iter().enumerate() {
                let py = (iy - 1 + j as i64).clamp(0, ih - 1) as usize;
                for (i, wxi) in wx.iter().enumerate() {
                    let px = (ix - 1 + i as i64).clamp(0, iw - 1) as usize;
                    let p = img.pixel(px, py);
                    let wgt = wyj * wxi;
                    for (a, v) in acc.iter_mut().zip(p) {
                        *a += v as f32 * wgt;
                    }
                }
            }
            let o = (y * w + x) * 3;
            for (d, a) in out.data[o..o + 3].iter_mut().zip(acc) {
                *d = a.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    if h as f64 / w as f64 >= 1.5 {
        Some(out.rot90())
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap(rows: &[&str]) -> (Vec<bool>, usize, usize) {
        let h = rows.len();
        let w = rows[0].len();
        let b = rows.iter().flat_map(|r| r.bytes().map(|c| c == b'#')).collect();
        (b, w, h)
    }

    fn sorted(mut v: Vec<IPoint>) -> Vec<IPoint> {
        v.sort();
        v
    }

    #[test]
    fn hull_drops_interior_and_collinear_points() {
        let pts = [[0, 0], [2, 0], [4, 0], [4, 2], [4, 4], [0, 4], [2, 2], [1, 3], [0, 0]];
        assert_eq!(sorted(convex_hull(&pts)), vec![[0, 0], [0, 4], [4, 0], [4, 4]]);
        assert_eq!(convex_hull(&[[1, 1], [1, 1]]), vec![[1, 1]]);
        assert!(convex_hull(&[]).is_empty());
    }

    #[test]
    fn min_area_rect_of_axis_aligned_and_rotated_sets() {
        // A 10 x 4 block of pixel centres: the rectangle through them is 9 x 3.
        let pts: Vec<IPoint> = (0..4).flat_map(|y| (0..10).map(move |x| [x + 5, y + 7])).collect();
        let r = min_area_rect(&pts).unwrap();
        let (w, h) = (r.width.max(r.height), r.short_side());
        assert!((w - 9.0).abs() < 1e-9 && (h - 3.0).abs() < 1e-9, "{r:?}");
        let (b, side) = mini_box(&r);
        assert_eq!(side, 3.0);
        assert_eq!(b, [[5.0, 7.0], [14.0, 7.0], [14.0, 10.0], [5.0, 10.0]]);

        // A diamond (square rotated 45 degrees): side sqrt(2) * 3.
        let r = min_area_rect(&[[3, 0], [6, 3], [3, 6], [0, 3], [3, 3]]).unwrap();
        assert!((r.width - 18f64.sqrt()).abs() < 1e-9 && (r.height - 18f64.sqrt()).abs() < 1e-9);
        // A thin diagonal line: zero width, so it would be dropped (< 3 px).
        let r = min_area_rect(&[[0, 0], [5, 5], [10, 10]]).unwrap();
        assert!(r.short_side() < 1e-9);
        assert!(min_area_rect(&[]).is_none());
    }

    #[test]
    fn unclip_grows_a_rectangle_by_the_distance() {
        // pyclipper on the same box with distance 2 gives x 8..52, y 18..32
        // (measured; float corners are truncated first).
        let b = [[10.7, 20.6], [50.2, 20.6], [50.2, 30.9], [10.7, 30.9]];
        let poly = offset_round(&b.iter().map(|p| [p[0] as i64, p[1] as i64]).collect::<Vec<_>>(), 2.0);
        let xs: Vec<i64> = poly.iter().map(|p| p[0]).collect();
        let ys: Vec<i64> = poly.iter().map(|p| p[1]).collect();
        assert_eq!((*xs.iter().min().unwrap(), *xs.iter().max().unwrap()), (8, 52));
        assert_eq!((*ys.iter().min().unwrap(), *ys.iter().max().unwrap()), (18, 32));
        // Every arc point lies within the grown rectangle.
        assert!(poly
            .iter()
            .all(|p| (8..=52).contains(&p[0]) && (18..=32).contains(&p[1])));

        // unclip: distance = area * 1.6 / perimeter = 40 * 10 * 1.6 / 100 = 6.4.
        let b = [[0.0, 0.0], [40.0, 0.0], [40.0, 10.0], [0.0, 10.0]];
        let r = min_area_rect(&unclip(&b, 1.6)).unwrap();
        let (bx, side) = mini_box(&r);
        assert!((side - 22.8).abs() <= 1.0, "{side}");
        assert!((r.width.max(r.height) - 52.8).abs() <= 1.0, "{r:?}");
        assert!((bx[0][0] + 6.4).abs() <= 1.0 && (bx[0][1] + 6.4).abs() <= 1.0, "{bx:?}");

        // Orientation does not matter, and a rotated box grows the same way.
        let rot = |p: [f64; 2]| [100.0 + p[0] * 0.8 - p[1] * 0.6, 50.0 + p[0] * 0.6 + p[1] * 0.8];
        let rb = [rot(b[3]), rot(b[2]), rot(b[1]), rot(b[0])];
        let r2 = min_area_rect(&unclip(&rb, 1.6)).unwrap();
        assert!((r2.short_side() - 22.8).abs() <= 1.5, "{r2:?}");
        assert!((r2.width.max(r2.height) - 52.8).abs() <= 1.5, "{r2:?}");
    }

    #[test]
    fn clockwise_order() {
        let pts = [[10.0, 30.0], [50.0, 1.0], [12.0, 2.0], [48.0, 31.0]];
        assert_eq!(
            order_clockwise(&pts),
            [[12.0, 2.0], [50.0, 1.0], [48.0, 31.0], [10.0, 30.0]]
        );
    }

    #[test]
    fn dilation_extends_right_and_down() {
        let (b, w, h) = bitmap(&["#..", "...", "..."]);
        let d = dilate2x2(&b, w, h);
        let (e, _, _) = bitmap(&["##.", "##.", "..."]);
        assert_eq!(d, e);
    }

    #[test]
    fn contours_outer_and_holes() {
        let (b, w, h) = bitmap(&[
            "..........",
            ".#####..#.",
            ".#...#....",
            ".#####....",
            ".......##.",
            "........#.",
        ]);
        let c = contours(&b, w, h);
        // Ring, its hole, the dot, and the diagonal pair (8-connected).
        assert_eq!(c.len(), 4, "{c:?}");
        assert_eq!(c[0].start, 11);
        assert_eq!(sorted(convex_hull(&c[0].points)), vec![[1, 1], [1, 3], [5, 1], [5, 3]]);
        assert_eq!(c[1].start, 18); // the dot at (8, 1)
        assert_eq!(c[2].start, 21); // the hole's border starts left of (2, 2)
        assert_eq!(
            sorted(convex_hull(&c[2].points)),
            vec![[1, 2], [2, 1], [2, 3], [4, 1], [4, 3], [5, 2]]
        );
        assert_eq!(c[3].start, 47);
        assert_eq!(sorted(convex_hull(&c[3].points)), vec![[7, 4], [8, 4], [8, 5]]);

        // Pixels touching only at a corner are one component.
        let (b, w, h) = bitmap(&["#..", ".#.", "..#"]);
        assert_eq!(contours(&b, w, h).len(), 1);

        // Background reaching the edge is not a hole.
        let (b, w, h) = bitmap(&["###", "#.#", "#.#"]);
        assert_eq!(contours(&b, w, h).len(), 1);
        assert!(contours(&[], 0, 0).is_empty());
    }

    #[test]
    fn polygon_fill_and_score() {
        let mut m = vec![false; 25];
        fill_poly(&mut m, 5, 5, &[[1, 1], [3, 1], [3, 3], [1, 3]]);
        let filled: Vec<usize> = (0..25).filter(|&i| m[i]).collect();
        assert_eq!(filled, vec![6, 7, 8, 11, 12, 13, 16, 17, 18]);

        // Score: mean probability inside the box only.
        let mut pred = vec![0.0f32; 100];
        for y in 2..5 {
            for x in 2..8 {
                pred[y * 10 + x] = 0.9;
            }
        }
        let s = box_score(&pred, 10, 10, &[[2.0, 2.0], [7.0, 2.0], [7.0, 4.0], [2.0, 4.0]]);
        assert!((s - 0.9).abs() < 1e-6, "{s}");
        let s = box_score(&pred, 10, 10, &[[0.0, 0.0], [9.0, 0.0], [9.0, 9.0], [0.0, 9.0]]);
        assert!((s - 0.9 * 18.0 / 100.0).abs() < 1e-6, "{s}");
    }

    #[test]
    fn homography_maps_corners() {
        let from = [[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]];
        let to = [[3.0, 4.0], [20.0, 2.0], [22.0, 12.0], [2.0, 11.0]];
        let m = perspective_transform(&from, &to).unwrap();
        for (f, t) in from.iter().zip(&to) {
            let w = m[6] * f[0] + m[7] * f[1] + m[8];
            let x = (m[0] * f[0] + m[1] * f[1] + m[2]) / w;
            let y = (m[3] * f[0] + m[4] * f[1] + m[5]) / w;
            assert!((x - t[0]).abs() < 1e-9 && (y - t[1]).abs() < 1e-9);
        }
        assert!(perspective_transform(&from, &[[0.0, 0.0]; 4]).is_none());
    }

    fn gradient(w: usize, h: usize) -> Image3 {
        let mut img = Image3::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                img.data[i..i + 3].copy_from_slice(&[(x * 10) as u8, (y * 10) as u8, 7]);
            }
        }
        img
    }

    #[test]
    fn perspective_crop_of_an_upright_box_is_a_copy() {
        let img = gradient(20, 10);
        let c = crop_quad(&img, &[[2.0, 3.0], [12.0, 3.0], [12.0, 7.0], [2.0, 7.0]]).unwrap();
        assert_eq!((c.width, c.height), (10, 4));
        for y in 0..4 {
            for x in 0..10 {
                assert_eq!(c.pixel(x, y), img.pixel(x + 2, y + 3));
            }
        }
        // Outside the picture the border is replicated.
        let c = crop_quad(&img, &[[15.0, 0.0], [25.0, 0.0], [25.0, 4.0], [15.0, 4.0]]).unwrap();
        assert_eq!(c.pixel(9, 0), img.pixel(19, 0));
        assert!(crop_quad(&img, &[[1.0, 1.0]; 4]).is_none());
    }

    #[test]
    fn perspective_crop_of_a_tall_box_is_turned() {
        let img = gradient(10, 20);
        // 3 wide, 12 high: rotated to 12 x 3, the top of the box on the left.
        let c = crop_quad(&img, &[[4.0, 2.0], [7.0, 2.0], [7.0, 14.0], [4.0, 14.0]]).unwrap();
        assert_eq!((c.width, c.height), (12, 3));
        assert_eq!(c.pixel(0, 2), img.pixel(4, 2));
        assert_eq!(c.pixel(11, 0), img.pixel(6, 13));
    }

    #[test]
    fn bilinear_resize_matches_opencv_samples() {
        // 2x upscale of a 2-pixel ramp: cv2.resize gives 0, 25, 75, 100 (rounded).
        let src = [[0u8; 3], [100u8; 3]];
        let out = resize_bilinear(2, 1, 4, 1, |x, _| src[x]);
        let row: Vec<u8> = (0..4).map(|x| out.pixel(x, 0)[0]).collect();
        assert_eq!(row, vec![0, 25, 75, 100]);
        // Halving takes the mean of each pair, without wider filtering.
        let src: Vec<[u8; 3]> = [0u8, 100, 200, 40].iter().map(|&v| [v; 3]).collect();
        let out = resize_bilinear(4, 1, 2, 1, |x, _| src[x]);
        assert_eq!((out.pixel(0, 0)[0], out.pixel(1, 0)[0]), (50, 120));
        // Identity size is a copy.
        let img = gradient(5, 3);
        let same = resize_bilinear(5, 3, 5, 3, |x, y| img.pixel(x, y));
        assert_eq!(same, img);
    }
}
