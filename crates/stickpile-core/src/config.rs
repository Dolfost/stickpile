//! Client configuration (`$XDG_CONFIG_HOME/stickpile/config.toml`), shared by
//! the CLI and the desktop client. Mirrored one-to-one by the home-manager
//! module (`programs.stickpile.settings`); unknown keys are rejected.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use stickpile_api::{MediaKind, TargetApp};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ClientConfig {
    /// Base URL of the stickpile server.
    pub server_url: String,
    /// API token file (written by `stickpile login`, mode 0600).
    /// Default: `$XDG_STATE_HOME/stickpile/token`.
    pub token_file: Option<PathBuf>,
    /// Default: `$XDG_CACHE_HOME/stickpile`.
    pub cache_dir: Option<PathBuf>,
    /// Thumbnails + cached media files; least recently used evicted beyond this.
    pub cache_max_mb: u64,
    /// Target app when the focused window is not recognised.
    pub default_target: TargetApp,
    /// For targets with sticker support (Telegram), copy the sticker rendition
    /// by default. Toggle per copy with Shift+Enter in the popup / `--sticker`.
    pub prefer_stickers: bool,
    /// Server request timeout before falling back to the offline cache.
    pub timeout_secs: u64,
    pub ui: UiConfig,
    pub focus: FocusConfig,
    pub clipboard: ClipboardConfig,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            server_url: "http://localhost:8080".into(),
            token_file: None,
            cache_dir: None,
            cache_max_mb: 512,
            default_target: TargetApp::Generic,
            prefer_stickers: false,
            timeout_secs: 5,
            ui: UiConfig::default(),
            focus: FocusConfig::default(),
            clipboard: ClipboardConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct UiConfig {
    pub columns: u32,
    pub thumb_px: u32,
    /// Hide the popup after copying.
    pub hide_on_copy: bool,
    /// Start resident without showing the popup (e.g. from a systemd user unit).
    pub start_hidden: bool,
    /// Items shown before typing: most recently used first.
    pub recent_count: u32,
    pub max_results: u32,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig { columns: 6, thumb_px: 128, hide_on_copy: true, start_hidden: true, recent_count: 24, max_results: 60 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct FocusConfig {
    /// Detect the focused window (Hyprland, Sway, X11) when the popup opens and
    /// map it to a target app with `rules`.
    pub detect: bool,
    /// First match wins; matched case-insensitively against window class/app id and title.
    pub rules: Vec<FocusRule>,
}

impl Default for FocusConfig {
    fn default() -> Self {
        let r = |contains: &[&str], target| FocusRule { contains: contains.iter().map(|s| s.to_string()).collect(), target };
        FocusConfig {
            detect: true,
            rules: vec![
                r(&["telegram", "ayugram", "materialgram", "64gram", "kotatogram"], TargetApp::Telegram),
                r(&["discord", "vesktop", "webcord", "armcord", "legcord"], TargetApp::Discord),
                r(&["signal"], TargetApp::Signal),
                r(&["whatsapp", "zapzap", "wasistlos"], TargetApp::Whatsapp),
                r(&["slack"], TargetApp::Slack),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FocusRule {
    /// Substrings; any match selects `target`.
    pub contains: Vec<String>,
    pub target: TargetApp,
}

/// MIME types offered on the clipboard, in offer order. Provisional defaults
/// until the docs/clipboard.md matrix is filled in; tune without rebuilding.
///
/// Supported entries: `image/png`, `image/gif`, `image/webp`, `image/jpeg`
/// (raw bytes of the chosen variant, converted if needed), `text/uri-list`
/// (file:// URI of the materialised file), `x-special/gnome-copied-files`
/// (`copy\nfile://...`), `text/plain` (the file path).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ClipboardOffers {
    pub still: Vec<String>,
    pub animation: Vec<String>,
    pub video: Vec<String>,
    pub sticker: Vec<String>,
}

impl ClipboardOffers {
    pub fn for_kind(&self, kind: MediaKind, sticker: bool) -> &[String] {
        if sticker {
            return &self.sticker;
        }
        match kind {
            MediaKind::Still => &self.still,
            MediaKind::Animation => &self.animation,
            MediaKind::Video => &self.video,
        }
    }
}

impl Default for ClipboardOffers {
    fn default() -> Self {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        ClipboardOffers {
            still: v(&["image/png", "text/uri-list", "x-special/gnome-copied-files"]),
            animation: v(&["image/gif", "text/uri-list", "x-special/gnome-copied-files"]),
            video: v(&["text/uri-list", "x-special/gnome-copied-files"]),
            sticker: v(&["text/uri-list", "x-special/gnome-copied-files"]),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields, default)]
pub struct ClipboardConfig {
    /// Used for any target without an override.
    pub default: ClipboardOffers,
    /// Per-target overrides, keyed by target name (`telegram`, `discord`, ...).
    pub targets: std::collections::BTreeMap<TargetApp, ClipboardOffers>,
}

impl ClipboardConfig {
    pub fn offers(&self, target: TargetApp) -> &ClipboardOffers {
        self.targets.get(&target).unwrap_or(&self.default)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("parsing {path}: {source}")]
    Parse { path: PathBuf, source: toml::de::Error },
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

impl ClientConfig {
    /// `$XDG_CONFIG_HOME/stickpile/config.toml`
    pub fn default_path() -> PathBuf {
        directories::ProjectDirs::from("", "", "stickpile")
            .map(|d| d.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("stickpile.toml"))
    }

    /// Missing file = defaults.
    pub fn load(path: &Path) -> Result<ClientConfig, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ClientConfig::default()),
            Err(source) => return Err(ConfigError::Io { path: path.into(), source }),
        };
        Self::from_toml(&text).map_err(|e| match e {
            ConfigError::Parse { source, .. } => ConfigError::Parse { path: path.into(), source },
            other => other,
        })
    }

    pub fn from_toml(text: &str) -> Result<ClientConfig, ConfigError> {
        let cfg: ClientConfig =
            toml::from_str(text).map_err(|source| ConfigError::Parse { path: PathBuf::from("<string>"), source })?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(self.server_url.starts_with("http://") || self.server_url.starts_with("https://")) {
            return Err(ConfigError::Invalid(format!("server_url must be http(s), got {:?}", self.server_url)));
        }
        if self.ui.columns == 0 || self.ui.thumb_px < 32 {
            return Err(ConfigError::Invalid("ui.columns must be > 0 and ui.thumb_px >= 32".into()));
        }
        let known = ["image/png", "image/gif", "image/webp", "image/jpeg", "text/uri-list", "x-special/gnome-copied-files", "text/plain"];
        let all = std::iter::once(&self.clipboard.default).chain(self.clipboard.targets.values());
        for offers in all {
            for m in offers.still.iter().chain(&offers.animation).chain(&offers.video).chain(&offers.sticker) {
                if !known.contains(&m.as_str()) {
                    return Err(ConfigError::Invalid(format!("clipboard: unsupported MIME type {m:?}")));
                }
            }
        }
        Ok(())
    }

    pub fn token_path(&self) -> PathBuf {
        self.token_file.clone().unwrap_or_else(|| {
            let base = directories::ProjectDirs::from("", "", "stickpile")
                .map(|d| d.state_dir().map(Path::to_path_buf).unwrap_or_else(|| d.data_local_dir().to_path_buf()))
                .unwrap_or_else(|| PathBuf::from("."));
            base.join("token")
        })
    }

    pub fn cache_path(&self) -> PathBuf {
        self.cache_dir.clone().unwrap_or_else(|| {
            directories::ProjectDirs::from("", "", "stickpile")
                .map(|d| d.cache_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from(".stickpile-cache"))
        })
    }

    /// Target for a focused window class/title, by `focus.rules`.
    pub fn target_for_window(&self, class: &str, title: &str) -> Option<TargetApp> {
        let class = class.to_lowercase();
        let title = title.to_lowercase();
        self.focus
            .rules
            .iter()
            .find(|r| r.contains.iter().any(|s| {
                let s = s.to_lowercase();
                class.contains(&s) || title.contains(&s)
            }))
            .map(|r| r.target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_and_round_trip() {
        let c = ClientConfig::default();
        c.validate().unwrap();
        let s = toml::to_string(&c).unwrap();
        assert_eq!(ClientConfig::from_toml(&s).unwrap(), c);
    }

    #[test]
    fn per_target_override() {
        let c = ClientConfig::from_toml(
            "[clipboard.targets.telegram]\nstill = [\"text/uri-list\"]\nanimation = []\nvideo = []\nsticker = []\n",
        )
        .unwrap();
        assert_eq!(c.clipboard.offers(TargetApp::Telegram).still, vec!["text/uri-list"]);
        assert_eq!(c.clipboard.offers(TargetApp::Discord).still[0], "image/png");
    }

    #[test]
    fn rejects_unknown_mime_and_keys() {
        assert!(ClientConfig::from_toml("[clipboard.default]\nstill = [\"image/bmp\"]").is_err());
        assert!(ClientConfig::from_toml("serverurl = \"x\"").is_err());
    }

    #[test]
    fn focus_rules_match_ayugram() {
        let c = ClientConfig::default();
        assert_eq!(c.target_for_window("com.ayugram.desktop", ""), Some(TargetApp::Telegram));
        assert_eq!(c.target_for_window("vesktop", "Discord"), Some(TargetApp::Discord));
        assert_eq!(c.target_for_window("firefox", "Element"), None);
    }
}
