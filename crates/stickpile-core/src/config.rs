//! Client configuration (`$XDG_CONFIG_HOME/stickpile/config.toml`), shared by
//! the CLI and the desktop client. Mirrored one-to-one by the home-manager
//! module (`programs.stickpile.settings`).
//!
//! A key this version does not know is skipped, not refused, so a setting a
//! newer popup saved never stops an older client: [`ClientConfig::parse`]
//! lists the skipped keys, the loaders warn about them, and `stickpile config
//! check` fails on them, so typos still show. [`ClientConfig::from_toml_strict`]
//! refuses them, as every client up to 0.7.0 does. Anything else (a TOML
//! error, a wrong type, an invalid value) is an error either way.
//!
//! The standard loader, `ClientConfig::load`, lives in [`crate::settings`]: it
//! layers the GUI's `settings.toml` under a read-only `config.toml`. This file
//! stays self-contained (std, serde, toml, toml_edit, thiserror, directories,
//! stickpile_api) because the Nix drift guard compiles it on its own.

use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use stickpile_api::{MediaKind, TargetApp};
use toml_edit::{ImDocument, Item, Key, Table, Value};

// The structs below keep `deny_unknown_fields`: serde then names each key it
// does not know, with its place in the text, and `ClientConfig::parse` skips
// exactly that key (see `skip_unknown_key`).

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
    pub copy: CopyConfig,
    pub focus: FocusConfig,
    pub paste: PasteConfig,
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
            copy: CopyConfig::default(),
            focus: FocusConfig::default(),
            paste: PasteConfig::default(),
            clipboard: ClipboardConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct UiConfig {
    /// Thumbnails per row at the popup's default size. The grid reflows
    /// when the window is resized, so a wider window shows more.
    pub columns: u32,
    pub thumb_px: u32,
    /// Hide the popup after copying.
    pub hide_on_copy: bool,
    /// A bare `stickpile-desktop` that starts the resident instance does not
    /// show the popup. Off by default so the first key press shows it; the
    /// systemd unit / autostart pass `--hidden` explicitly instead.
    pub start_hidden: bool,
    /// Items shown before typing: most recently used first.
    pub recent_count: u32,
    pub max_results: u32,
    /// Vim-style Insert/Normal modes in the popup; false keeps the plain keys.
    pub modal: bool,
    /// Tab shown when the popup opens: `last` (the tab it was closed on,
    /// kept across restarts) or one of [`TABS`].
    pub start_tab: String,
    /// Mode the popup opens in when `modal` is on.
    pub start_mode: StartMode,
    /// A heart on the tiles of items the viewer liked.
    pub mark_favorites: bool,
    /// An item's emojis on its tile.
    pub show_emojis: bool,
    /// An item's tags on its tile.
    pub show_tags: bool,
    /// An item's vibe label on its tile.
    pub show_vibe: bool,
    /// The words in an item's picture on its tile, one line. Off by
    /// default: the line covers the picture whose words it repeats.
    pub show_text: bool,
    /// The selection's stats (name, pack, format, copies, likes, quality)
    /// in a row above the footer.
    pub show_details: bool,
    /// Normal-mode digits start a count (`3j`, `5G`); a lone 1-7 then
    /// switches tabs after a short wait instead of at once.
    pub counts: bool,
    /// Safe mode: the popup hides items tagged `nsfw` (`S` in Normal mode
    /// switches it and saves this).
    pub safe_mode: bool,
    /// The popup's language: `auto` (the account's language when it has
    /// one, else the system's), or one of [`LANGUAGES`]. Choosing one in
    /// the popup also sets the account's, so the web UI follows.
    pub language: String,
    /// The popup's colours (`ui.theme`).
    pub theme: Theme,
}

/// `ui.theme`: the popup's colours.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// The desktop's light or dark style.
    #[default]
    System,
    Dark,
    /// Dark on pure black, for OLED screens.
    Oled,
    White,
}

impl Theme {
    pub const NAMES: &'static [&'static str] = &["system", "dark", "oled", "white"];
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Dark => "dark",
            Theme::Oled => "oled",
            Theme::White => "white",
        }
    }
}

/// Values of `ui.language`: `auto`, then the translated languages.
pub const LANGUAGES: &[&str] = &["auto", "en", "uk"];

/// The popup's tabs in display order.
pub const TABS: &[&str] = &["search", "recent", "most_used", "packs", "favorites", "me", "settings"];

/// Values of `ui.start_tab`: `last`, then [`TABS`].
pub const START_TABS: &[&str] = &[
    "last",
    "search",
    "recent",
    "most_used",
    "packs",
    "favorites",
    "me",
    "settings",
];

/// `ui.start_mode`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum StartMode {
    /// Vim keys right away; `i` or `/` to type.
    #[default]
    Normal,
    /// Typing right away on the tabs with a text field (Search, Packs);
    /// the other tabs open in Normal.
    Insert,
}

impl StartMode {
    pub const NAMES: &'static [&'static str] = &["normal", "insert"];
    pub fn as_str(self) -> &'static str {
        match self {
            StartMode::Normal => "normal",
            StartMode::Insert => "insert",
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            columns: 6,
            thumb_px: 128,
            hide_on_copy: true,
            start_hidden: false,
            recent_count: 24,
            max_results: 60,
            modal: true,
            start_tab: "last".into(),
            start_mode: StartMode::Normal,
            mark_favorites: true,
            show_emojis: true,
            show_tags: true,
            show_vibe: true,
            show_text: false,
            show_details: true,
            counts: false,
            safe_mode: false,
            language: "auto".into(),
            theme: Theme::System,
        }
    }
}

/// Copy-time adjustments of still images, applied when a copy is prepared so
/// the clipboard image data and the copied file carry the same pixels.
/// Animations and videos are never changed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields, default)]
pub struct CopyConfig {
    /// Pad non-square stills and sticker renditions to a centred square
    /// (never scaled or cropped).
    pub square: SquarePad,
    /// Fill the transparent pixels of stills copied as images (not stickers).
    pub background: Background,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SquarePad {
    #[default]
    Off,
    Transparent,
    White,
}

impl SquarePad {
    pub const NAMES: &'static [&'static str] = &["off", "transparent", "white"];
    pub fn as_str(self) -> &'static str {
        match self {
            SquarePad::Off => "off",
            SquarePad::Transparent => "transparent",
            SquarePad::White => "white",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Background {
    #[default]
    Keep,
    White,
}

impl Background {
    pub const NAMES: &'static [&'static str] = &["keep", "white"];
    pub fn as_str(self) -> &'static str {
        match self {
            Background::Keep => "keep",
            Background::White => "white",
        }
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
        let r = |contains: &[&str], target| FocusRule {
            contains: contains.iter().map(|s| s.to_string()).collect(),
            target,
        };
        FocusConfig {
            detect: true,
            rules: vec![
                r(
                    &["telegram", "ayugram", "materialgram", "64gram", "kotatogram"],
                    TargetApp::Telegram,
                ),
                r(
                    &["discord", "vesktop", "webcord", "armcord", "legcord"],
                    TargetApp::Discord,
                ),
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

/// Pressing the paste shortcut after a pick: once the popup has hidden and
/// the window it was opened over has the keyboard focus again, the desktop
/// popup presses the shortcut there (Hyprland, Sway, X11), and the macOS
/// app presses Cmd+V. Never in another window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct PasteConfig {
    /// Paste automatically after a pick.
    pub auto: bool,
    /// Longest wait (ms) for the window to get the focus back; after it,
    /// nothing is pressed.
    pub wait_ms: u32,
    /// Pause (ms) between the window getting the focus back and the key press.
    pub delay_ms: u32,
    /// Shortcut ([`Shortcut`] syntax) for windows no rule matches.
    pub shortcut: String,
    /// First match wins; matched case-insensitively against the window
    /// class / app id only (titles are free text: a chat called "Kitty"
    /// must not get the terminal shortcut). Not used on macOS (Cmd+V).
    pub rules: Vec<PasteRule>,
}

/// Longest `paste.wait_ms`.
pub const PASTE_WAIT_MAX_MS: u32 = 5000;
/// Longest `paste.delay_ms`.
pub const PASTE_DELAY_MAX_MS: u32 = 1000;

impl Default for PasteConfig {
    fn default() -> Self {
        PasteConfig {
            auto: true,
            wait_ms: 1000,
            delay_ms: 50,
            shortcut: "ctrl+v".into(),
            rules: vec![PasteRule {
                contains: [
                    "kitty",
                    "foot",
                    "alacritty",
                    "wezterm",
                    "ghostty",
                    "konsole",
                    "terminal",
                    "org.gnome.console",
                    "kgx",
                    "ptyxis",
                    "xterm",
                    "rxvt",
                    "terminator",
                    "tilix",
                    "terminology",
                    "blackbox",
                    "st-256color",
                    "contour",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
                shortcut: "ctrl+shift+v".into(),
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PasteRule {
    /// Substrings of the window class; any match selects `shortcut`.
    pub contains: Vec<String>,
    /// [`Shortcut`] syntax, or `none` to never paste into those windows.
    pub shortcut: String,
}

impl PasteConfig {
    /// The shortcut for a window class (`paste.rules`, else `paste.shortcut`);
    /// `None` when it is `none`. The config is validated, so the text parses.
    pub fn shortcut_for(&self, class: &str) -> Option<Shortcut> {
        let class = class.to_lowercase();
        let text = self
            .rules
            .iter()
            .find(|r| r.contains.iter().any(|s| class.contains(&s.to_lowercase())))
            .map_or(&self.shortcut, |r| &r.shortcut);
        Shortcut::parse(text).ok().flatten()
    }
}

/// A key the paste shortcut can press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutKey {
    /// `a`..=`z`, lower case.
    Letter(char),
    /// `0`..=`9`.
    Digit(char),
    Insert,
}

/// A key combination: modifiers and one key joined with `+`, case and
/// spaces ignored: `ctrl+v`, `Ctrl+Shift+V`, `shift+insert`. Modifiers are
/// `ctrl` (`control`), `shift`, `alt` and `super`; the key is a letter, a
/// digit or `insert`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub super_key: bool,
    pub key: ShortcutKey,
}

impl Shortcut {
    /// `Ok(None)` for `none` (no paste).
    pub fn parse(text: &str) -> Result<Option<Shortcut>, String> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("none") {
            return Ok(None);
        }
        let err = |why: &str| {
            Err(format!(
                "{text:?}: {why} (write it like ctrl+v, ctrl+shift+v or shift+insert)"
            ))
        };
        let parts: Vec<String> = text.split('+').map(|p| p.trim().to_ascii_lowercase()).collect();
        let Some((key, mods)) = parts.split_last() else {
            return err("empty");
        };
        let mut s = Shortcut {
            ctrl: false,
            shift: false,
            alt: false,
            super_key: false,
            key: ShortcutKey::Insert,
        };
        for m in mods {
            let flag = match m.as_str() {
                "ctrl" | "control" => &mut s.ctrl,
                "shift" => &mut s.shift,
                "alt" => &mut s.alt,
                "super" => &mut s.super_key,
                "" => return err("an empty part"),
                _ => return err("unknown modifier"),
            };
            if *flag {
                return err("a modifier twice");
            }
            *flag = true;
        }
        let mut chars = key.chars();
        s.key = match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_lowercase() => ShortcutKey::Letter(c),
            (Some(c), None) if c.is_ascii_digit() => ShortcutKey::Digit(c),
            _ if key == "insert" => ShortcutKey::Insert,
            (None, _) => return err("no key"),
            _ => return err("the key must be a letter, a digit or insert"),
        };
        if mods.is_empty() && s.key != ShortcutKey::Insert {
            return err("a letter or a digit alone would type it");
        }
        Ok(Some(s))
    }
}

impl std::fmt::Display for Shortcut {
    /// `Ctrl+Shift+V`
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl+"),
            (self.shift, "Shift+"),
            (self.alt, "Alt+"),
            (self.super_key, "Super+"),
        ] {
            if on {
                f.write_str(name)?;
            }
        }
        match self.key {
            ShortcutKey::Letter(c) | ShortcutKey::Digit(c) => write!(f, "{}", c.to_ascii_uppercase()),
            ShortcutKey::Insert => f.write_str("Insert"),
        }
    }
}

/// Paste hint offered next to the file reference: the copied file is a
/// sticker, so an app that has stickers should send it as one, not as a
/// photo or a file (docs/clipboard.md, section 5). Apps that do not know
/// the hint ignore it.
pub const STICKER_HINT_MIME: &str = "application/x-sticker";

/// Paste hint: the paste should complete without the confirmation dialog
/// the receiving app would otherwise show, such as a send box
/// (docs/clipboard.md, section 5). Apps that do not know the hint ignore it.
pub const NO_DIALOG_HINT_MIME: &str = "application/x-no-dialog";

/// MIME types offered on the clipboard, in offer order. The still, animation
/// and video defaults are confirmed for Telegram on Wayland; the rest of the
/// docs/clipboard.md matrix is still provisional. Tune without rebuilding.
///
/// Supported entries: `image/png`, `image/gif`, `image/webp`, `image/jpeg`
/// (raw bytes of the chosen variant, converted if needed), `text/uri-list`
/// (file:// URI of the materialised file), `x-special/gnome-copied-files`
/// (`copy\nfile://...`), `text/plain` (the file path), and the paste hints
/// `application/x-sticker` and `application/x-no-dialog` (`1`; see
/// [`STICKER_HINT_MIME`] and [`NO_DIALOG_HINT_MIME`]).
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
            sticker: v(&[
                "text/uri-list",
                "x-special/gnome-copied-files",
                STICKER_HINT_MIME,
                NO_DIALOG_HINT_MIME,
            ]),
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
    #[error("{path}: {message}")]
    Edit { path: PathBuf, message: String },
    #[error("{path} is read-only")]
    ReadOnly { path: PathBuf },
    #[error("writing {path}: {source}")]
    Write { path: PathBuf, source: std::io::Error },
}

/// A config file read the way the clients read it ([`ClientConfig::parse`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedConfig {
    pub config: ClientConfig,
    /// Keys the file sets that this version does not know, skipped, in the
    /// order they were found: dotted (`ui.show_text`), with the index of a
    /// list entry (`paste.rules[0].wait`).
    pub unknown_keys: Vec<String>,
    /// The text with those keys blanked out: it parses to `config`, and
    /// what is left keeps its line and column (in an inline table that lost
    /// a key, the entries after it move left).
    pub clean_text: String,
}

impl ClientConfig {
    /// `$XDG_CONFIG_HOME/stickpile/config.toml`
    pub fn default_path() -> PathBuf {
        directories::ProjectDirs::from("", "", "stickpile")
            .map(|d| d.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("stickpile.toml"))
    }

    /// Exactly this file, without the `settings.toml` layer that
    /// `ClientConfig::load` adds. Missing file = defaults. Unknown keys are
    /// skipped and listed, as by [`ClientConfig::parse`].
    pub fn load_file(path: &Path) -> Result<ParsedConfig, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ParsedConfig::default()),
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.into(),
                    source,
                })
            }
        };
        Self::parse(&text).map_err(|e| match e {
            ConfigError::Parse { source, .. } => ConfigError::Parse {
                path: path.into(),
                source,
            },
            other => other,
        })
    }

    /// The config in `text`; keys this version does not know are skipped
    /// ([`ClientConfig::parse`] lists them).
    pub fn from_toml(text: &str) -> Result<ClientConfig, ConfigError> {
        Ok(Self::parse(text)?.config)
    }

    /// The config in `text`, refusing a key this version does not know with
    /// serde's "unknown field" error, as clients up to 0.7.0 load every file.
    pub fn from_toml_strict(text: &str) -> Result<ClientConfig, ConfigError> {
        let cfg: ClientConfig = toml::from_str(text).map_err(parse_error)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// The config in `text` as the clients load it: every key this version
    /// does not know, at any depth (a typo, or a newer version's setting), is
    /// skipped and listed in [`ParsedConfig::unknown_keys`]. Anything else (a
    /// TOML error, a wrong type, an invalid value) is the error
    /// [`ClientConfig::from_toml_strict`] gives without the skipped keys, at
    /// the file's line and column (see [`ParsedConfig::clean_text`]).
    pub fn parse(text: &str) -> Result<ParsedConfig, ConfigError> {
        let mut clean = text.to_string();
        let mut unknown_keys = Vec::new();
        loop {
            match toml::from_str::<ClientConfig>(&clean) {
                Ok(config) => {
                    config.validate()?;
                    return Ok(ParsedConfig {
                        config,
                        unknown_keys,
                        clean_text: clean,
                    });
                }
                // Every skip blanks out text, so this ends.
                Err(e) => match skip_unknown_key(&clean, &e) {
                    Some((key, rest)) => {
                        unknown_keys.push(key);
                        clean = rest;
                    }
                    None => return Err(parse_error(e)),
                },
            }
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(self.server_url.starts_with("http://") || self.server_url.starts_with("https://")) {
            return Err(ConfigError::Invalid(format!(
                "server_url must be http(s), got {:?}",
                self.server_url
            )));
        }
        if self.ui.columns == 0 || self.ui.thumb_px < 32 {
            return Err(ConfigError::Invalid(
                "ui.columns must be > 0 and ui.thumb_px >= 32".into(),
            ));
        }
        if !LANGUAGES.contains(&self.ui.language.as_str()) {
            return Err(ConfigError::Invalid(format!(
                "ui.language must be one of {}, got {:?}",
                LANGUAGES.join(", "),
                self.ui.language
            )));
        }
        if !START_TABS.contains(&self.ui.start_tab.as_str()) {
            return Err(ConfigError::Invalid(format!(
                "ui.start_tab must be one of {}, got {:?}",
                START_TABS.join(", "),
                self.ui.start_tab
            )));
        }
        if self.paste.wait_ms > PASTE_WAIT_MAX_MS || self.paste.delay_ms > PASTE_DELAY_MAX_MS {
            return Err(ConfigError::Invalid(format!(
                "paste.wait_ms must be at most {PASTE_WAIT_MAX_MS} and paste.delay_ms at most {PASTE_DELAY_MAX_MS}"
            )));
        }
        let shortcuts = std::iter::once(("paste.shortcut", &self.paste.shortcut))
            .chain(self.paste.rules.iter().map(|r| ("paste.rules", &r.shortcut)));
        for (key, text) in shortcuts {
            Shortcut::parse(text).map_err(|e| ConfigError::Invalid(format!("{key}: {e}")))?;
        }
        let known = [
            "image/png",
            "image/gif",
            "image/webp",
            "image/jpeg",
            "text/uri-list",
            "x-special/gnome-copied-files",
            "text/plain",
            STICKER_HINT_MIME,
            NO_DIALOG_HINT_MIME,
        ];
        let all = std::iter::once(&self.clipboard.default).chain(self.clipboard.targets.values());
        for offers in all {
            for m in offers
                .still
                .iter()
                .chain(&offers.animation)
                .chain(&offers.video)
                .chain(&offers.sticker)
            {
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
                .map(|d| {
                    d.state_dir()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| d.data_local_dir().to_path_buf())
                })
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
            .find(|r| {
                r.contains.iter().any(|s| {
                    let s = s.to_lowercase();
                    class.contains(&s) || title.contains(&s)
                })
            })
            .map(|r| r.target)
    }
}

fn parse_error(source: toml::de::Error) -> ConfigError {
    ConfigError::Parse {
        path: PathBuf::from("<string>"),
        source,
    }
}

/// One step of a path into a TOML document.
#[derive(Debug, Clone)]
enum Step {
    Key(String),
    /// An entry of an array or an array of tables.
    Index(usize),
}

impl Step {
    fn get_mut<'a>(&self, item: &'a mut Item) -> Option<&'a mut Item> {
        match self {
            Step::Key(k) => item.get_mut(k.as_str()),
            Step::Index(i) => item.get_mut(*i),
        }
    }
}

/// `ui.show_text`, `paste.rules[0].wait`, `"odd key"`.
fn dotted(path: &[Step]) -> String {
    let mut out = String::new();
    for step in path {
        match step {
            Step::Key(k) => {
                if !out.is_empty() {
                    out.push('.');
                }
                let bare = !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                if bare {
                    out.push_str(k);
                } else {
                    out.push_str(&format!("{k:?}"));
                }
            }
            Step::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

/// One step of [`ClientConfig::parse`]: when `error` is serde's "unknown
/// field" for a key of `text`, that key's dotted name and `text` without it.
///
/// The key is the one at the error's span, so a name known in another table,
/// nested and inline tables, and the entries of lists are told apart. Its
/// entry is blanked out (spaces; line breaks stay): a key/value line, or a
/// table with everything under it. Inside an inline table, which cannot
/// simply lose its text, that table is written again without the key and
/// padded to its old length. Either way the rest of the text keeps its
/// place, so a later error still names the file's line and column.
fn skip_unknown_key(text: &str, error: &toml::de::Error) -> Option<(String, String)> {
    let span = error.span()?;
    let doc = ImDocument::parse(text).ok()?;
    let mut path = Vec::new();
    let found = find_key(doc.as_item(), &span, &mut path, None)?;
    let Some(Step::Key(name)) = path.last() else {
        return None;
    };
    if !error.message().starts_with(&format!("unknown field `{name}`")) {
        return None;
    }
    let rest = match &found.inline {
        None => {
            let mut ranges = Vec::new();
            entry_ranges(text, found.key, found.item, &mut ranges);
            blank(text, &ranges)?
        }
        Some((depth, table_span)) => {
            let rewritten = inline_without(text, &path, *depth)?;
            let (before, old, after) = (
                text.get(..table_span.start)?,
                text.get(table_span.clone())?,
                text.get(table_span.end..)?,
            );
            // Line breaks inside the key's value go after the table, so later
            // lines keep their numbers; spaces if the text does not parse so
            // (TOML 1.0 has no line breaks in an inline table outside values).
            let lost = old
                .matches('\n')
                .count()
                .saturating_sub(rewritten.matches('\n').count());
            let fill = old.len().saturating_sub(rewritten.len() + lost);
            let lines_kept = format!("{before}{rewritten}{}{}{after}", "\n".repeat(lost), " ".repeat(fill));
            if lost == 0 || ImDocument::parse(lines_kept.as_str()).is_ok() {
                lines_kept
            } else {
                format!("{before}{rewritten}{}{after}", " ".repeat(fill + lost))
            }
        }
    };
    // Something went, and what is left is still TOML.
    let solid = |t: &str| t.bytes().filter(|b| !b.is_ascii_whitespace()).count();
    if solid(&rest) >= solid(text) || ImDocument::parse(rest.as_str()).is_err() {
        return None;
    }
    Some((dotted(&path), rest))
}

struct Found<'a> {
    key: &'a Key,
    item: &'a Item,
    /// The innermost inline table the key is in (not counting dotted ones):
    /// the length of its path, and its span.
    inline: Option<(usize, Range<usize>)>,
}

/// The key whose text is at `span`; `path` ends with it.
fn find_key<'a>(
    item: &'a Item,
    span: &Range<usize>,
    path: &mut Vec<Step>,
    inline: Option<(usize, Range<usize>)>,
) -> Option<Found<'a>> {
    if let Some(table) = item.as_table_like() {
        let inline = match item {
            Item::Value(Value::InlineTable(t)) if !t.is_dotted() => Some((path.len(), t.span()?)),
            _ => inline,
        };
        for (name, child) in table.iter() {
            let key = table.key(name)?;
            path.push(Step::Key(name.to_string()));
            if key.span().as_ref() == Some(span) {
                return Some(Found {
                    key,
                    item: child,
                    inline,
                });
            }
            if let Some(found) = find_key(child, span, path, inline.clone()) {
                return Some(found);
            }
            path.pop();
        }
        return None;
    }
    let len = match item {
        Item::ArrayOfTables(a) => a.len(),
        Item::Value(Value::Array(a)) => a.len(),
        _ => 0,
    };
    for i in 0..len {
        path.push(Step::Index(i));
        if let Some(found) = find_key(item.get(i)?, span, path, inline.clone()) {
            return Some(found);
        }
        path.pop();
    }
    None
}

/// The text of an entry outside inline tables: a key/value pair from the
/// start of its line (it starts one, and a dotted key is one line) to the
/// end of its value; a `[table]` or `[[table]]` from its header to its last
/// value; and the same for what is under it. Dotted and implicit tables have
/// no text of their own.
fn entry_ranges(text: &str, key: &Key, item: &Item, out: &mut Vec<Range<usize>>) {
    let table_ranges = |t: &Table, out: &mut Vec<Range<usize>>| {
        out.extend(t.span());
        for (name, child) in t.iter() {
            if let Some(key) = t.key(name) {
                entry_ranges(text, key, child, out);
            }
        }
    };
    match item {
        Item::Value(v) => {
            if let (Some(k), Some(v)) = (key.span(), v.span()) {
                let before = text.get(..k.start).unwrap_or_default();
                let line = before.rfind('\n').map_or(0, |i| i + 1);
                out.push(line..v.end);
            }
        }
        Item::Table(t) => table_ranges(t, out),
        Item::ArrayOfTables(a) => {
            for t in a.iter() {
                table_ranges(t, out);
            }
        }
        Item::None => {}
    }
}

/// `text` with `ranges` blanked out: spaces, line breaks kept.
fn blank(text: &str, ranges: &[Range<usize>]) -> Option<String> {
    let mut bytes = text.as_bytes().to_vec();
    for r in ranges {
        for b in bytes.get_mut(r.clone())? {
            if !matches!(*b, b'\n' | b'\r') {
                *b = b' ';
            }
        }
    }
    String::from_utf8(bytes).ok()
}

/// The inline table at `path[..depth]` written again without the key at the
/// end of `path` (which may sit in dotted tables inside it).
fn inline_without(text: &str, path: &[Step], depth: usize) -> Option<String> {
    let (last, inside) = path.split_last()?;
    let Step::Key(name) = last else { return None };
    let mut doc: toml_edit::DocumentMut = text.parse().ok()?;
    let mut item = doc.as_item_mut();
    for step in inside.get(..depth)? {
        item = step.get_mut(item)?;
    }
    let mut table = Item::Value(item.as_value()?.clone());
    let mut parent = &mut table;
    for step in inside.get(depth..)? {
        parent = step.get_mut(parent)?;
    }
    parent.as_table_like_mut()?.remove(name)?;
    let mut value = table.into_value().ok()?;
    value.decor_mut().clear();
    Some(value.to_string())
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
    fn rejects_unknown_mime_and_strict_rejects_unknown_keys() {
        assert!(ClientConfig::from_toml("[clipboard.default]\nstill = [\"image/bmp\"]").is_err());
        let e = ClientConfig::from_toml_strict("serverurl = \"x\"")
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown field `serverurl`"), "{e}");
        assert_eq!(
            ClientConfig::from_toml("serverurl = \"x\"").unwrap(),
            ClientConfig::default()
        );
    }

    /// The keys `parse` skipped, and the config it made of the rest.
    fn skipped(text: &str) -> (Vec<String>, ClientConfig) {
        let p = ClientConfig::parse(text).unwrap_or_else(|e| panic!("{text}\n{e}"));
        assert_eq!(p.clean_text.len(), text.len(), "positions kept:\n{}", p.clean_text);
        assert_eq!(p.clean_text.lines().count(), text.lines().count());
        assert_eq!(ClientConfig::from_toml_strict(&p.clean_text).unwrap(), p.config);
        assert_eq!(
            ClientConfig::from_toml_strict(text).is_err(),
            !p.unknown_keys.is_empty(),
            "strict refuses exactly what parse skips"
        );
        (p.unknown_keys, p.config)
    }

    #[test]
    fn unknown_keys_are_skipped_at_any_depth() {
        // A clean file: nothing skipped, optional keys (unset by default) included.
        let (keys, c) = skipped("token_file = \"/t/token\"\ncache_dir = \"/t/cache\"\n[ui]\ncolumns = 3\n");
        assert!(keys.is_empty(), "{keys:?}");
        assert_eq!(c.token_file.as_deref(), Some(Path::new("/t/token")));
        assert_eq!(c.ui.columns, 3);

        // Two of them, at the top and in [ui], around known keys.
        let (keys, c) = skipped(
            "# my config\nnonsense = 1\nserver_url = \"https://s.example\"\n\n[ui]\ncolumns = 3\n\
             show_nonsense = true   # from a newer popup\nmodal = false\n",
        );
        assert_eq!(keys, ["nonsense", "ui.show_nonsense"]);
        assert_eq!(c.server_url, "https://s.example");
        assert_eq!((c.ui.columns, c.ui.modal), (3, false));

        // A typo is skipped like a newer key: the default stays.
        let (keys, c) = skipped("[ui]\ncolums = 3\n");
        assert_eq!(keys, ["ui.colums"]);
        assert_eq!(c.ui.columns, 6);

        // Nested tables.
        let (keys, c) = skipped(
            "[clipboard.targets.telegram]\nstill = [\"text/uri-list\"]\nhint = \"x\"\n\
             [clipboard.default]\nsticker = []\nextra = [\"a\",\n  \"b\"]\n",
        );
        assert_eq!(keys, ["clipboard.targets.telegram.hint", "clipboard.default.extra"]);
        assert_eq!(c.clipboard.offers(TargetApp::Telegram).still, ["text/uri-list"]);
        assert!(c.clipboard.default.sticker.is_empty());

        // The name of a known key in another table.
        let (keys, c) = skipped("[copy]\ncolumns = 4\nsquare = \"white\"\n[ui]\ncolumns = 3\n");
        assert_eq!(keys, ["copy.columns"]);
        assert_eq!((c.ui.columns, c.copy.square), (3, SquarePad::White));
        let (keys, c) = skipped("[ui]\nsquare = \"white\"\n");
        assert_eq!(keys, ["ui.square"]);
        assert_eq!(c.copy.square, SquarePad::Off);

        // Dotted keys, at the top and in a table.
        let (keys, c) = skipped("ui.show_nonsense = true\nui.columns = 2\n[paste]\nx.y = 1\nx.z = 2\nauto = false\n");
        assert_eq!(keys, ["ui.show_nonsense", "paste.x"]);
        assert_eq!(c.ui.columns, 2);
        assert!(!c.paste.auto);
    }

    #[test]
    fn unknown_tables_are_skipped_whole() {
        let (keys, c) = skipped(
            "[sync]\nevery = 5\nwhere = { a = 1 }\n\n[sync.more]\nx = [1, 2]\n\
             [ui]\ncolumns = 2\n[ui.panel]\nwidth = 3\n[[hooks]]\nrun = \"a\"\n[[hooks]]\nrun = \"b\"\n\
             [hooks.env]\nX = 1\n[later.deep]\nx = 1\n[copy]\nsquare = \"white\"\n",
        );
        assert_eq!(keys, ["sync", "ui.panel", "hooks", "later"]);
        assert_eq!(c.ui.columns, 2);
        assert_eq!(c.copy.square, SquarePad::White);
        let (keys, _) = skipped("[ui]\nsection.a = 1\nsection.b = { c = 2 }\ncolumns = 4\n");
        assert_eq!(keys, ["ui.section"]);
    }

    #[test]
    fn unknown_keys_in_lists_and_inline_tables() {
        // Arrays of tables: the entry's index.
        let (keys, c) = skipped(
            "[[paste.rules]]\ncontains = [\"a\"]\nshortcut = \"none\"\n\
             [[paste.rules]]\ncontains = [\"b\"]\nwait = 1\nshortcut = \"ctrl+v\"\n",
        );
        assert_eq!(keys, ["paste.rules[1].wait"]);
        assert_eq!(c.paste.rules.len(), 2);
        assert_eq!(c.paste.rules[1].shortcut, "ctrl+v");

        // Inline tables: first, middle and last entries, in lists, dotted.
        let (keys, c) = skipped("ui = { odd = 1, columns = 3, show_nonsense = true, modal = false, end = [1] }\n");
        assert_eq!(keys, ["ui.odd", "ui.show_nonsense", "ui.end"]);
        assert_eq!((c.ui.columns, c.ui.modal), (3, false));
        let (keys, c) = skipped(
            "[focus]\nrules = [\n  { contains = [\"x\"], target = \"telegram\", why = \"chat\" },\n  \
             { contains = [\"y\"], target = \"discord\" },\n]\n",
        );
        assert_eq!(keys, ["focus.rules[0].why"]);
        assert_eq!(c.focus.rules.len(), 2);
        assert_eq!(c.focus.rules[0].target, TargetApp::Telegram);
        let (keys, c) = skipped("clipboard = { default.still = [\"text/plain\"], default.more = [] }\n");
        assert_eq!(keys, ["clipboard.default.more"]);
        assert_eq!(c.clipboard.default.still, ["text/plain"]);
        let (keys, _) = skipped("ui = { odd = 1 }\n");
        assert_eq!(keys, ["ui.odd"]);

        // A quoted key is shown quoted.
        let (keys, _) = skipped("\"odd key\" = 1\n[ui]\n\"a.b\" = 2\n");
        assert_eq!(keys, ["\"odd key\"", "ui.\"a.b\""]);
    }

    #[test]
    fn map_keys_are_values_not_settings() {
        // `clipboard.targets` is keyed by target app: those keys are known,
        // and one that is not a target app is a bad value, not a skipped key.
        let (keys, c) = skipped("[clipboard.targets.discord]\nstill = []\n[clipboard.targets.slack]\nvideo = []\n");
        assert!(keys.is_empty(), "{keys:?}");
        assert!(c.clipboard.offers(TargetApp::Discord).still.is_empty());
        let e = ClientConfig::parse("[clipboard.targets.telgram]\nstill = []\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown variant `telgram`"), "{e}");
    }

    #[test]
    fn other_errors_stay_errors_at_their_place() {
        for (text, needle) in [
            ("[ui]\ncolumns = \"x\"\n", "invalid type"),
            ("server_url = \"ftp://s.example\"\n", "server_url must be http(s)"),
            ("[ui]\nstart_tab = \"home\"\n", "ui.start_tab must be one of"),
            ("[ui]\ncolumns = \n", "TOML parse error"),
            (
                "[paste]\nwiat_ms = 1\nwait_ms = 9000\n",
                "paste.wait_ms must be at most",
            ),
        ] {
            let e = ClientConfig::parse(text).unwrap_err().to_string();
            assert!(e.contains(needle), "{text}: {e}");
            let strict = ClientConfig::from_toml_strict(text).unwrap_err().to_string();
            if !text.contains("wiat") {
                assert_eq!(e, strict, "the same error as before");
            }
        }
        // Skipped keys before it do not move the error's line and column.
        let text = "nonsense = 1\n[sync]\na = 1\n[ui]\nodd = { x = [\n 1,\n 2] }\ncolumns = \"x\"\n";
        let e = ClientConfig::parse(text).unwrap_err().to_string();
        assert!(e.contains("line 8, column 11"), "{e}");
        // Also when the skipped key's value had line breaks in an inline table.
        let text = "focus.rules = [{ contains = [], target = \"slack\", why = [\n1] }]\nui.columns = \"x\"\n";
        let e = ClientConfig::parse(text).unwrap_err().to_string();
        assert!(e.contains("line 3, column 14"), "{e}");
        // In an inline table inside another one the line breaks may not stay
        // (TOML 1.0); the error still shows the right text.
        let text = "clipboard = { targets = { slack = { still = [], why = [\n1] } } }\n[copy]\nsquare = 1\n";
        let e = ClientConfig::parse(text).unwrap_err().to_string();
        assert!(e.contains("square = 1") && e.contains("wanted string or table"), "{e}");
    }

    #[test]
    fn load_file_lists_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\r\nshow_nonsense = true # newer\r\ncolumns = 5\r\n").unwrap();
        let p = ClientConfig::load_file(&path).unwrap();
        assert_eq!(p.unknown_keys, ["ui.show_nonsense"]);
        assert_eq!(p.config.ui.columns, 5);
        std::fs::write(&path, "[ui]\ncolumns = 0\nodd = 1\n").unwrap();
        let e = ClientConfig::load_file(&path).unwrap_err().to_string();
        assert!(e.contains("ui.columns must be > 0"), "{e}");
        std::fs::write(&path, "[ui]\nodd = 1\ncolumns = []\n").unwrap();
        let e = ClientConfig::load_file(&path).unwrap_err().to_string();
        assert!(e.starts_with(&format!("parsing {}", path.display())), "{e}");
        let missing = dir.path().join("missing.toml");
        assert_eq!(ClientConfig::load_file(&missing).unwrap(), ParsedConfig::default());
    }

    #[test]
    fn ui_and_copy_keys() {
        let c = ClientConfig::from_toml(
            "[ui]\nmodal = false\nstart_tab = \"most_used\"\nstart_mode = \"insert\"\nmark_favorites = false\n\
             show_emojis = false\nshow_tags = false\nshow_vibe = false\nshow_text = true\nshow_details = false\ncounts = true\nsafe_mode = true\n[copy]\nsquare = \"white\"\nbackground = \"white\"\n",
        )
        .unwrap();
        assert!(!c.ui.modal);
        assert_eq!(c.ui.start_tab, "most_used");
        assert_eq!(c.ui.start_mode, StartMode::Insert);
        assert!(!c.ui.mark_favorites && !c.ui.show_emojis && !c.ui.show_tags && !c.ui.show_vibe);
        assert!(c.ui.show_text);
        assert!(c.ui.counts && !c.ui.show_details);
        assert!(c.ui.safe_mode);
        assert_eq!(c.copy.square, SquarePad::White);
        assert_eq!(c.copy.background, Background::White);
        let d = ClientConfig::default();
        assert!(d.ui.modal);
        assert_eq!(d.ui.start_tab, "last");
        assert_eq!(d.ui.start_mode, StartMode::Normal);
        assert!(d.ui.mark_favorites && d.ui.show_emojis && d.ui.show_tags && d.ui.show_vibe);
        assert!(!d.ui.show_text, "the text line is off by default");
        assert!(!d.ui.counts && d.ui.show_details);
        assert!(!d.ui.safe_mode);
        assert_eq!(d.ui.language, "auto");
        for l in LANGUAGES {
            let c = ClientConfig::from_toml(&format!("[ui]\nlanguage = {l:?}")).unwrap();
            assert_eq!(c.ui.language, *l);
        }
        assert!(ClientConfig::from_toml("[ui]\nlanguage = \"de\"").is_err());
        assert_eq!((d.copy.square, d.copy.background), (SquarePad::Off, Background::Keep));
        for name in SquarePad::NAMES {
            let c = ClientConfig::from_toml(&format!("[copy]\nsquare = {name:?}")).unwrap();
            assert_eq!(c.copy.square.as_str(), *name);
        }
        for name in Background::NAMES {
            let c = ClientConfig::from_toml(&format!("[copy]\nbackground = {name:?}")).unwrap();
            assert_eq!(c.copy.background.as_str(), *name);
        }
        for tab in START_TABS {
            let c = ClientConfig::from_toml(&format!("[ui]\nstart_tab = {tab:?}")).unwrap();
            assert_eq!(c.ui.start_tab, *tab);
        }
        assert_eq!(&START_TABS[1..], TABS);
        for name in StartMode::NAMES {
            let c = ClientConfig::from_toml(&format!("[ui]\nstart_mode = {name:?}")).unwrap();
            assert_eq!(c.ui.start_mode.as_str(), *name);
        }
    }

    #[test]
    fn rejects_bad_ui_and_copy_values() {
        let e = ClientConfig::from_toml("[ui]\nstart_tab = \"home\"")
            .unwrap_err()
            .to_string();
        assert!(e.contains("ui.start_tab must be one of last, search, recent"), "{e}");
        assert!(ClientConfig::from_toml("[ui]\nstart_mode = \"visual\"").is_err());
        let e = ClientConfig::from_toml("[copy]\nsquare = \"black\"")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("unknown variant `black`") && e.contains("transparent"),
            "{e}"
        );
        let e = ClientConfig::from_toml("[copy]\nbackground = true")
            .unwrap_err()
            .to_string();
        assert!(e.contains("background"), "{e}");
        assert!(ClientConfig::from_toml_strict("[copy]\nsqaure = \"off\"").is_err());
    }

    #[test]
    fn paste_defaults_and_keys() {
        let d = ClientConfig::default().paste;
        assert!(d.auto, "on by default");
        assert_eq!((d.wait_ms, d.delay_ms), (1000, 50));
        assert_eq!(d.shortcut, "ctrl+v");
        let c = ClientConfig::from_toml(
            "[paste]\nauto = false\nwait_ms = 300\ndelay_ms = 0\nshortcut = \"shift+insert\"\n\
             [[paste.rules]]\ncontains = [\"vesktop\"]\nshortcut = \"none\"\n",
        )
        .unwrap();
        assert!(!c.paste.auto);
        assert_eq!((c.paste.wait_ms, c.paste.delay_ms), (300, 0));
        assert_eq!(c.paste.rules.len(), 1, "rules replace the default list");
        assert!(ClientConfig::from_toml("[paste]\nshortcut = \"ctrl+\"").is_err());
        assert!(ClientConfig::from_toml("[paste]\nwait_ms = 5001").is_err());
        assert!(ClientConfig::from_toml("[paste]\ndelay_ms = 1001").is_err());
        assert!(ClientConfig::from_toml_strict("[paste]\nwiat_ms = 1").is_err());
        let e = ClientConfig::from_toml("[[paste.rules]]\ncontains = [\"x\"]\nshortcut = \"hyper+v\"")
            .unwrap_err()
            .to_string();
        assert!(e.contains("paste.rules") && e.contains("unknown modifier"), "{e}");
    }

    #[test]
    fn paste_shortcut_per_window_class() {
        let p = ClientConfig::default().paste;
        let shown = |class: &str| p.shortcut_for(class).map(|s| s.to_string());
        assert_eq!(shown("com.ayugram.desktop").as_deref(), Some("Ctrl+V"));
        assert_eq!(shown("vesktop").as_deref(), Some("Ctrl+V"));
        assert_eq!(shown("firefox").as_deref(), Some("Ctrl+V"));
        for terminal in [
            "kitty",
            "foot",
            "footclient",
            "Alacritty",
            "org.wezfurlong.wezterm",
            "com.mitchellh.ghostty",
            "org.gnome.Terminal",
            "gnome-terminal-server",
            "org.gnome.Console",
            "kgx",
            "org.gnome.Ptyxis",
            "org.kde.konsole",
            "XTerm",
            "URxvt",
            "xfce4-terminal",
            "com.gexperts.Tilix",
        ] {
            assert_eq!(shown(terminal).as_deref(), Some("Ctrl+Shift+V"), "{terminal}");
        }
        let c = ClientConfig::from_toml(
            "[paste]\nshortcut = \"shift+insert\"\n[[paste.rules]]\ncontains = [\"Vesktop\"]\nshortcut = \"none\"\n",
        )
        .unwrap();
        assert_eq!(c.paste.shortcut_for("vesktop"), None, "none: no paste");
        assert_eq!(c.paste.shortcut_for("kitty").unwrap().to_string(), "Shift+Insert");
    }

    #[test]
    fn shortcut_syntax() {
        let p = |s: &str| Shortcut::parse(s).map(|o| o.map(|s| s.to_string()));
        assert_eq!(p("ctrl+v").unwrap().as_deref(), Some("Ctrl+V"));
        assert_eq!(p(" Control + Shift + V ").unwrap().as_deref(), Some("Ctrl+Shift+V"));
        assert_eq!(p("super+alt+1").unwrap().as_deref(), Some("Alt+Super+1"));
        assert_eq!(p("SHIFT+Insert").unwrap().as_deref(), Some("Shift+Insert"));
        assert_eq!(p("insert").unwrap().as_deref(), Some("Insert"));
        assert_eq!(p("None").unwrap(), None);
        let v = Shortcut::parse("ctrl+v").unwrap().unwrap();
        assert!(v.ctrl && !v.shift && !v.alt && !v.super_key);
        assert_eq!(v.key, ShortcutKey::Letter('v'));
        for bad in [
            "",
            "v",
            "1",
            "+",
            "ctrl+",
            "+v",
            "ctrl+ctrl+v",
            "hyper+v",
            "ctrl+vv",
            "ctrl+f1",
            "ctrl+\u{e4}",
            "ctrl+;",
        ] {
            assert!(Shortcut::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn focus_rules_match_ayugram() {
        let c = ClientConfig::default();
        assert_eq!(
            c.target_for_window("com.ayugram.desktop", ""),
            Some(TargetApp::Telegram)
        );
        assert_eq!(c.target_for_window("vesktop", "Discord"), Some(TargetApp::Discord));
        assert_eq!(c.target_for_window("firefox", "Element"), None);
    }
}
