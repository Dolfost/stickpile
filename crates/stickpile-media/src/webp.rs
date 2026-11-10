//! WebP RIFF parsing and whitelist rebuild.

use crate::MediaError;

const KEEP: &[&[u8; 4]] = &[b"VP8 ", b"VP8L", b"VP8X", b"ALPH", b"ANIM", b"ANMF"];
const KEEP_IN_FRAME: &[&[u8; 4]] = &[b"VP8 ", b"VP8L", b"ALPH"];

/// VP8X flag bits.
const FLAG_ICC: u8 = 0x20;
const FLAG_ALPHA: u8 = 0x10;
const FLAG_EXIF: u8 = 0x08;
const FLAG_XMP: u8 = 0x04;
const FLAG_ANIM: u8 = 0x02;

fn invalid(msg: &str) -> MediaError {
    MediaError::Invalid(format!("webp: {msg}"))
}

pub(crate) struct Chunk<'a> {
    pub fourcc: [u8; 4],
    pub payload: &'a [u8],
}

fn chunks(mut data: &[u8]) -> Result<Vec<Chunk<'_>>, MediaError> {
    let mut out = Vec::new();
    while !data.is_empty() {
        if data.len() < 8 {
            return Err(invalid("truncated chunk header"));
        }
        let fourcc: [u8; 4] = data[0..4].try_into().expect("4 bytes");
        let size = u32::from_le_bytes(data[4..8].try_into().expect("4 bytes")) as usize;
        let end = 8usize.checked_add(size).ok_or_else(|| invalid("chunk size overflow"))?;
        if end > data.len() {
            return Err(invalid("chunk exceeds file"));
        }
        out.push(Chunk {
            fourcc,
            payload: &data[8..end],
        });
        let padded = (end + (size & 1)).min(data.len());
        data = &data[padded..];
    }
    Ok(out)
}

/// The chunk list of a RIFF/WEBP file. Bytes after the RIFF size are ignored.
pub(crate) fn parse(bytes: &[u8]) -> Result<Vec<Chunk<'_>>, MediaError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(invalid("not a RIFF/WEBP file"));
    }
    let riff_size = u32::from_le_bytes(bytes[4..8].try_into().expect("4 bytes")) as usize;
    let end = riff_size.checked_add(8).ok_or_else(|| invalid("size overflow"))?;
    if end > bytes.len() || riff_size < 4 {
        return Err(invalid("RIFF size exceeds file"));
    }
    chunks(&bytes[12..end])
}

fn push_chunk(out: &mut Vec<u8>, fourcc: &[u8], payload: &[u8]) {
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
}

/// Facts read from the chunk structure.
#[derive(Debug, Default, Clone)]
pub(crate) struct Layout {
    /// Lossy (VP8) bitstream present anywhere.
    pub lossy: bool,
    pub animated: bool,
    /// Frame durations (ms) from ANMF chunks.
    pub durations: Vec<u32>,
}

/// Rebuild keeping only VP8/VP8L/VP8X/ALPH/ANIM/ANMF chunks (and within
/// ANMF only ALPH/VP8/VP8L), clearing ICC/EXIF/XMP flags in VP8X and
/// recomputing every size field.
pub(crate) fn rebuild(bytes: &[u8]) -> Result<(Vec<u8>, Layout), MediaError> {
    let list = parse(bytes)?;
    let mut body = Vec::with_capacity(bytes.len());
    let mut layout = Layout::default();
    let mut has_image = false;
    for (i, c) in list.iter().enumerate() {
        if !KEEP.contains(&&c.fourcc) {
            continue;
        }
        match &c.fourcc {
            b"VP8X" => {
                if i != 0 || c.payload.len() < 10 {
                    return Err(invalid("misplaced or short VP8X"));
                }
                let mut p = c.payload[..10].to_vec();
                p[0] &= FLAG_ALPHA | FLAG_ANIM;
                debug_assert_eq!(p[0] & (FLAG_ICC | FLAG_EXIF | FLAG_XMP), 0);
                layout.animated = p[0] & FLAG_ANIM != 0;
                push_chunk(&mut body, b"VP8X", &p);
            }
            b"ANMF" => {
                if c.payload.len() < 16 {
                    return Err(invalid("short ANMF"));
                }
                let header = &c.payload[..16];
                let duration = u32::from_le_bytes([header[12], header[13], header[14], 0]);
                layout.durations.push(duration);
                let mut p = header.to_vec();
                for sub in chunks(&c.payload[16..])? {
                    if KEEP_IN_FRAME.contains(&&sub.fourcc) {
                        if &sub.fourcc == b"VP8 " {
                            layout.lossy = true;
                        }
                        if &sub.fourcc != b"ALPH" {
                            has_image = true;
                        }
                        push_chunk(&mut p, &sub.fourcc, sub.payload);
                    }
                }
                push_chunk(&mut body, b"ANMF", &p);
            }
            fourcc => {
                if fourcc == b"VP8 " {
                    layout.lossy = true;
                }
                if fourcc == b"VP8 " || fourcc == b"VP8L" {
                    has_image = true;
                }
                push_chunk(&mut body, fourcc, c.payload);
            }
        }
    }
    if !has_image {
        return Err(invalid("no image data"));
    }
    let mut out = Vec::with_capacity(body.len() + 12);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    Ok((out, layout))
}

/// Structure facts without rebuilding (used by the quality score).
pub(crate) fn layout(bytes: &[u8]) -> Option<Layout> {
    rebuild(bytes).ok().map(|(_, l)| l)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn still(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([
                (x * 7) as u8,
                (y * 5) as u8,
                90,
                if alpha && x < w / 2 { 0 } else { 255 },
            ])
        });
        webp::Encoder::from_rgba(&img, w, h).encode(80.0).to_vec()
    }

    /// Wrap a simple-format WebP into VP8X with ICC/EXIF/XMP chunks.
    fn with_metadata(simple: &[u8]) -> Vec<u8> {
        let list = parse(simple).unwrap();
        let mut body = Vec::new();
        let mut vp8x = vec![FLAG_ICC | FLAG_EXIF | FLAG_XMP | FLAG_ALPHA, 0, 0, 0];
        vp8x.extend_from_slice(&[15, 0, 0, 9, 0, 0]); // 16x10
        push_chunk(&mut body, b"VP8X", &vp8x);
        push_chunk(&mut body, b"ICCP", b"SECRET-ICC");
        for c in list.iter().filter(|c| &c.fourcc != b"VP8X") {
            push_chunk(&mut body, &c.fourcc, c.payload);
        }
        push_chunk(&mut body, b"EXIF", b"SECRET-EXIF-GPS");
        push_chunk(&mut body, b"XMP ", b"<x:xmpmeta>SECRET</x:xmpmeta>");
        push_chunk(&mut body, b"ZZZZ", b"SECRET-UNKNOWN");
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn strips_metadata_chunks_and_flags() {
        let simple = still(16, 10, true);
        let mut dirty = with_metadata(&simple);
        dirty.extend_from_slice(b"<html><script>alert(1)</script></html>");
        let (clean, layout) = rebuild(&dirty).unwrap();
        assert!(!String::from_utf8_lossy(&clean).contains("SECRET"));
        assert!(!String::from_utf8_lossy(&clean).contains("script"));
        assert!(layout.lossy);
        assert!(!layout.animated);
        let list = parse(&clean).unwrap();
        let flags = list[0].payload[0];
        assert_eq!(flags, FLAG_ALPHA);
        assert_eq!(
            u32::from_le_bytes(clean[4..8].try_into().unwrap()) as usize + 8,
            clean.len()
        );
        let a = image::load_from_memory(&simple).unwrap().to_rgba8();
        let b = image::load_from_memory(&clean).unwrap().to_rgba8();
        assert_eq!(a, b);
    }

    #[test]
    fn simple_file_round_trips() {
        let simple = still(9, 7, false);
        let (clean, _) = rebuild(&simple).unwrap();
        assert_eq!(clean, simple);
    }

    #[test]
    fn rejects_broken_structure() {
        assert!(rebuild(b"RIFF\x04\0\0\0WEBP").is_err());
        assert!(rebuild(b"RIFF\xff\xff\0\0WEBPVP8 ").is_err());
        let mut s = still(8, 8, false);
        let n = s.len();
        s[16..20].copy_from_slice(&(n as u32 * 2).to_le_bytes());
        assert!(rebuild(&s).is_err());
    }
}
