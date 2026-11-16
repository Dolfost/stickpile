//! The pack manifest (`manifest.proto` on the CDN, once decrypted): a
//! protobuf (proto2) message, encoded and decoded by hand.
//!
//! ```proto
//! message Pack {
//!   message Sticker {
//!     optional uint32 id          = 1;
//!     optional string emoji       = 2;
//!     optional string contentType = 3;
//!   }
//!   optional string  title    = 1;
//!   optional string  author   = 2;
//!   optional Sticker cover    = 3;
//!   repeated Sticker stickers = 4;
//! }
//! ```
//!
//! Sources: Signal-Android `lib/libsignal-service/src/main/protowire/
//! StickerResources.proto` (this message, with `contentType`) and
//! Signal-Desktop `protos/Stickers.proto` (the same without `contentType`;
//! unknown fields are ignored by protobuf decoders, so both read the
//! other's manifests).

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestSticker {
    /// The sticker's file is `stickers/<pack id>/full/<id>` on the CDN.
    pub id: u32,
    pub emoji: Option<String>,
    /// MIME type, when the uploader recorded it (Signal-Android's field).
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub title: Option<String>,
    pub author: Option<String>,
    pub cover: Option<ManifestSticker>,
    pub stickers: Vec<ManifestSticker>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid manifest: {0}")]
pub struct ManifestError(pub &'static str);

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_bytes(out: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    put_varint(out, u64::from(field << 3 | 2));
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

impl ManifestSticker {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(&mut out, 1 << 3);
        put_varint(&mut out, u64::from(self.id));
        if let Some(e) = &self.emoji {
            put_bytes(&mut out, 2, e.as_bytes());
        }
        if let Some(c) = &self.content_type {
            put_bytes(&mut out, 3, c.as_bytes());
        }
        out
    }

    fn decode(bytes: &[u8]) -> Result<ManifestSticker, ManifestError> {
        let mut s = ManifestSticker::default();
        let mut r = Reader { buf: bytes, at: 0 };
        while let Some((field, value)) = r.field()? {
            match (field, value) {
                (1, Value::Varint(v)) => s.id = u32::try_from(v).map_err(|_| ManifestError("sticker id too large"))?,
                (2, Value::Bytes(b)) => s.emoji = Some(text(b)),
                (3, Value::Bytes(b)) => s.content_type = Some(text(b)),
                _ => {}
            }
        }
        Ok(s)
    }
}

impl Manifest {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(t) = &self.title {
            put_bytes(&mut out, 1, t.as_bytes());
        }
        if let Some(a) = &self.author {
            put_bytes(&mut out, 2, a.as_bytes());
        }
        if let Some(c) = &self.cover {
            put_bytes(&mut out, 3, &c.encode());
        }
        for s in &self.stickers {
            put_bytes(&mut out, 4, &s.encode());
        }
        out
    }

    /// Unknown fields are skipped; groups (wire types 3 and 4) and
    /// truncated data are errors.
    pub fn decode(bytes: &[u8]) -> Result<Manifest, ManifestError> {
        let mut m = Manifest::default();
        let mut r = Reader { buf: bytes, at: 0 };
        while let Some((field, value)) = r.field()? {
            match (field, value) {
                (1, Value::Bytes(b)) => m.title = Some(text(b)),
                (2, Value::Bytes(b)) => m.author = Some(text(b)),
                (3, Value::Bytes(b)) => m.cover = Some(ManifestSticker::decode(b)?),
                (4, Value::Bytes(b)) => m.stickers.push(ManifestSticker::decode(b)?),
                _ => {}
            }
        }
        Ok(m)
    }
}

/// Protobuf strings are UTF-8; a broken one is read lossily rather than
/// failing the whole pack.
fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed,
}

struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn varint(&mut self) -> Result<u64, ManifestError> {
        let mut v: u64 = 0;
        for shift in (0..64).step_by(7) {
            let b = *self.buf.get(self.at).ok_or(ManifestError("truncated varint"))?;
            self.at += 1;
            v |= u64::from(b & 0x7F) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(ManifestError("varint too long"))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ManifestError> {
        let end = self.at.checked_add(n).ok_or(ManifestError("length overflow"))?;
        let out = self.buf.get(self.at..end).ok_or(ManifestError("truncated field"))?;
        self.at = end;
        Ok(out)
    }

    /// The next (field number, value), or `None` at the end.
    fn field(&mut self) -> Result<Option<(u64, Value<'a>)>, ManifestError> {
        if self.at >= self.buf.len() {
            return Ok(None);
        }
        let tag = self.varint()?;
        let field = tag >> 3;
        if field == 0 {
            return Err(ManifestError("field number 0"));
        }
        let value = match tag & 7 {
            0 => Value::Varint(self.varint()?),
            1 => {
                self.take(8)?;
                Value::Fixed
            }
            2 => {
                let len = usize::try_from(self.varint()?).map_err(|_| ManifestError("length overflow"))?;
                Value::Bytes(self.take(len)?)
            }
            5 => {
                self.take(4)?;
                Value::Fixed
            }
            _ => return Err(ManifestError("unsupported wire type")),
        };
        Ok(Some((field, value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sticker(id: u32, emoji: &str, ct: Option<&str>) -> ManifestSticker {
        ManifestSticker {
            id,
            emoji: Some(emoji.into()),
            content_type: ct.map(str::to_string),
        }
    }

    #[test]
    fn round_trip() {
        let m = Manifest {
            title: Some("Cats & co 🐱".into()),
            author: Some("someone".into()),
            cover: Some(sticker(2, "😺", Some("image/webp"))),
            stickers: vec![
                sticker(0, "😼", Some("image/webp")),
                sticker(1, "😾", None),
                sticker(300, "🙀", Some("image/png")),
            ],
        };
        assert_eq!(Manifest::decode(&m.encode()).unwrap(), m);
        assert_eq!(Manifest::decode(&[]).unwrap(), Manifest::default());
    }

    #[test]
    fn known_bytes() {
        // title "A", one sticker {id 0, emoji "x"}: what protobuf.js writes.
        let m = Manifest {
            title: Some("A".into()),
            author: None,
            cover: None,
            stickers: vec![ManifestSticker {
                id: 0,
                emoji: Some("x".into()),
                content_type: None,
            }],
        };
        assert_eq!(m.encode(), [0x0A, 1, b'A', 0x22, 5, 0x08, 0, 0x12, 1, b'x']);
        // Unknown fields of every wire type are skipped.
        let mut bytes = m.encode();
        bytes.extend([0x28, 0x96, 0x01]); // field 5 varint
        bytes.extend([0x31, 1, 2, 3, 4, 5, 6, 7, 8]); // field 6 fixed64
        bytes.extend([0x3D, 1, 2, 3, 4]); // field 7 fixed32
        bytes.extend([0x42, 2, 0xFF, 0xFF]); // field 8 bytes
        assert_eq!(Manifest::decode(&bytes).unwrap(), m);
    }

    #[test]
    fn broken_input_is_an_error() {
        let good = Manifest {
            title: Some("Title".into()),
            ..Default::default()
        }
        .encode();
        assert!(Manifest::decode(&good[..good.len() - 1]).is_err());
        assert!(Manifest::decode(&[0x0A]).is_err());
        assert!(Manifest::decode(&[0x0B]).is_err(), "group wire type");
        assert!(Manifest::decode(&[0x00, 0x00]).is_err(), "field 0");
        assert!(Manifest::decode(&[0x22, 3, 0x08, 0xFF, 0xFF]).is_err(), "truncated id");
        // An id beyond u32.
        let mut big = vec![0x22, 7, 0x08];
        big.extend([0xFF, 0xFF, 0xFF, 0xFF, 0x7F, 0x00]);
        assert!(Manifest::decode(&big[..9]).is_err());
        assert!(Manifest::decode(&[0xFF; 12]).is_err());
    }
}
