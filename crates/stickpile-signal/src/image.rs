//! Header-level facts about an image file: format, size and animation, read
//! without decoding pixels. Enough to check a file against a platform's rules
//! before uploading it. (The same module as `stickpile_discord::image`; the
//! two crates do not depend on each other.)

use serde::{Deserialize, Serialize};

/// Image container formats the platforms accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Png,
    /// PNG with an `acTL` chunk (animated PNG).
    Apng,
    Gif,
    Jpeg,
    Webp,
}

impl ImageFormat {
    pub fn mime(self) -> &'static str {
        match self {
            ImageFormat::Png | ImageFormat::Apng => "image/png",
            ImageFormat::Gif => "image/gif",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Webp => "image/webp",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png | ImageFormat::Apng => "png",
            ImageFormat::Gif => "gif",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG",
            ImageFormat::Apng => "APNG",
            ImageFormat::Gif => "GIF",
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Webp => "WebP",
        }
    }
}

/// What [`inspect`] found out about a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    /// More than one frame.
    pub animated: bool,
    /// Sum of the frame delays of an animation (frame delays of 10 ms or
    /// less count as 100 ms, as in browsers).
    pub duration_ms: Option<u32>,
    /// For animations that declare it: whether they loop forever.
    pub loops_forever: Option<bool>,
}

/// Format, dimensions and animation of `bytes`, or `None` when the file is
/// not a PNG, GIF, JPEG or WebP or its header is broken.
pub fn inspect(bytes: &[u8]) -> Option<ImageInfo> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(bytes)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        gif(bytes)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        jpeg(bytes)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        webp(bytes)
    } else {
        None
    }
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn le24(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 3)?;
    Some(u32::from(s[0]) | u32::from(s[1]) << 8 | u32::from(s[2]) << 16)
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Browsers show frames of 10 ms or less for 100 ms.
fn frame_ms(ms: u32) -> u32 {
    if ms <= 10 {
        100
    } else {
        ms
    }
}

fn png(b: &[u8]) -> Option<ImageInfo> {
    // Signature, then IHDR must be the first chunk.
    if be32(b, 8)? != 13 || b.get(12..16)? != b"IHDR" {
        return None;
    }
    let width = be32(b, 16)?;
    let height = be32(b, 20)?;
    let mut at = 8;
    let mut frames: Option<u32> = None;
    let mut plays = None;
    let mut duration: u32 = 0;
    while at + 8 <= b.len() {
        let len = be32(b, at)? as usize;
        let kind = &b[at + 4..at + 8];
        let data = b.get(at + 8..(at + 8).checked_add(len)?);
        match (kind, data) {
            (b"acTL", Some(d)) if d.len() >= 8 => {
                frames = Some(be32(d, 0)?);
                plays = Some(be32(d, 4)?);
            }
            (b"fcTL", Some(d)) if d.len() >= 26 => {
                let num = u32::from(be16(d, 20)?);
                let den = match be16(d, 22)? {
                    0 => 100,
                    den => u32::from(den),
                };
                duration = duration.saturating_add(frame_ms(num * 1000 / den));
            }
            (b"IEND", _) | (_, None) => break,
            _ => {}
        }
        at += 12 + len;
    }
    let animated = frames.is_some_and(|n| n > 1);
    Some(ImageInfo {
        format: if frames.is_some() {
            ImageFormat::Apng
        } else {
            ImageFormat::Png
        },
        width,
        height,
        animated,
        duration_ms: animated.then_some(duration),
        loops_forever: plays.map(|p| p == 0),
    })
}

fn gif(b: &[u8]) -> Option<ImageInfo> {
    let width = u32::from(le16(b, 6)?);
    let height = u32::from(le16(b, 8)?);
    let packed = *b.get(10)?;
    let mut at = 13;
    if packed & 0x80 != 0 {
        at += 3 << ((packed & 7) + 1);
    }
    let mut frames = 0u32;
    let mut duration: u32 = 0;
    let mut delay_cs: u32 = 0;
    let mut loops = None;
    // Skips data sub-blocks starting at `at`; returns the position after the terminator.
    let skip = |mut at: usize| -> Option<usize> {
        loop {
            let n = *b.get(at)? as usize;
            at += 1 + n;
            if n == 0 {
                return Some(at);
            }
        }
    };
    while let Some(&block) = b.get(at) {
        match block {
            0x21 => {
                let label = *b.get(at + 1)?;
                if label == 0xF9 && b.get(at + 2) == Some(&4) {
                    delay_cs = u32::from(le16(b, at + 4)?);
                } else if label == 0xFF
                    && b.get(at + 2) == Some(&11)
                    && b.get(at + 3..at + 14) == Some(b"NETSCAPE2.0")
                    && b.get(at + 14) == Some(&3)
                    && b.get(at + 15) == Some(&1)
                {
                    loops = Some(le16(b, at + 16)? == 0);
                }
                at = skip(at + 2)?;
            }
            0x2C => {
                frames += 1;
                duration = duration.saturating_add(frame_ms(delay_cs * 10));
                delay_cs = 0;
                let local = *b.get(at + 9)?;
                at += 10;
                if local & 0x80 != 0 {
                    at += 3 << ((local & 7) + 1);
                }
                // LZW minimum code size, then the image data sub-blocks.
                at = skip(at + 1)?;
            }
            _ => break, // 0x3B trailer, or garbage
        }
    }
    let animated = frames > 1;
    Some(ImageInfo {
        format: ImageFormat::Gif,
        width,
        height,
        animated,
        duration_ms: animated.then_some(duration),
        loops_forever: if animated { Some(loops.unwrap_or(false)) } else { None },
    })
}

fn jpeg(b: &[u8]) -> Option<ImageInfo> {
    let mut at = 2;
    while at + 4 <= b.len() {
        if b[at] != 0xFF {
            return None;
        }
        let marker = b[at + 1];
        if marker == 0xFF {
            at += 1;
            continue;
        }
        if matches!(marker, 0x01 | 0xD0..=0xD8) {
            at += 2;
            continue;
        }
        let len = usize::from(be16(b, at + 2)?);
        let is_sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            let height = u32::from(be16(b, at + 5)?);
            let width = u32::from(be16(b, at + 7)?);
            return Some(ImageInfo {
                format: ImageFormat::Jpeg,
                width,
                height,
                animated: false,
                duration_ms: None,
                loops_forever: None,
            });
        }
        if marker == 0xDA {
            return None;
        }
        at += 2 + len;
    }
    None
}

fn webp(b: &[u8]) -> Option<ImageInfo> {
    let first = b.get(12..16)?;
    let data = b.get(20..)?;
    let (width, height, animated_flag) = match first {
        b"VP8 " => {
            if data.get(3..6)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            (
                u32::from(le16(data, 6)? & 0x3FFF),
                u32::from(le16(data, 8)? & 0x3FFF),
                false,
            )
        }
        b"VP8L" => {
            if *data.first()? != 0x2F {
                return None;
            }
            let bits = le32(data, 1)?;
            ((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1, false)
        }
        b"VP8X" => (le24(data, 4)? + 1, le24(data, 7)? + 1, data.first()? & 0x02 != 0),
        _ => return None,
    };
    let mut frames = 0u32;
    let mut duration: u32 = 0;
    let mut loops = None;
    if animated_flag {
        let mut at = 12;
        while at + 8 <= b.len() {
            let kind = &b[at..at + 4];
            let len = le32(b, at + 4)? as usize;
            let Some(d) = b.get(at + 8..(at + 8).checked_add(len)?) else {
                break;
            };
            match kind {
                b"ANIM" if d.len() >= 6 => loops = Some(le16(d, 4)? == 0),
                b"ANMF" if d.len() >= 16 => {
                    frames += 1;
                    duration = duration.saturating_add(frame_ms(le24(d, 12)?));
                }
                _ => {}
            }
            at += 8 + len + (len & 1);
        }
    }
    let animated = frames > 1;
    Some(ImageInfo {
        format: ImageFormat::Webp,
        width,
        height,
        animated,
        duration_ms: animated.then_some(duration),
        loops_forever: if animated { loops } else { None },
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn png(w: u32, h: u32, frames: Option<(u32, u32, u16)>) -> Vec<u8> {
        fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
            out.extend((data.len() as u32).to_be_bytes());
            out.extend(kind);
            out.extend(data);
            out.extend([0, 0, 0, 0]); // CRC is not checked here
        }
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend(h.to_be_bytes());
        ihdr.extend([8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        if let Some((n, plays, delay_ms)) = frames {
            let mut actl = n.to_be_bytes().to_vec();
            actl.extend(plays.to_be_bytes());
            chunk(&mut out, b"acTL", &actl);
            for i in 0..n {
                let mut fctl = i.to_be_bytes().to_vec();
                fctl.extend(w.to_be_bytes());
                fctl.extend(h.to_be_bytes());
                fctl.extend([0; 8]);
                fctl.extend(delay_ms.to_be_bytes());
                fctl.extend(1000u16.to_be_bytes());
                fctl.extend([0, 0]);
                chunk(&mut out, b"fcTL", &fctl);
            }
        }
        chunk(&mut out, b"IDAT", &[1, 2, 3]);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    pub fn gif(w: u16, h: u16, frames: u32, delay_cs: u16, looping: bool) -> Vec<u8> {
        let mut out = b"GIF89a".to_vec();
        out.extend(w.to_le_bytes());
        out.extend(h.to_le_bytes());
        out.extend([0x80, 0, 0]); // 2-colour global table
        out.extend([0, 0, 0, 255, 255, 255]);
        if looping {
            out.extend([0x21, 0xFF, 11]);
            out.extend(b"NETSCAPE2.0");
            out.extend([3, 1, 0, 0, 0]);
        }
        for _ in 0..frames {
            out.extend([0x21, 0xF9, 4, 0]);
            out.extend(delay_cs.to_le_bytes());
            out.extend([0, 0]);
            out.push(0x2C);
            out.extend([0, 0, 0, 0]);
            out.extend(w.to_le_bytes());
            out.extend(h.to_le_bytes());
            out.push(0);
            out.extend([2, 2, 0x4C, 0x01, 0]);
        }
        out.push(0x3B);
        out
    }

    #[test]
    fn png_and_apng() {
        let still = inspect(&png(320, 300, None)).unwrap();
        assert_eq!(
            (still.format, still.width, still.height, still.animated),
            (ImageFormat::Png, 320, 300, false)
        );
        let anim = inspect(&png(320, 320, Some((4, 0, 250)))).unwrap();
        assert_eq!(anim.format, ImageFormat::Apng);
        assert!(anim.animated);
        assert_eq!(anim.duration_ms, Some(1000));
        assert_eq!(anim.loops_forever, Some(true));
        let once = inspect(&png(320, 320, Some((2, 1, 100)))).unwrap();
        assert_eq!(once.loops_forever, Some(false));
        assert!(inspect(b"\x89PNG\r\n\x1a\n\0\0").is_none());
    }

    #[test]
    fn gifs() {
        let still = inspect(&gif(128, 64, 1, 0, false)).unwrap();
        assert_eq!((still.width, still.height, still.animated), (128, 64, false));
        let anim = inspect(&gif(320, 320, 3, 50, true)).unwrap();
        assert!(anim.animated);
        assert_eq!(anim.duration_ms, Some(1500));
        assert_eq!(anim.loops_forever, Some(true));
        // Zero delays count as 100 ms.
        assert_eq!(inspect(&gif(8, 8, 4, 0, true)).unwrap().duration_ms, Some(400));
    }

    #[test]
    fn jpeg_and_webp() {
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0];
        jpeg.extend([0xFF, 0xC0, 0, 11, 8, 0, 100, 0, 200, 3]);
        let info = inspect(&jpeg).unwrap();
        assert_eq!((info.format, info.width, info.height), (ImageFormat::Jpeg, 200, 100));

        let mut vp8l = b"RIFF\0\0\0\0WEBPVP8L\x05\0\0\0\x2f".to_vec();
        let bits: u32 = 511 | (255 << 14);
        vp8l.extend(bits.to_le_bytes());
        let info = inspect(&vp8l).unwrap();
        assert_eq!((info.format, info.width, info.height), (ImageFormat::Webp, 512, 256));

        let mut vp8x = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0".to_vec();
        vp8x.extend([0x02, 0, 0, 0, 0xFF, 0x01, 0, 0xFF, 0x01, 0]);
        for _ in 0..2 {
            vp8x.extend(b"ANMF\x10\0\0\0");
            vp8x.extend([0; 12]);
            vp8x.extend([200, 0, 0, 0]);
        }
        let info = inspect(&vp8x).unwrap();
        assert_eq!((info.width, info.height, info.animated), (512, 512, true));
        assert_eq!(info.duration_ms, Some(400));
        assert!(inspect(b"RIFF\0\0\0\0WEBPXXXX").is_none());
        assert!(inspect(b"not an image").is_none());
    }
}
