//! 64-bit perceptual hashes.

/// DCT-based pHash of a frame (32x32 grey, low 8x8 DCT block vs median).
pub fn phash(img: &image::RgbaImage) -> u64 {
    let _ = img;
    todo!("media agent")
}

/// Difference hash (9x8 grey, horizontal gradients).
pub fn dhash(img: &image::RgbaImage) -> u64 {
    let _ = img;
    todo!("media agent")
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
