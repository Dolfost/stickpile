//! Settings edited from the GUI, layered with `config.toml`.
//!
//! Where the GUI's edits go depends on whether `config.toml` can be written:
//!
//! * writable, or missing: the GUI writes `config.toml` itself (comments,
//!   ordering and other keys are kept) and nothing is locked;
//! * read-only (resolving into `/nix/store`, as home-manager links it, or no
//!   write permission): edits go to `settings.toml` in the same directory,
//!   which is layered under it. `config.toml` wins over `settings.toml`,
//!   which wins over the defaults, and the keys `config.toml` sets are
//!   locked.
//!
//! `config.toml` keeps its strict errors. Entries of `settings.toml` that are
//! unknown or invalid are skipped with a warning, so a stale GUI value never
//! stops the client from starting.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, TableLike};

use crate::config::{Background, ClientConfig, ConfigError, SquarePad, StartMode, LANGUAGES, START_TABS};

/// The GUI's settings file, next to `config.toml`.
pub const SETTINGS_FILE: &str = "settings.toml";

/// Groups of [`settings_schema`], in display order.
pub const GROUPS: &[&str] = &["Popup", "Copying", "Server"];

const NIX_STORE: &str = "/nix/store";

const TARGET_NAMES: &[&str] = &["telegram", "discord", "signal", "whatsapp", "slack", "generic"];

const SETTINGS_HEADER: &str = "# Settings saved by the stickpile popup. config.toml is read-only, so they\n\
                               # live here; any key config.toml sets takes precedence.\n\n";

/// The effective client configuration plus what the GUI needs to edit it.
#[derive(Debug, Clone, PartialEq)]
pub struct LayeredConfig {
    pub config: ClientConfig,
    /// Dotted keys set by a read-only `config.toml` (`ui.columns`, ...). The
    /// GUI shows them read-only.
    pub locked: BTreeSet<String>,
    /// File the GUI writes with [`set_value`] / [`unset_value`]:
    /// `config.toml`, or `settings.toml` when `config.toml` is read-only.
    pub write_path: PathBuf,
    /// Entries of `settings.toml` that were skipped, for the log or the GUI.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingKind {
    Bool,
    Int {
        min: i64,
        max: i64,
    },
    /// One of these strings.
    Choice(&'static [&'static str]),
    Text,
}

/// One GUI-editable key, enough to render the Settings page generically.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingSpec {
    /// Dotted key in `config.toml`.
    pub key: &'static str,
    /// One of [`GROUPS`].
    pub group: &'static str,
    pub label: &'static str,
    /// One line.
    pub help: &'static str,
    pub kind: SettingKind,
    /// `ClientConfig::default()`'s value.
    pub default: toml::Value,
}

impl SettingSpec {
    /// Type, range and choice check of a value for this key. The config's own
    /// validation (for example the `server_url` scheme) runs in [`set_value`].
    pub fn check(&self, value: &toml::Value) -> Result<(), String> {
        match (&self.kind, value) {
            (SettingKind::Bool, toml::Value::Boolean(_)) => Ok(()),
            (SettingKind::Int { min, max }, toml::Value::Integer(i)) => {
                if (*min..=*max).contains(i) {
                    Ok(())
                } else {
                    Err(format!("{i} is out of range {min}..={max}"))
                }
            }
            (SettingKind::Choice(names), toml::Value::String(s)) => {
                if names.contains(&s.as_str()) {
                    Ok(())
                } else {
                    Err(format!("{s:?} is not one of {}", names.join(", ")))
                }
            }
            (SettingKind::Text, toml::Value::String(_)) => Ok(()),
            (kind, v) => Err(format!("expected {}, got {}", kind_name(kind), v.type_str())),
        }
    }
}

fn kind_name(kind: &SettingKind) -> &'static str {
    match kind {
        SettingKind::Bool => "a boolean",
        SettingKind::Int { .. } => "an integer",
        SettingKind::Choice(_) | SettingKind::Text => "a string",
    }
}

/// Every key the Settings page edits, grouped (see [`GROUPS`]) and in display
/// order. Clipboard offer lists and focus rules stay config-file only.
pub fn settings_schema() -> Vec<SettingSpec> {
    use SettingKind::*;
    let spec = |key, group, label, help, kind, default: toml::Value| SettingSpec {
        key,
        group,
        label,
        help,
        kind,
        default,
    };
    let s = |v: &str| toml::Value::String(v.into());
    let b = toml::Value::Boolean;
    let i = toml::Value::Integer;
    vec![
        spec(
            "ui.language",
            "Popup",
            "Language",
            "auto follows your account's language (else the system's); a choice here sets the account's too.",
            Choice(LANGUAGES),
            s("auto"),
        ),
        spec(
            "ui.start_tab",
            "Popup",
            "Start tab",
            "Tab shown when the popup opens; last is the tab it was closed on.",
            Choice(START_TABS),
            s("last"),
        ),
        spec(
            "ui.start_mode",
            "Popup",
            "Start mode",
            "Mode the popup opens in: normal (vim keys), or insert to type a search right away.",
            Choice(StartMode::NAMES),
            s("normal"),
        ),
        spec(
            "ui.modal",
            "Popup",
            "Vim-style modes",
            "Insert and Normal modes with vim keys; off keeps the plain keys.",
            Bool,
            b(true),
        ),
        spec(
            "ui.columns",
            "Popup",
            "Grid columns",
            "Thumbnails per row at the default window size; resizing reflows the grid.",
            Int { min: 1, max: 20 },
            i(6),
        ),
        spec(
            "ui.thumb_px",
            "Popup",
            "Thumbnail size",
            "Grid cell size in logical pixels.",
            Int { min: 32, max: 512 },
            i(128),
        ),
        spec(
            "ui.mark_favorites",
            "Popup",
            "Mark favorites",
            "A heart on the items you liked (zf in Normal mode).",
            Bool,
            b(true),
        ),
        spec(
            "ui.show_emojis",
            "Popup",
            "Show emojis",
            "An item's emojis on its tile (ze in Normal mode).",
            Bool,
            b(true),
        ),
        spec(
            "ui.show_tags",
            "Popup",
            "Show tags",
            "An item's tags on its tile (zt in Normal mode).",
            Bool,
            b(true),
        ),
        spec(
            "ui.recent_count",
            "Popup",
            "Recent items",
            "Recently copied items shown before typing.",
            Int { min: 0, max: 500 },
            i(24),
        ),
        spec(
            "ui.max_results",
            "Popup",
            "Search results",
            "Most results shown for a search.",
            Int { min: 1, max: 1000 },
            i(60),
        ),
        spec(
            "ui.hide_on_copy",
            "Popup",
            "Hide after copying",
            "Close the popup once an item is copied or dropped.",
            Bool,
            b(true),
        ),
        spec(
            "ui.start_hidden",
            "Popup",
            "Start hidden",
            "A first bare stickpile-desktop starts resident without showing the popup.",
            Bool,
            b(false),
        ),
        spec(
            "default_target",
            "Copying",
            "Default target",
            "Chat app assumed when the focused window is not recognised.",
            Choice(TARGET_NAMES),
            s("generic"),
        ),
        spec(
            "focus.detect",
            "Copying",
            "Detect the focused window",
            "Pick the target app from the focused window (Hyprland, Sway, X11).",
            Bool,
            b(true),
        ),
        spec(
            "prefer_stickers",
            "Copying",
            "Prefer stickers",
            "Copy the Telegram sticker rendition by default.",
            Bool,
            b(false),
        ),
        spec(
            "copy.square",
            "Copying",
            "Square padding",
            "Pad non-square stills and stickers to a square: off, transparent or white.",
            Choice(SquarePad::NAMES),
            s("off"),
        ),
        spec(
            "copy.background",
            "Copying",
            "Background",
            "Fill transparent pixels of stills copied as images: keep or white.",
            Choice(Background::NAMES),
            s("keep"),
        ),
        spec(
            "server_url",
            "Server",
            "Server URL",
            "Base URL of the stickpile server (http or https).",
            Text,
            s("http://localhost:8080"),
        ),
        spec(
            "timeout_secs",
            "Server",
            "Timeout",
            "Seconds to wait for the server before using the offline cache.",
            Int { min: 1, max: 300 },
            i(5),
        ),
        spec(
            "cache_max_mb",
            "Server",
            "Cache size",
            "MiB of thumbnails and copied files kept for offline use.",
            Int { min: 16, max: 102_400 },
            i(512),
        ),
    ]
}

/// The schema entry for `key`.
pub fn setting_spec(key: &str) -> Option<SettingSpec> {
    settings_schema().into_iter().find(|s| s.key == key)
}

/// The value of a dotted key in `config`, as TOML (`None` for unset optional
/// keys such as `token_file`, and for unknown keys).
pub fn get_value(config: &ClientConfig, key: &str) -> Option<toml::Value> {
    let mut v = toml::Value::try_from(config).ok()?;
    for part in key.split('.') {
        v = v.as_table_mut()?.remove(part)?;
    }
    Some(v)
}

/// `settings.toml` next to `config_path`.
pub fn settings_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name(SETTINGS_FILE)
}

/// True if the GUI must not write `config_path`: it resolves into the Nix
/// store (home-manager), it is a dangling symlink, or this user lacks write
/// permission. A missing file is writable (the GUI creates it).
pub fn is_read_only(config_path: &Path) -> bool {
    read_only_with_store(config_path, Path::new(NIX_STORE))
}

fn read_only_with_store(path: &Path, store: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    }
    let Ok(real) = std::fs::canonicalize(path) else {
        return true;
    };
    real.starts_with(store) || !writable(&real)
}

#[cfg(unix)]
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // access(2) asks the kernel, so ACLs, read-only mounts and ownership count.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(not(unix))]
fn writable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| !m.permissions().readonly())
}

impl ClientConfig {
    /// The standard loader of the CLI and the desktop client: `path` layered
    /// over the GUI's `settings.toml` when `path` is read-only (see
    /// [`load_layered`]). Skipped `settings.toml` entries are logged.
    pub fn load(path: &Path) -> Result<ClientConfig, ConfigError> {
        let layered = load_layered(path)?;
        for w in &layered.warnings {
            tracing::warn!("{w}");
        }
        Ok(layered.config)
    }
}

/// Load `config_path` strictly and, when it is read-only, layer
/// `settings.toml` under it. See the module docs for the rules.
pub fn load_layered(config_path: &Path) -> Result<LayeredConfig, ConfigError> {
    load_layered_as(config_path, is_read_only(config_path))
}

fn load_layered_as(config_path: &Path, read_only: bool) -> Result<LayeredConfig, ConfigError> {
    let text = read_optional(config_path)?;
    let base = match &text {
        Some(t) => ClientConfig::from_toml(t).map_err(|e| with_path(e, config_path))?,
        None => ClientConfig::default(),
    };
    let settings = settings_path(config_path);
    let mut warnings = Vec::new();

    if !read_only {
        if settings.exists() {
            warnings.push(format!(
                "{} is ignored: {} is writable, so settings are saved there",
                settings.display(),
                config_path.display()
            ));
        }
        return Ok(LayeredConfig {
            config: base,
            locked: BTreeSet::new(),
            write_path: config_path.to_path_buf(),
            warnings,
        });
    }

    let mut merged: toml::Table = match &text {
        Some(t) => toml::from_str(t).map_err(|source| ConfigError::Parse {
            path: config_path.into(),
            source,
        })?,
        None => toml::Table::new(),
    };
    let mut locked = BTreeSet::new();
    let mut leaves = Vec::new();
    collect_leaves("", &merged, &mut leaves);
    locked.extend(leaves.into_iter().map(|(k, _)| k));

    let mut config = base;
    let entries = match read_optional(&settings) {
        Ok(None) => Vec::new(),
        Ok(Some(t)) => match toml::from_str::<toml::Table>(&t) {
            Ok(table) => {
                let mut out = Vec::new();
                collect_leaves("", &table, &mut out);
                out
            }
            Err(e) => {
                warnings.push(format!("{}: {e}; ignoring the file", settings.display()));
                Vec::new()
            }
        },
        Err(e) => {
            warnings.push(format!("{e}; ignoring it"));
            Vec::new()
        }
    };
    for (key, value) in entries {
        if locked.contains(&key) {
            continue;
        }
        let Some(spec) = setting_spec(&key) else {
            warnings.push(format!(
                "{}: {key} is not a GUI setting, ignoring it (set it in config.toml)",
                settings.display()
            ));
            continue;
        };
        if let Err(e) = spec.check(&value) {
            warnings.push(format!("{}: {key}: {e}; using the default", settings.display()));
            continue;
        }
        let mut candidate = merged.clone();
        insert_dotted(&mut candidate, &key, value);
        match config_from_table(candidate.clone()) {
            Ok(c) => {
                merged = candidate;
                config = c;
            }
            Err(e) => warnings.push(format!("{}: {key}: {e}; using the default", settings.display())),
        }
    }
    Ok(LayeredConfig {
        config,
        locked,
        write_path: settings,
        warnings,
    })
}

/// Set a GUI setting (`key` from [`settings_schema`], like `ui.columns`) in
/// `path`, normally [`LayeredConfig::write_path`]. The file is created if
/// missing; comments, ordering and other keys are kept. Invalid values are
/// rejected without touching the file.
pub fn set_value(path: &Path, key: &str, value: toml::Value) -> Result<(), ConfigError> {
    let spec = setting_spec(key).ok_or_else(|| ConfigError::Invalid(format!("{key} is not a GUI setting")))?;
    spec.check(&value)
        .map_err(|e| ConfigError::Invalid(format!("{key}: {e}")))?;
    let mut alone = toml::Table::new();
    insert_dotted(&mut alone, key, value.clone());
    config_from_table(alone)?;

    let target = write_target(path)?;
    let existing = read_optional(&target)?;
    let mut doc = parse_document(existing.as_deref().unwrap_or(""), &target)?;
    let new_value = edit_value(&value);
    let (parents, last) = split_key(key);
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    for part in parents {
        table = table
            .entry(part)
            .or_insert(toml_edit::table())
            .as_table_like_mut()
            .ok_or_else(|| ConfigError::Edit {
                path: target.clone(),
                message: format!("{part} is not a table"),
            })?;
    }
    match table.get_mut(last) {
        Some(Item::Value(old)) => {
            let decor = old.decor().clone();
            *old = new_value;
            *old.decor_mut() = decor;
        }
        _ => {
            table.insert(last, Item::Value(new_value));
        }
    }
    let mut out = doc.to_string();
    if existing.is_none() && target.file_name() == Some(SETTINGS_FILE.as_ref()) {
        out.insert_str(0, SETTINGS_HEADER);
    }
    write_atomic(&target, &out)
}

/// Remove `key` from `path`, so the lower layer (or the default) applies
/// again. A table left empty is removed too unless it carries a comment. A
/// missing file or key is not an error.
pub fn unset_value(path: &Path, key: &str) -> Result<(), ConfigError> {
    let target = write_target(path)?;
    let Some(text) = read_optional(&target)? else {
        return Ok(());
    };
    let mut doc = parse_document(&text, &target)?;
    if remove_dotted(doc.as_table_mut(), key) {
        write_atomic(&target, &doc.to_string())?;
    }
    Ok(())
}

fn remove_dotted(table: &mut dyn TableLike, key: &str) -> bool {
    let Some((head, rest)) = key.split_once('.') else {
        return table.remove(key).is_some();
    };
    let Some(child) = table.get_mut(head).and_then(Item::as_table_like_mut) else {
        return false;
    };
    if !remove_dotted(child, rest) {
        return false;
    }
    let now_empty = child.is_empty();
    let commented = match table.get(head) {
        Some(Item::Table(t)) => t
            .decor()
            .prefix()
            .and_then(|p| p.as_str())
            .is_some_and(|p| p.contains('#')),
        _ => false,
    };
    if now_empty && !commented {
        table.remove(head);
    }
    true
}

fn split_key(key: &str) -> (Vec<&str>, &str) {
    let mut parts: Vec<&str> = key.split('.').collect();
    let last = parts.pop().unwrap_or_default();
    (parts, last)
}

fn edit_value(v: &toml::Value) -> toml_edit::Value {
    match v {
        toml::Value::String(s) => s.as_str().into(),
        toml::Value::Integer(i) => (*i).into(),
        toml::Value::Float(f) => (*f).into(),
        toml::Value::Boolean(b) => (*b).into(),
        // Not produced by the schema; round-trip through the TOML text.
        other => other
            .to_string()
            .parse()
            .unwrap_or_else(|_| other.to_string().as_str().into()),
    }
}

/// The file to write for `path`: a symlink is followed so it stays a link
/// (dotfile managers), and read-only files (home-manager) are refused.
fn write_target(path: &Path) -> Result<PathBuf, ConfigError> {
    if is_read_only(path) {
        return Err(ConfigError::ReadOnly { path: path.into() });
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path).map_err(|source| ConfigError::Io {
            path: path.into(),
            source,
        }),
        _ => Ok(path.to_path_buf()),
    }
}

fn write_atomic(path: &Path, text: &str) -> Result<(), ConfigError> {
    let err = |source| ConfigError::Write {
        path: path.into(),
        source,
    };
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(dir).map_err(err)?;
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text).map_err(err)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        err(e)
    })
}

fn read_optional(path: &Path) -> Result<Option<String>, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Io {
            path: path.into(),
            source,
        }),
    }
}

fn parse_document(text: &str, path: &Path) -> Result<DocumentMut, ConfigError> {
    text.parse().map_err(|e: toml_edit::TomlError| ConfigError::Edit {
        path: path.into(),
        message: e.to_string(),
    })
}

fn with_path(e: ConfigError, path: &Path) -> ConfigError {
    match e {
        ConfigError::Parse { source, .. } => ConfigError::Parse {
            path: path.into(),
            source,
        },
        other => other,
    }
}

fn config_from_table(table: toml::Table) -> Result<ClientConfig, ConfigError> {
    let cfg: ClientConfig = toml::Value::Table(table)
        .try_into()
        .map_err(|e: toml::de::Error| ConfigError::Invalid(e.message().to_string()))?;
    cfg.validate()?;
    Ok(cfg)
}

/// Leaf values by dotted key; arrays (`focus.rules`) count as leaves.
fn collect_leaves(prefix: &str, table: &toml::Table, out: &mut Vec<(String, toml::Value)>) {
    for (k, v) in table {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            toml::Value::Table(t) => collect_leaves(&key, t, out),
            other => out.push((key, other.clone())),
        }
    }
}

fn insert_dotted(table: &mut toml::Table, key: &str, value: toml::Value) {
    match key.split_once('.') {
        None => {
            table.insert(key.to_string(), value);
        }
        Some((head, rest)) => {
            let child = table
                .entry(head.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if !child.is_table() {
                *child = toml::Value::Table(toml::Table::new());
            }
            if let toml::Value::Table(t) = child {
                insert_dotted(t, rest, value);
            }
        }
    }
}

#[cfg(test)]
mod tests;
