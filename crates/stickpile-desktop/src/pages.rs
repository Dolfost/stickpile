//! Display-free decisions behind the Me and Settings tabs: what the profile
//! says in words, how setting values are labelled, and which config changes
//! need a new core session.

use std::path::Path;

use stickpile_api::TargetApp;
use stickpile_core::ClientConfig;
use stickpile_i18n::global::{t, t_dyn, tf, tp};
use stickpile_i18n::Lang;
use stickpile_quick::model::target_name;
use stickpile_quick::{HelpSection, Tab};

use crate::backend::{AuthKind, Profile};

/// "512 B", "1.5 KB", "12.3 MB", in the current language.
pub fn human_bytes(n: u64) -> String {
    if n < 1000 {
        return tf("{n} B", &[("n", &n)]);
    }
    let mut v = n as f64 / 1000.0;
    let mut unit = 0;
    while v >= 999.95 && unit < 3 {
        v /= 1000.0;
        unit += 1;
    }
    let v = format!("{v:.1}");
    match unit {
        0 => tf("{n} KB", &[("n", &v)]),
        1 => tf("{n} MB", &[("n", &v)]),
        2 => tf("{n} GB", &[("n", &v)]),
        _ => tf("{n} TB", &[("n", &v)]),
    }
}

/// Heading of the Me page.
pub fn who(p: &Profile) -> String {
    p.display_name
        .clone()
        .or_else(|| p.username.clone())
        .unwrap_or_else(|| t("Not signed in").into())
}

/// The line under the heading: "@alice, admin".
pub fn who_detail(p: &Profile) -> String {
    let mut parts = Vec::new();
    if let Some(u) = &p.username {
        parts.push(format!("@{u}"));
    }
    if p.admin {
        parts.push(t("admin").into());
    }
    parts.join(", ")
}

/// How the popup is signed in.
pub fn auth_text(p: &Profile) -> &'static str {
    match p.auth {
        AuthKind::Token => t("With a token stored on this device"),
        AuthKind::Tailnet => t("By tailnet identity, no token stored"),
        AuthKind::None => t("Not signed in"),
    }
}

/// The counts shown as tiles, (value, caption); unknown ones are left out.
pub fn stat_tiles(p: &Profile) -> Vec<(String, &'static str)> {
    let count = |v: Option<u64>, caption| v.map(|n| (n.to_string(), caption));
    let mut tiles: Vec<_> = [
        count(p.copies, t("Copies")),
        count(p.copies_last_7d, t("This week")),
        count(p.likes, t("Likes given")),
        count(p.likes_received, t("Likes received")),
        count(p.packs, t("Packs")),
        count(p.items, t("Items")),
        p.storage_bytes.map(|b| (human_bytes(b), t("Storage"))),
    ]
    .into_iter()
    .flatten()
    .collect();
    if p.pending_usage > 0 {
        tiles.push((p.pending_usage.to_string(), t("Copies not yet uploaded")));
    }
    tiles
}

/// Display text of a choice value of the setting `key`.
pub fn choice_label(key: &str, value: &str) -> String {
    let named = match key {
        "ui.start_tab" if value == "last" => Some(t("Last used tab").to_string()),
        "ui.start_tab" => Tab::from_key(value).map(|tab| tab.label().to_string()),
        "ui.language" if value == "auto" => Some(t("Automatic").to_string()),
        "ui.language" => Lang::parse(value).map(|l| l.native_name().to_string()),
        "default_target" => TargetApp::parse(value).map(target_name),
        _ => None,
    };
    named.unwrap_or_else(|| {
        let text = value.replace('_', " ");
        let mut chars = text.chars();
        let shown: String = match chars.next() {
            Some(first) => first.to_uppercase().chain(chars).collect(),
            None => text,
        };
        // "Transparent", "White", ...: translated when the catalog has them.
        t_dyn(&shown)
    })
}

/// Shown on a setting that `config.toml` sets while it is read-only.
pub fn lock_reason(config_path: &Path) -> &'static str {
    let in_store = std::fs::canonicalize(config_path).is_ok_and(|p| p.starts_with("/nix/store"));
    if in_store {
        t("Set by home-manager")
    } else {
        t("Set in config.toml")
    }
}

/// Where the Settings page saves, in words; `home` is shortened to `~`.
pub fn save_note(config_path: &Path, write_path: &Path, home: Option<&Path>) -> String {
    let short = |p: &Path| match home.and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    };
    if config_path == write_path {
        return tf("Changes are saved to {file}.", &[("file", &short(config_path))]);
    }
    let name = |p: &Path| {
        p.file_name()
            .map_or_else(|| short(p), |n| n.to_string_lossy().into_owned())
    };
    tf(
        "{config} is read-only, so changes are saved to {settings} next to it; the keys {name} sets are locked.",
        &[
            ("config", &short(config_path)),
            ("settings", &name(write_path)),
            ("name", &name(config_path)),
        ],
    )
}

/// The change touches what the core session was opened with (server,
/// timeout, cache, token file, copy and clipboard settings): the session
/// must be opened again. Popup-only keys apply to the running one.
pub fn needs_new_session(old: &ClientConfig, new: &ClientConfig) -> bool {
    let session_part = |c: &ClientConfig| ClientConfig {
        ui: Default::default(),
        focus: Default::default(),
        default_target: TargetApp::Generic,
        prefer_stickers: false,
        ..c.clone()
    };
    session_part(old) != session_part(new)
}

/// Another server or token: the account may have changed, so every list
/// fetched so far is stale.
pub fn account_changed(old: &ClientConfig, new: &ClientConfig) -> bool {
    old.server_url != new.server_url || old.token_path() != new.token_path()
}

/// "1 item", "12 items".
pub fn item_count(n: u64) -> String {
    tp(n, "{n} item", "{n} items")
}

/// The key help in two columns of about the same height, sections kept in order.
pub fn help_columns(sections: &[HelpSection]) -> [Vec<&HelpSection>; 2] {
    let height = |s: &HelpSection| s.keys.len() + 2;
    let total: usize = sections.iter().map(height).sum();
    let mut out = [Vec::new(), Vec::new()];
    let mut left = 0;
    for s in sections {
        let col = usize::from(left >= total / 2);
        if col == 0 {
            left += height(s);
        }
        out[col].push(s);
    }
    out
}

/// The next cursor position on a page of `len` stops, without wrapping.
pub fn step(len: usize, current: Option<usize>, down: bool) -> Option<usize> {
    let last = len.checked_sub(1)?;
    Some(match current.map(|i| i.min(last)) {
        None => 0,
        Some(i) if down => (i + 1).min(last),
        Some(i) => i.saturating_sub(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alice() -> Profile {
        Profile {
            auth: AuthKind::Token,
            username: Some("alice".into()),
            display_name: Some("Alice".into()),
            admin: true,
            copies: Some(12),
            copies_last_7d: Some(3),
            likes: Some(4),
            likes_received: Some(0),
            packs: Some(2),
            items: Some(30),
            storage_bytes: Some(1_536_000),
            ..Profile::signed_out("http://srv")
        }
    }

    #[test]
    fn sizes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1000), "1.0 KB");
        assert_eq!(human_bytes(1_536_000), "1.5 MB");
        assert_eq!(human_bytes(999_999), "1.0 MB");
        assert_eq!(human_bytes(2_000_000_000), "2.0 GB");
        assert_eq!(human_bytes(u64::MAX), "18446744.1 TB");
    }

    #[test]
    fn profile_texts() {
        let p = alice();
        assert_eq!(who(&p), "Alice");
        assert_eq!(who_detail(&p), "@alice, admin");
        assert_eq!(auth_text(&p), "With a token stored on this device");
        let tiles = stat_tiles(&p);
        let captions: Vec<_> = tiles.iter().map(|t| t.1).collect();
        assert_eq!(
            captions,
            [
                "Copies",
                "This week",
                "Likes given",
                "Likes received",
                "Packs",
                "Items",
                "Storage"
            ]
        );
        assert_eq!(tiles[6].0, "1.5 MB");

        let tailnet = Profile {
            auth: AuthKind::Tailnet,
            display_name: None,
            admin: false,
            pending_usage: 2,
            ..alice()
        };
        assert_eq!(who(&tailnet), "alice");
        assert_eq!(who_detail(&tailnet), "@alice");
        assert_eq!(auth_text(&tailnet), "By tailnet identity, no token stored");
        assert_eq!(
            stat_tiles(&tailnet).last().unwrap(),
            &("2".to_string(), "Copies not yet uploaded")
        );

        let out = Profile::signed_out("http://srv");
        assert_eq!(who(&out), "Not signed in");
        assert_eq!(who_detail(&out), "");
        assert!(stat_tiles(&out).is_empty());
    }

    /// The Settings rows (labels from the core config) and the key help
    /// are shown through `t_dyn`: each text needs a Ukrainian one.
    #[test]
    fn ukrainian_covers_the_settings_and_the_help() {
        use stickpile_core::settings::{settings_schema, SettingKind, GROUPS};
        use stickpile_i18n::{lookup, Lang};
        let mut missing = Vec::new();
        let mut need = |text: &str| {
            if lookup(Lang::Uk, text).is_none() {
                missing.push(text.to_string());
            }
        };
        for g in GROUPS {
            need(g);
        }
        for spec in settings_schema() {
            need(spec.label);
            need(spec.help);
            if let SettingKind::Choice(names) = spec.kind {
                for n in names {
                    // Under English (the tests' language) the label is the
                    // catalog key; tabs and languages are named elsewhere.
                    let shown = choice_label(spec.key, n);
                    let named = matches!(spec.key, "ui.start_tab" | "default_target")
                        || (spec.key == "ui.language" && *n != "auto");
                    if !named {
                        need(&shown);
                    }
                }
            }
        }
        for section in stickpile_quick::HELP {
            need(section.title);
            for (keys, what) in section.keys {
                need(keys);
                need(what);
            }
        }
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "{} texts without Ukrainian:\n{}",
            missing.len(),
            missing.join("\n")
        );
    }

    #[test]
    fn choice_labels() {
        assert_eq!(choice_label("ui.start_tab", "most_used"), "Most used");
        assert_eq!(choice_label("ui.start_tab", "last"), "Last used tab");
        assert_eq!(choice_label("ui.start_mode", "normal"), "Normal");
        assert_eq!(choice_label("default_target", "whatsapp"), "WhatsApp");
        assert_eq!(
            choice_label("default_target", "generic"),
            target_name(TargetApp::Generic)
        );
        assert_eq!(choice_label("copy.square", "transparent"), "Transparent");
        assert_eq!(choice_label("x", "some_value"), "Some value");
        assert_eq!(choice_label("x", ""), "");
    }

    #[test]
    fn where_settings_go() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "").unwrap();
        assert_eq!(lock_reason(&cfg), "Set in config.toml");
        assert_eq!(lock_reason(&dir.path().join("missing.toml")), "Set in config.toml");
        assert_eq!(
            save_note(&cfg, &cfg, None),
            format!("Changes are saved to {}.", cfg.display())
        );
        assert_eq!(
            save_note(&cfg, &cfg, Some(dir.path())),
            "Changes are saved to ~/config.toml."
        );
        let settings = dir.path().join("settings.toml");
        assert_eq!(
            save_note(&cfg, &settings, Some(dir.path())),
            "~/config.toml is read-only, so changes are saved to settings.toml next to it; \
             the keys config.toml sets are locked."
        );
    }

    #[test]
    fn what_needs_a_new_session() {
        let old = ClientConfig::default();
        let mut new = old.clone();
        new.ui.columns = 3;
        new.ui.modal = false;
        new.focus.detect = false;
        new.default_target = TargetApp::Telegram;
        new.prefer_stickers = true;
        assert!(!needs_new_session(&old, &new));
        assert!(!account_changed(&old, &new));
        for change in [
            |c: &mut ClientConfig| c.timeout_secs = 9,
            |c: &mut ClientConfig| c.cache_max_mb = 64,
            |c: &mut ClientConfig| c.copy.square = stickpile_core::config::SquarePad::White,
        ] {
            let mut c = old.clone();
            change(&mut c);
            assert!(needs_new_session(&old, &c));
            assert!(!account_changed(&old, &c));
        }
        let mut c = old.clone();
        c.server_url = "http://other:8080".into();
        assert!(needs_new_session(&old, &c) && account_changed(&old, &c));
    }

    #[test]
    fn counts_and_help_columns() {
        assert_eq!(item_count(0), "0 items");
        assert_eq!(item_count(1), "1 item");
        assert_eq!(item_count(12), "12 items");

        let help = stickpile_quick::HELP;
        let [a, b] = help_columns(help);
        let titles: Vec<_> = a.iter().chain(&b).map(|s| s.title).collect();
        assert_eq!(titles, help.iter().map(|s| s.title).collect::<Vec<_>>(), "in order");
        assert!(!a.is_empty() && !b.is_empty());
        let rows = |c: &[&HelpSection]| c.iter().map(|s| s.keys.len() + 2).sum::<usize>();
        let tallest = help.iter().map(|s| s.keys.len() + 2).max().unwrap();
        assert!(rows(&a).abs_diff(rows(&b)) <= tallest);
    }

    #[test]
    fn cursor_steps() {
        assert_eq!(step(0, None, true), None);
        assert_eq!(step(0, Some(3), false), None);
        assert_eq!(step(3, None, true), Some(0));
        assert_eq!(step(3, None, false), Some(0));
        assert_eq!(step(3, Some(0), true), Some(1));
        assert_eq!(step(3, Some(2), true), Some(2));
        assert_eq!(step(3, Some(0), false), Some(0));
        assert_eq!(step(3, Some(9), false), Some(1), "a stale position is clamped first");
    }
}
