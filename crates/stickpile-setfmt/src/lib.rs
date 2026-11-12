//! The set directory format (docs/set-format.md): a directory with `set.toml`
//! and media files ordered by lexical filename. It is an import/export format,
//! not a live mirror of the database.
//!
//! ```toml
//! version = 3
//! name = "My cats"
//! description = ""
//! visibility = "unlisted"
//! aspect_ratio = "1:1"
//! recommended_px = 512
//!
//! [items."001-cat.png"]
//! name = "cat"
//! keywords = ["cat", "stare"]
//! keywords_source = "ai"
//! emojis = ["😼"]
//! vibe = "smug"
//! vibe_source = "ai"
//! tags = ["reaction"]
//! ```
//!
//! Unknown keys at any level are preserved through parse -> serialise so a
//! round trip never loses user data. Every path coming from a manifest, a
//! directory or a tar is validated with [`safe_filename`].
//!
//! Ordering is the byte-wise (`LC_ALL=C`) order of the UTF-8 filenames. The
//! per-file metadata of a [`SetBundle`] lives in [`SetFile::meta`]; the
//! writers ([`write_dir`], [`write_tar`]) rebuild `manifest.items` from the
//! files, and the readers return a manifest whose `items` match the files
//! exactly.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
pub use stickpile_api::{FieldSource, Visibility};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

pub const MANIFEST: &str = "set.toml";
/// File extensions considered media inside a set directory.
pub const MEDIA_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "mp4", "webm"];
/// Upper bound for `set.toml` itself (it only holds text metadata).
pub const MAX_MANIFEST_BYTES: u64 = 4 << 20;
/// Modification time written into every tar header (reproducible archives).
pub const TAR_MTIME: u64 = 1;
/// Longest filename (in bytes) accepted by [`safe_filename`].
pub const MAX_FILENAME_BYTES: usize = 255;

#[derive(Debug, thiserror::Error)]
pub enum SetFmtError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("set.toml: {0}")]
    Parse(String),
    #[error("unsafe or invalid filename {0:?}")]
    BadFilename(String),
    #[error("archive: {0}")]
    Archive(String),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> SetFmtError + '_ {
    move |source| SetFmtError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ItemMeta {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "is_user")]
    pub name_source: FieldSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "is_user")]
    pub keywords_source: FieldSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emojis: Vec<String>,
    #[serde(default, skip_serializing_if = "is_user")]
    pub emojis_source: FieldSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vibe: Option<String>,
    #[serde(default, skip_serializing_if = "is_user")]
    pub vibe_source: FieldSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Unknown keys, preserved verbatim.
    #[serde(flatten)]
    pub extra: toml::Table,
}

fn is_user(s: &FieldSource) -> bool {
    *s == FieldSource::User
}

const ITEM_KEYS: &[&str] = &[
    "name",
    "name_source",
    "keywords",
    "keywords_source",
    "emojis",
    "emojis_source",
    "vibe",
    "vibe_source",
    "tags",
];
const SET_KEYS: &[&str] = &[
    "version",
    "name",
    "slug",
    "description",
    "visibility",
    "aspect_ratio",
    "recommended_px",
    "cover",
    "items",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Bumped on every change; lets clients tell an upstream update from a local edit.
    #[serde(default = "one")]
    pub version: u64,
    /// Empty when `set.toml` has no `name`; the readers then fall back to the
    /// directory name.
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub visibility: Visibility,
    /// `"W:H"`, e.g. `"1:1"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_px: Option<u32>,
    /// Filename of the item that is the set's cover (Telegram: the pack's
    /// thumbnail). Absent: the first item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    /// Keyed by filename (not path).
    #[serde(default)]
    pub items: BTreeMap<String, ItemMeta>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

fn one() -> u64 {
    1
}

impl Manifest {
    pub fn new(name: impl Into<String>) -> Manifest {
        Manifest {
            version: 1,
            name: name.into(),
            slug: None,
            description: String::new(),
            visibility: Visibility::default(),
            aspect_ratio: None,
            recommended_px: None,
            cover: None,
            items: BTreeMap::new(),
            extra: toml::Table::new(),
        }
    }

    pub fn parse(text: &str) -> Result<Manifest, SetFmtError> {
        toml::from_str(text).map_err(|e| SetFmtError::Parse(e.to_string().trim_end().to_string()))
    }

    /// Deterministic TOML: known keys in a fixed order, unknown keys after
    /// them, items sorted by filename. `parse(to_toml(m)) == m` for every
    /// manifest whose `extra` tables do not shadow known keys and whose
    /// `version` fits a TOML integer (both are enforced here: shadowing keys
    /// are dropped, `version` is clamped to `i64::MAX`).
    pub fn to_toml(&self) -> String {
        let mut m = self.clone();
        m.version = m.version.min(i64::MAX as u64);
        for key in SET_KEYS {
            m.extra.remove(*key);
        }
        for item in m.items.values_mut() {
            for key in ITEM_KEYS {
                item.extra.remove(*key);
            }
        }
        toml::to_string(&m).expect("a sanitised manifest always serialises")
    }

    /// `"16:9"` -> (16, 9)
    pub fn aspect(&self) -> Option<(u32, u32)> {
        parse_aspect(self.aspect_ratio.as_deref()?)
    }

    /// Sets `aspect_ratio` from a (W, H) pair (as used by the REST API).
    pub fn set_aspect(&mut self, aspect: Option<(u32, u32)>) {
        self.aspect_ratio = aspect.map(|(w, h)| format!("{w}:{h}"));
    }
}

/// `"W:H"` with both sides > 0.
pub fn parse_aspect(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.trim().split_once(':')?;
    let w: u32 = w.trim().parse().ok()?;
    let h: u32 = h.trim().parse().ok()?;
    (w > 0 && h > 0).then_some((w, h))
}

/// One media file of a set, in set order.
#[derive(Debug, Clone, PartialEq)]
pub struct SetFile {
    pub filename: String,
    pub meta: ItemMeta,
    pub bytes: Vec<u8>,
}

/// A whole set in memory (import/export unit).
#[derive(Debug, Clone, PartialEq)]
pub struct SetBundle {
    pub manifest: Manifest,
    /// Lexical filename order.
    pub files: Vec<SetFile>,
    /// Non-fatal problems: files listed in the manifest but missing, unknown
    /// extensions skipped, ...
    pub warnings: Vec<String>,
}

impl SetBundle {
    /// The manifest as written by the exporters: `items` rebuilt from `files`.
    pub fn export_manifest(&self) -> Manifest {
        let mut manifest = self.manifest.clone();
        manifest.items = self
            .files
            .iter()
            .map(|f| (f.filename.clone(), f.meta.clone()))
            .collect();
        manifest
    }

    /// Checks every filename (safe, media extension, unique under NFC).
    fn validate_files(&self) -> Result<(), SetFmtError> {
        let mut seen = HashSet::new();
        for f in &self.files {
            safe_filename(&f.filename)?;
            if !is_media_filename(&f.filename) || f.filename == MANIFEST {
                return Err(SetFmtError::BadFilename(f.filename.clone()));
            }
            if !seen.insert(nfc(&f.filename)) {
                return Err(SetFmtError::BadFilename(format!("{} (duplicate)", f.filename)));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Read a set directory. Missing `set.toml` is allowed (name from the
/// directory name). Hidden files and non-media files are skipped. Symlinks are
/// not followed.
pub fn read_dir(dir: &Path) -> Result<SetBundle, SetFmtError> {
    let meta = fs::metadata(dir).map_err(io_err(dir))?;
    if !meta.is_dir() {
        return Err(SetFmtError::Io {
            path: dir.to_path_buf(),
            source: io::Error::other("not a directory"),
        });
    }
    let mut warnings = Vec::new();
    let mut manifest_text = None;
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(io_err(dir))? {
        let entry = entry.map_err(io_err(dir))?;
        let path = entry.path();
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(raw) => {
                warnings.push(format!(
                    "{:?}: skipped (filename is not valid UTF-8)",
                    raw.to_string_lossy()
                ));
                continue;
            }
        };
        if name.starts_with('.') {
            continue;
        }
        // `DirEntry::file_type` does not follow symlinks.
        let ft = entry.file_type().map_err(io_err(&path))?;
        if ft.is_symlink() {
            warnings.push(format!("{name}: symlink skipped"));
            continue;
        }
        if name == MANIFEST {
            if ft.is_file() {
                let bytes = read_regular_file(&path, MAX_MANIFEST_BYTES)?;
                manifest_text = Some(manifest_from_bytes(bytes)?);
            } else {
                warnings.push(format!("{MANIFEST}: not a regular file, ignored"));
            }
            continue;
        }
        if ft.is_dir() {
            warnings.push(format!("{name}/: subdirectory skipped"));
            continue;
        }
        if !ft.is_file() {
            warnings.push(format!("{name}: not a regular file, skipped"));
            continue;
        }
        if safe_filename(&name).is_err() {
            warnings.push(format!("{name:?}: unsafe filename, skipped"));
            continue;
        }
        if !is_media_filename(&name) {
            warnings.push(format!("{name}: not a media file, skipped"));
            continue;
        }
        files.push((name, read_regular_file(&path, u64::MAX)?));
    }
    let default_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .or_else(|| {
            fs::canonicalize(dir)
                .ok()?
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "set".to_string());
    assemble(manifest_text.as_deref(), &default_name, files, warnings)
}

/// Opens without following a final symlink (closing the race between the
/// `file_type` check and the open) and reads at most `cap` bytes.
fn read_regular_file(path: &Path, cap: u64) -> Result<Vec<u8>, SetFmtError> {
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts.open(path).map_err(io_err(path))?;
    let meta = file.metadata().map_err(io_err(path))?;
    if !meta.is_file() {
        return Err(SetFmtError::Io {
            path: path.to_path_buf(),
            source: io::Error::other("not a regular file"),
        });
    }
    let mut bytes = Vec::with_capacity(meta.len().min(cap) as usize);
    file.take(cap.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io_err(path))?;
    if bytes.len() as u64 > cap {
        return Err(SetFmtError::Io {
            path: path.to_path_buf(),
            source: io::Error::other("file too large"),
        });
    }
    Ok(bytes)
}

fn manifest_from_bytes(bytes: Vec<u8>) -> Result<String, SetFmtError> {
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(SetFmtError::Parse(format!("larger than {MAX_MANIFEST_BYTES} bytes")));
    }
    let mut text = String::from_utf8(bytes).map_err(|_| SetFmtError::Parse("not valid UTF-8".into()))?;
    if text.starts_with('\u{feff}') {
        text.remove(0);
    }
    Ok(text)
}

/// Combine a manifest and loose media files into a bundle: sort, match
/// manifest entries to files (exact name first, then NFC-equal name), give
/// files without an entry default metadata, warn about missing files.
fn assemble(
    manifest_text: Option<&str>,
    default_name: &str,
    mut files: Vec<(String, Vec<u8>)>,
    mut warnings: Vec<String>,
) -> Result<SetBundle, SetFmtError> {
    let mut manifest = match manifest_text {
        Some(text) => Manifest::parse(text)?,
        None => Manifest::new(default_name),
    };
    if manifest.name.trim().is_empty() {
        manifest.name = default_name.to_string();
    }
    if let Some(aspect) = &manifest.aspect_ratio {
        if parse_aspect(aspect).is_none() {
            warnings.push(format!("aspect_ratio {aspect:?} is not of the form \"W:H\"; ignored"));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut listed = std::mem::take(&mut manifest.items);
    let mut by_nfc: HashMap<String, String> = HashMap::new();
    for key in listed.keys() {
        by_nfc.entry(nfc(key)).or_insert_with(|| key.clone());
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(files.len());
    for (filename, bytes) in files {
        let key = nfc(&filename);
        if !seen.insert(key.clone()) {
            warnings.push(format!(
                "{filename}: same name as another file after Unicode normalisation, skipped"
            ));
            continue;
        }
        let mut meta = match listed.remove(&filename) {
            Some(meta) => meta,
            None => by_nfc
                .get(&key)
                .and_then(|orig| listed.remove(orig))
                .unwrap_or_default(),
        };
        if meta.name.trim().is_empty() {
            meta.name = display_stem(&filename);
        }
        manifest.items.insert(filename.clone(), meta.clone());
        out.push(SetFile { filename, meta, bytes });
    }
    for missing in listed.keys() {
        warnings.push(format!("{MANIFEST} lists {missing:?} but the file is missing"));
    }
    Ok(SetBundle {
        manifest,
        files: out,
        warnings,
    })
}

/// Collects the entries of a tar or a folder upload: at most one top-level
/// directory, no deeper nesting, no traversal, hidden entries skipped.
#[derive(Default)]
struct Collector {
    top: Option<String>,
    root_files: bool,
    manifest: Option<String>,
    files: Vec<(String, Vec<u8>)>,
    seen: HashSet<String>,
    warnings: Vec<String>,
}

enum Slot {
    Skip,
    Manifest,
    Media(String),
}

impl Collector {
    fn classify(&mut self, path: &str, is_dir: bool) -> Result<Slot, SetFmtError> {
        let bad = |why: &str| SetFmtError::Archive(format!("{path:?}: {why}"));
        if path.starts_with('/') || path.starts_with('\\') {
            return Err(bad("absolute path"));
        }
        if path.contains('\0') || path.contains('\\') {
            return Err(bad("invalid characters in path"));
        }
        let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
        if comps.iter().any(|c| nfc(c) == "..") {
            return Err(bad("path traversal"));
        }
        if comps.is_empty() {
            return Ok(Slot::Skip);
        }
        // Hidden entries (and everything below a hidden directory, e.g. `.git/`) are ignored.
        if comps.iter().any(|c| c.starts_with('.')) {
            return Ok(Slot::Skip);
        }
        for c in &comps {
            safe_filename(c)?;
        }
        if is_dir {
            return match comps.len() {
                1 => self.set_top(comps[0], path).map(|_| Slot::Skip),
                _ => Err(bad(
                    "nested directories are not allowed (one top-level directory at most)",
                )),
            };
        }
        let name = match comps.len() {
            1 => {
                if self.top.is_some() {
                    return Err(bad("files both inside and outside the top-level directory"));
                }
                self.root_files = true;
                comps[0]
            }
            2 => {
                self.set_top(comps[0], path)?;
                comps[1]
            }
            _ => {
                return Err(bad(
                    "nested directories are not allowed (one top-level directory at most)",
                ))
            }
        };
        if name == MANIFEST {
            if self.manifest.is_some() {
                return Err(bad("duplicate set.toml"));
            }
            return Ok(Slot::Manifest);
        }
        if !is_media_filename(name) {
            self.warnings.push(format!("{name}: not a media file, skipped"));
            return Ok(Slot::Skip);
        }
        if !self.seen.insert(nfc(name)) {
            return Err(bad("duplicate entry"));
        }
        Ok(Slot::Media(name.to_string()))
    }

    fn set_top(&mut self, dir: &str, path: &str) -> Result<(), SetFmtError> {
        if self.root_files {
            return Err(SetFmtError::Archive(format!(
                "{path:?}: files both inside and outside the top-level directory"
            )));
        }
        match &self.top {
            Some(top) if top != dir => Err(SetFmtError::Archive(format!(
                "{path:?}: more than one top-level directory ({top:?}, {dir:?})"
            ))),
            Some(_) => Ok(()),
            None => {
                self.top = Some(dir.to_string());
                Ok(())
            }
        }
    }

    fn finish(self, default_name: &str) -> Result<SetBundle, SetFmtError> {
        let name = self.top.clone().unwrap_or_else(|| default_name.to_string());
        assemble(self.manifest.as_deref(), &name, self.files, self.warnings)
    }
}

/// Read a tar produced by [`write_tar`] (or any tar of a set directory, with
/// or without a single top-level directory). Rejects absolute paths, `..`,
/// links and device entries; enforces `max_total_bytes`.
pub fn read_tar(tar: &[u8], max_total_bytes: u64) -> Result<SetBundle, SetFmtError> {
    let arch = |e: io::Error| SetFmtError::Archive(e.to_string());
    let mut archive = tar::Archive::new(io::Cursor::new(tar));
    let mut c = Collector::default();
    let mut total: u64 = 0;
    for entry in archive.entries().map_err(arch)? {
        let mut entry = entry.map_err(arch)?;
        let kind = entry.header().entry_type();
        let path_bytes = entry.path_bytes().into_owned();
        let path = String::from_utf8(path_bytes).map_err(|e| {
            SetFmtError::Archive(format!(
                "{:?}: path is not UTF-8",
                String::from_utf8_lossy(e.as_bytes())
            ))
        })?;
        let is_dir = match kind {
            tar::EntryType::Regular | tar::EntryType::Continuous => false,
            tar::EntryType::Directory => true,
            // `git archive` writes a pax global header; plain metadata, no file.
            tar::EntryType::XGlobalHeader | tar::EntryType::XHeader => continue,
            other => {
                return Err(SetFmtError::Archive(format!(
                    "{path:?}: entry type {other:?} not allowed (only regular files and directories)"
                )))
            }
        };
        let slot = c.classify(&path, is_dir)?;
        let cap = match slot {
            Slot::Skip => continue,
            Slot::Manifest => MAX_MANIFEST_BYTES.min(max_total_bytes.saturating_sub(total)),
            Slot::Media(_) => max_total_bytes.saturating_sub(total),
        };
        let too_big = || SetFmtError::Archive(format!("contents exceed {max_total_bytes} bytes"));
        // The header size is only a hint; the real count is what we read.
        if entry.header().size().map_err(arch)? > cap {
            return Err(too_big());
        }
        let mut bytes = Vec::new();
        (&mut entry)
            .take(cap.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(arch)?;
        if bytes.len() as u64 > cap {
            return Err(too_big());
        }
        total += bytes.len() as u64;
        match slot {
            Slot::Manifest => c.manifest = Some(manifest_from_bytes(bytes)?),
            Slot::Media(name) => c.files.push((name, bytes)),
            Slot::Skip => unreachable!(),
        }
    }
    c.finish("Imported set")
}

/// Build a bundle from loose (relative path, bytes) pairs, e.g. a multipart
/// folder upload. Paths may include one leading directory component.
///
/// `manifest` (the text of a separately uploaded `set.toml`) takes precedence
/// over a `set.toml` found among `files`.
pub fn from_files(manifest: Option<&str>, files: Vec<(String, Vec<u8>)>) -> Result<SetBundle, SetFmtError> {
    let mut c = Collector::default();
    let mut found_manifest = None;
    for (path, bytes) in files {
        match c.classify(&path, false)? {
            Slot::Skip => {}
            Slot::Manifest => {
                found_manifest = Some(bytes);
                c.manifest = Some(String::new()); // marks "seen" for the duplicate check
            }
            Slot::Media(name) => c.files.push((name, bytes)),
        }
    }
    c.manifest = match (manifest, found_manifest) {
        (Some(text), Some(_)) => {
            c.warnings.push(format!(
                "{MANIFEST} in the upload ignored; the separately supplied manifest is used"
            ));
            Some(text.to_string())
        }
        (Some(text), None) => Some(text.to_string()),
        (None, Some(bytes)) => Some(manifest_from_bytes(bytes)?),
        (None, None) => None,
    };
    c.finish("Imported set")
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Write a set directory (creating it; existing media files not in the bundle
/// are removed so the directory equals the export).
///
/// `set.toml` is written from [`SetBundle::export_manifest`]. Files are
/// replaced atomically (temp file + rename, so a symlink at the target is
/// replaced rather than followed); files whose content is unchanged are left
/// alone. Only regular files and symlinks with a media extension are removed;
/// hidden entries (`.git`), subdirectories and other files are kept.
pub fn write_dir(dir: &Path, bundle: &SetBundle) -> Result<(), SetFmtError> {
    bundle.validate_files()?;
    fs::create_dir_all(dir).map_err(io_err(dir))?;
    write_if_changed(dir, MANIFEST, bundle.export_manifest().to_toml().as_bytes())?;
    for f in &bundle.files {
        write_if_changed(dir, &f.filename, &f.bytes)?;
    }
    let keep: HashSet<&str> = bundle.files.iter().map(|f| f.filename.as_str()).collect();
    for entry in fs::read_dir(dir).map_err(io_err(dir))? {
        let entry = entry.map_err(io_err(dir))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') || keep.contains(name.as_str()) || !is_media_filename(&name) {
            continue;
        }
        let ft = entry.file_type().map_err(io_err(&entry.path()))?;
        if ft.is_file() || ft.is_symlink() {
            fs::remove_file(entry.path()).map_err(io_err(&entry.path()))?;
        }
    }
    Ok(())
}

fn write_if_changed(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), SetFmtError> {
    let target = dir.join(name);
    if let Ok(meta) = fs::symlink_metadata(&target) {
        if meta.is_file() && meta.len() == bytes.len() as u64 {
            if let Ok(existing) = read_regular_file(&target, bytes.len() as u64) {
                if existing == bytes {
                    return Ok(());
                }
            }
        }
    }
    let tmp = dir.join(format!(".stickpile-tmp-{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(io_err(&tmp))?;
    let written = file.write_all(bytes).and_then(|_| file.flush());
    drop(file);
    if let Err(e) = written.and_then(|_| fs::rename(&tmp, &target)) {
        let _ = fs::remove_file(&tmp);
        return Err(SetFmtError::Io {
            path: target,
            source: e,
        });
    }
    Ok(())
}

/// Bundle as an uncompressed tar with a single top-level directory `<slug>/`.
///
/// Deterministic: `set.toml` first, then files in set order; GNU headers with
/// mode 0644/0755, uid/gid 0, empty user/group names and mtime [`TAR_MTIME`].
pub fn write_tar(bundle: &SetBundle, top_dir: &str) -> Result<Vec<u8>, SetFmtError> {
    safe_filename(top_dir)?;
    bundle.validate_files()?;
    let arch = |e: io::Error| SetFmtError::Archive(e.to_string());
    let mut builder = tar::Builder::new(Vec::new());
    let header = |kind: tar::EntryType, mode: u32, size: u64| {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(kind);
        h.set_mode(mode);
        h.set_size(size);
        h.set_mtime(TAR_MTIME);
        h.set_uid(0);
        h.set_gid(0);
        h
    };
    let mut h = header(tar::EntryType::Directory, 0o755, 0);
    builder
        .append_data(&mut h, format!("{top_dir}/"), io::empty())
        .map_err(arch)?;
    let manifest = bundle.export_manifest().to_toml();
    let mut h = header(tar::EntryType::Regular, 0o644, manifest.len() as u64);
    builder
        .append_data(&mut h, format!("{top_dir}/{MANIFEST}"), manifest.as_bytes())
        .map_err(arch)?;
    let mut files: Vec<&SetFile> = bundle.files.iter().collect();
    files.sort_by(|a, b| a.filename.cmp(&b.filename));
    for f in files {
        let mut h = header(tar::EntryType::Regular, 0o644, f.bytes.len() as u64);
        builder
            .append_data(&mut h, format!("{top_dir}/{}", f.filename), f.bytes.as_slice())
            .map_err(arch)?;
    }
    builder.into_inner().map_err(arch)
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// `001-cat.png`: zero-padded position (width = max(3, digits of total)),
/// slugified stem, extension.
///
/// `position` is written as given, so pass 1-based positions (the first item
/// is `001`). An ordering prefix already present in `stem` (`001-cat`) is
/// stripped first, so feeding back an old export stem does not stack
/// prefixes. A stem without any ASCII letters or digits becomes `item`.
pub fn export_filename(position: usize, total: usize, stem: &str, ext: &str) -> String {
    let width = total.to_string().len().max(3);
    let stem = slug_with(strip_order_prefix(stem), "item");
    let ext: String = ext
        .trim_start_matches('.')
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if ext.is_empty() {
        format!("{position:0width$}-{stem}")
    } else {
        format!("{position:0width$}-{stem}.{}", ext.to_ascii_lowercase())
    }
}

/// Strip the numeric ordering prefix and extension: `001-cat.png` -> `cat`.
///
/// The prefix is a run of ASCII digits followed by at least one of `-`, `_`,
/// `.`, space (`12_-dog.gif` -> `dog`); `2fast.png` keeps its digits. A name
/// that would become empty keeps its stem (`007.png` -> `007`).
pub fn display_stem(filename: &str) -> String {
    let stem = match filename.rfind('.') {
        Some(i) if i > 0 => &filename[..i],
        _ => filename,
    };
    strip_order_prefix(stem).to_string()
}

fn strip_order_prefix(stem: &str) -> &str {
    let rest = stem.trim_start_matches(|c: char| c.is_ascii_digit());
    if rest.len() == stem.len() {
        return stem;
    }
    let after = rest.trim_start_matches(['-', '_', '.', ' ']);
    if after.len() == rest.len() || after.is_empty() {
        stem
    } else {
        after
    }
}

/// `[a-z0-9-]`, collapsed dashes, max 64 chars; non-empty (falls back to "set").
///
/// Accents are folded (`Café` -> `cafe`); other non-ASCII characters act as
/// separators.
pub fn slugify(s: &str) -> String {
    slug_with(s, "set")
}

fn slug_with(s: &str, fallback: &str) -> String {
    const MAX: usize = 64;
    let mut out = String::new();
    let mut pending_dash = false;
    for c in s.nfkd() {
        if is_combining_mark(c) {
            continue;
        }
        if c.is_ascii_alphanumeric() {
            let need = if pending_dash && !out.is_empty() { 2 } else { 1 };
            if out.len() + need > MAX {
                break;
            }
            if need == 2 {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
            pending_dash = false;
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

/// A single path component that is safe to create inside a set directory.
///
/// Rejects: empty names, names longer than 255 bytes, `/` and `\`, NUL and
/// other control characters, bidirectional-override characters, `.`/`..`
/// and hidden names (leading `.`). The checks are made on the NFC form too,
/// so a decomposed spelling cannot slip past them.
pub fn safe_filename(name: &str) -> Result<&str, SetFmtError> {
    let bad = || Err(SetFmtError::BadFilename(name.to_string()));
    if name.is_empty() || name.len() > MAX_FILENAME_BYTES {
        return bad();
    }
    if name
        .chars()
        .any(|c| c == '/' || c == '\\' || c.is_control() || is_bidi_control(c))
    {
        return bad();
    }
    let normalised = nfc(name);
    if normalised.len() > MAX_FILENAME_BYTES || normalised.starts_with('.') || name.starts_with('.') {
        return bad();
    }
    if normalised.chars().any(|c| c == '/' || c == '\\' || c.is_control()) {
        return bad();
    }
    Ok(name)
}

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}' | '\u{061c}')
}

/// True if the (case-insensitive) extension is one of [`MEDIA_EXTENSIONS`].
pub fn is_media_filename(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => MEDIA_EXTENSIONS.iter().any(|m| m.eq_ignore_ascii_case(ext)),
        _ => false,
    }
}

/// NFC form of a filename; two names with the same NFC form are the same
/// item (macOS writes decomposed names).
pub fn nfc(name: &str) -> String {
    name.nfc().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_stem_table() {
        for (input, want) in [
            ("001-cat.png", "cat"),
            ("cat.png", "cat"),
            ("12_-dog.gif", "dog"),
            ("0001 big cat.webm", "big cat"),
            ("2fast.png", "2fast"),
            ("007.png", "007"),
            ("007-.png", "007-"),
            ("noext", "noext"),
            ("001-cat.tar.gz", "cat.tar"),
            ("001-котик.png", "котик"),
        ] {
            assert_eq!(display_stem(input), want, "{input}");
        }
    }

    #[test]
    fn slugify_table() {
        for (input, want) in [
            ("My Cats", "my-cats"),
            ("  --Hello__World--  ", "hello-world"),
            ("Café Crème", "cafe-creme"),
            ("котики", "set"),
            ("", "set"),
            ("a/b\\c..d", "a-b-c-d"),
            ("ÅNGSTRÖM 2024!", "angstrom-2024"),
        ] {
            assert_eq!(slugify(input), want, "{input}");
        }
        let long = "ab ".repeat(100);
        let s = slugify(&long);
        assert!(s.len() <= 64, "{s}");
        assert!(!s.ends_with('-'));
        assert_eq!(slugify(&"x".repeat(200)).len(), 64);
    }

    #[test]
    fn export_filename_table() {
        assert_eq!(export_filename(1, 3, "cat", "png"), "001-cat.png");
        assert_eq!(export_filename(12, 999, "Big Cat", ".GIF"), "012-big-cat.gif");
        assert_eq!(export_filename(1, 1000, "cat", "png"), "0001-cat.png");
        assert_eq!(export_filename(5, 12345, "x", "webm"), "00005-x.webm");
        assert_eq!(export_filename(2, 3, "001-cat", "png"), "002-cat.png");
        assert_eq!(export_filename(3, 3, "😂", "png"), "003-item.png");
        assert_eq!(export_filename(3, 3, "cat", ""), "003-cat");
        // Round trip through display_stem.
        assert_eq!(display_stem(&export_filename(7, 10, "sigh", "mp4")), "sigh");
    }

    #[test]
    fn safe_filename_table() {
        for ok in ["cat.png", "001-cat.png", "котик.webp", "a b.gif", &"x".repeat(251)] {
            assert!(safe_filename(ok).is_ok(), "{ok:?}");
        }
        let long = "é".repeat(128); // 256 bytes
        for bad in [
            "",
            ".",
            "..",
            ".hidden.png",
            "a/b.png",
            "..\\x.png",
            "a\\b.png",
            "nul\0.png",
            "tab\t.png",
            "nl\n.png",
            "del\u{7f}.png",
            "c1\u{85}.png",
            "rtl\u{202e}gnp.exe",
            long.as_str(),
            &"x".repeat(256),
        ] {
            assert!(safe_filename(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn aspect_parsing() {
        let mut m = Manifest::new("x");
        assert_eq!(m.aspect(), None);
        m.aspect_ratio = Some("16:9".into());
        assert_eq!(m.aspect(), Some((16, 9)));
        m.aspect_ratio = Some(" 1 : 1 ".into());
        assert_eq!(m.aspect(), Some((1, 1)));
        for bad in ["16x9", "0:1", "1:0", "-1:2", "a:b", "1:2:3", ""] {
            m.aspect_ratio = Some(bad.into());
            assert_eq!(m.aspect(), None, "{bad}");
        }
        m.set_aspect(Some((4, 3)));
        assert_eq!(m.aspect_ratio.as_deref(), Some("4:3"));
    }

    #[test]
    fn media_extensions() {
        assert!(is_media_filename("a.PNG"));
        assert!(is_media_filename("a.b.webm"));
        assert!(!is_media_filename(".png"));
        assert!(!is_media_filename("png"));
        assert!(!is_media_filename("a.txt"));
        assert!(!is_media_filename("a.tgs"));
    }
}
