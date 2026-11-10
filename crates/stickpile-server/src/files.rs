//! Media files on disk: `<data_dir>/media/<sha[0..2]>/<sha>/<variant>.<ext>`,
//! written atomically (temp file in the same directory + rename). Paths are
//! derived only from server-computed hashes and the fixed variant names, never
//! from user input.

use std::path::{Path, PathBuf};

use stickpile_api::{MediaFormat, Variant};

use crate::error::AppResult;

/// Relative directory of a media item (`ab/abcdef...`).
pub fn media_rel_dir(sha256: &str) -> String {
    debug_assert!(sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit()));
    format!("{}/{}", &sha256[..2], sha256)
}

/// File extension for a variant.
pub fn variant_ext(variant: Variant, original: MediaFormat) -> &'static str {
    match variant {
        Variant::Original => original.extension(),
        Variant::Thumb | Variant::TgSticker => "webp",
        Variant::Png => "png",
        Variant::TgVideoSticker => "webm",
        Variant::DiscordGif => "gif",
        Variant::Mp4 => "mp4",
    }
}

/// Relative path of a variant file.
pub fn variant_rel_path(sha256: &str, variant: Variant, original: MediaFormat) -> String {
    format!(
        "{}/{}.{}",
        media_rel_dir(sha256),
        variant.as_str(),
        variant_ext(variant, original)
    )
}

/// Resolve a stored relative path under the media dir, refusing anything that
/// is not a plain relative path (defence in depth: paths come from our DB).
pub fn resolve(media_dir: &Path, rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    if p.is_absolute() || p.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return None;
    }
    Some(media_dir.join(p))
}

/// Write `bytes` to `path` atomically, creating parent directories.
pub fn write_atomic_blocking(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("path without parent"))?;
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new().prefix(".tmp-").tempfile_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub async fn write_atomic(path: PathBuf, bytes: Vec<u8>) -> AppResult<()> {
    tokio::task::spawn_blocking(move || write_atomic_blocking(&path, &bytes)).await??;
    Ok(())
}

pub async fn read(path: PathBuf) -> AppResult<Vec<u8>> {
    Ok(tokio::fs::read(path).await?)
}

/// Total size of regular files under `dir` (storage usage).
pub fn dir_size_blocking(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

/// Delete files in `dir` (non-recursive) older than `max_age` (crash leftovers in tmp/).
pub fn purge_old_files_blocking(dir: &Path, max_age: std::time::Duration) -> usize {
    let mut n = 0;
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let now = std::time::SystemTime::now();
    for entry in rd.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > max_age);
        if !old {
            continue;
        }
        let res = if meta.is_dir() {
            std::fs::remove_dir_all(entry.path())
        } else {
            std::fs::remove_file(entry.path())
        };
        if res.is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        let sha = "ab".repeat(32);
        assert_eq!(
            variant_rel_path(&sha, Variant::Thumb, MediaFormat::Png),
            format!("ab/{sha}/thumb.webp")
        );
        assert_eq!(
            variant_rel_path(&sha, Variant::Original, MediaFormat::Jpeg),
            format!("ab/{sha}/original.jpg")
        );
        let base = Path::new("/data/media");
        assert!(resolve(base, "../etc/passwd").is_none());
        assert!(resolve(base, "/etc/passwd").is_none());
        assert_eq!(resolve(base, "ab/x.png").unwrap(), Path::new("/data/media/ab/x.png"));
    }

    #[test]
    fn atomic_write() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b/c.bin");
        write_atomic_blocking(&p, b"hello").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
        write_atomic_blocking(&p, b"bye").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"bye");
        assert_eq!(dir_size_blocking(dir.path()), 3);
    }
}
