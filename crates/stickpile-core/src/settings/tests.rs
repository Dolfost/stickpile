use stickpile_api::TargetApp;

use super::*;
use crate::config::CopyConfig;

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

fn running_as_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[test]
fn schema_keys_exist_with_the_rust_defaults() {
    let schema = settings_schema();
    let keys: Vec<&str> = schema.iter().map(|s| s.key).collect();
    let mut want = vec![
        "server_url",
        "timeout_secs",
        "cache_max_mb",
        "default_target",
        "prefer_stickers",
        "ui.columns",
        "ui.thumb_px",
        "ui.hide_on_copy",
        "ui.recent_count",
        "ui.max_results",
        "ui.start_hidden",
        "ui.modal",
        "ui.language",
        "ui.start_tab",
        "ui.start_mode",
        "ui.mark_favorites",
        "ui.show_emojis",
        "ui.show_tags",
        "copy.square",
        "copy.background",
        "focus.detect",
    ];
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    want.sort_unstable();
    assert_eq!(sorted, want);

    let defaults = ClientConfig::default();
    for spec in &schema {
        let rust = get_value(&defaults, spec.key).unwrap_or_else(|| panic!("{} is not in ClientConfig", spec.key));
        assert_eq!(rust, spec.default, "default of {}", spec.key);
        spec.check(&spec.default).unwrap();
        assert!(GROUPS.contains(&spec.group), "{}", spec.key);
        assert!(!spec.label.is_empty() && !spec.help.is_empty() && !spec.help.contains('\n'));
        // Every value the GUI may produce is one the loader accepts.
        let mut t = toml::Table::new();
        let sample = match &spec.kind {
            SettingKind::Int { min, max } => vec![toml::Value::Integer(*min), toml::Value::Integer(*max)],
            SettingKind::Choice(names) => names.iter().map(|n| toml::Value::String(n.to_string())).collect(),
            _ => vec![spec.default.clone()],
        };
        for v in sample {
            insert_dotted(&mut t, spec.key, v.clone());
            let c = config_from_table(t.clone()).unwrap_or_else(|e| panic!("{} = {v}: {e}", spec.key));
            assert_eq!(get_value(&c, spec.key), Some(v));
        }
    }
    // Groups appear in GROUPS order, each one contiguous.
    let mut order: Vec<&str> = schema.iter().map(|s| s.group).collect();
    order.dedup();
    assert_eq!(order, GROUPS);
}

#[test]
fn target_choices_are_the_target_apps() {
    let names: Vec<&str> = TargetApp::ALL.iter().map(|t| t.as_str()).collect();
    assert_eq!(TARGET_NAMES, names.as_slice());
}

#[test]
fn spec_check_rejects_wrong_values() {
    let cols = setting_spec("ui.columns").unwrap();
    assert!(cols.check(&toml::Value::Integer(0)).is_err());
    assert!(cols.check(&toml::Value::Integer(21)).is_err());
    assert!(cols.check(&toml::Value::String("6".into())).is_err());
    let tab = setting_spec("ui.start_tab").unwrap();
    assert!(tab.check(&toml::Value::String("home".into())).is_err());
    tab.check(&toml::Value::String("last".into())).unwrap();
    let mode = setting_spec("ui.start_mode").unwrap();
    mode.check(&toml::Value::String("insert".into())).unwrap();
    assert!(mode.check(&toml::Value::String("visual".into())).is_err());
    assert!(setting_spec("ui.modal")
        .unwrap()
        .check(&toml::Value::Integer(1))
        .is_err());
    assert!(setting_spec("clipboard.default.still").is_none());
}

#[test]
fn get_value_reads_nested_and_optional_keys() {
    let mut c = ClientConfig::default();
    c.copy.square = SquarePad::White;
    assert_eq!(get_value(&c, "copy.square"), Some(toml::Value::String("white".into())));
    assert_eq!(get_value(&c, "ui.columns"), Some(toml::Value::Integer(6)));
    assert_eq!(get_value(&c, "token_file"), None);
    assert_eq!(get_value(&c, "ui.nope"), None);
}

#[test]
fn writable_config_is_edited_in_place_keeping_comments() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let original = "\
# my stickpile config
server_url = \"https://a.example\" # tailnet

[ui]
# wide screen
columns = 8  # eight
max_results = 40

[clipboard.targets.telegram]
still = [\"text/uri-list\"]
";
    std::fs::write(&cfg, original).unwrap();

    let l = load_layered(&cfg).unwrap();
    assert_eq!(l.write_path, cfg);
    assert!(l.locked.is_empty());
    assert!(l.warnings.is_empty());
    assert_eq!(l.config.ui.columns, 8);

    set_value(&cfg, "ui.columns", toml::Value::Integer(9)).unwrap();
    set_value(&cfg, "copy.square", toml::Value::String("white".into())).unwrap();
    set_value(&cfg, "timeout_secs", toml::Value::Integer(7)).unwrap();
    set_value(&cfg, "server_url", toml::Value::String("https://b.example".into())).unwrap();
    assert_eq!(
        read(&cfg),
        "\
# my stickpile config
server_url = \"https://b.example\" # tailnet
timeout_secs = 7

[ui]
# wide screen
columns = 9  # eight
max_results = 40

[clipboard.targets.telegram]
still = [\"text/uri-list\"]

[copy]
square = \"white\"
"
    );
    let c = load_layered(&cfg).unwrap().config;
    assert_eq!((c.ui.columns, c.timeout_secs), (9, 7));
    assert_eq!(c.copy.square, SquarePad::White);
    assert_eq!(c.clipboard.offers(TargetApp::Telegram).still, ["text/uri-list"]);

    unset_value(&cfg, "copy.square").unwrap();
    unset_value(&cfg, "timeout_secs").unwrap();
    unset_value(&cfg, "ui.columns").unwrap();
    unset_value(&cfg, "ui.columns").unwrap(); // already gone
    assert_eq!(
        read(&cfg),
        "\
# my stickpile config
server_url = \"https://b.example\" # tailnet

[ui]
max_results = 40

[clipboard.targets.telegram]
still = [\"text/uri-list\"]
",
        "the empty [copy] goes, [ui] stays"
    );
    assert_eq!(load_layered(&cfg).unwrap().config.ui.columns, 6);
}

#[test]
fn missing_config_is_created_on_first_write() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("sub/config.toml");
    let l = load_layered(&cfg).unwrap();
    assert_eq!(l.config, ClientConfig::default());
    assert_eq!(l.write_path, cfg);
    unset_value(&cfg, "ui.modal").unwrap();
    assert!(!cfg.exists(), "unset on a missing file writes nothing");
    set_value(&cfg, "ui.modal", toml::Value::Boolean(false)).unwrap();
    assert_eq!(read(&cfg), "[ui]\nmodal = false\n");
    assert!(!load_layered(&cfg).unwrap().config.ui.modal);
}

#[test]
fn invalid_values_are_refused_and_the_file_kept() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, "# keep me\n").unwrap();
    let bad = [
        ("ui.columns", toml::Value::Integer(0)),
        ("ui.columns", toml::Value::Boolean(true)),
        ("ui.start_tab", toml::Value::String("home".into())),
        ("copy.background", toml::Value::String("black".into())),
        ("server_url", toml::Value::String("ftp://x".into())),
        ("clipboard.default.still", toml::Value::Array(vec![])),
        ("nope", toml::Value::Integer(1)),
    ];
    for (key, value) in bad {
        let e = set_value(&cfg, key, value.clone()).unwrap_err();
        assert!(matches!(e, ConfigError::Invalid(_)), "{key} = {value}: {e}");
    }
    assert_eq!(read(&cfg), "# keep me\n");
}

#[test]
fn writes_follow_a_symlink_to_a_writable_file() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("dotfiles/stickpile.toml");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(&real, "timeout_secs = 3\n").unwrap();
    let link = dir.path().join("config.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(!is_read_only(&link));
    set_value(&link, "ui.thumb_px", toml::Value::Integer(96)).unwrap();
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert_eq!(read(&real), "timeout_secs = 3\n\n[ui]\nthumb_px = 96\n");
}

#[test]
fn read_only_detection() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let store = std::fs::canonicalize(&store).unwrap();
    let generated = store.join("abc-stickpile-client.toml");
    std::fs::write(&generated, "").unwrap();
    let link = dir.path().join("config.toml");
    std::os::unix::fs::symlink(&generated, &link).unwrap();
    assert!(read_only_with_store(&link, &store), "symlink into the store");
    assert!(read_only_with_store(&generated, &store), "a store path itself");
    assert!(!read_only_with_store(&dir.path().join("missing.toml"), &store));

    let dangling = dir.path().join("dangling.toml");
    std::os::unix::fs::symlink(dir.path().join("gone"), &dangling).unwrap();
    assert!(is_read_only(&dangling));

    let plain = dir.path().join("plain.toml");
    std::fs::write(&plain, "").unwrap();
    assert!(!is_read_only(&plain));
    if !running_as_root() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(is_read_only(&plain));
        let e = set_value(&plain, "ui.columns", toml::Value::Integer(3)).unwrap_err();
        assert!(matches!(e, ConfigError::ReadOnly { .. }), "{e}");
        let l = load_layered(&plain).unwrap();
        assert_eq!(l.write_path, dir.path().join(SETTINGS_FILE));
    }
}

#[test]
fn settings_toml_layers_under_a_read_only_config() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(
        &cfg,
        "server_url = \"https://hm.example\"\n[ui]\ncolumns = 8\n[[focus.rules]]\ncontains = [\"x\"]\ntarget = \"slack\"\n",
    )
    .unwrap();
    let settings = dir.path().join(SETTINGS_FILE);
    std::fs::write(
        &settings,
        "\
server_url = \"https://gui.example\"
timeout_secs = \"soon\"
bogus = 1
[ui]
columns = 3
thumb_px = 96
start_tab = \"nope\"
modal = false
[copy]
square = \"white\"
[clipboard.default]
still = [\"text/plain\"]
",
    )
    .unwrap();

    let l = load_layered_as(&cfg, true).unwrap();
    assert_eq!(l.write_path, settings);
    let locked: Vec<&str> = l.locked.iter().map(String::as_str).collect();
    assert_eq!(locked, ["focus.rules", "server_url", "ui.columns"]);
    let c = &l.config;
    assert_eq!(c.server_url, "https://hm.example", "config.toml wins");
    assert_eq!(c.ui.columns, 8);
    assert_eq!(c.ui.thumb_px, 96, "settings.toml over the default");
    assert!(!c.ui.modal);
    assert_eq!(
        c.copy,
        CopyConfig {
            square: SquarePad::White,
            background: Background::Keep
        }
    );
    assert_eq!(c.ui.start_tab, "last");
    assert_eq!(c.timeout_secs, 5);
    assert_eq!(c.focus.rules.len(), 1);
    assert_eq!(c.clipboard, ClientConfig::default().clipboard);
    assert_eq!(l.warnings.len(), 4, "{:#?}", l.warnings);
    for needle in ["bogus", "clipboard.default.still", "timeout_secs", "ui.start_tab"] {
        assert!(
            l.warnings
                .iter()
                .any(|w| w.contains(needle) && w.contains("settings.toml")),
            "{needle}: {:#?}",
            l.warnings
        );
    }

    // The GUI writes settings.toml; config.toml is never touched.
    let before = read(&cfg);
    set_value(&settings, "copy.background", toml::Value::String("white".into())).unwrap();
    unset_value(&settings, "ui.thumb_px").unwrap();
    let c = load_layered_as(&cfg, true).unwrap().config;
    assert_eq!(c.copy.background, Background::White);
    assert_eq!(c.ui.thumb_px, 128);
    assert_eq!(read(&cfg), before);
}

#[test]
fn new_settings_file_gets_a_header() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join(SETTINGS_FILE);
    set_value(&settings, "ui.start_tab", toml::Value::String("favorites".into())).unwrap();
    let text = read(&settings);
    assert!(text.starts_with("# Settings saved by the stickpile popup."), "{text}");
    assert!(text.ends_with("[ui]\nstart_tab = \"favorites\"\n"), "{text}");
    set_value(&settings, "ui.modal", toml::Value::Boolean(false)).unwrap();
    assert_eq!(read(&settings).matches("# Settings saved").count(), 1);
}

#[test]
fn broken_settings_toml_only_warns() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, "[ui]\ncolumns = 4\n").unwrap();
    std::fs::write(dir.path().join(SETTINGS_FILE), "[ui\ncolumns = ").unwrap();
    let l = load_layered_as(&cfg, true).unwrap();
    assert_eq!(l.config.ui.columns, 4);
    assert_eq!(l.warnings.len(), 1);
    assert!(l.warnings[0].contains("ignoring the file"), "{}", l.warnings[0]);

    // config.toml keeps its strict errors, read-only or not.
    std::fs::write(&cfg, "[ui]\ncolums = 4\n").unwrap();
    assert!(matches!(load_layered_as(&cfg, true), Err(ConfigError::Parse { .. })));
    std::fs::write(&cfg, "[ui]\nstart_tab = \"x\"\n").unwrap();
    assert!(matches!(load_layered(&cfg), Err(ConfigError::Invalid(_))));
}

#[test]
fn settings_toml_is_ignored_next_to_a_writable_config() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(dir.path().join(SETTINGS_FILE), "[ui]\ncolumns = 3\n").unwrap();
    let l = load_layered(&cfg).unwrap();
    assert_eq!(l.config.ui.columns, 6);
    assert_eq!(l.write_path, cfg);
    assert_eq!(l.warnings.len(), 1);
    assert!(l.warnings[0].contains("is ignored"), "{}", l.warnings[0]);
    // The standard loader is the layered one.
    assert_eq!(ClientConfig::load(&cfg).unwrap().ui.columns, 6);
}
