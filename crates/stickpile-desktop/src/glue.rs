//! Display-free glue between the client config, focus detection, the popup
//! model and the window geometry.

use std::path::{Path, PathBuf};

use stickpile_core::config::StartMode;
use stickpile_core::ClientConfig;
use stickpile_i18n::Lang;
use stickpile_quick::focus::{self, FocusEnv, FocusError, WindowInfo};
use stickpile_quick::history::WindowHistory;
use stickpile_quick::model::{ModelConfig, OpenInfo, Tile, Tiles, DEFAULT_DEBOUNCE_MS};
use stickpile_quick::{Mode, Tab};

/// GApplication id: D-Bus name, Wayland app id / X11 class of the popup.
pub const APP_ID: &str = "io.github.dolfost.Stickpile";

/// Rows the window shows without scrolling at its default size.
pub const VISIBLE_ROWS: u32 = 4;
/// Space around a thumbnail inside its grid cell: the GridView child's
/// padding, 8 px a side (the shell's CSS).
pub const CELL_EXTRA_PX: u32 = 16;
/// Most grid columns a wide window gets.
pub const MAX_COLUMNS: u32 = 64;
/// Height a pack list cell adds under the cover: its name and item count.
pub const PACK_LABELS_PX: u32 = 40;
/// Tab row, header (title and text field) and footer heights plus outer margins.
const CHROME_HEIGHT_PX: u32 = 40 + 46 + 26;
const CHROME_WIDTH_PX: u32 = 24;
/// The tab row needs this much (seven numbered tabs and the help button);
/// narrower grids get wider cells instead.
pub const MIN_WIDTH_PX: u32 = 700;

pub fn model_config(cfg: &ClientConfig) -> ModelConfig {
    ModelConfig {
        columns: cfg.ui.columns.max(1) as usize,
        visible_rows: VISIBLE_ROWS as usize,
        recent_count: cfg.ui.recent_count as usize,
        max_results: cfg.ui.max_results.max(1) as usize,
        default_target: cfg.default_target,
        prefer_stickers: cfg.prefer_stickers,
        hide_on_copy: cfg.ui.hide_on_copy,
        debounce_ms: DEFAULT_DEBOUNCE_MS,
        modal: cfg.ui.modal,
        // The config is validated: anything but a tab's name is `last`.
        start_tab: Tab::from_key(&cfg.ui.start_tab),
        start_mode: match cfg.ui.start_mode {
            StartMode::Normal => Mode::Normal,
            StartMode::Insert => Mode::Insert,
        },
        tiles: Tiles {
            favorites: cfg.ui.mark_favorites,
            emojis: cfg.ui.show_emojis,
            tags: cfg.ui.show_tags,
        },
        ..ModelConfig::default()
    }
}

/// The config key a `z` key's change is saved to.
pub fn tile_key(tile: Tile) -> &'static str {
    match tile {
        Tile::Favorites => "ui.mark_favorites",
        Tile::Emojis => "ui.show_emojis",
        Tile::Tags => "ui.show_tags",
    }
}

/// Grid columns and fully visible rows for a grid `width` x `height`
/// logical pixels with cells `cell_w` x `cell_h`, the way GtkGridView
/// counts its columns (no border spacing): as many as fit, at least one.
pub fn grid_layout(width: i32, height: i32, cell_w: i32, cell_h: i32) -> (usize, usize) {
    let columns = (width / cell_w.max(1)).clamp(1, MAX_COLUMNS as i32);
    let rows = (height / cell_h.max(1)).max(1);
    (columns as usize, rows as usize)
}

/// Default popup size in logical pixels for `ui.columns` x [`VISIBLE_ROWS`]
/// cells, at least [`MIN_WIDTH_PX`] wide.
pub fn window_size(cfg: &ClientConfig) -> (i32, i32) {
    let cell = cfg.ui.thumb_px + CELL_EXTRA_PX;
    let w = (cfg.ui.columns.max(1) * cell + CHROME_WIDTH_PX).max(MIN_WIDTH_PX);
    let h = VISIBLE_ROWS * cell + CHROME_HEIGHT_PX;
    (w as i32, h as i32)
}

/// Rows of the pack list that fit where [`VISIBLE_ROWS`] item rows do (its
/// cells are taller).
pub fn pack_rows(cfg: &ClientConfig) -> usize {
    let cell = cfg.ui.thumb_px + CELL_EXTRA_PX;
    ((VISIBLE_ROWS * cell) / (cell + PACK_LABELS_PX)).max(1) as usize
}

/// Map a focus detection result to what the model needs. The popup's own
/// window (if it was still focused) counts as "no window"; detection errors
/// are logged and fall back to the default target.
pub fn open_info(cfg: &ClientConfig, detected: Result<Option<WindowInfo>, FocusError>) -> OpenInfo {
    let window = match detected {
        Ok(Some(w)) if !w.is_app(APP_ID) => w,
        Ok(_) => return OpenInfo::default(),
        Err(e) => {
            tracing::debug!("focus detection failed: {e}");
            return OpenInfo::default();
        }
    };
    let detected = cfg.target_for_window(&window.class, &window.title);
    OpenInfo {
        window: Some(window),
        detected,
    }
}

/// Detect the focused window (if `focus.detect`) with the given environment
/// (the invoking process's, forwarded by GApplication).
pub fn capture_focus(cfg: &ClientConfig, env: &FocusEnv) -> OpenInfo {
    if !cfg.focus.detect {
        return OpenInfo::default();
    }
    open_info(cfg, focus::detect_with(env))
}

pub fn history_path(cfg: &ClientConfig) -> PathBuf {
    cfg.cache_path().join("desktop-window-history.json")
}

/// The popup's language: `ui.language` when it names one, else the
/// account's (`account`, from the server or its cache), else the system's
/// locale (`env`), else English.
pub fn effective_lang(cfg: &ClientConfig, account: Option<Lang>, env: impl Fn(&str) -> Option<String>) -> Lang {
    Lang::parse(&cfg.ui.language)
        .or(account)
        .or_else(|| Lang::from_env(env))
        .unwrap_or_default()
}

/// Where the account's language is kept between starts (offline too).
pub fn account_lang_path(cfg: &ClientConfig) -> PathBuf {
    cfg.cache_path().join("desktop-account-language")
}

/// `None` when missing or "automatic".
pub fn load_account_lang(path: &Path) -> Option<Lang> {
    Lang::parse(std::fs::read_to_string(path).ok()?.trim())
}

/// Remember the account's language (`None`: automatic).
pub fn save_account_lang(path: &Path, lang: Option<Lang>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, lang.map(|l| l.code()).unwrap_or("auto"))?;
    std::fs::rename(&tmp, path)
}

/// The tab the popup was last closed on, for `ui.start_tab = "last"`.
pub fn last_tab_path(cfg: &ClientConfig) -> PathBuf {
    cfg.cache_path().join("desktop-last-tab")
}

/// Missing, unreadable or unknown: none.
pub fn load_last_tab(path: &Path) -> Option<Tab> {
    Tab::from_key(std::fs::read_to_string(path).ok()?.trim())
}

/// Atomic write (temp file + rename).
pub fn save_last_tab(path: &Path, tab: Tab) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, format!("{}\n", tab.key()))?;
    std::fs::rename(&tmp, path)
}

/// Missing or unreadable history starts empty.
pub fn load_history(path: &Path) -> WindowHistory {
    match std::fs::read_to_string(path) {
        Ok(text) => WindowHistory::from_json(&text).unwrap_or_else(|e| {
            tracing::warn!("ignoring corrupt {}: {e}", path.display());
            WindowHistory::default()
        }),
        Err(_) => WindowHistory::default(),
    }
}

/// Atomic write (temp file + rename).
pub fn save_history(path: &Path, history: &WindowHistory) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, history.to_json())?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use stickpile_api::TargetApp;
    use stickpile_quick::focus::FocusBackend;

    fn win(class: &str, title: &str) -> WindowInfo {
        WindowInfo {
            class: class.into(),
            title: title.into(),
            backend: FocusBackend::Hyprland,
        }
    }

    #[test]
    fn model_config_from_client_config() {
        let mut cfg = ClientConfig::default();
        cfg.ui.columns = 5;
        cfg.ui.recent_count = 12;
        cfg.ui.hide_on_copy = false;
        cfg.default_target = TargetApp::Discord;
        cfg.prefer_stickers = true;
        let m = model_config(&cfg);
        assert_eq!(m.columns, 5);
        assert_eq!(m.recent_count, 12);
        assert!(!m.hide_on_copy);
        assert_eq!(m.default_target, TargetApp::Discord);
        assert!(m.prefer_stickers);
        assert_eq!(m.debounce_ms, 120);
        assert!(m.modal);
        assert_eq!(m.start_tab, None, "the last tab");
        assert_eq!(m.start_mode, Mode::Normal);
        assert_eq!(m.tiles, Tiles::default());

        cfg.ui.modal = false;
        cfg.ui.start_tab = "most_used".into();
        cfg.ui.start_mode = StartMode::Insert;
        cfg.ui.show_tags = false;
        let m = model_config(&cfg);
        assert!(!m.modal);
        assert_eq!(m.start_tab, Some(Tab::MostUsed));
        assert_eq!(m.start_mode, Mode::Insert);
        assert!(!m.tiles.tags && m.tiles.emojis && m.tiles.favorites);
    }

    #[test]
    fn tile_keys_are_settings() {
        for tile in Tile::ALL {
            let spec = stickpile_core::settings::setting_spec(tile_key(tile)).unwrap();
            assert_eq!(spec.default, toml::Value::Boolean(true));
        }
    }

    #[test]
    fn grid_layout_fits_whole_cells() {
        // The default window: 6 columns of 144 px.
        assert_eq!(grid_layout(864, 576, 144, 144), (6, 4));
        assert_eq!(grid_layout(1000, 600, 144, 144), (6, 4));
        assert_eq!(grid_layout(1008, 300, 144, 144), (7, 2));
        assert_eq!(grid_layout(100, 50, 144, 144), (1, 1), "at least one");
        assert_eq!(grid_layout(100_000, 50, 144, 144).0, MAX_COLUMNS as usize);
        assert_eq!(grid_layout(0, 0, 0, 0), (1, 1));
    }

    #[test]
    fn language_choice() {
        let mut cfg = ClientConfig::default();
        let no_env = |_: &str| None;
        let uk_env = |k: &str| (k == "LANG").then(|| "uk_UA.UTF-8".to_string());
        assert_eq!(effective_lang(&cfg, None, no_env), Lang::En);
        assert_eq!(effective_lang(&cfg, None, uk_env), Lang::Uk, "the system's");
        assert_eq!(
            effective_lang(&cfg, Some(Lang::En), uk_env),
            Lang::En,
            "the account's first"
        );
        cfg.ui.language = "uk".into();
        assert_eq!(
            effective_lang(&cfg, Some(Lang::En), no_env),
            Lang::Uk,
            "a choice here wins"
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache/lang");
        assert_eq!(load_account_lang(&path), None);
        save_account_lang(&path, Some(Lang::Uk)).unwrap();
        assert_eq!(load_account_lang(&path), Some(Lang::Uk));
        save_account_lang(&path, None).unwrap();
        assert_eq!(load_account_lang(&path), None);
    }

    #[test]
    fn last_tab_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ClientConfig {
            cache_dir: Some(dir.path().join("cache")),
            ..ClientConfig::default()
        };
        let path = last_tab_path(&cfg);
        assert_eq!(load_last_tab(&path), None);
        save_last_tab(&path, Tab::Packs).unwrap();
        assert_eq!(load_last_tab(&path), Some(Tab::Packs));
        std::fs::write(&path, "nope").unwrap();
        assert_eq!(load_last_tab(&path), None);
    }

    #[test]
    fn window_size_follows_grid() {
        let mut cfg = ClientConfig::default(); // 6 columns, 128 px
        assert_eq!(window_size(&cfg), (6 * 144 + 24, 4 * 144 + 112));
        assert_eq!(pack_rows(&cfg), 3);
        cfg.ui.columns = 3;
        assert_eq!(window_size(&cfg).0, MIN_WIDTH_PX as i32, "the tabs still fit");
        cfg.ui.thumb_px = 64;
        assert_eq!(pack_rows(&cfg), 2);
    }

    #[test]
    fn owner_apps_map_to_targets() {
        let cfg = ClientConfig::default();
        let info = open_info(&cfg, Ok(Some(win("com.ayugram.desktop", "AyuGram"))));
        assert_eq!(info.detected, Some(TargetApp::Telegram));
        assert_eq!(info.window.unwrap().class, "com.ayugram.desktop");
        assert_eq!(
            open_info(&cfg, Ok(Some(win("vesktop", "#general | Server")))).detected,
            Some(TargetApp::Discord)
        );
        // Browser chats: matched by title.
        assert_eq!(
            open_info(&cfg, Ok(Some(win("firefox", "Telegram Web")))).detected,
            Some(TargetApp::Telegram)
        );
        let ff = open_info(&cfg, Ok(Some(win("firefox", "Element"))));
        assert_eq!(ff.detected, None);
        assert!(ff.window.is_some(), "window kept for per-window history");
    }

    #[test]
    fn own_window_errors_and_none_give_defaults() {
        let cfg = ClientConfig::default();
        assert_eq!(open_info(&cfg, Ok(Some(win(APP_ID, "stickpile")))), OpenInfo::default());
        assert_eq!(open_info(&cfg, Ok(None)), OpenInfo::default());
        assert_eq!(open_info(&cfg, Err(FocusError::X11Disabled)), OpenInfo::default());
    }

    #[test]
    fn custom_rules_from_toml() {
        let cfg = ClientConfig::from_toml(
            "[focus]\ndetect = true\n[[focus.rules]]\ncontains = [\"element\"]\ntarget = \"slack\"\n",
        )
        .unwrap();
        assert_eq!(
            open_info(&cfg, Ok(Some(win("element-desktop", "Room")))).detected,
            Some(TargetApp::Slack)
        );
    }

    #[test]
    fn detection_can_be_disabled() {
        let mut cfg = ClientConfig::default();
        cfg.focus.detect = false;
        let env = FocusEnv {
            hyprland_instance_signature: Some("x".into()),
            ..FocusEnv::default()
        };
        assert_eq!(capture_focus(&cfg, &env), OpenInfo::default());
        // Enabled, but no compositor IPC in this environment.
        cfg.focus.detect = true;
        assert_eq!(capture_focus(&cfg, &FocusEnv::default()), OpenInfo::default());
    }

    #[test]
    fn data_files_match_the_app() {
        let desktop = include_str!("../data/io.github.dolfost.Stickpile.desktop");
        assert!(desktop.contains("\nExec=stickpile-desktop\n"));
        assert!(desktop.contains("\nNoDisplay=false\n"));
        assert!(desktop.contains(&format!("\nIcon={APP_ID}\n")));
        let unit = include_str!("../data/stickpile-desktop.service");
        assert!(unit
            .lines()
            .any(|l| l.starts_with("ExecStart=") && l.ends_with("stickpile-desktop --hidden")));
        let icon = format!(
            "{}/data/icons/hicolor/scalable/apps/{APP_ID}.svg",
            env!("CARGO_MANIFEST_DIR")
        );
        assert!(std::path::Path::new(&icon).exists(), "{icon}");
    }

    #[test]
    fn history_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ClientConfig {
            cache_dir: Some(dir.path().join("cache")),
            ..ClientConfig::default()
        };
        let path = history_path(&cfg);
        assert!(path.starts_with(dir.path()));
        assert!(load_history(&path).is_empty());
        let mut h = WindowHistory::default();
        h.record("vesktop", "a");
        save_history(&path, &h).unwrap();
        assert_eq!(load_history(&path), h);
        std::fs::write(&path, "garbage").unwrap();
        assert!(load_history(&path).is_empty());
    }
}
