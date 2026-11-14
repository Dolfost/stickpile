//! What the Swift app sees, through UniFFI.
//!
//! The split is the GTK client's: the popup's behaviour lives in Rust (the
//! `stickpile-quick` model, driven by [`crate::driver::Driver`]), and the
//! app only draws [`ViewState`] and reports what the user does.
//!
//! * Swift owns a [`Popup`] and calls its methods for input: keys, typing,
//!   clicks, the global shortcut, the Me and Settings pages.
//! * Rust calls back through [`PopupUi`], which Swift implements: draw a
//!   new [`ViewState`], show or hide the window, a thumbnail is ready, put
//!   these bytes on the pasteboard, open a URL, and run a [`MainTask`] on
//!   the main thread.
//!
//! **Threads.** Every [`Popup`] method must be called on the main thread,
//! and [`PopupUi`] is only called on the main thread too: background work
//! (network, files) ends with [`PopupUi::post`], whose task the app runs on
//! the main thread. So the driver is single-threaded like the GTK shell.
//! [`PopupUi::render`] may be called from inside a [`Popup`] method; the
//! app must not call back into [`Popup`] from inside `render` (it updates
//! its views only; a text field changed by `render` is not user input).

use std::sync::{Arc, Mutex};

use crate::driver::Driver;

/// The popup's tabs, in tab bar order (`1`..`7`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum TabId {
    Search,
    Recent,
    MostUsed,
    Packs,
    Favorites,
    Me,
    Settings,
}

impl From<stickpile_quick::Tab> for TabId {
    fn from(t: stickpile_quick::Tab) -> Self {
        use stickpile_quick::Tab;
        match t {
            Tab::Search => TabId::Search,
            Tab::Recent => TabId::Recent,
            Tab::MostUsed => TabId::MostUsed,
            Tab::Packs => TabId::Packs,
            Tab::Favorites => TabId::Favorites,
            Tab::Me => TabId::Me,
            Tab::Settings => TabId::Settings,
        }
    }
}

impl From<TabId> for stickpile_quick::Tab {
    fn from(t: TabId) -> Self {
        use stickpile_quick::Tab;
        match t {
            TabId::Search => Tab::Search,
            TabId::Recent => Tab::Recent,
            TabId::MostUsed => Tab::MostUsed,
            TabId::Packs => Tab::Packs,
            TabId::Favorites => Tab::Favorites,
            TabId::Me => Tab::Me,
            TabId::Settings => Tab::Settings,
        }
    }
}

/// One button of the tab bar.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TabLabel {
    pub tab: TabId,
    /// In the current language.
    pub label: String,
    /// 1..=7
    pub number: u8,
}

/// What the main area shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ViewKind {
    /// The item grid ([`ViewState::entries`]).
    Items,
    /// The pack list ([`ViewState::packs`]).
    Packs,
    /// The Me page ([`ViewState::me`]).
    Me,
    /// The Settings page ([`ViewState::settings`]).
    Settings,
}

/// Where the keyboard focus belongs after a render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FocusWish {
    /// The tab's text field (search, pack filter), Insert mode.
    Field,
    /// No text field: Normal mode. Take the focus out of any text field.
    NoField,
    /// Leave the focus where it is (Insert on the Me / Settings pages, where
    /// the user picked a field).
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Level {
    Info,
    /// Something is in progress (searching, preparing a copy, loading).
    Busy,
    Warning,
    Error,
}

/// The footer's sticker toggle (Cmd+S).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum StickerState {
    On,
    Off,
    /// The target app has no stickers: hide the toggle.
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Footer {
    /// `INSERT` / `NORMAL`; `None` with modes off (`ui.modal = false`).
    pub mode: Option<String>,
    /// The mode is Insert (accent colour for the mode label).
    pub insert: bool,
    /// Keys of an unfinished Normal-mode command ("3", "g"), like vim's showcmd.
    pub pending: String,
    /// The target app: "Telegram".
    pub target: String,
    /// How it was chosen: "auto: org.telegram.desktop", "default", "chosen".
    pub target_hint: String,
    pub sticker: StickerState,
    /// "sticker: on" / "sticker: off", translated.
    pub sticker_label: String,
    pub status: String,
    pub level: Level,
}

/// One cell of the item grid.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EntryView {
    /// The item id; also the key of its thumbnail ([`PopupUi::thumbnail`]).
    pub id: String,
    pub name: String,
    /// "GIF", "WEBP", "ANIM", "VIDEO" for anything that moves.
    pub badge: Option<String>,
    /// Several lines: name, pack, emojis and keywords, why it does not fit.
    pub tooltip: String,
    /// The item cannot go to the target app as it is: draw it dimmed.
    pub incompatible: bool,
    /// Draw a heart (only when [`Tiles::favorites`] is on).
    pub liked: bool,
    /// The item's emojis, joined (only when [`Tiles::emojis`] is on).
    pub emojis: String,
    /// The item's tags, comma separated (only when [`Tiles::tags`] is on).
    pub tags: String,
}

/// One cell of the pack list.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PackView {
    pub id: String,
    /// Key of the cover image ([`PopupUi::thumbnail`]); changes with the cover.
    pub cover_key: String,
    pub name: String,
    /// "12 items · alice", translated.
    pub detail: String,
}

/// What the item tiles show besides the thumbnail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct Tiles {
    pub favorites: bool,
    pub emojis: bool,
    pub tags: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct HelpKey {
    /// "h j k l, arrows", with macOS key names (Cmd, Option).
    pub keys: String,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct HelpSection {
    pub title: String,
    pub keys: Vec<HelpKey>,
}

/// One number of the Me page ("124" / "copies").
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Stat {
    pub value: String,
    pub label: String,
}

/// The Me page when signed in.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MeAccount {
    pub server_url: String,
    /// Show a button that opens the web UI ([`Popup::open_server`]).
    pub open_server: bool,
    /// How requests are authenticated ("API token", "Tailnet identity").
    pub auth: String,
    /// The tailnet login the account is bound to; `None`: hide the row.
    pub tailnet: Option<String>,
    /// "On" / "Off", translated.
    pub two_factor: String,
    pub stats: Vec<Stat>,
    /// Show "Log out" ([`Popup::logout`]).
    pub logout: bool,
    /// Show "Store a token" ([`Popup::login_tailnet`]): signed in by tailnet
    /// identity, a token would keep working off the tailnet.
    pub store_token: bool,
}

/// The login form when signed out.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MeForm {
    /// "Server: http://...", translated.
    pub description: String,
    /// Show the two-factor code field.
    pub totp: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Message {
    pub text: String,
    pub error: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MeView {
    /// Fetching the profile for the first time: show a spinner and "Loading...".
    pub loading: bool,
    /// Name and detail line above everything; `None` before the first profile.
    pub who: Option<String>,
    pub detail: String,
    pub account: Option<MeAccount>,
    pub form: Option<MeForm>,
    /// Show "Log in with tailnet" ([`Popup::login_tailnet`]).
    pub tailnet_login: bool,
    /// A login or logout is running: disable the buttons.
    pub busy: bool,
    pub message: Option<Message>,
    /// Show "Try again" ([`Popup::retry_profile`]).
    pub retry: bool,
    /// Clear the password and code fields (after a login went through).
    /// Set in one render only.
    pub clear_secrets: bool,
}

/// A value of a setting.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum SettingValue {
    Bool { value: bool },
    Int { value: i64 },
    Text { value: String },
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ChoiceOption {
    /// What is saved.
    pub value: String,
    /// What is shown, translated.
    pub label: String,
}

/// How a setting is edited.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum SettingKind {
    /// A switch.
    Bool,
    /// A number field or stepper within the range.
    Int { min: i64, max: i64 },
    /// A drop-down.
    Choice { options: Vec<ChoiceOption> },
    /// A text field, saved on Return or when it loses the focus.
    Text,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SettingRow {
    /// Dotted key in `config.toml`: `ui.columns`.
    pub key: String,
    pub label: String,
    /// One line; says why when the row is locked.
    pub help: String,
    pub kind: SettingKind,
    pub value: SettingValue,
    /// Set in a read-only `config.toml`: draw it disabled.
    pub locked: bool,
    /// Differs from the default: show a reset button ([`Popup::reset_setting`]).
    pub resettable: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SettingsGroup {
    pub title: String,
    pub rows: Vec<SettingRow>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SettingsView {
    /// The settings could not be read: show this instead of the rows.
    pub error: Option<String>,
    /// Where changes are saved, one line.
    pub note: String,
    /// The full path of that file (tooltip).
    pub path: String,
    /// Entries of `settings.toml` that were skipped.
    pub warnings: Vec<String>,
    pub groups: Vec<SettingsGroup>,
    /// The last change could not be saved.
    pub save_error: Option<String>,
}

/// Everything the window shows. Sent whole on every change, except the two
/// lists, which are only sent when they changed.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ViewState {
    /// The window should be on screen (it is shown and hidden through
    /// [`PopupUi::present`] / [`PopupUi::hide`]; this is for the drawing).
    pub visible: bool,
    /// The popup cannot work (a bad `config.toml`, no cache): show only this,
    /// with the config path below it.
    pub error: Option<String>,
    pub config_path: String,
    /// The current language's code (`en`, `uk`); every text in here is in it.
    pub language: String,
    pub tabs: Vec<TabLabel>,
    pub tab: TabId,
    pub view: ViewKind,
    pub focus: FocusWish,
    /// Title of the header row when the tab has no text field ("Recent",
    /// the opened pack's name).
    pub title: String,
    /// Show the text field (Search, Packs).
    pub has_field: bool,
    /// The field's text. Set it when it differs; that is not user input.
    pub query: String,
    pub placeholder: String,
    /// "24 items", shown in the header of the tabs without a field.
    pub count: String,
    /// Show a back button (an opened pack; [`Popup::pack_back`]).
    pub back: bool,
    /// Changes whenever the entries or the packs change.
    pub entries_revision: u64,
    /// The item grid; `None`: unchanged since the last render.
    pub entries: Option<Vec<EntryView>>,
    /// The pack list; `None`: unchanged since the last render.
    pub packs: Option<Vec<PackView>>,
    /// The selected cell of the grid shown (items or packs); scroll it into view.
    pub selected: Option<u32>,
    /// Columns the model navigates with; lay the grid out with this many.
    pub columns: u32,
    /// Thumbnail size in points (a cell is this plus [`CELL_EXTRA`]).
    pub thumb_px: u32,
    pub tiles: Tiles,
    /// The key help overlay, when it is shown (F1, `?`).
    pub help: Option<Vec<HelpSection>>,
    pub footer: Footer,
    pub me: MeView,
    pub settings: SettingsView,
}

/// Padding around a thumbnail in a grid cell, in points.
pub const CELL_EXTRA: u32 = stickpile_desktop::glue::CELL_EXTRA_PX;

/// A key press, as `NSEvent` describes it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct KeyInput {
    /// `NSEvent.keyCode` (a virtual key code: 36 Return, 53 Escape, ...).
    pub key_code: u16,
    /// `NSEvent.charactersIgnoringModifiers` (Shift still applies).
    pub characters: String,
    /// The same physical key in an ASCII-capable layout without modifiers
    /// (`UCKeyTranslate` with `TISCopyCurrentASCIICapableKeyboardLayoutInputSource`),
    /// so Cmd+T and the vim keys work with a Cyrillic layout active.
    pub latin: Option<String>,
    pub command: bool,
    pub control: bool,
    pub option: bool,
    pub shift: bool,
}

/// The frontmost application when the shortcut was pressed
/// (`NSWorkspace.shared.frontmostApplication`).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FrontApp {
    /// `org.telegram.desktop`
    pub bundle_id: String,
    /// `Telegram`
    pub name: String,
}

/// One representation of the copied item for `NSPasteboardItem.setData(_:forType:)`.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PasteboardData {
    /// A pasteboard type (UTI): `public.png`, `public.file-url`, ...
    pub kind: String,
    pub data: Vec<u8>,
}

/// A Normal-mode key for the Me / Settings page: move the page's own focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PageKey {
    Up,
    Down,
    Left,
    Right,
    /// Press the focused button or flip the switch.
    Activate,
    /// Put the focus into the text field of the focused row.
    Edit,
}

impl From<stickpile_quick::PageKey> for PageKey {
    fn from(k: stickpile_quick::PageKey) -> Self {
        use stickpile_quick::PageKey as K;
        match k {
            K::Up => PageKey::Up,
            K::Down => PageKey::Down,
            K::Left => PageKey::Left,
            K::Right => PageKey::Right,
            K::Activate => PageKey::Activate,
            K::Edit => PageKey::Edit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct StartOptions {
    /// `config.toml`; `None`: `~/Library/Application Support/stickpile/config.toml`.
    pub config_path: Option<String>,
    /// Serve the media files of this directory instead of a server (`--demo DIR`).
    pub demo_dir: Option<String>,
    /// Append the log here; `None`: stderr.
    pub log_file: Option<String>,
    /// The system's preferred languages, most preferred first
    /// (`Locale.preferredLanguages`: `uk-UA`, `en-US`); `ui.language = "auto"`
    /// uses them when the account has no language.
    pub system_languages: Vec<String>,
}

/// The app's side, called by Rust on the main thread.
#[uniffi::export(foreign)]
pub trait PopupUi: Send + Sync {
    /// Draw this state.
    fn render(&self, view: ViewState);
    /// Show the window and make it key (without activating the app: the
    /// chat app stays frontmost).
    fn present(&self);
    fn hide(&self);
    /// The image of `key` (an item id, or a pack's [`PackView::cover_key`]) is
    /// in this file (PNG, WebP, GIF, JPEG).
    fn thumbnail(&self, key: String, path: String);
    /// Replace the pasteboard's contents with one item carrying all these
    /// representations. `None` when it worked, else what went wrong.
    fn set_clipboard(&self, items: Vec<PasteboardData>) -> Option<String>;
    fn open_url(&self, url: String);
    /// A Normal-mode key for the Me / Settings page.
    fn page_key(&self, key: PageKey);
    /// Run `task.run()` on the main thread, soon (`DispatchQueue.main.async`).
    /// Called from any thread.
    fn post(&self, task: Arc<MainTask>);
}

/// Work for the main thread; see [`PopupUi::post`]. Running it twice does nothing.
#[derive(uniffi::Object)]
pub struct MainTask {
    f: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl MainTask {
    pub fn new(f: impl FnOnce() + Send + 'static) -> Arc<MainTask> {
        Arc::new(MainTask {
            f: Mutex::new(Some(Box::new(f))),
        })
    }
}

#[uniffi::export]
impl MainTask {
    pub fn run(&self) {
        let f = self.f.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(f) = f {
            f();
        }
    }
}

/// The popup. Create it once at launch, then [`Popup::start`].
#[derive(uniffi::Object)]
pub struct Popup {
    driver: Arc<Driver>,
}

#[uniffi::export]
impl Popup {
    #[uniffi::constructor]
    pub fn new(options: StartOptions, ui: Arc<dyn PopupUi>) -> Arc<Popup> {
        Arc::new(Popup {
            driver: Driver::new(options, ui),
        })
    }

    /// Open the session and draw the first state (the window stays hidden).
    pub fn start(&self) {
        self.driver.start();
    }

    /// The global shortcut: show the popup, or hide it when it has the
    /// keyboard (`popup_active`). `front` is the frontmost app before the
    /// popup showed; it picks the target app.
    pub fn toggle(&self, front: Option<FrontApp>, popup_active: bool) {
        self.driver.toggle(front, popup_active);
    }

    /// Show the popup (the menu bar item), on `tab` if given.
    pub fn open(&self, front: Option<FrontApp>, tab: Option<TabId>) {
        self.driver.open(front, tab);
    }

    /// Hide the popup (the menu bar item).
    pub fn hide(&self) {
        self.driver.hide();
    }

    /// The window went away by other means (it lost the key focus, it was closed).
    pub fn hidden(&self) {
        self.driver.hidden();
    }

    /// A key press in the window, before any text field sees it. `true`: the
    /// popup used it, swallow it; `false`: let the focused control have it.
    pub fn key(&self, key: KeyInput) -> bool {
        self.driver.key(key)
    }

    /// The user changed the text field.
    pub fn query_changed(&self, text: String) {
        self.driver.query_changed(text);
    }

    /// A click on a grid cell (items or packs, whichever is shown).
    pub fn select(&self, index: u32) {
        self.driver.select(index);
    }

    /// A double click on a grid cell: copy the item, or open the pack.
    pub fn activate(&self, index: u32) {
        self.driver.activate(index);
    }

    /// A click on the tab bar.
    pub fn show_tab(&self, tab: TabId) {
        self.driver.show_tab(tab);
    }

    pub fn toggle_help(&self) {
        self.driver.toggle_help();
    }

    /// A click on the footer's sticker toggle.
    pub fn toggle_sticker(&self) {
        self.driver.toggle_sticker();
    }

    /// A click on the footer's target app: the next one.
    pub fn cycle_target(&self) {
        self.driver.cycle_target();
    }

    /// The back button of an opened pack.
    pub fn pack_back(&self) {
        self.driver.pack_back();
    }

    /// A text field got (`true`) or lost (`false`) the keyboard focus by the
    /// user's doing (a click); not when `render` moved it.
    pub fn text_focus(&self, focused: bool) {
        self.driver.text_focus(focused);
    }

    /// The visible size of the grid's scroll area, in points, whenever it changes.
    pub fn grid_size(&self, width: f64, height: f64) {
        self.driver.grid_size(width, height);
    }

    /// The window's size for the config (`ui.columns`, `ui.thumb_px`).
    pub fn window_size(&self) -> Size {
        self.driver.window_size()
    }

    // -- Me ----------------------------------------------------------------------

    pub fn login(&self, username: String, password: String, totp: String) {
        self.driver.login(username, password, totp);
    }

    /// "Log in with tailnet", or "Store a token" when signed in by tailnet identity.
    pub fn login_tailnet(&self) {
        self.driver.login_tailnet();
    }

    pub fn logout(&self) {
        self.driver.logout();
    }

    pub fn retry_profile(&self) {
        self.driver.retry_profile();
    }

    /// Open the server's web UI.
    pub fn open_server(&self) {
        self.driver.open_server();
    }

    // -- Settings ----------------------------------------------------------------

    pub fn set_setting(&self, key: String, value: SettingValue) {
        self.driver.set_setting(key, value);
    }

    pub fn reset_setting(&self, key: String) {
        self.driver.reset_setting(key);
    }
}

/// `text` in the current language (the English text is the key). For the
/// app's own labels: write them as `tr("...")` so the catalog test finds them.
#[uniffi::export]
pub fn tr(text: String) -> String {
    stickpile_i18n::global::t_dyn(&text)
}

/// [`tr`] with `{name}` placeholders filled.
#[uniffi::export]
pub fn trf(text: String, args: std::collections::HashMap<String, String>) -> String {
    let mut out = stickpile_i18n::global::t_dyn(&text);
    for (k, v) in args {
        out = out.replace(&format!("{{{k}}}"), &v);
    }
    out
}

/// The stickpile version, for the About item.
#[uniffi::export]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The default `config.toml`.
#[uniffi::export]
pub fn default_config_path() -> String {
    stickpile_core::ClientConfig::default_path().display().to_string()
}
