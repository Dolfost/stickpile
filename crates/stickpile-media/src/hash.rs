//! 64-bit perceptual hashes.
//!
//! Both hashes work on the BT.601 luma of the frame composited on white.
//! Bit order: bit 63 is the first cell (top-left), bit 0 the last, row-major.

use crate::grey::{dct2, dct_matrix, luma_on_white, resize};

/// DCT-based pHash of a frame (32x32 grey, low 8x8 DCT block vs median).
///
/// The frame is reduced to 32x32 with an anti-aliasing triangle filter, a
/// 2-D DCT-II is taken and the top-left 8x8 block of low frequencies is kept.
/// Each of the 64 coefficients becomes one bit: set when it is above the
/// median of the 63 AC coefficients (the DC term is left out of the median
/// because it only encodes mean brightness and dwarfs everything else).
pub fn phash(img: &image::RgbaImage) -> u64 {
    if img.width() == 0 || img.height() == 0 {
        return 0;
    }
    let small = resize(&luma_on_white(img), 32, 32);
    let mut block = [[0f32; 32]; 32];
    for (x, y, p) in small.enumerate_pixels() {
        block[y as usize][x as usize] = p[0];
    }
    let m = dct_matrix::<32>();
    let c = dct2(&m, &block);
    let mut low = [0f32; 64];
    for y in 0..8 {
        for x in 0..8 {
            low[y * 8 + x] = c[y][x];
        }
    }
    let mut ac: Vec<f32> = low[1..].to_vec();
    ac.sort_by(f32::total_cmp);
    let median = ac[ac.len() / 2];
    low.iter().fold(0u64, |h, v| (h << 1) | u64::from(*v > median))
}

/// Difference hash (9x8 grey, horizontal gradients).
///
/// The frame is reduced to 9 columns x 8 rows; each bit compares a pixel
/// with its right neighbour and is set when left < right.
pub fn dhash(img: &image::RgbaImage) -> u64 {
    if img.width() == 0 || img.height() == 0 {
        return 0;
    }
    let small = resize(&luma_on_white(img), 9, 8);
    let mut h = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            let l = small.get_pixel(x, y)[0];
            let r = small.get_pixel(x + 1, y)[0];
            h = (h << 1) | u64::from(l < r);
        }
    }
    h
}

pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// 16 lowercase hex chars (API representation).
pub fn to_hex(h: u64) -> String {
    format!("{h:016x}")
}

pub fn from_hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s, 16).ok()
}

#[cfg(test)]
pub(crate) mod fixtures {
    use image::{Rgba, RgbaImage};

    /// Structured synthetic "photo": diagonal gradient, a disc, a bar and a
    /// soft texture.
    pub fn scene(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let mut r = 40.0 + 150.0 * fx;
            let mut g = 60.0 + 120.0 * fy;
            let mut b = 200.0 - 120.0 * (fx + fy) / 2.0;
            let (dx, dy) = (fx - 0.35, fy - 0.4);
            if dx * dx + dy * dy < 0.04 {
                r = 230.0;
                g = 210.0;
                b = 40.0;
            }
            if (0.6..0.8).contains(&fx) && (0.15..0.85).contains(&fy) {
                r *= 0.4;
                g *= 0.5;
                b = 180.0;
            }
            let t = 10.0 * ((fx * 37.0).sin() * (fy * 23.0).cos());
            Rgba([
                (r + t).clamp(0.0, 255.0) as u8,
                (g + t).clamp(0.0, 255.0) as u8,
                (b + t).clamp(0.0, 255.0) as u8,
                255,
            ])
        })
    }

    pub fn checkerboard(w: u32, h: u32, cell: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            if (x / cell + y / cell) & 1 == 0 {
                Rgba([240, 240, 240, 255])
            } else {
                Rgba([20, 30, 40, 255])
            }
        })
    }

    /// Concentric rings around the lower-right corner.
    pub fn rings(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let (dx, dy) = (x as f32 - w as f32 * 0.8, y as f32 - h as f32 * 0.75);
            let d = (dx * dx + dy * dy).sqrt() / w as f32;
            let v = (128.0 + 110.0 * (d * 40.0).sin()) as u8;
            Rgba([v, 255 - v, v / 2, 255])
        })
    }

    /// Text-like horizontal stripes of varying thickness on white.
    pub fn stripes(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let row = y * 16 / h;
            let on = row % 3 != 2 && (x * 7 / w + row) & 3 != 0;
            if on && (y % (h / 16).max(1)) < (h / 32).max(1) {
                Rgba([10, 10, 10, 255])
            } else {
                Rgba([250, 250, 245, 255])
            }
        })
    }

    pub fn jpeg_round_trip(img: &RgbaImage, q: u8) -> RgbaImage {
        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q)
            .encode_image(&rgb)
            .unwrap();
        image::load_from_memory(&out).unwrap().to_rgba8()
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use image::imageops::{resize as img_resize, FilterType};

    fn both(img: &image::RgbaImage) -> (u64, u64) {
        (phash(img), dhash(img))
    }

    #[test]
    fn identical_images_have_distance_zero() {
        let a = scene(300, 200);
        let b = a.clone();
        assert_eq!(hamming(phash(&a), phash(&b)), 0);
        assert_eq!(hamming(dhash(&a), dhash(&b)), 0);
    }

    #[test]
    fn resized_and_recompressed_stays_close() {
        for base in [scene(400, 300), rings(320, 320), stripes(256, 384)] {
            let (p0, d0) = both(&base);
            let small = img_resize(&base, base.width() * 3 / 5, base.height() * 3 / 5, FilterType::Lanczos3);
            let big = img_resize(&base, base.width() * 2, base.height() * 2, FilterType::CatmullRom);
            for variant in [
                small.clone(),
                big,
                jpeg_round_trip(&base, 30),
                jpeg_round_trip(&small, 50),
            ] {
                let (p, d) = both(&variant);
                assert!(hamming(p0, p) <= 8, "phash distance {}", hamming(p0, p));
                assert!(hamming(d0, d) <= 8, "dhash distance {}", hamming(d0, d));
            }
        }
    }

    #[test]
    fn unrelated_images_are_far_apart() {
        let imgs = [
            scene(320, 240),
            checkerboard(320, 240, 40),
            rings(320, 240),
            stripes(320, 240),
        ];
        for i in 0..imgs.len() {
            for j in i + 1..imgs.len() {
                let (pi, di) = both(&imgs[i]);
                let (pj, dj) = both(&imgs[j]);
                assert!(hamming(pi, pj) >= 20, "phash {i} vs {j}: {}", hamming(pi, pj));
                assert!(hamming(di, dj) >= 20, "dhash {i} vs {j}: {}", hamming(di, dj));
            }
        }
    }

    #[test]
    fn transparency_is_flattened_on_white() {
        let mut a = scene(64, 64);
        let mut b = a.clone();
        for (x, y, p) in a.enumerate_pixels_mut() {
            if x < 16 && y < 16 {
                *p = image::Rgba([0, 0, 0, 0]);
            }
        }
        for (x, y, p) in b.enumerate_pixels_mut() {
            if x < 16 && y < 16 {
                *p = image::Rgba([255, 255, 255, 255]);
            }
        }
        assert_eq!(phash(&a), phash(&b));
        assert_eq!(dhash(&a), dhash(&b));
    }

    #[test]
    fn hex_round_trip() {
        let h = 0x0123_4567_89ab_cdef;
        assert_eq!(to_hex(h), "0123456789abcdef");
        assert_eq!(from_hex(&to_hex(h)), Some(h));
        assert_eq!(from_hex("xyz"), None);
        assert_eq!(hamming(0, u64::MAX), 64);
    }
}
