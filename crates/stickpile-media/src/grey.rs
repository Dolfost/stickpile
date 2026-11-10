//! Float greyscale planes shared by the hashes and the quality score.

use image::{ImageBuffer, Luma, RgbaImage};

/// Greyscale plane, values 0..=255.
pub(crate) type Grey = ImageBuffer<Luma<f32>, Vec<f32>>;

/// BT.601 luma of the image composited on white (transparent areas count as
/// white, so a sticker hashes the same with or without its alpha channel
/// flattened by some app).
pub(crate) fn luma_on_white(img: &RgbaImage) -> Grey {
    let (w, h) = img.dimensions();
    let data = img
        .pixels()
        .map(|p| {
            let a = f32::from(p[3]) / 255.0;
            let c = |v: u8| f32::from(v) * a + 255.0 * (1.0 - a);
            0.299 * c(p[0]) + 0.587 * c(p[1]) + 0.114 * c(p[2])
        })
        .collect();
    Grey::from_raw(w, h, data).expect("sized")
}

/// Resize with a triangle filter whose support grows with the downscale
/// factor (anti-aliased). The image crate treats f32 samples as 0..=1 and
/// clamps, so the plane is normalised around the call.
pub(crate) fn resize(g: &Grey, w: u32, h: u32) -> Grey {
    if g.dimensions() == (w, h) {
        return g.clone();
    }
    let mut unit = g.clone();
    unit.iter_mut().for_each(|v| *v /= 255.0);
    let mut out = image::imageops::resize(&unit, w, h, image::imageops::FilterType::Triangle);
    out.iter_mut().for_each(|v| *v *= 255.0);
    out
}

/// 2x2 box downsample (odd trailing row/column dropped).
pub(crate) fn downsample2(g: &Grey) -> Grey {
    let (w, h) = (g.width() / 2, g.height() / 2);
    let src = g.as_raw();
    let sw = g.width() as usize;
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let i = 2 * y * sw + 2 * x;
            out.push((src[i] + src[i + 1] + src[i + sw] + src[i + sw + 1]) * 0.25);
        }
    }
    Grey::from_raw(w, h, out).expect("sized")
}

/// Orthonormal DCT-II basis: `m[k][n]`.
pub(crate) fn dct_matrix<const N: usize>() -> [[f32; N]; N] {
    let mut m = [[0f32; N]; N];
    for (k, row) in m.iter_mut().enumerate() {
        let scale = if k == 0 {
            (1.0 / N as f64).sqrt()
        } else {
            (2.0 / N as f64).sqrt()
        };
        for (n, v) in row.iter_mut().enumerate() {
            *v = (scale * (std::f64::consts::PI * (2 * n + 1) as f64 * k as f64 / (2 * N) as f64).cos()) as f32;
        }
    }
    m
}

/// 2-D DCT-II of an N x N block (row-major input), orthonormal.
pub(crate) fn dct2<const N: usize>(m: &[[f32; N]; N], block: &[[f32; N]; N]) -> [[f32; N]; N] {
    let mut tmp = [[0f32; N]; N];
    for y in 0..N {
        for k in 0..N {
            tmp[y][k] = (0..N).map(|n| m[k][n] * block[y][n]).sum();
        }
    }
    let mut out = [[0f32; N]; N];
    for k in 0..N {
        for l in 0..N {
            out[l][k] = (0..N).map(|n| m[l][n] * tmp[n][k]).sum();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dct_of_constant_is_dc_only() {
        let m = dct_matrix::<8>();
        let block = [[10f32; 8]; 8];
        let c = dct2(&m, &block);
        assert!((c[0][0] - 80.0).abs() < 1e-3);
        for (i, row) in c.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                if (i, j) != (0, 0) {
                    assert!(v.abs() < 1e-3);
                }
            }
        }
    }

    #[test]
    fn transparent_is_white() {
        let img = RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 0]));
        assert!(luma_on_white(&img).pixels().all(|p| (p[0] - 255.0).abs() < 1e-3));
    }

    #[test]
    fn resize_keeps_the_value_range() {
        let g = Grey::from_fn(64, 32, |x, _| Luma([x as f32 * 4.0]));
        let r = resize(&g, 16, 8);
        assert!(r.get_pixel(15, 0)[0] > 200.0, "{:?}", r.get_pixel(15, 0));
        assert!(r.get_pixel(0, 0)[0] < 20.0);
    }

    #[test]
    fn box_downsample_halves() {
        let g = Grey::from_fn(5, 4, |x, _| Luma([x as f32]));
        let d = downsample2(&g);
        assert_eq!(d.dimensions(), (2, 2));
        assert!((d.get_pixel(1, 0)[0] - 2.5).abs() < 1e-6);
    }
}
