//! CLIP image preprocessing, matching Hugging Face `CLIPImageProcessor` for
//! `openai/clip-vit-base-patch32`:
//!
//! 1. flatten alpha onto white (stickers are transparent; CLIP's own
//!    `convert("RGB")` would drop alpha and expose arbitrary colour in fully
//!    transparent pixels, usually black),
//! 2. resize so the shortest side is 224 with bicubic filtering (Catmull-Rom,
//!    antialiased when downscaling, like PIL `BICUBIC`); the long side is
//!    `floor(224 * long / short)`,
//! 3. centre crop 224x224 (offsets `floor((size - 224) / 2)`),
//! 4. scale to `[0,1]` and normalise with the CLIP mean/std,
//! 5. lay out as NCHW (planar R, G, B).
//!
//! Pure Rust and model-free, so it is always compiled and tested.

use image::{imageops, imageops::FilterType, RgbImage, RgbaImage};

use crate::AiError;

/// Side of the square model input.
pub const INPUT_SIZE: u32 = 224;
/// CLIP normalisation (`image_mean` / `image_std` of the OpenAI preprocessor).
pub const CLIP_MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
pub const CLIP_STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];

/// Images more elongated than this are first centre-cropped to this aspect
/// ratio. The 224 crop only ever sees the central `short x short` square, so
/// the result is the same, but a 1x10000 image no longer resizes to 224x2240000.
const MAX_ASPECT: u32 = 3;

/// Composites `rgba` over an opaque white background.
pub fn flatten_alpha(rgba: &RgbaImage) -> RgbImage {
    let mut out = RgbImage::new(rgba.width(), rgba.height());
    for (dst, src) in out.pixels_mut().zip(rgba.pixels()) {
        let a = src[3] as u32;
        for c in 0..3 {
            dst[c] = ((src[c] as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        }
    }
    out
}

/// Output size of a shortest-side resize to `size` (HF `get_resize_output_image_size`).
pub fn resize_dims(width: u32, height: u32, size: u32) -> (u32, u32) {
    let (w, h) = (width as u64, height as u64);
    let s = size as u64;
    if w <= h {
        (size, ((s * h) / w) as u32)
    } else {
        (((s * w) / h) as u32, size)
    }
}

/// Resizes so the shortest side equals `size` (bicubic, antialiased).
pub fn resize_shortest(rgb: &RgbImage, size: u32) -> RgbImage {
    let (w, h) = resize_dims(rgb.width(), rgb.height(), size);
    if (w, h) == rgb.dimensions() {
        return rgb.clone();
    }
    imageops::resize(rgb, w, h, FilterType::CatmullRom)
}

/// Centre crop to `size`x`size` (the image must be at least that large).
pub fn center_crop(rgb: &RgbImage, size: u32) -> RgbImage {
    let left = rgb.width().saturating_sub(size) / 2;
    let top = rgb.height().saturating_sub(size) / 2;
    imageops::crop_imm(rgb, left, top, size.min(rgb.width()), size.min(rgb.height())).to_image()
}

/// Normalised planar float tensor (3 x H x W), row-major.
pub fn to_nchw(rgb: &RgbImage) -> Vec<f32> {
    let (w, h) = rgb.dimensions();
    let plane = (w * h) as usize;
    let mut out = vec![0f32; 3 * plane];
    for (i, p) in rgb.pixels().enumerate() {
        for c in 0..3 {
            out[c * plane + i] = (p[c] as f32 / 255.0 - CLIP_MEAN[c]) / CLIP_STD[c];
        }
    }
    out
}

/// Crops overly elongated images to at most `MAX_ASPECT`:1 around the centre.
fn limit_aspect(rgb: RgbImage) -> RgbImage {
    let (w, h) = rgb.dimensions();
    if w > h.saturating_mul(MAX_ASPECT) {
        let nw = h * MAX_ASPECT;
        imageops::crop_imm(&rgb, (w - nw) / 2, 0, nw, h).to_image()
    } else if h > w.saturating_mul(MAX_ASPECT) {
        let nh = w * MAX_ASPECT;
        imageops::crop_imm(&rgb, 0, (h - nh) / 2, w, nh).to_image()
    } else {
        rgb
    }
}

/// Full pipeline: returns `3 * 224 * 224` floats for one image (batch of 1, NCHW).
pub fn preprocess(rgba: &RgbaImage) -> Result<Vec<f32>, AiError> {
    if rgba.width() == 0 || rgba.height() == 0 {
        return Err(AiError::Inference("empty image".into()));
    }
    let rgb = limit_aspect(flatten_alpha(rgba));
    let resized = resize_shortest(&rgb, INPUT_SIZE);
    let cropped = center_crop(&resized, INPUT_SIZE);
    debug_assert_eq!(cropped.dimensions(), (INPUT_SIZE, INPUT_SIZE));
    Ok(to_nchw(&cropped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, Rgba};

    const PLANE: usize = (INPUT_SIZE * INPUT_SIZE) as usize;

    fn norm(v: u8, c: usize) -> f32 {
        (v as f32 / 255.0 - CLIP_MEAN[c]) / CLIP_STD[c]
    }

    #[test]
    fn alpha_is_flattened_onto_white() {
        let mut img = RgbaImage::new(3, 1);
        img.put_pixel(0, 0, Rgba([0, 0, 0, 0])); // fully transparent black -> white
        img.put_pixel(1, 0, Rgba([255, 0, 0, 255])); // opaque red stays red
        img.put_pixel(2, 0, Rgba([0, 0, 255, 128])); // half blue -> light blue
        let out = flatten_alpha(&img);
        assert_eq!(out.get_pixel(0, 0), &Rgb([255, 255, 255]));
        assert_eq!(out.get_pixel(1, 0), &Rgb([255, 0, 0]));
        assert_eq!(out.get_pixel(2, 0), &Rgb([127, 127, 255]));
    }

    #[test]
    fn resize_keeps_aspect_like_hf() {
        assert_eq!(resize_dims(640, 480, 224), (298, 224));
        assert_eq!(resize_dims(480, 640, 224), (224, 298));
        assert_eq!(resize_dims(100, 100, 224), (224, 224));
        assert_eq!(resize_dims(512, 512, 224), (224, 224));
        // floor, not round: 224 * 300 / 299 = 224.749...
        assert_eq!(resize_dims(300, 299, 224), (224, 224));
    }

    #[test]
    fn output_shape_and_normalisation() {
        let img = RgbaImage::from_pixel(640, 480, Rgba([255, 0, 0, 255]));
        let t = preprocess(&img).unwrap();
        assert_eq!(t.len(), 3 * PLANE);
        // Uniform colour survives resize/crop exactly.
        assert!(t[..PLANE].iter().all(|&v| (v - norm(255, 0)).abs() < 1e-5));
        assert!(t[PLANE..2 * PLANE].iter().all(|&v| (v - norm(0, 1)).abs() < 1e-5));
        assert!(t[2 * PLANE..].iter().all(|&v| (v - norm(0, 2)).abs() < 1e-5));
        // Known values: red channel of pure red ~ 1.93, green of black ~ -1.75.
        assert!((t[0] - 1.930_336).abs() < 1e-4, "{}", t[0]);
        assert!((t[PLANE] - (-1.752_09)).abs() < 1e-4, "{}", t[PLANE]);
    }

    #[test]
    fn transparent_sticker_becomes_white_background() {
        // Transparent canvas with an opaque black square in the middle.
        let mut img = RgbaImage::from_pixel(300, 300, Rgba([0, 0, 0, 0]));
        for y in 100..200 {
            for x in 100..200 {
                img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let t = preprocess(&img).unwrap();
        // Corner is white, centre is black (in every channel).
        let idx = |c: usize, x: usize, y: usize| c * PLANE + y * INPUT_SIZE as usize + x;
        for c in 0..3 {
            assert!((t[idx(c, 0, 0)] - norm(255, c)).abs() < 1e-5);
            assert!((t[idx(c, 112, 112)] - norm(0, c)).abs() < 1e-5);
        }
    }

    #[test]
    fn crop_is_centred_and_channels_are_planar() {
        // Left third red, middle third green, right third blue; 672x224 -> no
        // resize, crop takes exactly the green middle.
        let mut img = RgbaImage::new(672, 224);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = match x / 224 {
                0 => Rgba([255, 0, 0, 255]),
                1 => Rgba([0, 255, 0, 255]),
                _ => Rgba([0, 0, 255, 255]),
            };
        }
        let t = preprocess(&img).unwrap();
        assert!(t[..PLANE].iter().all(|&v| (v - norm(0, 0)).abs() < 1e-5));
        assert!(t[PLANE..2 * PLANE].iter().all(|&v| (v - norm(255, 1)).abs() < 1e-5));
        assert!(t[2 * PLANE..].iter().all(|&v| (v - norm(0, 2)).abs() < 1e-5));
    }

    #[test]
    fn tiny_and_extreme_images() {
        assert!(preprocess(&RgbaImage::new(0, 10)).is_err());
        let t = preprocess(&RgbaImage::from_pixel(1, 1, Rgba([10, 20, 30, 255]))).unwrap();
        assert_eq!(t.len(), 3 * PLANE);
        // 2 x 5000 would be 224 x 560000 without the aspect limit.
        let t = preprocess(&RgbaImage::from_pixel(2, 5000, Rgba([10, 20, 30, 255]))).unwrap();
        assert_eq!(t.len(), 3 * PLANE);
        assert!((t[0] - norm(10, 0)).abs() < 1e-5);
        let t = preprocess(&RgbaImage::from_pixel(5000, 3, Rgba([10, 20, 30, 255]))).unwrap();
        assert_eq!(t.len(), 3 * PLANE);
    }

    #[test]
    fn center_crop_offsets() {
        let mut img = RgbImage::new(5, 3);
        img.put_pixel(1, 0, Rgb([9, 9, 9]));
        let c = center_crop(&img, 3);
        assert_eq!(c.dimensions(), (3, 3));
        assert_eq!(c.get_pixel(0, 0), &Rgb([9, 9, 9]));
    }
}
