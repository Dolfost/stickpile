//! A Signal-ready folder (as a zip) for people without credentials.
//!
//! Signal packs are made in two ways without stickpile's upload:
//!
//! * Signal Desktop, File > Create/upload sticker pack: the sticker creator
//!   takes PNG, WebP and APNG files by drag and drop
//!   (`sticker-creator/src/util/useStickerDropzone.ts`) and then asks for one
//!   emoji per sticker, a title, an author and a cover. It reads no list or
//!   manifest, so the zip holds the images numbered in pack order and
//!   `emoji.txt` saying which emoji to pick for each.
//! * signal-cli `uploadStickerPack <path>`: takes a `manifest.json` or a zip
//!   containing one, `{"title", "author", "cover": {"file", "emoji",
//!   "contentType"}, "stickers": [{"file", "emoji", "contentType"}]}` with
//!   files relative to it (signal-cli `StickerUtils.java`,
//!   `JsonStickerPack.java`). The zip is such a pack as it is.
//!
//! Layout: `001.webp` ... (3-digit numbers in pack order, the extension
//! from the file's format), `cover.<ext>` for a separate cover,
//! `manifest.json`, `emoji.txt`, `README.txt`.

use serde_json::json;

use crate::pack::{prepare, UploadPack};
use crate::SignalError;

/// One file of the folder.
#[derive(Clone, PartialEq, Eq)]
pub struct PackFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for PackFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackFile")
            .field("path", &self.path)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

const README: &str = "\
This folder is a Signal sticker pack made by stickpile.

Signal Desktop: File > Create/upload sticker pack. Drag the numbered images
in, in order (001 first). For each sticker pick the emoji emoji.txt lists,
then enter the title and author below and choose the cover. Uploading needs
a Signal account linked to Signal Desktop.

signal-cli: signal-cli -a <your number> uploadStickerPack <this zip or
manifest.json>. It prints the link to the new pack.

";

/// The files of the Signal-ready folder for `pack`. The pack is checked
/// like an upload ([`crate::check_sticker`], 1-200 stickers, a title and an
/// author).
pub fn signal_ready_files(pack: &UploadPack) -> Result<Vec<PackFile>, SignalError> {
    let prepared = prepare(pack)?;
    let mut files = Vec::new();
    let mut stickers_json = Vec::new();
    let mut emoji_lines = String::new();
    for (i, (s, (bytes, format))) in prepared.manifest.stickers.iter().zip(&prepared.files).enumerate() {
        let path = format!("{:03}.{}", i + 1, format.extension());
        let emoji = s.emoji.clone().unwrap_or_default();
        emoji_lines.push_str(&format!("{path} {emoji}\n"));
        stickers_json.push(json!({"file": path, "emoji": emoji, "contentType": format.mime()}));
        files.push(PackFile {
            path,
            bytes: bytes.clone(),
        });
    }
    let title = prepared.manifest.title.clone().unwrap_or_default();
    let author = prepared.manifest.author.clone().unwrap_or_default();
    let mut manifest = json!({"title": title, "author": author, "stickers": stickers_json});
    let cover_note = if prepared.separate_cover {
        let (bytes, format) = prepared.files.last().expect("the separate cover is the last file");
        let path = format!("cover.{}", format.extension());
        manifest["cover"] = json!({"file": path, "emoji": "", "contentType": format.mime()});
        files.push(PackFile {
            path: path.clone(),
            bytes: bytes.clone(),
        });
        path
    } else {
        let path = files[prepared.cover_id as usize].path.clone();
        let s = &stickers_json[prepared.cover_id as usize];
        manifest["cover"] = s.clone();
        path
    };
    files.push(PackFile {
        path: "manifest.json".into(),
        bytes: serde_json::to_vec_pretty(&manifest).expect("JSON values serialize"),
    });
    files.push(PackFile {
        path: "emoji.txt".into(),
        bytes: emoji_lines.into_bytes(),
    });
    let readme = format!("{README}Title: {title}\nAuthor: {author}\nCover: {cover_note}\n");
    files.push(PackFile {
        path: "README.txt".into(),
        bytes: readme.into_bytes(),
    });
    Ok(files)
}

/// The Signal-ready folder as a zip (stored, not compressed: the images
/// are compressed already).
pub fn signal_ready_zip(pack: &UploadPack) -> Result<Vec<u8>, SignalError> {
    zip_stored(&signal_ready_files(pack)?)
}

/// A zip archive of `files`, stored without compression (method 0), UTF-8
/// names, no ZIP64. Paths must be relative and use `/`.
pub fn zip_stored(files: &[PackFile]) -> Result<Vec<u8>, SignalError> {
    if files.len() > u16::MAX as usize {
        return Err(SignalError::Invalid("too many files for a zip".into()));
    }
    let mut out = Vec::new();
    let mut central = Vec::new();
    // 1980-01-01 00:00, the earliest DOS date: archives are reproducible.
    let (dos_time, dos_date): (u16, u16) = (0, (1 << 5) | 1);
    for f in files {
        if f.path.is_empty()
            || f.path.starts_with('/')
            || f.path.contains('\\')
            || f.path.split('/').any(|seg| seg == ".." || seg.is_empty())
        {
            return Err(SignalError::Invalid(format!("{:?} is not a relative path", f.path)));
        }
        let name = f.path.as_bytes();
        let size = u32::try_from(f.bytes.len()).map_err(|_| SignalError::Invalid("file too large for a zip".into()))?;
        let offset = u32::try_from(out.len()).map_err(|_| SignalError::Invalid("zip too large".into()))?;
        let crc = crc32fast::hash(&f.bytes);
        let name_len = u16::try_from(name.len()).map_err(|_| SignalError::Invalid("file name too long".into()))?;
        // Local file header.
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes()); // version needed
        out.extend(0x0800u16.to_le_bytes()); // flags: UTF-8 names
        out.extend(0u16.to_le_bytes()); // stored
        out.extend(dos_time.to_le_bytes());
        out.extend(dos_date.to_le_bytes());
        out.extend(crc.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend(name_len.to_le_bytes());
        out.extend(0u16.to_le_bytes()); // extra length
        out.extend(name);
        out.extend(&f.bytes);
        // Central directory entry.
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(0x031Eu16.to_le_bytes()); // made by: Unix, 3.0
        central.extend(20u16.to_le_bytes());
        central.extend(0x0800u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(dos_time.to_le_bytes());
        central.extend(dos_date.to_le_bytes());
        central.extend(crc.to_le_bytes());
        central.extend(size.to_le_bytes());
        central.extend(size.to_le_bytes());
        central.extend(name_len.to_le_bytes());
        central.extend(0u16.to_le_bytes()); // extra
        central.extend(0u16.to_le_bytes()); // comment
        central.extend(0u16.to_le_bytes()); // disk
        central.extend(0u16.to_le_bytes()); // internal attributes
        central.extend((0o100644u32 << 16).to_le_bytes()); // regular file, rw-r--r--
        central.extend(offset.to_le_bytes());
        central.extend(name);
    }
    let central_offset = u32::try_from(out.len()).map_err(|_| SignalError::Invalid("zip too large".into()))?;
    let central_len = u32::try_from(central.len()).map_err(|_| SignalError::Invalid("zip too large".into()))?;
    out.extend(&central);
    let count = files.len() as u16;
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend(0u16.to_le_bytes()); // this disk
    out.extend(0u16.to_le_bytes()); // central directory disk
    out.extend(count.to_le_bytes());
    out.extend(count.to_le_bytes());
    out.extend(central_len.to_le_bytes());
    out.extend(central_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes()); // comment length
    Ok(out)
}
