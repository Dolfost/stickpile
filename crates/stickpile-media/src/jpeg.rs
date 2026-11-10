//! JPEG marker parsing and whitelist rebuild (lossless metadata stripping),
//! plus quantisation-table extraction for the quality score.

use crate::MediaError;

/// Natural (row-major) index of the n-th coefficient in zig-zag order.
pub(crate) const ZIGZAG_TO_NATURAL: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21,
    28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54,
    47, 55, 62, 63,
];

const SOI: u8 = 0xD8;
const EOI: u8 = 0xD9;
const SOS: u8 = 0xDA;
const DQT: u8 = 0xDB;
const DHT: u8 = 0xC4;
const DAC: u8 = 0xCC;
const DRI: u8 = 0xDD;
const APP1: u8 = 0xE1;
const APP14: u8 = 0xEE;

fn is_sof(m: u8) -> bool {
    matches!(m, 0xC0..=0xCF) && m != DHT && m != 0xC8 && m != DAC
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Segment<'a> {
    /// A marker segment with its payload (without the length field).
    Marker { marker: u8, payload: &'a [u8] },
    /// SOS header payload plus the entropy-coded data that follows it
    /// (including RST markers and stuffed bytes).
    Scan { header: &'a [u8], data: &'a [u8] },
}

#[derive(Debug)]
pub(crate) struct Parsed<'a> {
    pub segments: Vec<Segment<'a>>,
}

fn invalid(msg: &str) -> MediaError {
    MediaError::Invalid(format!("jpeg: {msg}"))
}

/// Split a JPEG into segments. Bytes between segments that are not markers
/// are skipped (libjpeg does the same), anything after EOI is ignored. A
/// stream that ends without EOI (truncated) is accepted here; the decoder
/// decides whether the pixels are usable.
pub(crate) fn parse(bytes: &[u8]) -> Result<Parsed<'_>, MediaError> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != SOI {
        return Err(invalid("missing SOI"));
    }
    let mut segments = Vec::new();
    let mut pos = 2;
    loop {
        // Find the next 0xFF, skip fill bytes.
        while pos < bytes.len() && bytes[pos] != 0xFF {
            pos += 1;
        }
        while pos < bytes.len() && bytes[pos] == 0xFF {
            pos += 1;
        }
        if pos >= bytes.len() {
            return Ok(Parsed { segments });
        }
        let marker = bytes[pos];
        pos += 1;
        match marker {
            EOI => return Ok(Parsed { segments }),
            SOI => return Err(invalid("nested SOI")),
            0x00 | 0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }
        if pos + 2 > bytes.len() {
            return Err(invalid("truncated segment header"));
        }
        let len = usize::from(u16::from_be_bytes([bytes[pos], bytes[pos + 1]]));
        if len < 2 || pos + len > bytes.len() {
            return Err(invalid("segment length out of range"));
        }
        let payload = &bytes[pos + 2..pos + len];
        pos += len;
        if marker != SOS {
            segments.push(Segment::Marker { marker, payload });
            continue;
        }
        // Entropy-coded data runs until a marker that is neither a stuffed
        // zero nor a restart marker.
        let start = pos;
        let mut end = bytes.len();
        let mut i = pos;
        while i + 1 < bytes.len() {
            if bytes[i] == 0xFF {
                let next = bytes[i + 1];
                if next == 0x00 || (0xD0..=0xD7).contains(&next) {
                    i += 2;
                    continue;
                }
                if next == 0xFF {
                    // Fill byte before a marker.
                    i += 1;
                    continue;
                }
                end = i;
                break;
            }
            i += 1;
        }
        segments.push(Segment::Scan {
            header: payload,
            data: &bytes[start..end],
        });
        pos = end;
    }
}

/// EXIF orientation (1..=8) from an APP1 "Exif" segment, if any.
pub(crate) fn exif_orientation(parsed: &Parsed<'_>) -> Option<u8> {
    parsed.segments.iter().find_map(|s| match s {
        Segment::Marker { marker: APP1, payload } if payload.starts_with(b"Exif\0\0") => {
            image::metadata::Orientation::from_exif_chunk(&payload[6..]).map(|o| o.to_exif())
        }
        _ => None,
    })
}

/// Adobe APP14 colour transform byte, if present.
fn adobe_transform(parsed: &Parsed<'_>) -> Option<u8> {
    parsed.segments.iter().find_map(|s| match s {
        Segment::Marker { marker: APP14, payload } if payload.len() >= 12 && payload.starts_with(b"Adobe") => {
            Some(payload[11])
        }
        _ => None,
    })
}

fn push_segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(payload);
}

/// Minimal big-endian EXIF block holding only the Orientation tag.
fn orientation_exif(value: u8) -> Vec<u8> {
    let mut v = b"Exif\0\0MM\0\x2a\0\0\0\x08".to_vec();
    v.extend_from_slice(&[0, 1]); // one IFD entry
    v.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, value, 0, 0]);
    v.extend_from_slice(&[0, 0, 0, 0]); // no next IFD
    v
}

/// Rebuild a JPEG from whitelisted segments only: SOI, DQT, DHT, DAC, SOFn,
/// DRI, SOS + entropy data, EOI. Every APPn and COM segment and everything
/// after EOI is dropped. Two synthetic segments without user data may be
/// emitted so the image still renders the same: a canonical Adobe APP14 with
/// the original colour transform byte, and a minimal EXIF with only the
/// orientation tag (when it is not 1).
pub(crate) fn rebuild(bytes: &[u8]) -> Result<(Vec<u8>, u8), MediaError> {
    let parsed = parse(bytes)?;
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&[0xFF, SOI]);
    let orientation = exif_orientation(&parsed).filter(|o| (1..=8).contains(o)).unwrap_or(1);
    if orientation != 1 {
        push_segment(&mut out, APP1, &orientation_exif(orientation));
    }
    if let Some(t) = adobe_transform(&parsed) {
        let mut p = b"Adobe".to_vec();
        p.extend_from_slice(&[0, 100, 0, 0, 0, 0, t]);
        push_segment(&mut out, APP14, &p);
    }
    let mut sofs = 0;
    let mut scans = 0;
    for seg in &parsed.segments {
        match *seg {
            Segment::Marker { marker, payload } if marker == DQT || marker == DHT || marker == DAC || marker == DRI => {
                push_segment(&mut out, marker, payload)
            }
            Segment::Marker { marker, payload } if is_sof(marker) => {
                sofs += 1;
                push_segment(&mut out, marker, payload)
            }
            Segment::Marker { .. } => {}
            Segment::Scan { header, data } => {
                if sofs == 0 {
                    return Err(invalid("scan before frame header"));
                }
                scans += 1;
                push_segment(&mut out, SOS, header);
                out.extend_from_slice(data);
            }
        }
    }
    if sofs != 1 {
        return Err(invalid("expected exactly one frame header"));
    }
    if scans == 0 {
        return Err(invalid("no image data"));
    }
    out.extend_from_slice(&[0xFF, EOI]);
    Ok((out, orientation))
}

/// One quantisation table in natural (row-major) order.
#[derive(Debug, Clone)]
pub(crate) struct QuantTable {
    pub id: u8,
    pub natural: [u16; 64],
    /// Values exactly as stored in the file (normally zig-zag order).
    pub stored: [u16; 64],
}

/// All quantisation tables of the file, and the table id used by the first
/// (luma) component of the frame header.
pub(crate) fn quant_tables(bytes: &[u8]) -> Option<(Vec<QuantTable>, Option<u8>)> {
    let parsed = parse(bytes).ok()?;
    let mut tables: Vec<QuantTable> = Vec::new();
    let mut luma_id = None;
    for seg in &parsed.segments {
        match *seg {
            Segment::Marker { marker: DQT, payload } => {
                let mut p = payload;
                while !p.is_empty() {
                    let pq = p[0] >> 4;
                    let id = p[0] & 0x0F;
                    let n = if pq == 0 { 64 } else { 128 };
                    if p.len() < 1 + n {
                        return None;
                    }
                    let mut stored = [0u16; 64];
                    for (i, v) in stored.iter_mut().enumerate() {
                        *v = if pq == 0 {
                            u16::from(p[1 + i])
                        } else {
                            u16::from_be_bytes([p[1 + 2 * i], p[2 + 2 * i]])
                        };
                    }
                    let mut natural = [0u16; 64];
                    for (zz, nat) in ZIGZAG_TO_NATURAL.iter().enumerate() {
                        natural[*nat] = stored[zz];
                    }
                    tables.retain(|t| t.id != id);
                    tables.push(QuantTable { id, natural, stored });
                    p = &p[1 + n..];
                }
            }
            // P, Y, X, Nf, then Nf * (C, H/V, Tq).
            Segment::Marker { marker, payload } if is_sof(marker) && luma_id.is_none() && payload.len() >= 9 => {
                luma_id = Some(payload[8]);
            }
            _ => {}
        }
    }
    (!tables.is_empty()).then_some((tables, luma_id))
}

/// The luminance quantisation table (natural order) and its stored form.
pub(crate) fn luma_table(bytes: &[u8]) -> Option<QuantTable> {
    let (tables, luma_id) = quant_tables(bytes)?;
    let want = luma_id.unwrap_or(0);
    tables.iter().find(|t| t.id == want).or_else(|| tables.first()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample_jpeg(q: u8) -> Vec<u8> {
        let img = image::RgbImage::from_fn(40, 24, |x, y| image::Rgb([(x * 6) as u8, (y * 10) as u8, 128]));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q)
            .encode_image(&img)
            .unwrap();
        out
    }

    /// Insert `segments` (complete, with markers) right after SOI.
    fn with_segments(jpeg: &[u8], segments: &[Vec<u8>]) -> Vec<u8> {
        let mut out = jpeg[..2].to_vec();
        for s in segments {
            out.extend_from_slice(s);
        }
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    fn seg(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        push_segment(&mut v, marker, payload);
        v
    }

    fn exif_with_orientation(o: u8) -> Vec<u8> {
        // Little-endian EXIF with orientation plus a fake GPS tag pointer and a maker string.
        let mut p = b"Exif\0\0II\x2a\0\x08\0\0\0".to_vec();
        p.extend_from_slice(&[2, 0]);
        p.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, o, 0, 0, 0]);
        p.extend_from_slice(&[0x25, 0x88, 4, 0, 1, 0, 0, 0, 0x40, 0, 0, 0]);
        p.extend_from_slice(&[0, 0, 0, 0]);
        p.extend_from_slice(b"SECRET-GPS-48.8584N-2.2945E");
        seg(APP1, &p)
    }

    #[test]
    fn rebuild_strips_metadata_and_trailer() {
        let base = sample_jpeg(80);
        let mut dirty = with_segments(
            &base,
            &[
                seg(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"),
                exif_with_orientation(1),
                seg(0xE2, b"ICC_PROFILE\0\x01\x01SECRET-ICC"),
                seg(0xFE, b"SECRET-COMMENT"),
                seg(0xED, b"Photoshop 3.0\0SECRET-IPTC"),
            ],
        );
        dirty.extend_from_slice(b"PK\x03\x04SECRET-ZIP-PAYLOAD");
        let (clean, orientation) = rebuild(&dirty).unwrap();
        assert_eq!(orientation, 1);
        let hay = String::from_utf8_lossy(&clean);
        assert!(!hay.contains("SECRET"), "metadata survived");
        assert!(!hay.contains("JFIF"));
        assert!(clean.ends_with(&[0xFF, EOI]));
        // Entropy data unchanged: decoding gives identical pixels.
        let a = image::load_from_memory(&base).unwrap().to_rgb8();
        let b = image::load_from_memory(&clean).unwrap().to_rgb8();
        assert_eq!(a, b);
        // Idempotent.
        assert_eq!(rebuild(&clean).unwrap().0, clean);
    }

    #[test]
    fn rebuild_keeps_orientation_only() {
        let base = sample_jpeg(80);
        let dirty = with_segments(&base, &[exif_with_orientation(6)]);
        let (clean, orientation) = rebuild(&dirty).unwrap();
        assert_eq!(orientation, 6);
        assert!(!String::from_utf8_lossy(&clean).contains("SECRET"));
        let parsed = parse(&clean).unwrap();
        assert_eq!(exif_orientation(&parsed), Some(6));
    }

    #[test]
    fn rebuild_keeps_adobe_transform() {
        let base = sample_jpeg(80);
        let dirty = with_segments(&base, &[seg(APP14, b"Adobe\0\x64\x80\0\0\0\x01SECRET")]);
        let (clean, _) = rebuild(&dirty).unwrap();
        let parsed = parse(&clean).unwrap();
        assert_eq!(adobe_transform(&parsed), Some(1));
        assert!(!String::from_utf8_lossy(&clean).contains("SECRET"));
    }

    #[test]
    fn rejects_garbage() {
        assert!(rebuild(b"\xFF\xD8\xFF").is_err());
        assert!(rebuild(b"\xFF\xD8\xFF\xDB\xFF\xFF").is_err());
        assert!(rebuild(b"\xFF\xD8\xFF\xD9").is_err());
        let base = sample_jpeg(80);
        assert!(
            rebuild(&base[..base.len() - 8]).is_ok(),
            "truncated scans still parse; decode decides"
        );
        assert!(rebuild(&base[..base.len() / 3]).is_err(), "truncated headers do not");
    }

    #[test]
    fn quant_tables_in_natural_order() {
        let jpeg = sample_jpeg(50);
        let t = luma_table(&jpeg).unwrap();
        // q=50 is the unscaled Annex K table: natural order starts 16 11 10 16, second row 12 12 14.
        assert_eq!(&t.natural[0..4], &[16, 11, 10, 16]);
        assert_eq!(&t.natural[8..11], &[12, 12, 14]);
        // Stored zig-zag order: 16 11 12 14 12 10.
        assert_eq!(&t.stored[0..6], &[16, 11, 12, 14, 12, 10]);
    }
}
