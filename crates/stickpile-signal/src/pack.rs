//! Pack-level import and upload, and the rules a sticker file must meet.

use std::collections::HashSet;

use serde::Serialize;

use crate::client::{SignalClient, MAX_CIPHERTEXT_BYTES};
use crate::crypto::{decrypt, derive_keys, encrypt, CryptoError};
use crate::image::{inspect, ImageFormat, ImageInfo};
use crate::link::{redact_pack_id, PackKey, PackLink};
use crate::manifest::{Manifest, ManifestSticker};
use crate::{Credentials, SignalError};

/// Stickers per pack (Signal-Desktop `sticker-creator/src/constants.ts`
/// `MAX_STICKERS`; Signal-Server allows a form for 201 files: 200 and a
/// cover).
pub const MAX_STICKERS: usize = 200;
/// Bytes per sticker file (`MAX_STICKER_BYTE_SIZE = 300 * 1024`).
pub const MAX_STICKER_BYTES: u64 = 300 * 1024;
/// Stickers are 512x512 (`STICKER_SIZE`).
pub const STICKER_SIDE_PX: u32 = 512;
/// Longest animation (Signal support, "Stickers": "maximum animation length
/// of 3 seconds").
pub const MAX_ANIMATION_MS: u32 = 3000;
/// Used for stickers without an emoji (the creator asks for one per
/// sticker).
pub const DEFAULT_EMOJI: &str = "🙂";
/// Encrypted manifests larger than this are refused by Signal-Server
/// (`MAXIMUM_STICKER_MANIFEST_SIZE_BYTES`, 10 KiB).
pub const MAX_MANIFEST_CIPHERTEXT_BYTES: usize = 10 * 1024;
/// Title and author are cut to this many characters (stickpile's choice:
/// Signal states no limit, the manifest must stay under 10 KiB).
pub const MAX_TITLE_CHARS: usize = 128;

/// What a sticker file must look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Requirements {
    pub formats: &'static [ImageFormat],
    pub max_bytes: u64,
    /// Render exactly this square; smaller squares are accepted.
    pub side_px: u32,
    pub max_animation_ms: u32,
    pub max_stickers: usize,
}

/// Static stickers: PNG or WebP; animated stickers: APNG that loops
/// forever, at most 3 s; every file at most 300 KiB and 512x512.
///
/// Sources: Signal-Desktop's sticker creator (`useStickerDropzone.ts`
/// accepts PNG, WebP and APNG; `processImage.ts` turns still images into
/// 512x512 WebP and checks APNG to be square, 10 to 512 px, at most 300 KiB
/// and looping forever) and Signal's support article "Stickers" (animated
/// stickers are APNG, not GIF; 300 KB; 3 seconds; 512x512).
pub const STICKER_REQUIREMENTS: Requirements = Requirements {
    formats: &[ImageFormat::Png, ImageFormat::Apng, ImageFormat::Webp],
    max_bytes: MAX_STICKER_BYTES,
    side_px: STICKER_SIDE_PX,
    max_animation_ms: MAX_ANIMATION_MS,
    max_stickers: MAX_STICKERS,
};

/// Checks one sticker file. `Err` is a reason fit for a report.
pub fn check_sticker(bytes: &[u8]) -> Result<ImageInfo, String> {
    if bytes.len() as u64 > MAX_STICKER_BYTES {
        return Err(format!(
            "the sticker is {} KiB, Signal takes at most {} KiB",
            bytes.len().div_ceil(1024),
            MAX_STICKER_BYTES / 1024
        ));
    }
    let info = inspect(bytes).ok_or("the sticker is not a PNG, APNG or WebP image")?;
    match info.format {
        ImageFormat::Gif => return Err("Signal does not take GIF stickers; animated stickers must be APNG".into()),
        ImageFormat::Jpeg => return Err("Signal stickers must be PNG, APNG or WebP, not JPEG".into()),
        ImageFormat::Webp if info.animated => {
            return Err("Signal takes animated stickers only as APNG, not animated WebP".into())
        }
        _ => {}
    }
    let side = STICKER_SIDE_PX;
    if info.width > side || info.height > side || info.width < 10 || info.height < 10 {
        return Err(format!(
            "the sticker is {}x{} px; Signal stickers are {side}x{side}",
            info.width, info.height
        ));
    }
    if info.format == ImageFormat::Apng && info.animated {
        if info.width != info.height {
            return Err("animated stickers must be square".into());
        }
        if info.loops_forever != Some(true) {
            return Err("animated stickers must loop forever".into());
        }
        if info.duration_ms.is_some_and(|ms| ms > MAX_ANIMATION_MS) {
            return Err(format!(
                "the animation lasts {:.1} s, Signal takes at most {} s",
                f64::from(info.duration_ms.unwrap_or(0)) / 1000.0,
                MAX_ANIMATION_MS / 1000
            ));
        }
    }
    Ok(info)
}

/// Can `s` be a sticker's emoji? Words typed into an emoji field are left
/// out (the same rule as `stickpile_telegram::is_emoji`).
pub fn is_emoji(s: &str) -> bool {
    let enclosed = |c: char| matches!(c, '\u{2460}'..='\u{24FF}' | '\u{1F100}'..='\u{1F1FF}');
    !s.is_ascii()
        && !s
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || (c.is_alphabetic() && !enclosed(c)))
        && (s.contains('\u{20E3}') || !s.chars().any(|c| c.is_ascii_digit()))
}

/// The one emoji a Signal sticker carries: the item's first emoji, or
/// [`DEFAULT_EMOJI`].
pub fn sticker_emoji(emojis: &[String]) -> String {
    emojis
        .iter()
        .map(|e| e.trim())
        .find(|e| is_emoji(e))
        .unwrap_or(DEFAULT_EMOJI)
        .to_string()
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
pub struct ImportedSticker {
    /// Id within the pack (the file is `full/<id>`).
    pub id: u32,
    pub emoji: Option<String>,
    /// MIME type from the manifest, if the uploader recorded one.
    pub content_type: Option<String>,
    /// Sniffed from the bytes.
    pub format: ImageFormat,
    pub animated: bool,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for ImportedSticker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportedSticker")
            .field("id", &self.id)
            .field("emoji", &self.emoji)
            .field("format", &self.format)
            .field("animated", &self.animated)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedPack {
    pub pack_id: String,
    pub title: String,
    pub author: String,
    /// In manifest order.
    pub stickers: Vec<ImportedSticker>,
    /// The id of the cover sticker (the first sticker when the manifest
    /// names none).
    pub cover_id: Option<u32>,
    /// The cover's image when it is not one of `stickers` (Signal Desktop's
    /// creator uploads a separately chosen cover as an extra file).
    pub cover: Option<ImportedSticker>,
    /// (sticker id, reason) of stickers that could not be imported.
    pub skipped: Vec<(u32, String)>,
}

enum Fetch {
    Ok(ImportedSticker),
    Skip(String),
}

async fn fetch_sticker(
    client: &SignalClient,
    link: &PackLink,
    keys: &crate::crypto::PackKeys,
    s: &ManifestSticker,
    max_file_bytes: u64,
) -> Result<Fetch, SignalError> {
    let path = format!("stickers/{}/full/{}", link.pack_id, s.id);
    let blob = match client.get_cdn(&path, MAX_CIPHERTEXT_BYTES).await {
        Ok(b) => b,
        Err(SignalError::NotFound) => return Ok(Fetch::Skip("the file is missing on Signal's CDN".into())),
        Err(SignalError::TooLarge(cap)) => return Ok(Fetch::Skip(format!("file is larger than {cap} bytes"))),
        Err(e) => return Err(e),
    };
    let bytes = match decrypt(keys, &blob) {
        Ok(b) => b,
        Err(e) => return Ok(Fetch::Skip(format!("cannot decrypt the file: {e}"))),
    };
    if bytes.len() as u64 > max_file_bytes {
        return Ok(Fetch::Skip(format!("file is larger than {max_file_bytes} bytes")));
    }
    let Some(info) = inspect(&bytes) else {
        return Ok(Fetch::Skip("not a PNG, APNG, GIF, JPEG or WebP image".into()));
    };
    Ok(Fetch::Ok(ImportedSticker {
        id: s.id,
        emoji: s.emoji.clone().filter(|e| !e.is_empty()),
        content_type: s.content_type.clone().filter(|c| !c.is_empty()),
        format: info.format,
        animated: info.animated,
        bytes,
    }))
}

/// Downloads and decrypts a pack: the manifest, then every sticker in
/// manifest order, then the cover if it is a separate file.
///
/// The MAC of every file is checked before it is decrypted; a manifest that
/// fails it means a wrong key ([`SignalError::Crypto`]). Stickers that are
/// missing, fail the MAC, exceed `max_file_bytes` or are not images are
/// listed in `skipped`. A missing pack is [`SignalError::NotFound`].
pub async fn import_pack(
    client: &SignalClient,
    link: &PackLink,
    max_file_bytes: u64,
) -> Result<ImportedPack, SignalError> {
    let keys = derive_keys(link.key.as_bytes());
    let blob = client
        .get_cdn(
            &format!("stickers/{}/manifest.proto", link.pack_id),
            MAX_CIPHERTEXT_BYTES,
        )
        .await?;
    let manifest = Manifest::decode(&decrypt(&keys, &blob).map_err(SignalError::Crypto)?)
        .map_err(|e| SignalError::Manifest(e.0.to_string()))?;
    tracing::debug!(
        pack = redact_pack_id(&link.pack_id),
        stickers = manifest.stickers.len(),
        "Signal pack manifest"
    );
    let mut out = ImportedPack {
        pack_id: link.pack_id.clone(),
        title: manifest.title.clone().unwrap_or_default(),
        author: manifest.author.clone().unwrap_or_default(),
        stickers: Vec::new(),
        cover_id: manifest
            .cover
            .as_ref()
            .map(|c| c.id)
            .or_else(|| manifest.stickers.first().map(|s| s.id)),
        cover: None,
        skipped: Vec::new(),
    };
    let mut seen = HashSet::new();
    for s in &manifest.stickers {
        if !seen.insert(s.id) {
            out.skipped.push((s.id, "listed twice in the manifest".into()));
            continue;
        }
        match fetch_sticker(client, link, &keys, s, max_file_bytes).await? {
            Fetch::Ok(sticker) => out.stickers.push(sticker),
            Fetch::Skip(reason) => out.skipped.push((s.id, reason)),
        }
    }
    if let Some(cover) = manifest.cover.as_ref().filter(|c| !seen.contains(&c.id)) {
        // A cover that cannot be fetched is not worth failing the import.
        if let Ok(Fetch::Ok(sticker)) = fetch_sticker(client, link, &keys, cover, max_file_bytes).await {
            out.cover = Some(sticker);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

/// One sticker to upload.
#[derive(Clone)]
pub struct UploadSticker {
    /// PNG, APNG or WebP; see [`check_sticker`].
    pub bytes: Vec<u8>,
    /// One emoji (see [`sticker_emoji`]).
    pub emoji: String,
}

impl std::fmt::Debug for UploadSticker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UploadSticker")
            .field("emoji", &self.emoji)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// A pack to upload, or to write as a Signal-ready zip.
#[derive(Debug, Clone)]
pub struct UploadPack {
    pub title: String,
    pub author: String,
    /// 1 to 200 stickers, in order.
    pub stickers: Vec<UploadSticker>,
    /// The cover image, if it is not simply the first sticker. A cover equal
    /// to one of the stickers points at that sticker instead of being
    /// uploaded twice.
    pub cover: Option<Vec<u8>>,
}

/// What [`upload_pack`] made.
#[derive(Debug, Clone)]
pub struct UploadedPack {
    /// The new pack. [`PackLink::url`] is the link to hand out; it contains
    /// the key, so treat it like one.
    pub link: PackLink,
    pub stickers: usize,
    /// Manifest id of the cover.
    pub cover_id: u32,
}

/// The files of a pack in upload order, with the manifest describing them.
pub(crate) struct PreparedPack {
    pub manifest: Manifest,
    /// (bytes, format) by manifest id: the stickers, then the separate
    /// cover if there is one.
    pub files: Vec<(Vec<u8>, ImageFormat)>,
    pub cover_id: u32,
    pub separate_cover: bool,
}

fn clamp(s: &str) -> String {
    s.trim()
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Validates a pack and builds its manifest.
pub(crate) fn prepare(pack: &UploadPack) -> Result<PreparedPack, SignalError> {
    let title = clamp(&pack.title);
    let author = clamp(&pack.author);
    if title.is_empty() || author.is_empty() {
        return Err(SignalError::Invalid("a Signal pack needs a title and an author".into()));
    }
    if pack.stickers.is_empty() || pack.stickers.len() > MAX_STICKERS {
        return Err(SignalError::Invalid(format!(
            "a Signal pack holds 1 to {MAX_STICKERS} stickers, not {}",
            pack.stickers.len()
        )));
    }
    let mut files = Vec::with_capacity(pack.stickers.len() + 1);
    let mut stickers = Vec::with_capacity(pack.stickers.len());
    for (i, s) in pack.stickers.iter().enumerate() {
        let info = check_sticker(&s.bytes).map_err(|why| SignalError::Invalid(format!("sticker {}: {why}", i + 1)))?;
        let emoji = s.emoji.trim();
        if emoji.is_empty() {
            return Err(SignalError::Invalid(format!("sticker {} has no emoji", i + 1)));
        }
        stickers.push(ManifestSticker {
            id: i as u32,
            emoji: Some(emoji.to_string()),
            content_type: Some(info.format.mime().to_string()),
        });
        files.push((s.bytes.clone(), info.format));
    }
    let (cover, separate_cover) = match &pack.cover {
        Some(bytes) => match pack.stickers.iter().position(|s| &s.bytes == bytes) {
            Some(i) => (stickers[i].clone(), false),
            None => {
                let info = check_sticker(bytes).map_err(|why| SignalError::Invalid(format!("cover: {why}")))?;
                let id = files.len() as u32;
                files.push((bytes.clone(), info.format));
                (
                    ManifestSticker {
                        id,
                        emoji: None,
                        content_type: Some(info.format.mime().to_string()),
                    },
                    true,
                )
            }
        },
        None => (stickers[0].clone(), false),
    };
    let cover_id = cover.id;
    let manifest = Manifest {
        title: Some(title),
        author: Some(author),
        cover: Some(cover),
        stickers,
    };
    // IV + padded ciphertext + MAC must fit Signal's 10 KiB policy.
    let encrypted = 16 + (manifest.encode().len() / 16 + 1) * 16 + 32;
    if encrypted > MAX_MANIFEST_CIPHERTEXT_BYTES {
        return Err(SignalError::Invalid(format!(
            "the pack manifest would be {encrypted} bytes encrypted, Signal takes at most {MAX_MANIFEST_CIPHERTEXT_BYTES}; shorten the emoji, title or author"
        )));
    }
    Ok(PreparedPack {
        manifest,
        files,
        cover_id,
        separate_cover,
    })
}

/// Uploads `pack` as a new Signal sticker pack and returns its link.
///
/// Signal packs cannot be changed once uploaded: publishing a set again
/// uploads a new pack with a new link. The steps follow Signal-Desktop
/// (`putStickers` in `ts/textsecure/WebAPI.preload.ts`, `encrypt` in
/// `sticker-creator/src/util/crypto.ts`) and signalstickers-client
/// (`uploader.py`):
///
/// 1. `GET <service>/v1/sticker/pack/form/<files>` with Basic auth returns
///    the pack id and one signed S3 POST policy for the manifest and for
///    each file (`stickers/<pack id>/full/<n>`).
/// 2. A fresh random 32-byte pack key; every file encrypted with fresh IVs.
/// 3. The manifest, then each sticker (and the separate cover, which gets
///    the next id) are POSTed to the CDN with their policy fields.
///
/// The pack is validated first ([`check_sticker`], 1-200 stickers, a title
/// and an author); nothing is sent when it is invalid. Rate limits are not
/// retried: the service allows a burst of 50 forms per account, then one
/// every 72 minutes (Signal-Server `RateLimiters`, `stickerPack`), so the
/// error says when to try again.
pub async fn upload_pack(
    client: &SignalClient,
    credentials: &Credentials,
    pack: &UploadPack,
) -> Result<UploadedPack, SignalError> {
    upload_prepared(client, credentials, &prepare(pack)?).await
}

/// [`upload_pack`] of a pack [`prepare`] checked already.
pub(crate) async fn upload_prepared(
    client: &SignalClient,
    credentials: &Credentials,
    prepared: &PreparedPack,
) -> Result<UploadedPack, SignalError> {
    let form = client.upload_form(credentials, prepared.files.len()).await?;
    let unexpected = |why: &str| SignalError::Invalid(format!("Signal's upload form is unusable: {why}"));
    if !crate::link::valid_pack_id(&form.pack_id) {
        return Err(unexpected("the pack id is not 32 hex digits"));
    }
    if form.stickers.len() < prepared.files.len() {
        return Err(unexpected("fewer upload slots than files"));
    }
    let key = PackKey::random();
    let link = PackLink::new(&form.pack_id, key)?;
    let keys = derive_keys(link.key.as_bytes());
    tracing::info!(
        pack = redact_pack_id(&link.pack_id),
        files = prepared.files.len(),
        "uploading a Signal sticker pack"
    );
    client
        .post_cdn(&form.manifest, encrypt(&keys, &prepared.manifest.encode()))
        .await?;
    for (i, (bytes, _)) in prepared.files.iter().enumerate() {
        // The server numbers the slots 0..count in order; match by id when
        // it says.
        let slot = form
            .stickers
            .iter()
            .find(|s| s.id == i as i64)
            .unwrap_or(&form.stickers[i]);
        client.post_cdn(slot, encrypt(&keys, bytes)).await?;
    }
    Ok(UploadedPack {
        link,
        stickers: prepared.manifest.stickers.len(),
        cover_id: prepared.cover_id,
    })
}

impl From<CryptoError> for SignalError {
    fn from(e: CryptoError) -> SignalError {
        SignalError::Crypto(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::tests::png;

    fn sticker(seed: u8) -> UploadSticker {
        let mut bytes = png(512, 512, None);
        bytes.push(seed);
        UploadSticker {
            bytes,
            emoji: "😼".into(),
        }
    }

    #[test]
    fn sticker_rules() {
        assert!(check_sticker(&png(512, 512, None)).is_ok());
        assert!(check_sticker(&png(256, 256, None)).is_ok());
        assert!(check_sticker(&png(513, 512, None)).unwrap_err().contains("513x512"));
        assert!(check_sticker(&png(512, 512, Some((3, 0, 1000)))).is_ok());
        let long = check_sticker(&png(512, 512, Some((4, 0, 1000)))).unwrap_err();
        assert!(long.contains("4.0 s"), "{long}");
        assert!(check_sticker(&png(512, 512, Some((2, 1, 100))))
            .unwrap_err()
            .contains("loop forever"));
        assert!(check_sticker(&png(512, 256, Some((2, 0, 100))))
            .unwrap_err()
            .contains("square"));
        let gif = crate::image::tests::gif(512, 512, 2, 10, true);
        assert!(check_sticker(&gif).unwrap_err().contains("APNG"));
        let mut big = png(512, 512, None);
        big.resize(301 * 1024, 0);
        assert!(check_sticker(&big).unwrap_err().contains("300 KiB"));
        assert!(check_sticker(b"nope").is_err());
    }

    #[test]
    fn emoji_choice() {
        let e = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(sticker_emoji(&e(&["cat", "😼", "🐱"])), "😼");
        assert_eq!(sticker_emoji(&[]), DEFAULT_EMOJI);
    }

    #[test]
    fn preparing_a_pack() {
        let pack = UploadPack {
            title: "  Cats ".into(),
            author: "me".into(),
            stickers: vec![sticker(1), sticker(2)],
            cover: None,
        };
        let p = prepare(&pack).unwrap();
        assert_eq!(p.manifest.title.as_deref(), Some("Cats"));
        assert_eq!(p.files.len(), 2);
        assert_eq!((p.cover_id, p.separate_cover), (0, false));
        assert_eq!(p.manifest.cover.as_ref().unwrap().emoji.as_deref(), Some("😼"));
        assert_eq!(p.manifest.stickers[1].content_type.as_deref(), Some("image/png"));

        // A cover equal to a sticker points at it; another one is a file.
        let same = UploadPack {
            cover: Some(sticker(2).bytes),
            ..pack.clone()
        };
        let p = prepare(&same).unwrap();
        assert_eq!((p.cover_id, p.separate_cover, p.files.len()), (1, false, 2));
        let other = UploadPack {
            cover: Some(sticker(9).bytes),
            ..pack.clone()
        };
        let p = prepare(&other).unwrap();
        assert_eq!((p.cover_id, p.separate_cover, p.files.len()), (2, true, 3));
        assert_eq!(p.manifest.stickers.len(), 2, "the cover is not a sticker");

        for bad in [
            UploadPack {
                title: " ".into(),
                ..pack.clone()
            },
            UploadPack {
                stickers: vec![],
                ..pack.clone()
            },
            UploadPack {
                stickers: (0..201).map(|i| sticker(i as u8)).collect(),
                ..pack.clone()
            },
            UploadPack {
                stickers: vec![UploadSticker {
                    bytes: b"x".to_vec(),
                    emoji: "😼".into(),
                }],
                ..pack.clone()
            },
            UploadPack {
                stickers: vec![UploadSticker {
                    bytes: sticker(1).bytes,
                    emoji: " ".into(),
                }],
                ..pack.clone()
            },
        ] {
            assert!(prepare(&bad).is_err(), "{bad:?}");
        }
        let full = UploadPack {
            stickers: (0..200).map(|i| sticker(i as u8)).collect(),
            title: "t".repeat(500),
            ..pack
        };
        let p = prepare(&full).unwrap();
        assert_eq!(p.manifest.title.unwrap().chars().count(), MAX_TITLE_CHARS);
    }
}
