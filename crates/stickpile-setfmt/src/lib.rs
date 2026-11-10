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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
pub use stickpile_api::{FieldSource, Visibility};

pub const MANIFEST: &str = "set.toml";
/// File extensions considered media inside a set directory.
pub const MEDIA_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "mp4", "webm"];

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Bumped on every change; lets clients tell an upstream update from a local edit.
    #[serde(default = "one")]
    pub version: u64,
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
        let _ = name;
        todo!("setfmt agent")
    }
    pub fn parse(text: &str) -> Result<Manifest, SetFmtError> {
        let _ = text;
        todo!("setfmt agent")
    }
    pub fn to_toml(&self) -> String {
        todo!("setfmt agent")
    }
    /// `"16:9"` -> (16, 9)
    pub fn aspect(&self) -> Option<(u32, u32)> {
        todo!("setfmt agent")
    }
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

/// Read a set directory. Missing `set.toml` is allowed (name from the
/// directory name). Hidden files and non-media files are skipped. Symlinks are
/// not followed.
pub fn read_dir(dir: &Path) -> Result<SetBundle, SetFmtError> {
    let _ = dir;
    todo!("setfmt agent")
}

/// Write a set directory (creating it; existing media files not in the bundle
/// are removed so the directory equals the export).
pub fn write_dir(dir: &Path, bundle: &SetBundle) -> Result<(), SetFmtError> {
    let _ = (dir, bundle);
    todo!("setfmt agent")
}

/// Bundle as an uncompressed tar with a single top-level directory `<slug>/`.
pub fn write_tar(bundle: &SetBundle, top_dir: &str) -> Result<Vec<u8>, SetFmtError> {
    let _ = (bundle, top_dir);
    todo!("setfmt agent")
}

/// Read a tar produced by [`write_tar`] (or any tar of a set directory, with
/// or without a single top-level directory). Rejects absolute paths, `..`,
/// links and device entries; enforces `max_total_bytes`.
pub fn read_tar(tar: &[u8], max_total_bytes: u64) -> Result<SetBundle, SetFmtError> {
    let _ = (tar, max_total_bytes);
    todo!("setfmt agent")
}

/// Build a bundle from loose (relative path, bytes) pairs, e.g. a multipart
/// folder upload. Paths may include one leading directory component.
pub fn from_files(manifest: Option<&str>, files: Vec<(String, Vec<u8>)>) -> Result<SetBundle, SetFmtError> {
    let _ = (manifest, files);
    todo!("setfmt agent")
}

/// `001-cat.png`: zero-padded position (width = max(3, digits of total)),
/// slugified stem, extension.
pub fn export_filename(position: usize, total: usize, stem: &str, ext: &str) -> String {
    let _ = (position, total, stem, ext);
    todo!("setfmt agent")
}

/// Strip the numeric ordering prefix and extension: `001-cat.png` -> `cat`.
pub fn display_stem(filename: &str) -> String {
    let _ = filename;
    todo!("setfmt agent")
}

/// `[a-z0-9-]`, collapsed dashes, max 64 chars; non-empty (falls back to "set").
pub fn slugify(s: &str) -> String {
    let _ = s;
    todo!("setfmt agent")
}

/// A single path component that is safe to create inside a set directory.
pub fn safe_filename(name: &str) -> Result<&str, SetFmtError> {
    let _ = name;
    todo!("setfmt agent")
}
