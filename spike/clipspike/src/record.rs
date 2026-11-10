//! Result recorder: session detection and the Markdown results table.

use std::io::Write;
use std::path::Path;

/// Target apps of the matrix.
pub const TARGETS: [&str; 4] = [
    "Telegram Desktop / AyuGram",
    "Discord / Vesktop",
    "Browser chat (Firefox)",
    "Other",
];

/// What the target app did with the paste or drop.
pub const OUTCOMES: [&str; 8] = [
    "photo (compressed image)",
    "sticker",
    "file / document",
    "animation (GIF)",
    "video",
    "text / path pasted",
    "nothing happened",
    "error / crash",
];

/// How the content got there.
pub const ROUTES: [&str; 4] = [
    "paste (GDK clipboard)",
    "paste after clipspike quit",
    "drag and drop",
    "paste (wl-data-control)",
];

const MANAGERS: [&str; 9] = [
    "wl-clip-persist",
    "cliphist",
    "clipman",
    "copyq",
    "clipse",
    "gpaste-daemon",
    "klipper",
    "parcellite",
    "greenclip",
];

/// Session facts written into every result row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// `Wayland` / `X11` / `unknown`
    pub kind: String,
    /// `Hyprland`, `sway`, `GNOME`, ...
    pub desktop: String,
    /// Clipboard managers seen running.
    pub managers: Vec<String>,
}

impl Session {
    pub fn describe(&self) -> String {
        let managers = if self.managers.is_empty() {
            "none".to_string()
        } else {
            self.managers.join("+")
        };
        format!("{} ({}), clipboard manager: {managers}", self.kind, self.desktop)
    }
}

/// From environment lookups and the names of running processes.
pub fn detect_session(get: impl Fn(&str) -> Option<String>, processes: &[String]) -> Session {
    let kind = match get("XDG_SESSION_TYPE").as_deref() {
        Some("wayland") => "Wayland",
        Some("x11") => "X11",
        _ if get("WAYLAND_DISPLAY").is_some() => "Wayland",
        _ if get("DISPLAY").is_some() => "X11",
        _ => "unknown",
    }
    .to_string();
    let desktop = if get("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        "Hyprland".to_string()
    } else if get("SWAYSOCK").is_some() {
        "sway".to_string()
    } else {
        get("XDG_CURRENT_DESKTOP").unwrap_or_else(|| "unknown".into())
    };
    let mut managers: Vec<String> = MANAGERS
        .iter()
        .filter(|m| processes.iter().any(|p| p == *m))
        .map(|m| m.to_string())
        .collect();
    managers.dedup();
    Session {
        kind,
        desktop,
        managers,
    }
}

/// Names (`/proc/<pid>/comm`) of the current user's processes.
pub fn process_names() -> Vec<String> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|e| std::fs::read_to_string(e.path().join("comm")).ok())
        .map(|s| s.trim().to_string())
        .collect()
}

pub fn current_session() -> Session {
    detect_session(|k| std::env::var(k).ok().filter(|v| !v.is_empty()), &process_names())
}

/// One recorded observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub when: String,
    pub session: String,
    pub route: String,
    pub file: String,
    pub offers: String,
    pub target: String,
    pub outcome: String,
    pub notes: String,
}

pub const HEADER: &str = "| when (UTC) | session | route | file | offers | target | result | notes |\n\
                          |---|---|---|---|---|---|---|---|\n";

fn cell(s: &str) -> String {
    let s = s.replace('|', "\\|").replace(['\n', '\r'], " ");
    if s.trim().is_empty() {
        "—".into()
    } else {
        s.trim().to_string()
    }
}

impl Row {
    pub fn to_markdown(&self) -> String {
        let cells = [
            &self.when,
            &self.session,
            &self.route,
            &self.file,
            &self.offers,
            &self.target,
            &self.outcome,
            &self.notes,
        ];
        format!(
            "| {} |\n",
            cells.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | ")
        )
    }
}

/// Append a row, writing the table header first if the file is new or empty.
pub fn append(path: &Path, row: &Row) -> std::io::Result<()> {
    let needs_header = std::fs::metadata(path).map(|m| m.len() == 0).unwrap_or(true);
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    if needs_header {
        f.write_all(b"# clipspike results\n\n")?;
        f.write_all(HEADER.as_bytes())?;
    }
    f.write_all(row.to_markdown().as_bytes())
}

pub fn now_utc() -> String {
    let fmt = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]");
    time::OffsetDateTime::now_utc().format(&fmt).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn hyprland_with_persist() {
        let s = detect_session(
            env(&[
                ("XDG_SESSION_TYPE", "wayland"),
                ("HYPRLAND_INSTANCE_SIGNATURE", "x"),
                ("XDG_CURRENT_DESKTOP", "Hyprland"),
            ]),
            &[
                "zsh".into(),
                "wl-clip-persist".into(),
                "wl-paste".into(),
                "cliphist".into(),
            ],
        );
        assert_eq!(s.kind, "Wayland");
        assert_eq!(s.desktop, "Hyprland");
        assert_eq!(
            s.describe(),
            "Wayland (Hyprland), clipboard manager: wl-clip-persist+cliphist"
        );
    }

    #[test]
    fn x11_without_manager() {
        let s = detect_session(env(&[("DISPLAY", ":0"), ("XDG_CURRENT_DESKTOP", "i3")]), &[]);
        assert_eq!(s.describe(), "X11 (i3), clipboard manager: none");
        assert_eq!(detect_session(env(&[]), &[]).kind, "unknown");
    }

    #[test]
    fn row_escaping() {
        let row = Row {
            when: "2026-11-10 12:00".into(),
            session: "Wayland (Hyprland)".into(),
            route: "paste".into(),
            file: "a|b.png".into(),
            offers: "image/png".into(),
            target: "Telegram".into(),
            outcome: "photo".into(),
            notes: "line1\nline2".into(),
        };
        assert_eq!(
            row.to_markdown(),
            "| 2026-11-10 12:00 | Wayland (Hyprland) | paste | a\\|b.png | image/png | Telegram | photo | line1 line2 |\n"
        );
        let empty = Row {
            notes: " ".into(),
            ..row
        };
        assert!(empty.to_markdown().ends_with("| photo | — |\n"));
    }

    #[test]
    fn append_writes_header_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("results.md");
        let row = Row {
            when: "t".into(),
            session: "s".into(),
            route: "r".into(),
            file: "f".into(),
            offers: "o".into(),
            target: "x".into(),
            outcome: "y".into(),
            notes: "".into(),
        };
        append(&p, &row).unwrap();
        append(&p, &row).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.matches("| when (UTC) |").count(), 1);
        assert_eq!(text.lines().filter(|l| l.starts_with("| t |")).count(), 2);
    }

    #[test]
    fn timestamp_shape() {
        let t = now_utc();
        assert_eq!(t.len(), 16, "{t}");
        assert_eq!(&t[4..5], "-");
    }

    #[test]
    fn lists_are_consistent() {
        assert_eq!(TARGETS.len(), 4);
        assert!(OUTCOMES.contains(&"sticker"));
        assert!(ROUTES.iter().any(|r| r.contains("wl-data-control")));
    }
}
