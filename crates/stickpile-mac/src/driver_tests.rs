//! The driver against a fake app and the demo backend (a directory of images).

use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use stickpile_api::{FieldSource, Visibility};
use stickpile_core::mock::MockServer;

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
    pastes: Vec<PasteRequest>,
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
    fn paste(&self, request: PasteRequest) {
        self.seen.lock().unwrap().pastes.push(request);
    }
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
        Harness::with(&[])
    }

    /// With `extra` images among the loose files.
    fn with(extra: &[&str]) -> Harness {
        let serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir_all(media.join("cats")).unwrap();
        png(&media.join("happy dog.png"), [200, 120, 0]);
        png(&media.join("cats/cat party.png"), [0, 120, 200]);
        png(&media.join("cats/cat nap.png"), [90, 90, 90]);
        for name in extra {
            png(&media.join(name), [10, 10, 10]);
        }
        Harness::start(serial, dir, "", Some(&media))
    }

    /// Against `server`, signed in as alice, with an empty cache.
    fn against(server: &MockServer) -> Harness {
        let serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let token = server.state().issue_token("alice", "mac test");
        stickpile_core::token::write_token(&dir.path().join("token"), &token).unwrap();
        Harness::start(serial, dir, &format!("server_url = {:?}\n", server.url), None)
    }

    /// [`Harness::against`] with the library in the cache already (synced
    /// before the start), so the offline index can answer.
    fn against_synced(server: &MockServer) -> Harness {
        let serial = serial();
        let dir = tempfile::tempdir().unwrap();
        let token = server.state().issue_token("alice", "mac test");
        stickpile_core::token::write_token(&dir.path().join("token"), &token).unwrap();
        let cfg = stickpile_core::ClientConfig {
            server_url: server.url.clone(),
            token_file: Some(dir.path().join("token")),
            cache_dir: Some(dir.path().join("cache")),
            ..stickpile_core::ClientConfig::default()
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let session = stickpile_core::Session::open(cfg).await.unwrap();
            assert!(session.sync().await.unwrap());
        });
        Harness::start(serial, dir, &format!("server_url = {:?}\n", server.url), None)
    }

    /// The driver on a config in `dir` that starts with `head`, for the
    /// demo files in `media` or else the configured server.
    fn start(
        serial: std::sync::MutexGuard<'static, ()>,
        dir: tempfile::TempDir,
        head: &str,
        media: Option<&Path>,
    ) -> Harness {
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "{head}cache_dir = {:?}\ntoken_file = {:?}\n[ui]\nlanguage = \"en\"\nstart_tab = \"search\"\nstart_mode = \"insert\"\n",
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
                demo_dir: media.map(|m| m.display().to_string()),
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

    /// Run posted tasks for `time`.
    fn pump_for(&self, time: Duration) {
        let deadline = Instant::now() + time;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
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
    // Cmd+V into Telegram, once it is frontmost again.
    assert_eq!(
        h.ui.seen.lock().unwrap().pastes,
        [PasteRequest {
            bundle_id: "org.telegram.desktop".into(),
            wait_ms: 1000,
            delay_ms: 50,
        }]
    );

    // Typing goes to the field; Escape goes to Normal mode, where j moves.
    h.driver.toggle(None, false);
    assert!(!h.key(0, "a"), "Insert: the field types it");
    assert!(h.key(53, "\u{1b}"));
    assert_eq!(h.last().focus, FocusWish::NoField);
    assert_eq!(h.last().footer.mode.as_deref(), Some("NORMAL"));
}

/// Open over `front`, search "cat", copy the selected result; wait for the copy.
fn pick_over(h: &Harness, front: Option<&str>) {
    let copies = h.ui.seen.lock().unwrap().clipboard.len();
    h.ui.seen.lock().unwrap().entries.clear();
    let front = front.map(|b| FrontApp {
        bundle_id: b.into(),
        name: "Chat".into(),
    });
    h.driver.toggle(front, false);
    h.driver.query_changed("cat".into());
    h.pump_until("search results", |s| s.entries.len() == 2);
    assert!(h.key(36, "\r"));
    h.pump_until("the copy", |s| s.clipboard.len() == copies + 1);
}

fn pastes(h: &Harness) -> Vec<PasteRequest> {
    h.ui.seen.lock().unwrap().pastes.clone()
}

#[test]
fn paste_after_a_pick_follows_the_settings() {
    let h = Harness::new();
    pick_over(&h, Some("com.hnc.Discord"));
    assert_eq!(pastes(&h).len(), 1);
    assert_eq!(pastes(&h)[0].bundle_id, "com.hnc.Discord");

    // Off in Settings: copy only.
    h.driver
        .set_setting("paste.auto".into(), SettingValue::Bool { value: false });
    pick_over(&h, Some("com.hnc.Discord"));
    assert_eq!(pastes(&h).len(), 1);

    // On, but the popup stays open after a copy: the focus is not going back.
    h.driver.reset_setting("paste.auto".into());
    h.driver
        .set_setting("ui.hide_on_copy".into(), SettingValue::Bool { value: false });
    pick_over(&h, Some("com.hnc.Discord"));
    assert!(h.last().visible);
    assert_eq!(pastes(&h).len(), 1);
    h.driver.hide();

    // No front app known: nothing to paste into.
    h.driver.reset_setting("ui.hide_on_copy".into());
    pick_over(&h, None);
    assert_eq!(pastes(&h).len(), 1);

    h.driver
        .set_setting("paste.delay_ms".into(), SettingValue::Int { value: 0 });
    pick_over(&h, Some("org.telegram.desktop"));
    assert_eq!(pastes(&h).len(), 2);
    assert_eq!(pastes(&h)[1].delay_ms, 0);
    assert_eq!(pastes(&h)[1].bundle_id, "org.telegram.desktop");

    // macOS always presses Cmd+V, so the shortcut row is not shown.
    h.driver.show_tab(TabId::Settings);
    let keys: Vec<String> = h
        .last()
        .settings
        .groups
        .iter()
        .flat_map(|g| &g.rows)
        .map(|r| r.key.clone())
        .collect();
    assert!(keys.contains(&"paste.auto".to_string()));
    assert!(!keys.contains(&"paste.shortcut".to_string()));
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

/// The syntax button (as F2) and the whole search syntax inside a pack.
#[test]
fn syntax_button_and_the_search_inside_a_pack() {
    let h = Harness::new();
    h.driver.open(None, Some(TabId::Search));
    assert!(h.last().placeholder.contains("v:tired"), "{}", h.last().placeholder);
    h.driver.toggle_syntax();
    let help = h.last().help.expect("the syntax shown");
    assert!(help.iter().flat_map(|s| &s.keys).any(|k| k.keys == "v: t: s: b: x:"));
    h.driver.toggle_syntax();
    assert!(h.last().help.is_none());

    h.driver.show_tab(TabId::Packs);
    h.pump_until("packs", |s| s.packs.len() == 2);
    let cats =
        h.ui.seen
            .lock()
            .unwrap()
            .packs
            .iter()
            .position(|p| p.name == "cats")
            .unwrap();
    h.driver.activate(cats as u32);
    h.pump_until("the pack's items", |s| s.entries.len() == 2);
    let v = h.last();
    assert!(v.back && v.has_field);
    assert!(v.placeholder.starts_with("Search in cats"), "{}", v.placeholder);
    h.driver.query_changed("-nap s:cats".into());
    h.pump_until("one cat", |s| s.entries.len() == 1);
    assert_eq!(h.ui.seen.lock().unwrap().entries[0].name, "cat party");
    h.driver.query_changed("b:nobody".into());
    h.pump_until("none", |s| s.entries.is_empty());
    h.driver.query_changed(String::new());
    h.pump_until("the pack again", |s| s.entries.len() == 2);
}

#[test]
fn safe_mode_hides_nsfw_items_and_is_saved() {
    let h = Harness::with(&["dog nsfw.png"]);
    h.driver.open(None, Some(TabId::Search));
    h.driver.query_changed("dog".into());
    h.pump_until("both dogs", |s| s.entries.len() == 2);
    let f = h.last().footer;
    assert!(!f.safe);
    assert_eq!(f.safe_label, "safe: off");
    assert_eq!(f.sticker_label, "sticker: off", "shown for every target");
    assert!(
        f.details.contains(" · Demo directory · PNG 64×48 · never copied"),
        "{}",
        f.details
    );

    h.driver.toggle_safe();
    h.pump_until("the search again, without nsfw", |s| s.entries.len() == 1);
    assert_eq!(h.ui.seen.lock().unwrap().entries[0].name, "happy dog");
    let f = h.last().footer;
    assert!(f.safe);
    assert_eq!(f.safe_label, "safe: on");
    let text = std::fs::read_to_string(&h.config).unwrap();
    assert!(text.contains("safe_mode = true"), "{text}");

    // S in Normal mode: back off.
    assert!(h.key(53, "\u{1b}"));
    assert!(h.driver.key(KeyInput {
        key_code: 1,
        characters: "S".into(),
        latin: None,
        command: false,
        control: false,
        option: false,
        shift: true,
    }));
    h.pump_until("both dogs again", |s| s.entries.len() == 2);
    assert!(!h.last().footer.safe);
    let text = std::fs::read_to_string(&h.config).unwrap();
    assert!(text.contains("safe_mode = false"), "{text}");
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

/// The first opening after a start finds the cache empty: a filter typed
/// before the sync is over gets its values when the sync brings them, with
/// the query as typed.
#[test]
fn a_sync_brings_the_values_to_a_filter_already_typed() {
    let server = MockServer::start();
    {
        let mut st = server.state();
        let set = st.add_set("alice", "cats", "Cats", Visibility::Public);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let item = st.add_item(&set.id, "grumpy cat", &["cat"], bytes.into_inner());
        let it = st.items.iter_mut().find(|i| i.item.id == item.id).unwrap();
        it.item.vibe = Some("sarcastic".into());
    }
    let h = Harness::against(&server);
    h.driver.toggle(None, false);
    h.driver.query_changed("v:".into());
    h.pump_until("the values the sync brought", |s| {
        s.renders
            .last()
            .and_then(|v| v.suggestions.as_ref())
            .is_some_and(|l| l.words == ["sarcastic"])
    });
    let v = h.last();
    assert_eq!(v.query, "v:");
    assert!(v.visible);
}

/// The words in the picture: in the tooltip with their line breaks, and
/// on the tile as one line once `zx` turns it on (off by default), saved
/// as `ui.show_text`.
#[test]
fn the_text_is_in_the_tooltip_and_zx_draws_it_on_the_tile() {
    let server = MockServer::start();
    {
        let mut st = server.state();
        let set = st.add_set("alice", "cats", "Cats", Visibility::Public);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let item = st.add_item(&set.id, "empty head", &["cat"], bytes.into_inner());
        st.set_text(&item.id, "NO THOUGHTS\nhead empty", FieldSource::Ai);
    }
    let h = Harness::against(&server);
    h.driver.toggle(None, false);
    h.driver.query_changed("cat".into());
    h.pump_until("the item", |s| s.entries.len() == 1);
    let v = h.last();
    assert!(!v.tiles.text, "off by default");
    let e = h.ui.seen.lock().unwrap().entries[0].clone();
    assert_eq!(e.text, "");
    assert!(e.tooltip.ends_with("\n\nNO THOUGHTS\nhead empty"), "{}", e.tooltip);

    assert!(h.key(53, "\u{1b}"));
    assert!(h.key(6, "z"));
    assert!(h.key(7, "x"));
    h.pump_until("the text on the tile", |s| {
        s.entries.first().is_some_and(|e| e.text == "NO THOUGHTS / head empty")
    });
    assert!(h.last().tiles.text);
    let config = std::fs::read_to_string(&h.config).unwrap();
    assert!(config.contains("show_text = true"), "{config}");
}

/// `e:` offers the emojis with their names, for the bar to draw them
/// side by side.
#[test]
fn emoji_offers_carry_their_names() {
    let server = MockServer::start();
    {
        let mut st = server.state();
        let set = st.add_set("alice", "cats", "Cats", Visibility::Public);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let item = st.add_item(&set.id, "dead", &["dead"], bytes.into_inner());
        let it = st.items.iter_mut().find(|i| i.item.id == item.id).unwrap();
        it.item.emojis = vec!["💀".into()];
    }
    let h = Harness::against(&server);
    h.driver.toggle(None, false);
    h.driver.query_changed("e:sk".into());
    h.pump_until("the emojis named sk", |s| {
        s.renders
            .last()
            .and_then(|v| v.suggestions.as_ref())
            .is_some_and(|l| l.words.first().is_some_and(|w| w == "💀"))
    });
    let line = h.last().suggestions.unwrap();
    assert_eq!(line.labels[..2], ["skull", "skull and crossbones"]);
    assert_eq!(line.labels.len(), line.words.len());
    assert_eq!(line.chosen, None, "a name is searched as typed");
}

// -- keeping offline (docs/offline.md) ------------------------------------------------

fn png_bytes(rgb: [u8; 3]) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(8, 8, image::Rgba([rgb[0], rgb[1], rgb[2], 255]))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

/// alice's "Cats" (grumpy cat, sleepy cat) and "Dogs" (good dog).
fn kennel() -> MockServer {
    let server = MockServer::start();
    {
        let mut st = server.state();
        let cats = st.add_set("alice", "cats", "Cats", Visibility::Public);
        st.add_item(&cats.id, "grumpy cat", &["cat"], png_bytes([1, 2, 3]));
        st.add_item(&cats.id, "sleepy cat", &["cat"], png_bytes([4, 5, 6]));
        let dogs = st.add_set("alice", "dogs", "Dogs", Visibility::Public);
        st.add_item(&dogs.id, "good dog", &["dog"], png_bytes([7, 8, 9]));
    }
    server
}

fn shift_key(code: u16, chars: &str) -> KeyInput {
    KeyInput {
        key_code: code,
        characters: chars.into(),
        latin: None,
        command: false,
        control: false,
        option: false,
        shift: true,
    }
}

fn status(h: &Harness) -> String {
    h.last().footer.status
}

fn marks(s: &Seen) -> Vec<(String, OfflineMark)> {
    s.entries.iter().map(|e| (e.name.clone(), e.offline)).collect()
}

fn block(s: &Seen) -> Option<&OfflineView> {
    s.renders.last().and_then(|v| v.settings.offline.as_ref())
}

/// `K` keeps the selected sticker, the downloader fetches it, and its
/// tile gets the mark; the menu says why it is kept and releases it.
#[test]
fn k_keeps_a_sticker_and_the_menu_releases_it() {
    let server = kennel();
    let h = Harness::against(&server);
    h.driver.toggle(None, false);
    h.driver.query_changed("cat".into());
    h.pump_until("the cats", |s| s.entries.len() == 2);
    assert!(h.last().tiles.offline, "the marks are on by default");
    assert!(h.key(53, "\u{1b}"));
    assert!(h.driver.key(shift_key(40, "K")));
    h.pump_until("the keep", |s| {
        s.renders
            .last()
            .is_some_and(|v| v.footer.status.ends_with("is kept offline"))
    });
    let kept = h.last().selected.unwrap() as usize;
    let name = h.ui.seen.lock().unwrap().entries[kept].name.clone();
    assert_eq!(status(&h), format!("{name} is kept offline"));
    h.pump_until("its file on the device, marked", |s| {
        s.entries
            .iter()
            .any(|e| e.name == name && e.offline == OfflineMark::Available)
    });

    h.driver.open_menu(None);
    h.pump_until("the menu", |s| s.renders.last().is_some_and(|v| v.menu.is_some()));
    let menu = h.last().menu.unwrap();
    assert_eq!(menu.index as usize, kept);
    let labels: Vec<(&str, Option<MenuChoice>)> = menu.entries.iter().map(|e| (e.label.as_str(), e.action)).collect();
    assert!(labels.contains(&("Copy", Some(MenuChoice::Copy))), "{labels:?}");
    assert!(
        labels.contains(&("Stop keeping offline", Some(MenuChoice::Release))),
        "{labels:?}"
    );
    assert!(labels.contains(&("Available offline", None)), "{labels:?}");
    let release = menu
        .entries
        .iter()
        .find(|e| e.action == Some(MenuChoice::Release))
        .unwrap();
    assert_eq!(release.keys, "K");
    h.driver.menu_choose(MenuChoice::Release);
    assert!(h.last().menu.is_none());
    h.pump_until("the release", |s| {
        s.renders
            .last()
            .is_some_and(|v| v.footer.status.ends_with("is no longer kept offline"))
    });

    // zo hides the marks and saves ui.show_offline.
    assert!(h.key(6, "z"));
    assert!(h.key(31, "o"));
    assert!(!h.last().tiles.offline);
    h.pump_until("the marks gone", |s| {
        s.entries.iter().all(|e| e.offline == OfflineMark::None)
    });
    let config = std::fs::read_to_string(&h.config).unwrap();
    assert!(config.contains("show_offline = false"), "{config}");
}

/// The Settings page's block: usage and the downloader, the library switch
/// (after the question), clear cache, "Remove kept files" (after the
/// question).
#[test]
fn the_settings_block_keeps_the_library_and_removes_the_kept_files() {
    let server = kennel();
    let h = Harness::against(&server);
    h.driver.open(None, Some(TabId::Settings));
    h.pump_until("the block with the usage", |s| {
        block(s).is_some_and(|o| !o.kept.is_empty())
    });
    let v = h.last();
    let before = v.settings.offline_before.unwrap() as usize;
    assert_eq!(v.settings.groups[before].title, "Offline");
    let o = v.settings.offline.unwrap();
    assert!(!o.library);
    assert_eq!(o.library_label, "Keep everything offline");
    assert!(o.kept.ends_with("of 1.0 GiB allowed."), "{}", o.kept);

    // The sync on opening ran the downloader: the thumbnails are kept.
    h.pump_until("a run of the downloader", |s| {
        block(s).is_some_and(|o| o.state.starts_with("Up to date"))
    });

    h.driver.offline_keep_library(true);
    h.pump_until("the question", |s| {
        s.renders.last().is_some_and(|v| v.confirm.is_some())
    });
    let c = h.last().confirm.unwrap();
    assert_eq!(c.title, "Keep the whole library offline?");
    assert!(c.body.starts_with("3 stickers, "), "{}", c.body);
    assert!(!c.destructive);
    assert!(!h.last().settings.offline.unwrap().library, "not before the yes");
    h.driver.confirm(true);
    h.pump_until("the library kept", |s| {
        s.renders.last().is_some_and(|v| {
            v.footer.status == "The whole library is kept offline"
                && v.settings.offline.as_ref().is_some_and(|o| o.library)
        })
    });
    h.pump_until("everything downloaded", |s| {
        block(s).is_some_and(|o| o.state.starts_with("Up to date") && o.kept.starts_with("Kept offline: 6 files"))
    });

    // Clear cache leaves the kept files and says so.
    h.driver.clear_cache();
    h.pump_until("the cache cleared", |s| {
        block(s).is_some_and(|o| o.message.starts_with("Cleared ") && o.message.ends_with("6 kept files stayed."))
    });

    h.driver.remove_kept_files();
    let c = h.last().confirm.expect("asked first");
    assert!(c.destructive);
    assert!(c.body.starts_with("6 kept files"), "{}", c.body);
    h.driver.confirm(true);
    h.pump_until("the kept files removed", |s| {
        s.renders
            .last()
            .is_some_and(|v| v.footer.status.starts_with("Removed 6 kept files"))
    });
    assert!(!h.last().settings.offline.unwrap().library);
}

/// With the server gone, a tile whose file is not on the device is
/// dimmed, and choosing it says why it does not copy; a kept one copies.
#[test]
fn unreachable_dims_what_is_not_here_and_explains_it() {
    let mut server = kennel();
    let h = Harness::against(&server);
    h.driver.toggle(None, false);
    h.driver.query_changed("cat".into());
    h.pump_until("the cats", |s| s.entries.len() == 2);
    assert!(h.key(53, "\u{1b}"));
    assert!(h.driver.key(shift_key(40, "K")));
    let kept = h.ui.seen.lock().unwrap().entries[0].name.clone();
    h.pump_until("the kept cat on the device", |s| {
        s.entries
            .iter()
            .any(|e| e.name == kept && e.offline == OfflineMark::Available)
    });
    h.driver.hide();

    server.kill();
    h.driver.toggle(None, false);
    h.driver.query_changed("cat".into());
    h.pump_until("the other cat dimmed", |s| {
        let m = marks(s);
        m.len() == 2
            // Available tiles are plain while the server is gone (as on Android).
            && m.contains(&(kept.clone(), OfflineMark::None))
            && m.iter().any(|(_, o)| *o == OfflineMark::Unavailable)
    });
    let (other, other_name, tooltip) = {
        let s = h.ui.seen.lock().unwrap();
        let i = s
            .entries
            .iter()
            .position(|e| e.offline == OfflineMark::Unavailable)
            .unwrap();
        (i, s.entries[i].name.clone(), s.entries[i].tooltip.clone())
    };
    assert!(
        tooltip.contains("Not on this device, and the server is unreachable"),
        "{tooltip}"
    );

    let copies = h.ui.seen.lock().unwrap().clipboard.len();
    h.driver.activate(other as u32);
    h.pump_until("why it does not copy", |s| {
        s.renders
            .last()
            .is_some_and(|v| v.footer.status.contains("is not on this device"))
    });
    assert_eq!(
        status(&h),
        format!(
            "Copy failed: {other_name} is not on this device and the server is unreachable; keep it offline (K) to copy it without the server"
        )
    );
    assert_eq!(h.last().footer.level, Level::Error);
    assert_eq!(h.ui.seen.lock().unwrap().clipboard.len(), copies);

    let kept_at =
        h.ui.seen
            .lock()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == kept)
            .unwrap();
    h.driver.activate(kept_at as u32);
    h.pump_until("the kept cat copied", |s| s.clipboard.len() == copies + 1);
}

// -- results without waiting (docs/offline.md) ------------------------------------------

const NOTICE: &str = "Offline: showing saved stickers";

fn notice_with(s: &Seen, n: usize) -> bool {
    s.entries.len() == n && s.renders.last().is_some_and(|v| v.footer.status == NOTICE)
}

/// A slow server: the offline index's results first with the notice (not
/// "Searching..." until the server answers), then the server's in their
/// place, the selected sticker still selected.
#[test]
fn offline_results_first_then_the_servers_in_place() {
    let server = kennel();
    let h = Harness::against_synced(&server);
    server.state().search_delay = Some(Duration::from_millis(1500));
    h.driver.toggle(None, false);
    let start = Instant::now();
    h.driver.query_changed("cat".into());
    h.pump_until("the offline results", |s| notice_with(s, 2));
    let first_at = start.elapsed();
    assert!(first_at < Duration::from_millis(1000), "{first_at:?}");
    assert_eq!(h.last().footer.level, Level::Warning);
    assert!(h.key(124, "\u{f703}"));
    assert_eq!(h.last().selected, Some(1));
    let chosen = h.ui.seen.lock().unwrap().entries[1].id.clone();

    h.pump_until("the server's results", |s| {
        s.renders.last().is_some_and(|v| v.footer.status.is_empty())
    });
    assert!(start.elapsed() >= Duration::from_millis(1500));
    let selected = h.last().selected.expect("selected") as usize;
    assert_eq!(
        h.ui.seen.lock().unwrap().entries[selected].id,
        chosen,
        "the selected sticker stays selected"
    );
}

/// Opening the popup checks the server. With it gone a search shows the
/// offline results and the notice at once; once the check of the next
/// opening finds it back, a sync starts at once (the schedule would wait).
#[test]
fn opening_checks_the_server_and_its_return_starts_a_sync() {
    let mut server = kennel();
    let h = Harness::against_synced(&server);
    h.driver.toggle(None, false);
    h.pump_until("the check on opening", |_| {
        server.state().requests.iter().any(|r| r == "GET /healthz")
    });

    server.kill();
    let start = Instant::now();
    h.driver.query_changed("cat".into());
    h.pump_until("the offline results", |s| notice_with(s, 2));
    assert!(start.elapsed() < Duration::from_millis(1000), "{:?}", start.elapsed());
    h.pump_for(Duration::from_millis(300));

    server.restart();
    server.state().requests.clear();
    h.driver.toggle(None, true);
    h.driver.toggle(None, false);
    h.pump_until("a sync once the server is back", |_| {
        let st = server.state();
        st.requests.iter().any(|r| r == "GET /healthz") && st.requests.iter().any(|r| r == "GET /api/v1/me")
    });
}
