//! "Add to WhatsApp" (docs/android.md, "Add to WhatsApp"): the server builds
//! a pack's WhatsApp sticker packs as `.wastickers` files
//! (docs/whatsapp.md), the library downloads and unpacks them on the
//! phone, and the app's WhatsApp provider serves the files to WhatsApp.
//! WhatsApp calls each of them a sticker pack; here they are the parts of
//! a stickpile pack.
//!
//! The files live under `<cache_dir>/whatsapp/`, outside the cache's own
//! files (`cache_bytes` and `clear_cache` leave them alone: WhatsApp reads
//! them), and go when the account or server changes:
//!
//! ```text
//! <pack id>/index.json          the parts, written last and atomically
//! <pack id>/<n>-<version>/      part n: its tray icon and stickers
//! .tmp/                         downloads under way
//! ```
//!
//! A part's directory is named after its `image_data_version`, so a part
//! that did not change is kept without a download, and a rebuilt one goes
//! into a new directory that the index then names.
//!
//! Nothing from the server becomes a path unchecked: pack ids, versions
//! and file names are checked against fixed character sets, and a
//! `.wastickers` is read with caps on its size, its number of entries and
//! each entry's size (WhatsApp's own limits). An entry whose name is not a
//! plain file name (`../x`, `/x`, `a/b`, `.x`) fails the whole part.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stickpile_api::{Job, JobStatus, WhatsappBuild, WhatsappPack};
use stickpile_core::{ApiClient, CoreError};

use crate::types::{WhatsappPart, WhatsappProgress, WhatsappStep, WhatsappSticker};
use crate::LibraryError;

/// Longest wait for the server's build; then the call fails (the build
/// goes on, and the next call waits for it again).
const JOB_WAIT: Duration = Duration::from_secs(10 * 60);
/// The first wait between two looks at the build; it doubles up to
/// [`POLL_MAX`].
const POLL_FIRST: Duration = Duration::from_millis(250);
const POLL_MAX: Duration = Duration::from_secs(2);

/// WhatsApp's limits, as its sample app checks them
/// (`StickerPackValidator.java`; `KB_IN_BYTES = 1024`): a static sticker
/// at most 100 KB, an animated one 500 KB, the tray icon 50 KB, at most 30
/// stickers in a pack, names and publishers of at most 128 characters.
const STATIC_MAX: u64 = 100 * 1024;
const ANIMATED_MAX: u64 = 500 * 1024;
const TRAY_MAX: u64 = 50 * 1024;
const STICKERS_MAX: usize = 30;
const TEXT_MAX: usize = 128;

/// Largest `.wastickers` read: 30 animated stickers of 500 KB, a tray
/// icon and two text files, with room (the server's own cap).
const PACK_FILE_MAX: u64 = 32 * 1024 * 1024;
/// Most entries in a `.wastickers`: 30 stickers, the tray icon, the title
/// and the author, with room for a few others, which are ignored.
const ENTRIES_MAX: usize = 64;
/// Longest file name, pack id and version.
const NAME_MAX: usize = 64;
/// The emoji of a sticker that has none, as the server does.
const DEFAULT_EMOJI: &str = "\u{1f642}";

const INDEX: &str = "index.json";
const TMP: &str = ".tmp";

/// A pack id as the server makes them: letters, digits, `-` and `_`.
pub(crate) fn is_pack_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= NAME_MAX && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// A plain file name: letters, digits, `.`, `-` and `_`, not starting
/// with a `.` (so neither `.` nor `..`), no separator.
fn is_file_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= NAME_MAX
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// An `image_data_version` fit for a directory name (the server's are 16
/// hex digits).
fn is_version(s: &str) -> bool {
    !s.is_empty() && s.len() <= NAME_MAX && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `s` trimmed and cut to `max` characters.
fn cut(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

/// The index of a pack's parts (`<pack id>/index.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Index {
    /// The server's `built_at` of the build the parts come from.
    built_at: i64,
    /// Items the server left out.
    pub(crate) skipped: u32,
    parts: Vec<StoredPart>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredPart {
    number: u32,
    name: String,
    publisher: String,
    animated: bool,
    version: String,
    tray_file: String,
    stickers: Vec<StoredSticker>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredSticker {
    file: String,
    emojis: Vec<String>,
}

impl StoredPart {
    /// Part `pack` of a server build, checked: what can become a path is
    /// plain, and the rest within WhatsApp's limits (cut, or the default
    /// emoji, where that is enough).
    fn from_build(pack: &WhatsappPack) -> Result<StoredPart, String> {
        let stickers: Vec<StoredSticker> = pack
            .stickers
            .iter()
            .map(|s| {
                let mut emojis: Vec<String> = s
                    .emojis
                    .iter()
                    .map(|e| e.trim().to_string())
                    .filter(|e| !e.is_empty())
                    .take(3)
                    .collect();
                if emojis.is_empty() {
                    emojis.push(DEFAULT_EMOJI.into());
                }
                StoredSticker {
                    file: s.file.clone(),
                    emojis,
                }
            })
            .collect();
        let publisher = match cut(&pack.publisher, TEXT_MAX) {
            p if p.is_empty() => "stickpile".to_string(),
            p => p,
        };
        let part = StoredPart {
            number: pack.number,
            name: cut(&pack.title, TEXT_MAX),
            publisher,
            animated: pack.animated,
            version: pack.image_data_version.clone(),
            tray_file: pack.tray_file.clone(),
            stickers,
        };
        part.check()?;
        Ok(part)
    }

    /// The part can be served: also for one read back from the index.
    fn check(&self) -> Result<(), String> {
        if self.number == 0 {
            return Err("a part numbered 0".into());
        }
        if !is_version(&self.version) {
            return Err(format!(
                "part {}: an unusable version {:?}",
                self.number,
                cut(&self.version, 80)
            ));
        }
        if self.name.is_empty() {
            return Err(format!("part {}: no name", self.number));
        }
        if !is_file_name(&self.tray_file) || !self.tray_file.ends_with(".png") {
            return Err(format!("part {}: an unusable tray icon name", self.number));
        }
        if self.stickers.is_empty() || self.stickers.len() > STICKERS_MAX {
            return Err(format!("part {}: {} stickers", self.number, self.stickers.len()));
        }
        let mut names = HashSet::from([self.tray_file.as_str()]);
        for s in &self.stickers {
            if !is_file_name(&s.file) || !s.file.ends_with(".webp") || !names.insert(&s.file) {
                return Err(format!("part {}: an unusable sticker name", self.number));
            }
        }
        Ok(())
    }

    fn dir_name(&self) -> String {
        format!("{}-{}", self.number, self.version)
    }

    /// The files of the part, with the most bytes each may have.
    fn files(&self) -> HashMap<&str, u64> {
        let cap = if self.animated { ANIMATED_MAX } else { STATIC_MAX };
        let mut files: HashMap<&str, u64> = self.stickers.iter().map(|s| (s.file.as_str(), cap)).collect();
        files.insert(&self.tray_file, TRAY_MAX);
        files
    }
}

/// The unpacked parts, under `<cache_dir>/whatsapp/`.
pub(crate) struct Store {
    root: PathBuf,
}

impl Store {
    pub(crate) fn new(cache_dir: &Path) -> Store {
        Store {
            root: cache_dir.join("whatsapp"),
        }
    }

    fn pack_dir(&self, pack_id: &str) -> PathBuf {
        self.root.join(pack_id)
    }

    fn tmp_dir(&self) -> PathBuf {
        self.root.join(TMP)
    }

    fn read_index(&self, pack_id: &str) -> Option<Index> {
        let path = self.pack_dir(pack_id).join(INDEX);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!("WhatsApp packs of {pack_id}: {e}");
                return None;
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(index) => Some(index),
            Err(e) => {
                tracing::warn!("WhatsApp packs of {pack_id}: a broken index: {e}");
                None
            }
        }
    }

    /// Every file of `part` is in `dir`, not empty.
    fn complete(dir: &Path, part: &StoredPart) -> bool {
        part.files()
            .keys()
            .all(|f| std::fs::metadata(dir.join(f)).is_ok_and(|m| m.is_file() && m.len() > 0))
    }

    fn part(&self, pack_id: &str, part: &StoredPart) -> WhatsappPart {
        let dir = self.pack_dir(pack_id).join(part.dir_name());
        WhatsappPart {
            pack_id: pack_id.to_string(),
            number: part.number,
            identifier: format!("{pack_id}-{}", part.number),
            name: part.name.clone(),
            publisher: part.publisher.clone(),
            animated: part.animated,
            version: part.version.clone(),
            dir: dir.to_string_lossy().into_owned(),
            tray_file: part.tray_file.clone(),
            stickers: part
                .stickers
                .iter()
                .map(|s| WhatsappSticker {
                    file: s.file.clone(),
                    emojis: s.emojis.clone(),
                })
                .collect(),
        }
    }

    /// The parts of `pack_id` unpacked here whose files are all there, and
    /// the count of items the server left out; `None` when there are none.
    pub(crate) fn unpacked(&self, pack_id: &str) -> Option<(Vec<WhatsappPart>, u32)> {
        if !is_pack_id(pack_id) {
            return None;
        }
        let index = self.read_index(pack_id)?;
        let parts: Vec<WhatsappPart> = index
            .parts
            .iter()
            .filter(|p| match p.check() {
                Ok(()) => Self::complete(&self.pack_dir(pack_id).join(p.dir_name()), p),
                Err(e) => {
                    tracing::warn!("WhatsApp packs of {pack_id}: {e}");
                    false
                }
            })
            .map(|p| self.part(pack_id, p))
            .collect();
        (!parts.is_empty()).then_some((parts, index.skipped))
    }

    /// Every part unpacked here, by pack id and number.
    pub(crate) fn all(&self) -> Vec<WhatsappPart> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| is_pack_id(n))
            .collect();
        ids.sort();
        ids.iter()
            .filter_map(|id| self.unpacked(id))
            .flat_map(|(parts, _)| parts)
            .collect()
    }

    /// Remove every unpacked part (the account or server changed).
    pub(crate) fn clear(&self) {
        match std::fs::remove_dir_all(&self.root) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("removing the WhatsApp packs: {e}"),
        }
    }

    /// Make `index` the pack's, then remove what it does not name (older
    /// parts, unpacking left half done).
    pub(crate) fn commit(&self, pack_id: &str, index: &Index) -> Result<Vec<WhatsappPart>, LibraryError> {
        let dir = self.pack_dir(pack_id);
        let bytes = serde_json::to_vec_pretty(index).map_err(|e| LibraryError::other(format!("index: {e}")))?;
        let tmp = dir.join(format!(".{INDEX}.{}", stickpile_core::new_id()));
        let written = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, dir.join(INDEX)));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(LibraryError::Io {
                detail: format!("writing the WhatsApp packs' index: {e}"),
            });
        }
        let keep: HashSet<String> = index
            .parts
            .iter()
            .map(StoredPart::dir_name)
            .chain([INDEX.to_string()])
            .collect();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if keep.contains(&name) {
                    continue;
                }
                let path = entry.path();
                let removed = if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                if let Err(e) = removed {
                    tracing::debug!("removing an old WhatsApp part: {e}");
                }
            }
        }
        Ok(index.parts.iter().map(|p| self.part(pack_id, p)).collect())
    }
}

/// Why a downloaded part cannot be used.
fn broken(number: u32, why: impl std::fmt::Display) -> LibraryError {
    LibraryError::other(format!("the server's WhatsApp pack {number} cannot be used: {why}"))
}

/// The magic bytes of the files a part holds.
fn looks_right(name: &str, data: &[u8]) -> bool {
    if name.ends_with(".png") {
        data.starts_with(b"\x89PNG\r\n\x1a\n")
    } else {
        data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP"
    }
}

/// Unpack the `.wastickers` at `file` into the new directory `dest`: the
/// tray icon and stickers `part` names, nothing else. Fails on an entry
/// that is not a plain file name, a duplicate, a directory or link, an
/// encrypted or compressed entry, a file over its cap or of the wrong
/// kind, more than [`ENTRIES_MAX`] entries, a file over
/// [`PACK_FILE_MAX`], and a missing file.
fn unpack(file: &Path, part: &StoredPart, dest: &Path) -> Result<(), String> {
    let size = std::fs::metadata(file).map_err(|e| e.to_string())?.len();
    if size > PACK_FILE_MAX {
        return Err(format!("{size} bytes, more than {PACK_FILE_MAX}"));
    }
    let reader = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(reader).map_err(|e| format!("not a ZIP: {e}"))?;
    // The archive keeps one entry per name: a name twice is refused here.
    if zip.has_duplicate_names() {
        return Err("a name twice".into());
    }
    if zip.len() > ENTRIES_MAX {
        return Err(format!("{} entries, more than {ENTRIES_MAX}", zip.len()));
    }
    // Each entry's own header says what the directory at the end does.
    zip.validate_header_consistency()
        .map_err(|e| format!("inconsistent headers: {e}"))?;
    let wanted = part.files();
    let mut seen: HashSet<String> = HashSet::new();
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index_raw(i).map_err(|e| e.to_string())?;
        let raw = entry.name_raw();
        let name = match std::str::from_utf8(raw) {
            Ok(n) if is_file_name(n) => n.to_string(),
            _ => {
                let shown = String::from_utf8_lossy(&raw[..raw.len().min(80)]).into_owned();
                return Err(format!("an entry named {shown:?}"));
            }
        };
        if !seen.insert(name.clone()) {
            return Err(format!("{name} twice"));
        }
        if !entry.is_file() || entry.is_symlink() {
            return Err(format!("{name} is not a file"));
        }
        if entry.encrypted() || entry.compression() != zip::CompressionMethod::Stored {
            return Err(format!("{name} is compressed or encrypted"));
        }
        let Some(&cap) = wanted.get(name.as_str()) else {
            // `title.txt`, `author.txt` and whatever else: not needed.
            continue;
        };
        if entry.size() > cap {
            return Err(format!("{name} has {} bytes, more than {cap}", entry.size()));
        }
        let mut data = Vec::new();
        (&mut entry)
            .take(cap + 1)
            .read_to_end(&mut data)
            .map_err(|e| format!("{name}: {e}"))?;
        if data.len() as u64 > cap {
            return Err(format!("{name} has more than {cap} bytes"));
        }
        if !looks_right(&name, &data) {
            return Err(format!("{name} is not the picture its name says"));
        }
        std::fs::write(dest.join(&name), &data).map_err(|e| format!("{name}: {e}"))?;
    }
    if let Some(missing) = wanted.keys().find(|f| !seen.contains(**f)) {
        return Err(format!("{missing} is missing"));
    }
    Ok(())
}

/// Wait for the build `job` until it is done or failed, looking every
/// 250 ms at first, then less often; [`JOB_WAIT`] at most.
async fn wait(client: &ApiClient, mut job: Job) -> Result<Job, LibraryError> {
    let deadline = tokio::time::Instant::now() + JOB_WAIT;
    let mut pause = POLL_FIRST;
    while !matches!(job.status, JobStatus::Done | JobStatus::Failed) {
        if tokio::time::Instant::now() >= deadline {
            return Err(LibraryError::other(format!(
                "the server is still making the WhatsApp packs after {} minutes",
                JOB_WAIT.as_secs() / 60
            )));
        }
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(POLL_MAX);
        job = client.job(&job.id).await?;
    }
    Ok(job)
}

/// A 404 from the WhatsApp routes: the pack is gone (or not this
/// account's), or the server is a stickpile without WhatsApp packs, which
/// answers 404 for routes it does not have. Asking for the pack tells them
/// apart: the second is `Server { status: 404 }`.
async fn not_there(client: &ApiClient, pack_id: &str, e: CoreError) -> LibraryError {
    if e.status() != Some(404) {
        return e.into();
    }
    match client.set(pack_id).await {
        Ok(_) => LibraryError::Server {
            status: 404,
            detail: "this server makes no WhatsApp packs: it runs an older stickpile".into(),
        },
        Err(_) => e.into(),
    }
}

/// The server's current build of `pack_id`: the last one if it still
/// matches the pack, else a new one (or the one the server is making
/// already), waited for.
async fn current_build(client: &ApiClient, pack_id: &str) -> Result<WhatsappBuild, LibraryError> {
    let status = match client.whatsapp_status(pack_id).await {
        Ok(status) => status,
        Err(e) => return Err(not_there(client, pack_id, e).await),
    };
    if status.current {
        if let Some(build) = status.build {
            return Ok(build);
        }
    }
    let job = match status.job {
        Some(job) => job,
        None => match client.whatsapp_build(pack_id).await {
            Ok(job) => job,
            Err(e) => return Err(not_there(client, pack_id, e).await),
        },
    };
    let job = wait(client, job).await?;
    if job.status == JobStatus::Failed {
        let error = job.error.unwrap_or_default();
        // The server's words for a pack with too few stickers
        // (docs/api.md, "WhatsApp packs").
        if error.starts_with("nothing to export") {
            return Err(LibraryError::invalid("pack", error));
        }
        return Err(LibraryError::other(format!(
            "the server could not make the WhatsApp packs: {error}"
        )));
    }
    if let Some(build) = job.result.and_then(|r| serde_json::from_value::<WhatsappBuild>(r).ok()) {
        return Ok(build);
    }
    // No result to read: the build is the last one now.
    client
        .whatsapp_status(pack_id)
        .await?
        .build
        .ok_or_else(|| LibraryError::other("the server made the WhatsApp packs but has none to show"))
}

/// Make `pack_id` ready for WhatsApp: the server's current build (asked
/// for and waited for if needed), each part downloaded and unpacked unless
/// it is here already. Returns the index for [`Store::commit`]; `progress`
/// hears each step.
pub(crate) async fn fetch(
    client: &ApiClient,
    store: &Store,
    pack_id: &str,
    progress: &(dyn Fn(WhatsappProgress) + Send + Sync),
) -> Result<Index, LibraryError> {
    progress(WhatsappProgress {
        step: WhatsappStep::Building,
        part: 0,
        parts: 0,
    });
    let build = current_build(client, pack_id).await?;
    if build.packs.is_empty() {
        return Err(LibraryError::other("the server made no WhatsApp packs"));
    }
    let pack_dir = store.pack_dir(pack_id);
    let tmp = store.tmp_dir();
    // Downloads a cancelled call left behind.
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(CoreError::from)?;
    std::fs::create_dir_all(&pack_dir).map_err(CoreError::from)?;
    let count = u32::try_from(build.packs.len()).unwrap_or(u32::MAX);
    let mut parts = Vec::with_capacity(build.packs.len());
    for pack in &build.packs {
        let part = StoredPart::from_build(pack).map_err(|e| broken(pack.number, e))?;
        let dir = pack_dir.join(part.dir_name());
        if !Store::complete(&dir, &part) {
            progress(WhatsappProgress {
                step: WhatsappStep::Downloading,
                part: pack.number.min(count),
                parts: count,
            });
            let file = tmp.join(format!("{}.wastickers", stickpile_core::new_id()));
            client.download_whatsapp_pack(pack_id, pack.number, &file).await?;
            let unpacking = pack_dir.join(format!(".{}.{}", part.dir_name(), stickpile_core::new_id()));
            let (file2, part2, unpacking2, dir2) = (file.clone(), part.clone(), unpacking.clone(), dir.clone());
            let unpacked = tokio::task::spawn_blocking(move || {
                let result = unpack(&file2, &part2, &unpacking2).and_then(|()| {
                    if dir2.exists() {
                        std::fs::remove_dir_all(&dir2).map_err(|e| e.to_string())?;
                    }
                    std::fs::rename(&unpacking2, &dir2).map_err(|e| e.to_string())
                });
                let _ = std::fs::remove_file(&file2);
                if result.is_err() {
                    let _ = std::fs::remove_dir_all(&unpacking2);
                }
                result
            })
            .await?;
            unpacked.map_err(|e| broken(pack.number, e))?;
        }
        parts.push(part);
    }
    parts.sort_by_key(|p| p.number);
    Ok(Index {
        built_at: build.built_at,
        skipped: u32::try_from(build.skipped.len()).unwrap_or(u32::MAX),
        parts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_the_server_must_be_plain() {
        for good in ["01.webp", "cover.png", "a-b_c.d", "X9"] {
            assert!(is_file_name(good), "{good}");
        }
        for bad in [
            "",
            ".",
            "..",
            "../x",
            "/etc/x",
            "a/b",
            "a\\b",
            ".hidden",
            "x\0",
            "caf\u{e9}.webp",
            "a b",
            &"x".repeat(65),
        ] {
            assert!(!is_file_name(bad), "{bad:?}");
        }
        assert!(is_pack_id("01hzy-x_9"));
        assert!(!is_pack_id("a.b") && !is_pack_id("") && !is_pack_id("../x"));
        assert!(is_version("0123abcdef") && !is_version("1.0") && !is_version("../x"));
    }

    fn pack(stickers: &[&str]) -> WhatsappPack {
        WhatsappPack {
            number: 2,
            identifier: "cats-2".into(),
            title: format!("  {}  ", "n".repeat(200)),
            publisher: " ".into(),
            animated: false,
            file: "cats-2.wastickers".into(),
            url: "/x".into(),
            tray_file: "cover.png".into(),
            image_data_version: "00ff".into(),
            size_bytes: 0,
            stickers: stickers
                .iter()
                .map(|f| stickpile_api::WhatsappSticker {
                    item_id: "i".into(),
                    name: "n".into(),
                    file: f.to_string(),
                    emojis: vec![" ".into(), "a".into(), "b".into(), "c".into(), "d".into()],
                    animated: false,
                    size_bytes: 0,
                    note: None,
                })
                .collect(),
        }
    }

    #[test]
    fn parts_are_checked_and_cut_to_whatsapps_limits() {
        let part = StoredPart::from_build(&pack(&["01.webp", "02.webp", "03.webp"])).unwrap();
        assert_eq!(part.name.chars().count(), 128);
        assert_eq!(part.publisher, "stickpile");
        assert_eq!(part.stickers[0].emojis, ["a", "b", "c"]);
        assert_eq!(part.dir_name(), "2-00ff");
        assert_eq!(part.files().len(), 4);
        assert_eq!(part.files()["cover.png"], TRAY_MAX);
        assert_eq!(part.files()["01.webp"], STATIC_MAX);

        for bad in [
            pack(&["../01.webp"]),
            pack(&["01.png"]),
            pack(&["01.webp", "01.webp"]),
            pack(&[]),
            WhatsappPack {
                tray_file: "../cover.png".into(),
                ..pack(&["01.webp"])
            },
            WhatsappPack {
                image_data_version: "../../x".into(),
                ..pack(&["01.webp"])
            },
            WhatsappPack {
                number: 0,
                ..pack(&["01.webp"])
            },
        ] {
            assert!(StoredPart::from_build(&bad).is_err(), "{bad:?}");
        }
        let many: Vec<String> = (1..=31).map(|i| format!("{i:02}.webp")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(StoredPart::from_build(&pack(&many)).is_err());
    }
}
