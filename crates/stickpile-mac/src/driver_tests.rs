//! The driver against a fake app and the demo backend (a directory of images).

use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::bridge::*;
use crate::driver::Driver;

#[derive(Default)]
struct Seen {
    renders: Vec<ViewState>,
    entries: Vec<EntryView>,
    packs: Vec<PackView>,
    presented: usize,
    hidden: usize,
    thumbnails: Vec<(String, String)>,
    clipboard: Vec<Vec<PasteboardData>>,
}

struct FakeUi {
    seen: Mutex<Seen>,
    tasks: Mutex<mpsc::Sender<Arc<MainTask>>>,
}

impl PopupUi for FakeUi {
    fn render(&self, view: ViewState) {
        let mut s = self.seen.lock().unwrap();
        if let Some(e) = &view.entries {
            s.entries = e.clone();
        }
        if let Some(p) = &view.packs {
            s.packs = p.clone();
        }
        s.renders.push(view);
    }
    fn present(&self) {
        self.seen.lock().unwrap().presented += 1;
    }
    fn hide(&self) {
        self.seen.lock().unwrap().hidden += 1;
    }
    fn thumbnail(&self, key: String, path: String) {
        self.seen.lock().unwrap().thumbnails.push((key, path));
    }
    fn set_clipboard(&self, items: Vec<PasteboardData>) -> Option<String> {
        self.seen.lock().unwrap().clipboard.push(items);
        None
    }
    fn open_url(&self, _url: String) {}
    fn page_key(&self, _key: PageKey) {}
    fn post(&self, task: Arc<MainTask>) {
        let _ = self.tasks.lock().unwrap().send(task);
    }
}

/// The language is one for the process: tests that start a driver take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

struct Harness {
    _serial: std::sync::MutexGuard<'static, ()>,
    ui: Arc<FakeUi>,
    tasks: mpsc::Receiver<Arc<MainTask>>,
    driver: Arc<Driver>,
    _dir: tempfile::TempDir,
    config: std::path::PathBuf,
}

fn png(path: &Path, rgb: [u8; 3]) {
    image::RgbaImage::from_pixel(64, 48, image::Rgba([rgb[0], rgb[1], rgb[2], 255]))
        .save(path)
        .unwrap();
}

impl Harness {
    fn new() -> Harness {
        let serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir_all(media.join("cats")).unwrap();
        png(&media.join("happy dog.png"), [200, 120, 0]);
        png(&media.join("cats/cat party.png"), [0, 120, 200]);
        png(&media.join("cats/cat nap.png"), [90, 90, 90]);
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "cache_dir = {:?}\ntoken_file = {:?}\n[ui]\nlanguage = \"en\"\nstart_tab = \"search\"\nstart_mode = \"insert\"\n",
                dir.path().join("cache"),
                dir.path().join("token")
            ),
        )
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let ui = Arc::new(FakeUi {
            seen: Mutex::new(Seen::default()),
            tasks: Mutex::new(tx),
        });
        let driver = Driver::new(
            StartOptions {
                config_path: Some(config.display().to_string()),
                demo_dir: Some(media.display().to_string()),
                log_file: None,
                system_languages: vec!["uk-UA".into()],
            },
            ui.clone(),
        );
        driver.start();
        Harness {
            _serial: serial,
            ui,
            tasks: rx,
            driver,
            _dir: dir,
            config,
        }
    }

    /// Run posted tasks, as the app's main thread would, until `done` holds.
    fn pump_until(&self, what: &str, done: impl Fn(&Seen) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(&self.ui.seen.lock().unwrap()) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}");
            if let Ok(task) = self.tasks.recv_timeout(left.min(Duration::from_millis(50))) {
                task.run();
            }
        }
    }

    fn last(&self) -> ViewState {
        self.ui.seen.lock().unwrap().renders.last().cloned().expect("rendered")
    }

    fn key(&self, code: u16, chars: &str) -> bool {
        self.driver.key(KeyInput {
            key_code: code,
            characters: chars.into(),
            latin: None,
            command: false,
            control: false,
            option: false,
            shift: false,
        })
    }
}

#[test]
fn search_copy_and_hide() {
    let h = Harness::new();
    let first = h.last();
    assert!(!first.visible && first.error.is_none(), "{:?}", first.error);
    assert_eq!(first.language, "en", "ui.language wins over the system's");

    h.driver.toggle(
        Some(FrontApp {
            bundle_id: "org.telegram.desktop".into(),
            name: "Telegram".into(),
        }),
        false,
    );
    assert_eq!(h.ui.seen.lock().unwrap().presented, 1);
    let v = h.last();
    assert!(v.visible && v.has_field);
    assert_eq!(v.tab, TabId::Search);
    assert_eq!(v.focus, FocusWish::Field);
    assert_eq!(v.footer.target, "Telegram");
    assert!(
        v.footer.target_hint.contains("org.telegram.desktop"),
        "{}",
        v.footer.target_hint
    );
    assert_eq!(v.tabs.len(), 7);

    h.driver.query_changed("cat".into());
    h.pump_until("search results", |s| s.entries.len() == 2);
    let names: Vec<String> =
        h.ui.seen
            .lock()
            .unwrap()
            .entries
            .iter()
            .map(|e| e.name.clone())
            .collect();
    assert!(names.iter().all(|n| n.contains("cat")), "{names:?}");
    h.pump_until("thumbnails", |s| s.thumbnails.len() >= 2);
    for (_, path) in &h.ui.seen.lock().unwrap().thumbnails {
        assert!(Path::new(path).exists(), "{path}");
    }

    // Right (one row of six holds both), then Return copies the second result.
    assert!(h.key(124, "\u{f703}"));
    assert_eq!(h.last().selected, Some(1));
    assert!(h.key(36, "\r"));
    h.pump_until("the copy", |s| !s.clipboard.is_empty());
    h.pump_until("hiding after the copy", |s| s.hidden == 1);
    let items = h.ui.seen.lock().unwrap().clipboard[0].clone();
    let kinds: Vec<&str> = items.iter().map(|i| i.kind.as_str()).collect();
    assert_eq!(kinds, ["public.png", "public.file-url"]);
    let url = String::from_utf8(items[1].data.clone()).unwrap();
    assert!(url.starts_with("file:///") && url.ends_with(".png"), "{url}");
    assert!(items[0].data.starts_with(b"\x89PNG"));
    let v = h.last();
    assert!(!v.visible);

    // Typing goes to the field; Escape goes to Normal mode, where j moves.
    h.driver.toggle(None, false);
    assert!(!h.key(0, "a"), "Insert: the field types it");
    assert!(h.key(53, "\u{1b}"));
    assert_eq!(h.last().focus, FocusWish::NoField);
    assert_eq!(h.last().footer.mode.as_deref(), Some("NORMAL"));
}

#[test]
fn tabs_pages_and_settings() {
    let h = Harness::new();
    h.driver.open(None, Some(TabId::Packs));
    h.pump_until("packs", |s| s.packs.len() == 2);
    assert_eq!(h.last().view, ViewKind::Packs);
    h.pump_until("pack covers", |s| s.thumbnails.len() == 2);

    h.driver.show_tab(TabId::Me);
    h.pump_until("the demo profile", |s| {
        s.renders.last().is_some_and(|r| r.me.account.is_some())
    });
    assert_eq!(h.last().view, ViewKind::Me);

    h.driver.show_tab(TabId::Settings);
    let v = h.last();
    assert_eq!(v.view, ViewKind::Settings);
    assert!(v.settings.error.is_none(), "{:?}", v.settings.error);
    assert!(!v.settings.groups.is_empty());

    h.driver
        .set_setting("ui.columns".into(), SettingValue::Int { value: 4 });
    let text = std::fs::read_to_string(&h.config).unwrap();
    assert!(text.contains("columns = 4"), "{text}");
    let v = h.last();
    assert!(v.settings.save_error.is_none(), "{:?}", v.settings.save_error);
    let row = v
        .settings
        .groups
        .iter()
        .flat_map(|g| &g.rows)
        .find(|r| r.key == "ui.columns")
        .unwrap()
        .clone();
    assert_eq!(row.value, SettingValue::Int { value: 4 });
    assert!(row.resettable);

    // Ukrainian at once, everywhere.
    h.driver
        .set_setting("ui.language".into(), SettingValue::Text { value: "uk".into() });
    let v = h.last();
    assert_eq!(v.language, "uk");
    assert_eq!(v.tabs[0].label, "Пошук");

    h.driver.toggle_help();
    let help = h.last().help.expect("help shown");
    assert!(help.iter().flat_map(|s| &s.keys).any(|k| k.keys.contains("Cmd+")));
    assert!(!help.iter().flat_map(|s| &s.keys).any(|k| k.keys.contains("Ctrl+")));
}

#[test]
fn grid_size_sets_the_columns() {
    let h = Harness::new();
    h.driver.open(None, None);
    // thumb_px 128 + 16 per cell.
    h.driver.grid_size(144.0 * 5.0 + 10.0, 144.0 * 3.0);
    assert_eq!(h.last().columns, 5);
    h.driver.grid_size(144.0 * 9.0, 600.0);
    assert_eq!(h.last().columns, 9);
}

#[test]
fn a_bad_config_is_shown() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "server_url = \"ftp://nope\"\n").unwrap();
    let (tx, _rx) = mpsc::channel();
    let ui = Arc::new(FakeUi {
        seen: Mutex::new(Seen::default()),
        tasks: Mutex::new(tx),
    });
    let driver = Driver::new(
        StartOptions {
            config_path: Some(config.display().to_string()),
            ..StartOptions::default()
        },
        ui.clone(),
    );
    driver.start();
    let v = ui.seen.lock().unwrap().renders.last().cloned().unwrap();
    assert!(v.error.unwrap().contains("server_url"));
}
