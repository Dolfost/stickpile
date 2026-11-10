//! Magic-byte sniffing. Only the leading bytes are inspected; the extension
//! and any client-supplied MIME type are never trusted.

use stickpile_api::MediaFormat;

/// ISO-BMFF brands we accept as video (major or compatible brand).
const VIDEO_BRANDS: &[&[u8; 4]] = &[
    b"isom", b"iso2", b"iso3", b"iso4", b"iso5", b"iso6", b"iso7", b"iso8", b"iso9", b"mp41", b"mp42", b"avc1",
    b"dash", b"M4V ", b"M4VH", b"M4VP", b"mmp4", b"MSNV", b"qt  ", b"3gp4", b"3gp5", b"3gp6", b"3g2a",
];

/// HEIF/AVIF image brands: never treated as video, even when a video brand
/// is also listed.
const IMAGE_BRANDS: &[&[u8; 4]] = &[
    b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"hevm", b"hevs", b"mif1", b"mif2", b"msf1", b"avif",
    b"avis", b"avio", b"MiHE", b"MiHA", b"MiHB", b"miaf",
];

pub fn sniff(bytes: &[u8]) -> Option<MediaFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(MediaFormat::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(MediaFormat::Jpeg);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(MediaFormat::Gif);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(MediaFormat::Webp);
    }
    if bytes.len() >= 16 && &bytes[4..8] == b"ftyp" {
        return sniff_ftyp(bytes);
    }
    if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return sniff_ebml(bytes);
    }
    None
}

fn sniff_ftyp(bytes: &[u8]) -> Option<MediaFormat> {
    let size = u32::from_be_bytes(bytes[0..4].try_into().ok()?) as usize;
    if size < 16 {
        return None;
    }
    let end = size.min(bytes.len());
    let major: &[u8; 4] = bytes[8..12].try_into().ok()?;
    let compat = bytes[16..end]
        .chunks_exact(4)
        .filter_map(|c| <&[u8; 4]>::try_from(c).ok());
    let brands: Vec<&[u8; 4]> = std::iter::once(major).chain(compat).collect();
    if brands.iter().any(|b| IMAGE_BRANDS.contains(b)) {
        return None;
    }
    if brands.iter().any(|b| VIDEO_BRANDS.contains(b)) {
        return Some(MediaFormat::Mp4);
    }
    None
}

/// Read an EBML variable-length integer. Returns (value, length). With
/// `keep_marker` the length marker bit is kept (element IDs).
fn vint(bytes: &[u8], keep_marker: bool) -> Option<(u64, usize)> {
    let first = *bytes.first()?;
    if first == 0 {
        return None;
    }
    let len = first.leading_zeros() as usize + 1;
    if bytes.len() < len {
        return None;
    }
    let mut v = if keep_marker {
        u64::from(first)
    } else {
        u64::from(first) & ((1u64 << (8 - len)) - 1)
    };
    for b in &bytes[1..len] {
        v = (v << 8) | u64::from(*b);
    }
    Some((v, len))
}

fn sniff_ebml(bytes: &[u8]) -> Option<MediaFormat> {
    let (id, id_len) = vint(bytes, true)?;
    if id != 0x1A45_DFA3 {
        return None;
    }
    let (size, size_len) = vint(&bytes[id_len..], false)?;
    let start = id_len + size_len;
    // An "unknown size" header (all ones) is bounded by what we have.
    let all_ones = (1u64 << (7 * size_len)) - 1;
    let end = if size == all_ones {
        bytes.len()
    } else {
        start.saturating_add(size as usize).min(bytes.len())
    };
    let mut pos = start;
    while pos < end {
        let (child, l1) = vint(&bytes[pos..end], true)?;
        let (len, l2) = vint(&bytes[pos + l1..end], false)?;
        let data_start = pos + l1 + l2;
        let data_end = data_start.checked_add(usize::try_from(len).ok()?)?;
        if data_end > end {
            return None;
        }
        if child == 0x4282 {
            let doctype = &bytes[data_start..data_end];
            let doctype = doctype.split(|b| *b == 0).next().unwrap_or_default();
            return (doctype == b"webm").then_some(MediaFormat::Webm);
        }
        pos = data_end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ftyp(major: &[u8; 4], compat: &[&[u8; 4]]) -> Vec<u8> {
        let size = 16 + 4 * compat.len();
        let mut v = (size as u32).to_be_bytes().to_vec();
        v.extend_from_slice(b"ftyp");
        v.extend_from_slice(major);
        v.extend_from_slice(&[0, 0, 2, 0]);
        for c in compat {
            v.extend_from_slice(*c);
        }
        v.extend_from_slice(&[0, 0, 0, 8, b'f', b'r', b'e', b'e']);
        v
    }

    fn ebml(doctype: &[u8]) -> Vec<u8> {
        let mut body = vec![0x42, 0x86, 0x81, 0x01]; // EBMLVersion 1
        body.extend_from_slice(&[0x42, 0x82, 0x80 | doctype.len() as u8]);
        body.extend_from_slice(doctype);
        body.extend_from_slice(&[0x42, 0x87, 0x81, 0x04]); // DocTypeVersion 4
        let mut v = vec![0x1A, 0x45, 0xDF, 0xA3, 0x80 | body.len() as u8];
        v.extend_from_slice(&body);
        v.extend_from_slice(&[0x18, 0x53, 0x80, 0x67]); // Segment
        v
    }

    #[test]
    fn images() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Some(MediaFormat::Png));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 16]), Some(MediaFormat::Jpeg));
        assert_eq!(sniff(b"GIF89a\x01\x00"), Some(MediaFormat::Gif));
        assert_eq!(sniff(b"GIF87a\x01\x00"), Some(MediaFormat::Gif));
        assert_eq!(sniff(b"RIFF\x10\0\0\0WEBPVP8 "), Some(MediaFormat::Webp));
        assert_eq!(sniff(b"RIFF\x10\0\0\0WAVEfmt "), None);
        assert_eq!(sniff(b"GIF90a"), None);
        assert_eq!(sniff(&[0xFF, 0xD8]), None);
        assert_eq!(sniff(b""), None);
        assert_eq!(sniff(b"<html><body>"), None);
        assert_eq!(sniff(b"PK\x03\x04zipzipzip"), None);
    }

    #[test]
    fn mp4_brands() {
        assert_eq!(
            sniff(&ftyp(b"isom", &[b"isom", b"iso2", b"avc1", b"mp41"])),
            Some(MediaFormat::Mp4)
        );
        assert_eq!(sniff(&ftyp(b"mp42", &[])), Some(MediaFormat::Mp4));
        assert_eq!(sniff(&ftyp(b"XAVC", &[b"iso6", b"mp42"])), Some(MediaFormat::Mp4));
        assert_eq!(sniff(&ftyp(b"heic", &[b"mif1", b"heic"])), None);
        assert_eq!(sniff(&ftyp(b"mif1", &[b"heic"])), None);
        assert_eq!(sniff(&ftyp(b"avif", &[b"avif", b"mif1", b"miaf"])), None);
        assert_eq!(sniff(&ftyp(b"msf1", &[b"iso8", b"msf1"])), None);
        assert_eq!(sniff(&ftyp(b"M4A ", &[b"M4A ", b"mp42"])), Some(MediaFormat::Mp4));
        assert_eq!(sniff(&ftyp(b"crx ", &[])), None);
    }

    #[test]
    fn webm_doctype() {
        assert_eq!(sniff(&ebml(b"webm")), Some(MediaFormat::Webm));
        assert_eq!(sniff(&ebml(b"matroska")), None);
        assert_eq!(sniff(&ebml(b"webm\0")), Some(MediaFormat::Webm));
        assert_eq!(sniff(&[0x1A, 0x45, 0xDF, 0xA3]), None);
        let mut truncated = ebml(b"webm");
        truncated.truncate(9);
        assert_eq!(sniff(&truncated), None);
    }
}
