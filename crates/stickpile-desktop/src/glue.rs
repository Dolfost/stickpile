//! Display-free glue between the client config, focus detection, the popup
//! model and the window geometry.

use std::path::{Path, PathBuf};

use stickpile_core::ClientConfig;
use stickpile_quick::focus::{self, FocusEnv, FocusError, WindowInfo};
use stickpile_quick::history::WindowHistory;
use stickpile_quick::model::{ModelConfig, OpenInfo, DEFAULT_DEBOUNCE_MS};

/// GApplication id: D-Bus name, Wayland app id / X11 class of the popup.
pub const APP_ID: &str = "io.github.dolfost.Stickpile";

/// Rows the window shows without scrolling.
pub const VISIBLE_ROWS: u32 = 4;
/// Space around a thumbnail inside its grid cell (GridView child padding + margins).
pub const CELL_EXTRA_PX: u32 = 16;
/// Search bar and footer heights plus outer margins.
const CHROME_HEIGHT_PX: u32 = 56 + 44;
const CHROME_WIDTH_PX: u32 = 24;

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
    }
}

/// Default popup size in logical pixels for `ui.columns` × [`VISIBLE_ROWS`] cells.
pub fn window_size(cfg: &ClientConfig) -> (i32, i32) {
    let cell = cfg.ui.thumb_px + CELL_EXTRA_PX;
    let w = cfg.ui.columns.max(1) * cell + CHROME_WIDTH_PX;
    let h = VISIBLE_ROWS * cell + CHROME_HEIGHT_PX;
    (w as i32, h as i32)
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
    }

    #[test]
    fn window_size_follows_grid() {
        let cfg = ClientConfig::default(); // 6 columns, 128 px
        assert_eq!(window_size(&cfg), (6 * 144 + 24, 4 * 144 + 100));
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
