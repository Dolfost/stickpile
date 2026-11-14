//! Popup state machine.
//!
//! The shell feeds [`Event`]s into [`Model::handle`] and executes the returned
//! [`Command`]s (search, load, copy, hide, load thumbnails); it then re-renders
//! from the model's read-only view ([`Model::tab`], [`Model::mode`],
//! [`Model::view`], [`Model::entries`], [`Model::packs`], [`Model::selected`],
//! [`Model::footer`]). Nothing here knows about a toolkit, a clock or the
//! network, so every behaviour is unit-tested below.
//!
//! Async results are tagged: every [`Command::Search`] carries a generation
//! number and [`Event::Results`] for any other generation are dropped, so a
//! slow answer to an old query can never overwrite a newer one. The list
//! loads (most used, favorites, packs, a pack's items) are tagged the same
//! way. Debouncing is the shell's job; the command carries a `debounce_ms`
//! hint (0 when the user did not type, e.g. after switching the target app).
//!
//! With `modal` on, the shell maps keys by [`Model::mode`]: in Insert the
//! text field has them, in Normal they arrive as [`Event::Normal`] and
//! [`crate::vim`] turns them into commands. An opening starts in
//! [`ModelConfig::start_mode`], a tab switched to in its
//! [`Tab::default_mode`]; the shell reports focus changes of text fields
//! ([`Event::TextFocus`]) and moves the focus to match [`Model::mode`].

use std::collections::HashSet;

use stickpile_api::profiles::{self, ItemShape};
use stickpile_api::{ItemSummary, MediaFormat, MediaKind, SearchHit, SetSummary, TargetApp};

use stickpile_i18n::global::{t, t_dyn, t_template, tf};

use crate::focus::WindowInfo;
use crate::history::WindowHistory;
use crate::tabs::{Mode, PageKey, Tab, View};
use crate::vim::{Cmd, Fed, NormalKey, Pending};

/// Default delay between the last keystroke and the search request.
pub const DEFAULT_DEBOUNCE_MS: u64 = 120;
/// Default wait of a lone `1`-`7` in Normal mode for a motion (`3j`)
/// before it switches to that tab.
pub const DEFAULT_KEY_TIMEOUT_MS: u64 = 400;

/// Static settings, usually taken from the client config (`ui.*`,
/// `default_target`, `prefer_stickers`).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelConfig {
    pub columns: usize,
    /// Rows visible at once, for PageUp/PageDown until the shell reports its layout.
    pub visible_rows: usize,
    pub recent_count: usize,
    pub max_results: usize,
    pub default_target: TargetApp,
    /// Starting value of the sticker preference; [`Event::ToggleSticker`]
    /// changes it for the life of the model, not just one opening.
    pub prefer_stickers: bool,
    pub hide_on_copy: bool,
    pub debounce_ms: u64,
    /// Items asked for on the Most used and Favorites tabs.
    pub list_limit: usize,
    /// Vim-style Insert / Normal modes (`ui.modal`). Off: the text field
    /// always has the keys and Escape hides.
    pub modal: bool,
    /// The tab every opening starts on (`ui.start_tab`); `None`: the tab
    /// the popup was closed on (see [`Model::restore_tab`]).
    pub start_tab: Option<Tab>,
    /// The mode an opening starts in with `modal` on (`ui.start_mode`):
    /// Normal, or Insert for the tab's own [`Tab::default_mode`] (Insert
    /// where it has a text field).
    pub start_mode: Mode,
    /// What the item tiles show next to the thumbnail; Normal-mode `z`
    /// keys change it for the life of the model.
    pub tiles: Tiles,
    /// See [`DEFAULT_KEY_TIMEOUT_MS`].
    pub key_timeout_ms: u64,
}

/// What the item tiles show next to the thumbnail (`ui.mark_favorites`,
/// `ui.show_emojis`, `ui.show_tags`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tiles {
    /// A heart on the items the viewer liked ([`Entry::liked`]).
    pub favorites: bool,
    pub emojis: bool,
    pub tags: bool,
}

impl Default for Tiles {
    fn default() -> Self {
        Tiles {
            favorites: true,
            emojis: true,
            tags: true,
        }
    }
}

impl Tiles {
    pub fn get(self, tile: Tile) -> bool {
        match tile {
            Tile::Favorites => self.favorites,
            Tile::Emojis => self.emojis,
            Tile::Tags => self.tags,
        }
    }

    fn set(&mut self, tile: Tile, on: bool) {
        match tile {
            Tile::Favorites => self.favorites = on,
            Tile::Emojis => self.emojis = on,
            Tile::Tags => self.tags = on,
        }
    }
}

/// One thing [`Tiles`] can show or hide (`zf`, `ze`, `zt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Favorites,
    Emojis,
    Tags,
}

impl Tile {
    pub const ALL: [Tile; 3] = [Tile::Favorites, Tile::Emojis, Tile::Tags];
}

impl Default for ModelConfig {
    fn default() -> Self {
        ModelConfig {
            columns: 6,
            visible_rows: 4,
            recent_count: 24,
            max_results: 60,
            default_target: TargetApp::Generic,
            prefer_stickers: false,
            hide_on_copy: true,
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            list_limit: 120,
            modal: true,
            start_tab: Some(Tab::Search),
            start_mode: Mode::Insert,
            tiles: Tiles::default(),
            key_timeout_ms: DEFAULT_KEY_TIMEOUT_MS,
        }
    }
}

/// Search results as delivered by the shell (mirrors `stickpile_core::SearchOutcome`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Results {
    pub hits: Vec<SearchHit>,
    /// The server was unreachable and the offline cache answered.
    pub offline: bool,
}

/// What the shell knew when it opened the popup.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpenInfo {
    /// Focused window, captured before the popup was shown.
    pub window: Option<WindowInfo>,
    /// `window` mapped through the configured focus rules.
    pub detected: Option<TargetApp>,
}

/// Grid navigation keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Show the popup (first start without `--hidden`, or a toggle while hidden).
    Open(OpenInfo),
    /// The global shortcut / `stickpile-desktop toggle` fired again.
    /// `popup_active`: the popup window currently has keyboard focus.
    Toggle {
        popup_active: bool,
        open: OpenInfo,
    },
    /// Hide the popup (`stickpile-desktop hide`).
    Hide,
    /// The window was hidden or closed by something else (WM close button).
    Hidden,
    /// The text of the text field changed: the search query on Search, the
    /// pack filter on Packs (the pack name filter, or the item filter inside
    /// an opened pack). Ignored on the other tabs.
    QueryChanged(String),
    Results {
        generation: u64,
        outcome: Result<Results, String>,
    },
    /// Most recently used items, newest first (answer to [`Command::LoadRecent`]).
    RecentLoaded(Vec<ItemSummary>),
    /// Answer to [`Command::LoadMostUsed`].
    MostUsedLoaded {
        generation: u64,
        outcome: Result<Vec<ItemSummary>, String>,
    },
    /// Answer to [`Command::LoadFavorites`].
    FavoritesLoaded {
        generation: u64,
        outcome: Result<Vec<ItemSummary>, String>,
    },
    /// Answer to [`Command::LoadPacks`].
    PacksLoaded {
        generation: u64,
        outcome: Result<Vec<SetSummary>, String>,
    },
    /// Answer to [`Command::LoadPackItems`].
    PackItemsLoaded {
        set_id: String,
        generation: u64,
        outcome: Result<Vec<ItemSummary>, String>,
    },
    /// The grid's real geometry, if it differs from the configured one.
    Layout {
        columns: usize,
        visible_rows: usize,
    },
    Navigate(Nav),
    /// Pointer selection (an item, or a pack in the pack list).
    Select(usize),
    /// Enter (`toggle_sticker: false`) or Shift+Enter (`true`) in Insert
    /// mode; in the pack list it opens the selected pack.
    CopySelected {
        toggle_sticker: bool,
    },
    /// Double click / activation of a grid cell; in the pack list it opens
    /// that pack.
    CopyAt {
        index: usize,
        toggle_sticker: bool,
    },
    /// Ctrl+T (Ctrl+Shift+T backwards).
    CycleTarget {
        backwards: bool,
    },
    SetTarget(TargetApp),
    /// Ctrl+S or the footer toggle: flip the sticker preference. It survives
    /// hiding and reopening; only a new model (process restart) goes back to
    /// `prefer_stickers`.
    ToggleSticker,
    /// Escape in Insert mode (or with `modal` off): to Normal mode; without
    /// `modal`, out of an opened pack, else hide.
    Escape,
    /// A key in Normal mode (with `modal` on), interpreted by [`crate::vim`].
    Normal(NormalKey),
    /// Show this tab (Alt+1..7, a click on the tab bar).
    ShowTab(Tab),
    /// Tab / Shift+Tab, wrapping.
    CycleTab {
        backwards: bool,
    },
    /// The timer of [`Command::KeyTimeout`] fired.
    KeyTimeout {
        seq: u64,
    },
    /// A text field (the search entry, or a field on the Me / Settings
    /// pages) took (`true`) or lost (`false`) the keyboard focus: Insert or
    /// Normal mode. Only used with `modal` on.
    TextFocus(bool),
    /// F1 or a help button: show or hide the key help.
    ToggleHelp,
    /// Ctrl+D or a like button: like / unlike the selected item.
    ToggleFavorite,
    /// Answer to [`Command::ToggleFavorite`]: the item's new liked state.
    FavoriteToggled {
        item_id: String,
        outcome: Result<bool, String>,
    },
    /// Back from an opened pack to the pack list (a back button).
    PackBack,
    /// New settings (the Settings tab saved a change): applied at once,
    /// without a new model.
    Configure(ModelConfig),
    /// The account or the server changed (login, logout, a new server URL):
    /// forget the lists fetched for the old one and fetch the shown tab again.
    Reload,
    /// The clipboard now holds the item (answer to [`Command::Copy`]).
    CopyFinished {
        item_id: String,
        warning: Option<String>,
    },
    CopyFailed {
        item_id: String,
        error: String,
    },
    /// A drag from the popup was accepted by a drop target.
    Dropped {
        item_id: String,
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Show and focus the popup window.
    Present,
    Hide,
    /// Run a search after `debounce_ms` (replacing any pending one).
    Search {
        query: String,
        target: TargetApp,
        limit: usize,
        generation: u64,
        debounce_ms: u64,
    },
    /// Drop a pending (debounced) search; the query was cleared.
    CancelSearch,
    LoadRecent {
        limit: usize,
    },
    /// Answer with [`Event::MostUsedLoaded`].
    LoadMostUsed {
        limit: usize,
        generation: u64,
    },
    /// Answer with [`Event::FavoritesLoaded`].
    LoadFavorites {
        limit: usize,
        generation: u64,
    },
    /// The user's sets; answer with [`Event::PacksLoaded`].
    LoadPacks {
        generation: u64,
    },
    /// All items of one set; answer with [`Event::PackItemsLoaded`].
    LoadPackItems {
        set_id: String,
        generation: u64,
    },
    /// Make sure these items' thumbnails are loaded (the shell skips ones it
    /// already has or is loading). Sent whenever the visible entries change.
    LoadThumbnails {
        items: Vec<ItemSummary>,
    },
    /// The same for the pack list: the cover of each set.
    LoadPackCovers {
        packs: Vec<SetSummary>,
    },
    /// Prepare the item for `target` and put it on the clipboard while the
    /// popup still has focus, then answer with `CopyFinished`/`CopyFailed`.
    Copy {
        item: Box<ItemSummary>,
        target: TargetApp,
        sticker: bool,
    },
    /// Persist the per-window history (changed after a copy).
    SaveHistory(WindowHistory),
    /// A `z` key showed or hid something on the tiles: save it as the
    /// matching setting, so it holds after a restart too.
    SaveTile {
        tile: Tile,
        on: bool,
    },
    /// Like or unlike; answer with [`Event::FavoriteToggled`].
    ToggleFavorite {
        item_id: String,
    },
    /// Open the item's page on the server in the browser.
    OpenInBrowser {
        item_id: String,
    },
    /// A Normal-mode key for the Me / Settings page.
    Page(PageKey),
    /// Send [`Event::KeyTimeout`] with `seq` after `after_ms`, unless
    /// another key comes first (a stale one is ignored, so the shell need
    /// not cancel it).
    KeyTimeout {
        after_ms: u64,
        seq: u64,
    },
}

/// How the current target app was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSource {
    /// `default_target`: no focused window, detection off, or no rule matched.
    Default,
    /// A focus rule matched this window class.
    Detected { class: String },
    /// Picked with Ctrl+T.
    Chosen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Busy,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Idle,
    Searching,
    NoResults {
        query: String,
    },
    NoRecent,
    /// The Recent tab with nothing used yet.
    NothingUsed,
    Offline,
    NothingSelected,
    Copying {
        name: String,
    },
    Copied {
        name: String,
        target: TargetApp,
        sticker: bool,
        warning: Option<String>,
    },
    Dropped {
        name: String,
    },
    Error(String),
    /// A list (most used, favorites, packs, a pack) is being fetched for the
    /// first time.
    Loading,
    NoMostUsed,
    NoFavorites,
    NoPacks,
    EmptyPack,
    /// Fetching the shown list failed (the whole message).
    LoadFailed(String),
    /// Answer to a favorite toggle.
    Favorite {
        added: bool,
    },
    /// A `z` key showed or hid something on the tiles.
    Tile {
        tile: Tile,
        on: bool,
    },
}

impl Status {
    pub fn level(&self) -> Level {
        match self {
            Status::Idle
            | Status::NoResults { .. }
            | Status::NoRecent
            | Status::NothingUsed
            | Status::NothingSelected
            | Status::Dropped { .. }
            | Status::NoMostUsed
            | Status::NoFavorites
            | Status::NoPacks
            | Status::EmptyPack
            | Status::Favorite { .. }
            | Status::Tile { .. } => Level::Info,
            Status::Searching | Status::Copying { .. } | Status::Loading => Level::Busy,
            Status::Offline => Level::Warning,
            Status::Copied { warning, .. } => {
                if warning.is_some() {
                    Level::Warning
                } else {
                    Level::Info
                }
            }
            Status::Error(_) | Status::LoadFailed(_) => Level::Error,
        }
    }

    /// The footer text, in the current language.
    pub fn text(&self) -> String {
        match self {
            Status::Idle => String::new(),
            Status::Searching => t("Searching…").into(),
            Status::NoResults { query } => tf("No matches for “{query}”", &[("query", query)]),
            Status::NoRecent => t("Nothing used yet — type to search").into(),
            Status::NothingUsed => t("Nothing used yet").into(),
            Status::Offline => t("Offline — showing cached results").into(),
            Status::NothingSelected => t("Nothing selected").into(),
            Status::Copying { name } => tf("Preparing {name}…", &[("name", name)]),
            Status::Copied {
                name,
                target,
                sticker,
                warning,
            } => {
                let target = target_name(*target);
                let args: [(&str, &dyn std::fmt::Display); 2] = [("name", name), ("target", &target)];
                let mut s = if *sticker {
                    tf("Copied {name} as sticker for {target}", &args)
                } else {
                    tf("Copied {name} for {target}", &args)
                };
                if let Some(w) = warning {
                    s.push_str(" — ");
                    s.push_str(&t_template(w, profiles::INCOMPATIBLE));
                }
                s
            }
            Status::Dropped { name } => tf("Dropped {name}", &[("name", name)]),
            Status::Error(e) => e.clone(),
            Status::Loading => t("Loading...").into(),
            Status::NoMostUsed => t("Nothing used yet").into(),
            Status::NoFavorites => t("No favorites yet").into(),
            Status::NoPacks => t("No packs yet").into(),
            Status::EmptyPack => t("This pack is empty").into(),
            Status::LoadFailed(e) => e.clone(),
            Status::Favorite { added: true } => t("Added to favorites").into(),
            Status::Favorite { added: false } => t("Removed from favorites").into(),
            Status::Tile { tile, on } => match (tile, on) {
                (Tile::Favorites, true) => t("Favorite marks shown"),
                (Tile::Favorites, false) => t("Favorite marks hidden"),
                (Tile::Emojis, true) => t("Emojis shown"),
                (Tile::Emojis, false) => t("Emojis hidden"),
                (Tile::Tags, true) => t("Tags shown"),
                (Tile::Tags, false) => t("Tags hidden"),
            }
            .into(),
        }
    }

    /// Describes what the shown list holds (rather than reporting something
    /// the user did), so new data for the list replaces it.
    fn describes_view(&self) -> bool {
        matches!(
            self,
            Status::Idle
                | Status::Searching
                | Status::NoResults { .. }
                | Status::NoRecent
                | Status::NothingUsed
                | Status::Offline
                | Status::Loading
                | Status::NoMostUsed
                | Status::NoFavorites
                | Status::NoPacks
                | Status::EmptyPack
                | Status::LoadFailed(_)
        )
    }
}

/// Sticker indicator in the footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StickerMode {
    On,
    Off,
    /// The target app has no sticker route.
    Unsupported,
}

/// Everything the footer shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Footer {
    /// "Telegram"
    pub target: String,
    /// "auto: com.ayugram.desktop", "default", "chosen"
    pub target_hint: String,
    pub sticker: StickerMode,
    pub status: String,
    pub level: Level,
    pub offline: bool,
    /// The input mode ([`Mode::label`]); `None` with `modal` off.
    pub mode: Option<Mode>,
    /// Keys of an incomplete Normal-mode command ("3", "g", "12g"), like
    /// vim's showcmd; empty when nothing is pending.
    pub pending: String,
}

/// One grid cell.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub item: ItemSummary,
    /// Why the item cannot be sent to the current target as-is (the copy still
    /// proceeds with the best variant; this dims the cell and fills the tooltip).
    pub incompatible: Option<String>,
    /// Matched signals from search (`name`, `keyword`, `image`, ...).
    pub matched: Vec<String>,
    /// The viewer liked the item, as far as the model knows: the favorites
    /// it loaded (the Favorites tab's list) and the likes made since.
    pub liked: bool,
}

impl Entry {
    /// Small corner badge for anything that moves.
    pub fn badge(&self) -> Option<&'static str> {
        match (self.item.kind, self.item.format) {
            (MediaKind::Still, _) => None,
            (MediaKind::Animation, MediaFormat::Gif) => Some("GIF"),
            (MediaKind::Animation, MediaFormat::Webp) => Some("WEBP"),
            (MediaKind::Animation, _) => Some("ANIM"),
            (MediaKind::Video, _) => Some("VIDEO"),
        }
    }

    pub fn is_animated(&self) -> bool {
        self.item.kind != MediaKind::Still
    }

    /// Multi-line tooltip: name, set, keywords/emojis, incompatibility.
    pub fn tooltip(&self) -> String {
        let it = &self.item;
        let mut s = format!("{}\n{}", it.name, it.set_name);
        if !it.emojis.is_empty() || !it.keywords.is_empty() {
            s.push('\n');
            s.push_str(&it.emojis.join(""));
            if !it.emojis.is_empty() && !it.keywords.is_empty() {
                s.push(' ');
            }
            s.push_str(&it.keywords.join(", "));
        }
        if let Some(why) = &self.incompatible {
            s.push_str("\n⚠ ");
            s.push_str(&t_template(why, profiles::INCOMPATIBLE));
        }
        s
    }
}

/// The mode an opening on `tab` starts in.
fn opening_mode(cfg: &ModelConfig, tab: Tab) -> Mode {
    if !cfg.modal {
        Mode::Insert
    } else if cfg.start_mode == Mode::Normal {
        Mode::Normal
    } else {
        tab.default_mode()
    }
}

/// The target's name, in the current language ("Other / browser").
pub fn target_name(target: TargetApp) -> String {
    match target {
        TargetApp::Generic => t("Other / browser").into(),
        _ => profiles::profile(target).display_name,
    }
}

pub fn supports_stickers(t: TargetApp) -> bool {
    profiles::profile(t).sticker.is_some()
}

/// Local compatibility check (the server does the same for search results).
/// Unknown variant lists (empty) are never flagged.
pub fn incompatibility(item: &ItemSummary, target: TargetApp, sticker: bool) -> Option<String> {
    if item.variants.is_empty() {
        return None;
    }
    let shape = ItemShape {
        kind: item.kind,
        format: item.format,
        duration_ms: None,
        variants: &item.variants,
    };
    profiles::choose_variant(target, &shape, sticker).err().map(|e| e.0)
}

/// Lowercase words of a filter; every one must match.
fn filter_terms(filter: &str) -> Vec<String> {
    filter.split_whitespace().map(str::to_lowercase).collect()
}

/// Client-side filter inside an opened pack: name, keywords, emojis, tags.
fn item_matches(item: &ItemSummary, terms: &[String]) -> bool {
    terms.iter().all(|t| {
        std::iter::once(&item.name)
            .chain(&item.keywords)
            .chain(&item.emojis)
            .chain(&item.tags)
            .any(|field| field.to_lowercase().contains(t.as_str()))
    })
}

fn pack_matches(set: &SetSummary, terms: &[String]) -> bool {
    let name = set.name.to_lowercase();
    let slug = set.slug.to_lowercase();
    terms
        .iter()
        .all(|t| name.contains(t.as_str()) || slug.contains(t.as_str()))
}

/// No list entries but a filter typed: say so instead of "empty".
fn filtered_status(base: Status, filter: &str, nothing_shown: bool) -> Status {
    let filter = filter.trim();
    if nothing_shown && !filter.is_empty() && matches!(base, Status::Idle) {
        Status::NoResults { query: filter.into() }
    } else {
        base
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Showing {
    Recent,
    Results,
}

/// A list fetched from the shell; refreshed every time it is shown.
#[derive(Debug, Clone, PartialEq)]
struct Listing<T> {
    items: Vec<T>,
    /// Generation of the request in flight.
    loading: Option<u64>,
    /// At least one answer arrived: `items` are real (if maybe stale).
    loaded: bool,
    error: Option<String>,
}

impl<T> Default for Listing<T> {
    fn default() -> Self {
        Listing {
            items: Vec::new(),
            loading: None,
            loaded: false,
            error: None,
        }
    }
}

impl<T> Listing<T> {
    /// Start a request unless one is in flight; its generation.
    fn start(&mut self, requests: &mut u64) -> Option<u64> {
        if self.loading.is_some() {
            return None;
        }
        *requests += 1;
        self.loading = Some(*requests);
        self.error = None;
        Some(*requests)
    }

    /// Take an answer; false (dropped) unless it answers the request in flight.
    fn accept(&mut self, generation: u64, outcome: Result<Vec<T>, String>) -> bool {
        if self.loading != Some(generation) {
            return false;
        }
        self.loading = None;
        match outcome {
            Ok(items) => {
                self.items = items;
                self.loaded = true;
            }
            Err(e) => self.error = Some(e),
        }
        true
    }

    fn status(&self, what: &str, empty: Status) -> Status {
        if let Some(e) = &self.error {
            Status::LoadFailed(tf(
                "Could not load {what}: {error}",
                &[("what", &what), ("error", &t_dyn(e))],
            ))
        } else if !self.loaded {
            if self.loading.is_some() {
                Status::Loading
            } else {
                Status::Idle
            }
        } else if self.items.is_empty() {
            empty
        } else {
            Status::Idle
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Packs {
    sets: Listing<SetSummary>,
    /// Pack name filter of the list.
    filter: String,
    /// `sets` matching `filter`: the list as shown.
    shown: Vec<SetSummary>,
    open: Option<OpenPack>,
    /// The pack selected in the list when `open` was entered.
    list_selection: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct OpenPack {
    set: SetSummary,
    items: Listing<ItemSummary>,
    /// Item filter inside the pack.
    filter: String,
}

/// Which entry to select after the grid was rebuilt.
#[derive(Debug, Clone, PartialEq)]
enum Sel {
    First,
    /// The same entry (by id, else the same position): same list, new compatibility.
    Keep,
    /// Fresh data for the shown list: [`Sel::Keep`] if the user moved the
    /// selection, else the first entry.
    Refresh,
    /// Coming back to a view: this entry, else the first.
    Id(Option<String>),
}

#[derive(Debug, Clone)]
pub struct Model {
    cfg: ModelConfig,
    visible: bool,
    tab: Tab,
    mode: Mode,
    pending: Pending,
    help: bool,
    /// The search query (the text field on the Search tab).
    query: String,
    recent: Vec<ItemSummary>,
    results: Vec<SearchHit>,
    offline: bool,
    /// Target the current results were searched for (server hints apply to it only).
    results_target: TargetApp,
    showing: Showing,
    /// A search is in flight.
    searching: bool,
    most_used: Listing<ItemSummary>,
    favorites: Listing<ItemSummary>,
    packs: Packs,
    /// Generation counter of the list loads.
    requests: u64,
    /// Per tab, the entry selected when it was left (within one opening).
    memory: [Option<String>; Tab::ALL.len()],
    /// The user moved the selection in the shown view, so refreshed data keeps it.
    touched: bool,
    entries: Vec<Entry>,
    entries_revision: u64,
    selected: Option<usize>,
    columns: usize,
    visible_rows: usize,
    target: TargetApp,
    target_source: TargetSource,
    window: Option<WindowInfo>,
    /// Sticker preference: starts as `prefer_stickers`, kept across openings.
    sticker: bool,
    status: Status,
    generation: u64,
    copying: Option<InFlight>,
    history: WindowHistory,
    /// Ids of the items the viewer liked, for [`Entry::liked`].
    liked: HashSet<String>,
    /// What the tiles show: [`ModelConfig::tiles`], changed by `z` keys.
    tiles: Tiles,
}

/// The copy currently being prepared.
#[derive(Debug, Clone, PartialEq)]
struct InFlight {
    item_id: String,
    name: String,
    target: TargetApp,
    sticker: bool,
}

/// Events typed or clicked by the user: they cancel a pending Normal-mode
/// sequence (answers, pointer hover and focus changes do not).
fn cancels_pending(event: &Event) -> bool {
    matches!(
        event,
        Event::Open(_)
            | Event::Toggle { .. }
            | Event::Hide
            | Event::Hidden
            | Event::QueryChanged(_)
            | Event::Navigate(_)
            | Event::CopySelected { .. }
            | Event::CopyAt { .. }
            | Event::CycleTarget { .. }
            | Event::SetTarget(_)
            | Event::ToggleSticker
            | Event::Escape
            | Event::ShowTab(_)
            | Event::CycleTab { .. }
            | Event::ToggleHelp
            | Event::ToggleFavorite
            | Event::PackBack
    )
}

impl Model {
    pub fn new(cfg: ModelConfig) -> Model {
        Model::with_history(cfg, WindowHistory::default())
    }

    /// Start with a per-window history loaded by the shell.
    pub fn with_history(cfg: ModelConfig, history: WindowHistory) -> Model {
        let tab = cfg.start_tab.unwrap_or_default();
        Model {
            visible: false,
            tab,
            mode: opening_mode(&cfg, tab),
            pending: Pending::default(),
            help: false,
            query: String::new(),
            recent: Vec::new(),
            results: Vec::new(),
            offline: false,
            results_target: cfg.default_target,
            showing: Showing::Recent,
            searching: false,
            most_used: Listing::default(),
            favorites: Listing::default(),
            packs: Packs::default(),
            requests: 0,
            memory: Default::default(),
            touched: false,
            entries: Vec::new(),
            entries_revision: 0,
            selected: None,
            columns: cfg.columns.max(1),
            visible_rows: cfg.visible_rows.max(1),
            target: cfg.default_target,
            target_source: TargetSource::Default,
            window: None,
            sticker: cfg.prefer_stickers,
            status: Status::Idle,
            generation: 0,
            copying: None,
            history,
            liked: HashSet::new(),
            tiles: cfg.tiles,
            cfg,
        }
    }

    /// The tab the popup was closed on last time (saved by the shell), for
    /// a `start_tab` of `None`. Ignored once the popup has been opened.
    pub fn restore_tab(&mut self, tab: Tab) {
        if self.cfg.start_tab.is_none() && !self.visible && self.generation == 0 {
            self.tab = tab;
            self.mode = opening_mode(&self.cfg, tab);
        }
    }

    // -- read-only view ------------------------------------------------------

    pub fn config(&self) -> &ModelConfig {
        &self.cfg
    }
    pub fn is_visible(&self) -> bool {
        self.visible
    }
    pub fn tab(&self) -> Tab {
        self.tab
    }
    /// Always [`Mode::Insert`] with `modal` off.
    pub fn mode(&self) -> Mode {
        self.mode
    }
    pub fn view(&self) -> View {
        match self.tab {
            Tab::Me | Tab::Settings => View::Page,
            Tab::Packs if self.packs.open.is_none() => View::Packs,
            _ => View::Items,
        }
    }
    /// Keys of an incomplete Normal-mode command (also in [`Model::footer`]).
    pub fn pending(&self) -> String {
        self.pending.display()
    }
    /// Draw the key help ([`crate::vim::HELP`]) over the popup.
    pub fn help_visible(&self) -> bool {
        self.help
    }
    /// Text of the text field on this tab: the search query, the pack
    /// filter, or the item filter of the opened pack; empty elsewhere.
    pub fn query(&self) -> &str {
        match (self.tab, &self.packs.open) {
            (Tab::Search, _) => &self.query,
            (Tab::Packs, Some(open)) => &open.filter,
            (Tab::Packs, None) => &self.packs.filter,
            _ => "",
        }
    }
    /// The search query, whatever tab is shown.
    pub fn search_query(&self) -> &str {
        &self.query
    }
    /// Header text: the tab's label, or "Packs / Cats" inside a pack.
    pub fn title(&self) -> String {
        match (self.tab, &self.packs.open) {
            (Tab::Packs, Some(open)) => tf("Packs / {name}", &[("name", &open.set.name)]),
            (tab, _) => tab.label().to_string(),
        }
    }
    /// Item grid of the shown tab ([`View::Items`]); empty for the pack
    /// list and the pages.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    /// The pack list as shown (filtered), for [`View::Packs`].
    pub fn packs(&self) -> &[SetSummary] {
        &self.packs.shown
    }
    /// The opened pack, if the Packs tab shows one.
    pub fn open_pack(&self) -> Option<&SetSummary> {
        self.packs.open.as_ref().map(|o| &o.set)
    }
    /// Bumped whenever [`Model::entries`] or [`Model::packs`] change, so the
    /// shell can skip rebuilding its list model.
    pub fn entries_revision(&self) -> u64 {
        self.entries_revision
    }
    /// Index into [`Model::entries`], or into [`Model::packs`] on the pack list.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }
    pub fn selected_entry(&self) -> Option<&Entry> {
        match self.view() {
            View::Items => self.selected.and_then(|i| self.entries.get(i)),
            _ => None,
        }
    }
    pub fn selected_pack(&self) -> Option<&SetSummary> {
        match self.view() {
            View::Packs => self.selected.and_then(|i| self.packs.shown.get(i)),
            _ => None,
        }
    }
    pub fn target(&self) -> TargetApp {
        self.target
    }
    pub fn target_source(&self) -> &TargetSource {
        &self.target_source
    }
    /// The focused window captured at the last open.
    pub fn window(&self) -> Option<&WindowInfo> {
        self.window.as_ref()
    }
    pub fn sticker(&self) -> bool {
        self.sticker
    }
    pub fn status(&self) -> &Status {
        &self.status
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn is_offline(&self) -> bool {
        self.tab == Tab::Search && self.showing == Showing::Results && self.offline
    }
    pub fn is_copying(&self) -> bool {
        self.copying.is_some()
    }
    pub fn columns(&self) -> usize {
        self.columns
    }
    pub fn history(&self) -> &WindowHistory {
        &self.history
    }
    /// What the item tiles show.
    pub fn tiles(&self) -> Tiles {
        self.tiles
    }

    pub fn sticker_mode(&self) -> StickerMode {
        if !supports_stickers(self.target) {
            StickerMode::Unsupported
        } else if self.sticker {
            StickerMode::On
        } else {
            StickerMode::Off
        }
    }

    pub fn footer(&self) -> Footer {
        let target_hint = match &self.target_source {
            TargetSource::Default => t("default").to_string(),
            TargetSource::Detected { class } => tf("auto: {class}", &[("class", class)]),
            TargetSource::Chosen => t("chosen").to_string(),
        };
        Footer {
            target: target_name(self.target),
            target_hint,
            sticker: self.sticker_mode(),
            status: self.status.text(),
            level: self.status.level(),
            offline: self.is_offline(),
            mode: self.cfg.modal.then_some(self.mode),
            pending: self.pending.display(),
        }
    }

    // -- events ----------------------------------------------------------------

    pub fn handle(&mut self, event: Event) -> Vec<Command> {
        if cancels_pending(&event) {
            self.pending.clear();
        }
        match event {
            Event::Open(info) => self.open(info),
            Event::Toggle { popup_active, open } => {
                if self.visible && popup_active {
                    self.hide()
                } else {
                    self.open(open)
                }
            }
            Event::Hide => {
                if self.visible {
                    self.hide()
                } else {
                    Vec::new()
                }
            }
            Event::Hidden => {
                self.visible = false;
                Vec::new()
            }
            Event::QueryChanged(q) => self.query_changed(q),
            Event::Results { generation, outcome } => self.results(generation, outcome),
            Event::RecentLoaded(items) => self.recent_loaded(items),
            Event::MostUsedLoaded { generation, outcome } => {
                let fresh = self.most_used.accept(generation, outcome);
                self.list_answered(fresh && self.tab == Tab::MostUsed)
            }
            Event::FavoritesLoaded { generation, outcome } => {
                let fresh = self.favorites.accept(generation, outcome);
                let shown = fresh && self.tab == Tab::Favorites;
                if fresh && self.favorites.error.is_none() {
                    let liked: HashSet<String> = self.favorites.items.iter().map(|i| i.id.clone()).collect();
                    if liked != self.liked {
                        self.liked = liked;
                        if self.tiles.favorites && !shown && self.view() == View::Items {
                            self.rebuild(Sel::Keep);
                        }
                    }
                }
                self.list_answered(shown)
            }
            Event::PacksLoaded { generation, outcome } => {
                let fresh = self.packs.sets.accept(generation, outcome);
                self.list_answered(fresh && self.view() == View::Packs)
            }
            Event::PackItemsLoaded {
                set_id,
                generation,
                outcome,
            } => {
                let fresh = match &mut self.packs.open {
                    Some(open) if open.set.id == set_id => open.items.accept(generation, outcome),
                    _ => false,
                };
                self.list_answered(fresh && self.tab == Tab::Packs)
            }
            Event::Layout { columns, visible_rows } => {
                self.columns = columns.max(1);
                self.visible_rows = visible_rows.max(1);
                Vec::new()
            }
            Event::Navigate(nav) => {
                if self.view() != View::Page {
                    self.navigate(nav);
                }
                Vec::new()
            }
            Event::Select(i) => {
                if i < self.grid_len() {
                    self.selected = Some(i);
                    self.touched = true;
                }
                Vec::new()
            }
            Event::CopySelected { toggle_sticker } => self.enter(toggle_sticker),
            Event::CopyAt { index, toggle_sticker } => match self.view() {
                View::Items if index < self.entries.len() => {
                    self.selected = Some(index);
                    self.copy_at(index, toggle_sticker)
                }
                View::Packs => self.open_pack_at(index),
                _ => Vec::new(),
            },
            Event::CycleTarget { backwards } => self.cycle_target(backwards),
            Event::SetTarget(t) => self.set_target(t),
            Event::ToggleSticker => self.toggle_sticker(),
            Event::Escape => self.escape(),
            Event::Normal(key) => self.normal_key(key),
            Event::ShowTab(tab) => self.show_tab(tab),
            Event::CycleTab { backwards } => {
                let tab = if backwards { self.tab.prev() } else { self.tab.next() };
                self.show_tab(tab)
            }
            Event::KeyTimeout { seq } => match self.pending.timeout(seq) {
                Some(tab) if self.visible => self.show_tab(tab),
                _ => Vec::new(),
            },
            Event::TextFocus(focused) => {
                self.text_focus(focused);
                Vec::new()
            }
            Event::ToggleHelp => {
                self.help = !self.help;
                Vec::new()
            }
            Event::ToggleFavorite => self.toggle_favorite(),
            Event::FavoriteToggled { item_id, outcome } => self.favorite_toggled(&item_id, outcome),
            Event::PackBack => self.pack_back(),
            Event::Configure(cfg) => self.configure(cfg),
            Event::Reload => self.reload(),
            Event::CopyFinished { item_id, warning } => self.copy_finished(item_id, warning),
            Event::CopyFailed { item_id, error } => {
                if self.copying.as_ref().is_none_or(|c| c.item_id != item_id) {
                    return Vec::new();
                }
                self.copying = None;
                self.status = Status::Error(tf("Copy failed: {error}", &[("error", &t_dyn(&error))]));
                Vec::new()
            }
            Event::Dropped { item_id, name } => {
                if self.copying.is_none() {
                    self.status = Status::Dropped { name };
                }
                self.after_use(&item_id)
            }
        }
    }

    fn open(&mut self, info: OpenInfo) -> Vec<Command> {
        self.visible = true;
        self.generation += 1;
        self.query.clear();
        self.results.clear();
        self.offline = false;
        self.searching = false;
        self.showing = Showing::Recent;
        self.copying = None;
        self.status = Status::Idle;
        // `sticker` is deliberately kept: the last Ctrl+S / footer toggle still holds.
        match (&info.window, info.detected) {
            (Some(w), Some(t)) => {
                self.target = t;
                self.target_source = TargetSource::Detected { class: w.class.clone() };
            }
            _ => {
                self.target = self.cfg.default_target;
                self.target_source = TargetSource::Default;
            }
        }
        self.window = info.window;
        if let Some(tab) = self.cfg.start_tab {
            self.tab = tab;
        }
        self.mode = opening_mode(&self.cfg, self.tab);
        self.help = false;
        self.memory = Default::default();
        self.packs.filter.clear();
        self.packs.open = None;
        self.packs.list_selection = None;
        // Every opening refreshes what it shows; answers to the last one's
        // requests are dropped.
        self.most_used.loading = None;
        self.favorites.loading = None;
        self.packs.sets.loading = None;
        let mut cmds = vec![Command::Present, Command::CancelSearch];
        cmds.extend(self.enter_tab());
        // The other tabs need the favorites for their marks.
        if self.tiles.favorites && self.tab != Tab::Favorites {
            cmds.extend(self.load_favorites());
        }
        cmds
    }

    fn hide(&mut self) -> Vec<Command> {
        self.visible = false;
        self.pending.clear();
        vec![Command::Hide]
    }

    fn configure(&mut self, cfg: ModelConfig) -> Vec<Command> {
        let old = std::mem::replace(&mut self.cfg, cfg);
        let cfg = &self.cfg;
        if cfg.columns != old.columns {
            self.columns = cfg.columns.max(1);
        }
        if cfg.visible_rows != old.visible_rows {
            self.visible_rows = cfg.visible_rows.max(1);
        }
        let mut compat = false;
        // A changed preference is the user's latest word on sticker mode.
        if cfg.prefer_stickers != old.prefer_stickers && self.sticker != cfg.prefer_stickers {
            self.sticker = cfg.prefer_stickers;
            compat = true;
        }
        if self.target_source == TargetSource::Default && self.target != cfg.default_target {
            self.target = cfg.default_target;
            compat = true;
        }
        if cfg.modal != old.modal {
            self.pending.clear();
            self.mode = if cfg.modal {
                self.tab.default_mode()
            } else {
                Mode::Insert
            };
        }
        // A changed setting is the user's latest word; a `z` key's change
        // holds until then.
        let mut cmds = Vec::new();
        let tiles = self.cfg.tiles;
        for tile in Tile::ALL {
            let on = tiles.get(tile);
            if on != old.tiles.get(tile) && on != self.tiles.get(tile) {
                cmds.extend(self.set_tile(tile, on));
            }
        }
        if compat {
            self.refresh_compat();
        }
        cmds
    }

    /// Show or hide something on the tiles.
    fn set_tile(&mut self, tile: Tile, on: bool) -> Vec<Command> {
        self.tiles.set(tile, on);
        if tile == Tile::Favorites && self.view() == View::Items {
            // Marks were not kept up to date while off.
            self.rebuild(Sel::Keep);
        } else {
            // Same entries, drawn differently.
            self.entries_revision += 1;
        }
        if tile == Tile::Favorites && on && self.visible {
            return self.load_favorites().into_iter().collect();
        }
        Vec::new()
    }

    /// `zf`, `ze`, `zt`.
    fn toggle_tile(&mut self, tile: Tile) -> Vec<Command> {
        let on = !self.tiles.get(tile);
        let mut cmds = self.set_tile(tile, on);
        cmds.push(Command::SaveTile { tile, on });
        if self.copying.is_none() {
            self.status = Status::Tile { tile, on };
        }
        cmds
    }

    fn load_favorites(&mut self) -> Option<Command> {
        let limit = self.cfg.list_limit;
        self.favorites
            .start(&mut self.requests)
            .map(|generation| Command::LoadFavorites { limit, generation })
    }

    fn reload(&mut self) -> Vec<Command> {
        self.most_used = Listing::default();
        self.favorites = Listing::default();
        self.packs.sets = Listing::default();
        self.packs.open = None;
        self.packs.list_selection = None;
        self.packs.shown.clear();
        self.memory = Default::default();
        self.results.clear();
        self.offline = false;
        self.searching = false;
        self.showing = Showing::Recent;
        // Answers to searches for the old account are dropped.
        self.generation += 1;
        let mut cmds = vec![Command::CancelSearch];
        if !self.visible {
            self.rebuild(Sel::First);
            return cmds;
        }
        if !self.query.trim().is_empty() {
            cmds.push(self.search_command(0));
        }
        cmds.extend(self.enter_tab());
        cmds
    }

    // -- tabs and modes ----------------------------------------------------------

    fn show_tab(&mut self, tab: Tab) -> Vec<Command> {
        if tab == self.tab {
            return Vec::new();
        }
        self.memory[self.tab.index()] = self.selected_id();
        self.tab = tab;
        self.pending.clear();
        if self.cfg.modal {
            self.mode = tab.default_mode();
        }
        self.enter_tab()
    }

    /// Show the current tab from what is known, ask for fresh data, and set
    /// the tab's status.
    fn enter_tab(&mut self) -> Vec<Command> {
        let mut cmds = Vec::new();
        match self.tab {
            Tab::Search => {
                if self.query.trim().is_empty() {
                    cmds.push(self.load_recent());
                }
            }
            Tab::Recent => cmds.push(self.load_recent()),
            Tab::MostUsed => {
                cmds.extend(
                    self.most_used
                        .start(&mut self.requests)
                        .map(|generation| Command::LoadMostUsed {
                            limit: self.cfg.list_limit,
                            generation,
                        }),
                )
            }
            Tab::Favorites => cmds.extend(self.load_favorites()),
            Tab::Packs => {
                cmds.extend(
                    self.packs
                        .sets
                        .start(&mut self.requests)
                        .map(|generation| Command::LoadPacks { generation }),
                );
                if let Some(open) = &mut self.packs.open {
                    cmds.extend(
                        open.items
                            .start(&mut self.requests)
                            .map(|generation| Command::LoadPackItems {
                                set_id: open.set.id.clone(),
                                generation,
                            }),
                    );
                }
            }
            Tab::Me | Tab::Settings => {}
        }
        let remembered = self.memory[self.tab.index()].take();
        self.rebuild(Sel::Id(remembered));
        if self.copying.is_none() {
            self.status = self.view_status();
        }
        cmds.extend(self.thumbnails());
        cmds
    }

    fn load_recent(&self) -> Command {
        Command::LoadRecent {
            limit: self.cfg.recent_count,
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        if self.cfg.modal && self.mode != mode {
            self.mode = mode;
            self.pending.clear();
        }
    }

    fn text_focus(&mut self, focused: bool) {
        if !self.visible {
            return;
        }
        if focused && (self.tab.has_text_field() || self.tab.is_page()) {
            self.set_mode(Mode::Insert);
        } else if !focused {
            self.set_mode(Mode::Normal);
        }
    }

    /// Escape from Insert mode (or with `modal` off).
    fn escape(&mut self) -> Vec<Command> {
        if self.cfg.modal && self.mode == Mode::Insert {
            self.set_mode(Mode::Normal);
            return Vec::new();
        }
        self.escape_normal()
    }

    /// Escape with no text field to leave: close the help, leave the pack, hide.
    fn escape_normal(&mut self) -> Vec<Command> {
        if self.help {
            self.help = false;
            return Vec::new();
        }
        if self.tab == Tab::Packs && self.packs.open.is_some() {
            return self.pack_back();
        }
        self.hide()
    }

    fn normal_key(&mut self, key: NormalKey) -> Vec<Command> {
        if !self.cfg.modal || !self.visible || self.mode != Mode::Normal {
            return Vec::new();
        }
        let counts = self.view() != View::Page;
        match self.pending.feed(key, counts) {
            Fed::Wait { timeout } => timeout
                .map(|seq| Command::KeyTimeout {
                    after_ms: self.cfg.key_timeout_ms,
                    seq,
                })
                .into_iter()
                .collect(),
            Fed::Run(cmds) => cmds.into_iter().flat_map(|c| self.run(c)).collect(),
        }
    }

    fn run(&mut self, cmd: Cmd) -> Vec<Command> {
        let view = self.view();
        let items = view == View::Items;
        let page = |key| vec![Command::Page(key)];
        match cmd {
            Cmd::Move(nav, count) => {
                if view == View::Page {
                    return match nav {
                        Nav::Up => page(PageKey::Up),
                        Nav::Down => page(PageKey::Down),
                        Nav::Left => page(PageKey::Left),
                        Nav::Right => page(PageKey::Right),
                        _ => Vec::new(),
                    };
                }
                for _ in 0..count.min(self.grid_len().max(1)) {
                    self.navigate(nav);
                }
                Vec::new()
            }
            Cmd::HalfPage { down } => {
                if view != View::Page {
                    let nav = if down { Nav::Down } else { Nav::Up };
                    for _ in 0..(self.visible_rows / 2).max(1) {
                        self.navigate(nav);
                    }
                }
                Vec::new()
            }
            Cmd::GotoItem(n) => {
                self.goto(n.saturating_sub(1));
                Vec::new()
            }
            Cmd::RowStart => {
                if let Some(i) = self.selected {
                    self.goto(i - i % self.columns);
                }
                Vec::new()
            }
            Cmd::RowEnd => {
                if let Some(i) = self.selected {
                    self.goto(i - i % self.columns + self.columns - 1);
                }
                Vec::new()
            }
            Cmd::Yank { flip } if items => self.copy_selected(flip),
            Cmd::Enter { .. } | Cmd::Space if view == View::Page => page(PageKey::Activate),
            Cmd::Enter { shift } => self.enter(shift),
            Cmd::Search => self.slash(),
            Cmd::Insert { append } => self.insert(append),
            Cmd::Tab(tab) => self.show_tab(tab),
            Cmd::NextTab(n) => {
                let tab = (0..n % Tab::ALL.len()).fold(self.tab, |t, _| t.next());
                self.show_tab(tab)
            }
            Cmd::PrevTab(n) => {
                let tab = (0..n % Tab::ALL.len()).fold(self.tab, |t, _| t.prev());
                self.show_tab(tab)
            }
            Cmd::Favorite if items => self.toggle_favorite(),
            Cmd::Target { backwards } => self.cycle_target(backwards),
            Cmd::Sticker => self.toggle_sticker(),
            Cmd::Browser if items => self.open_in_browser(),
            Cmd::Help => {
                self.help = !self.help;
                Vec::new()
            }
            Cmd::Quit if self.help => {
                self.help = false;
                Vec::new()
            }
            Cmd::Quit => self.hide(),
            Cmd::Escape => self.escape_normal(),
            Cmd::Back if self.tab == Tab::Packs => self.pack_back(),
            Cmd::Open if view == View::Packs => self.enter(false),
            Cmd::Tile(tile) => self.toggle_tile(tile),
            Cmd::Yank { .. } | Cmd::Space | Cmd::Favorite | Cmd::Browser | Cmd::Back | Cmd::Open => Vec::new(),
        }
    }

    /// `/`: an empty text field in Insert mode (Search from tabs without one).
    fn slash(&mut self) -> Vec<Command> {
        let mut cmds = Vec::new();
        if self.tab.has_text_field() {
            cmds.extend(self.query_changed(String::new()));
        } else {
            cmds.extend(self.search_query_changed(String::new()));
            cmds.extend(self.show_tab(Tab::Search));
        }
        self.set_mode(Mode::Insert);
        cmds
    }

    /// `i` / `a`: back to the text field as it is (Search from the other
    /// grids); on a page, `i` asks the shell to focus the row's field.
    fn insert(&mut self, append: bool) -> Vec<Command> {
        if self.tab.is_page() {
            return if append {
                Vec::new()
            } else {
                vec![Command::Page(PageKey::Edit)]
            };
        }
        let cmds = if self.tab.has_text_field() {
            Vec::new()
        } else {
            self.show_tab(Tab::Search)
        };
        self.set_mode(Mode::Insert);
        cmds
    }

    /// Enter: copy the selected item, or open the selected pack.
    fn enter(&mut self, toggle_sticker: bool) -> Vec<Command> {
        match self.view() {
            View::Items => self.copy_selected(toggle_sticker),
            View::Packs => match self.selected {
                Some(i) => self.open_pack_at(i),
                None => Vec::new(),
            },
            View::Page => Vec::new(),
        }
    }

    // -- query, search and lists ---------------------------------------------------

    fn query_changed(&mut self, q: String) -> Vec<Command> {
        match self.tab {
            Tab::Search => self.search_query_changed(q),
            Tab::Packs => self.pack_filter_changed(q),
            _ => Vec::new(),
        }
    }

    fn search_query_changed(&mut self, q: String) -> Vec<Command> {
        if q == self.query {
            return Vec::new();
        }
        self.query = q;
        self.generation += 1;
        if !self.query.trim().is_empty() {
            return vec![self.search_command(self.cfg.debounce_ms)];
        }
        self.results.clear();
        self.offline = false;
        self.searching = false;
        self.showing = Showing::Recent;
        let mut cmds = vec![Command::CancelSearch];
        if self.tab == Tab::Search {
            self.rebuild(Sel::First);
            if self.copying.is_none() {
                self.status = self.search_status();
            }
            cmds.extend(self.thumbnails());
        }
        cmds
    }

    fn search_command(&mut self, debounce_ms: u64) -> Command {
        self.searching = true;
        if self.copying.is_none() && self.tab == Tab::Search {
            self.status = Status::Searching;
        }
        Command::Search {
            query: self.query.trim().to_string(),
            target: self.target,
            limit: self.cfg.max_results,
            generation: self.generation,
            debounce_ms,
        }
    }

    fn results(&mut self, generation: u64, outcome: Result<Results, String>) -> Vec<Command> {
        if generation != self.generation || self.query.trim().is_empty() {
            return Vec::new();
        }
        self.searching = false;
        let shown = self.tab == Tab::Search;
        match outcome {
            Ok(mut r) => {
                r.hits.truncate(self.cfg.max_results);
                self.offline = r.offline;
                self.results = r.hits;
                self.results_target = self.target;
                self.showing = Showing::Results;
                if !shown {
                    return Vec::new();
                }
                self.rebuild(Sel::First);
                if self.copying.is_none() {
                    self.status = self.search_status();
                }
                self.thumbnails()
            }
            Err(e) => {
                if self.copying.is_none() && shown {
                    self.status = Status::Error(tf("Search failed: {error}", &[("error", &t_dyn(&e))]));
                }
                Vec::new()
            }
        }
    }

    fn recent_loaded(&mut self, items: Vec<ItemSummary>) -> Vec<Command> {
        let mut items = match &self.window {
            Some(w) => self.history.order(&w.class, items),
            None => items,
        };
        items.truncate(self.cfg.recent_count);
        self.recent = items;
        let shown = match self.tab {
            Tab::Recent => true,
            Tab::Search => self.showing == Showing::Recent,
            _ => false,
        };
        self.list_answered(shown)
    }

    /// New data (or an error) for a list; `shown`: it is on screen now.
    fn list_answered(&mut self, shown: bool) -> Vec<Command> {
        if !shown {
            return Vec::new();
        }
        self.rebuild(Sel::Refresh);
        if self.copying.is_none() && self.status.describes_view() {
            self.status = self.view_status();
        }
        self.thumbnails()
    }

    fn pack_filter_changed(&mut self, q: String) -> Vec<Command> {
        let filter = match &mut self.packs.open {
            Some(open) => &mut open.filter,
            None => &mut self.packs.filter,
        };
        if *filter == q {
            return Vec::new();
        }
        *filter = q;
        self.rebuild(Sel::First);
        if self.copying.is_none() {
            self.status = self.view_status();
        }
        self.thumbnails()
    }

    fn open_pack_at(&mut self, index: usize) -> Vec<Command> {
        if self.view() != View::Packs {
            return Vec::new();
        }
        let Some(set) = self.packs.shown.get(index).cloned() else {
            return Vec::new();
        };
        self.packs.list_selection = Some(set.id.clone());
        let mut items = Listing::default();
        let load = items
            .start(&mut self.requests)
            .map(|generation| Command::LoadPackItems {
                set_id: set.id.clone(),
                generation,
            });
        self.packs.open = Some(OpenPack {
            set,
            items,
            filter: String::new(),
        });
        self.rebuild(Sel::First);
        if self.copying.is_none() {
            self.status = self.view_status();
        }
        let mut cmds: Vec<Command> = load.into_iter().collect();
        cmds.extend(self.thumbnails());
        cmds
    }

    fn pack_back(&mut self) -> Vec<Command> {
        if self.tab != Tab::Packs || self.packs.open.take().is_none() {
            return Vec::new();
        }
        let selection = self.packs.list_selection.take();
        self.rebuild(Sel::Id(selection));
        if self.copying.is_none() {
            self.status = self.view_status();
        }
        self.thumbnails()
    }

    /// What the status says about the shown view when nothing else is to be said.
    fn view_status(&self) -> Status {
        match self.tab {
            Tab::Search => self.search_status(),
            Tab::Recent if self.recent.is_empty() => Status::NothingUsed,
            Tab::MostUsed => self.most_used.status(t("most used items"), Status::NoMostUsed),
            Tab::Favorites => self.favorites.status(t("favorites"), Status::NoFavorites),
            Tab::Packs => match &self.packs.open {
                Some(open) => filtered_status(
                    open.items.status(t("the pack"), Status::EmptyPack),
                    &open.filter,
                    self.entries.is_empty(),
                ),
                None => filtered_status(
                    self.packs.sets.status(t("packs"), Status::NoPacks),
                    &self.packs.filter,
                    self.packs.shown.is_empty(),
                ),
            },
            Tab::Recent | Tab::Me | Tab::Settings => Status::Idle,
        }
    }

    fn search_status(&self) -> Status {
        if self.query.trim().is_empty() {
            return if self.recent.is_empty() {
                Status::NoRecent
            } else {
                Status::Idle
            };
        }
        if self.searching {
            return Status::Searching;
        }
        match self.showing {
            Showing::Results if self.results.is_empty() => Status::NoResults {
                query: self.query.trim().to_string(),
            },
            Showing::Results if self.offline => Status::Offline,
            _ => Status::Idle,
        }
    }

    // -- grid ------------------------------------------------------------------------

    /// Entries in the shown grid (items or packs).
    fn grid_len(&self) -> usize {
        match self.view() {
            View::Items => self.entries.len(),
            View::Packs => self.packs.shown.len(),
            View::Page => 0,
        }
    }

    fn selected_id(&self) -> Option<String> {
        let i = self.selected?;
        match self.view() {
            View::Items => self.entries.get(i).map(|e| e.item.id.clone()),
            View::Packs => self.packs.shown.get(i).map(|s| s.id.clone()),
            View::Page => None,
        }
    }

    fn position(&self, id: &str) -> Option<usize> {
        match self.view() {
            View::Items => self.entries.iter().position(|e| e.item.id == id),
            View::Packs => self.packs.shown.iter().position(|s| s.id == id),
            View::Page => None,
        }
    }

    /// Recompute the shown grid (entries with compatibility, or the filtered
    /// pack list) from the tab's data.
    fn rebuild(&mut self, sel: Sel) {
        let prev_id = self.selected_id();
        let prev = self.selected;
        let target = self.target;
        let sticker = self.sticker && supports_stickers(target);
        let local = |item: &ItemSummary| Entry {
            incompatible: incompatibility(item, target, sticker),
            item: item.clone(),
            matched: Vec::new(),
            liked: false,
        };
        self.entries = match self.tab {
            Tab::Search => match self.showing {
                Showing::Recent => self.recent.iter().map(local).collect(),
                Showing::Results => self
                    .results
                    .iter()
                    .map(|hit| {
                        let server = if self.results_target == target && !sticker {
                            hit.incompatible.clone()
                        } else {
                            None
                        };
                        Entry {
                            incompatible: server.or_else(|| incompatibility(&hit.item, target, sticker)),
                            item: hit.item.clone(),
                            matched: hit.matched.clone(),
                            liked: false,
                        }
                    })
                    .collect(),
            },
            Tab::Recent => self.recent.iter().map(local).collect(),
            Tab::MostUsed => self.most_used.items.iter().map(local).collect(),
            Tab::Favorites => self.favorites.items.iter().map(local).collect(),
            Tab::Packs => match &self.packs.open {
                Some(open) => {
                    let terms = filter_terms(&open.filter);
                    open.items
                        .items
                        .iter()
                        .filter(|i| item_matches(i, &terms))
                        .map(local)
                        .collect()
                }
                None => Vec::new(),
            },
            Tab::Me | Tab::Settings => Vec::new(),
        };
        for e in &mut self.entries {
            e.liked = self.liked.contains(&e.item.id);
        }
        if self.view() == View::Packs {
            let terms = filter_terms(&self.packs.filter);
            self.packs.shown = self
                .packs
                .sets
                .items
                .iter()
                .filter(|s| pack_matches(s, &terms))
                .cloned()
                .collect();
        }
        self.entries_revision += 1;

        let len = self.grid_len();
        if len == 0 {
            self.selected = None;
            return;
        }
        let keep = |m: &Model| {
            prev_id
                .as_deref()
                .and_then(|id| m.position(id))
                .or(prev.map(|i| i.min(len - 1)))
                .unwrap_or(0)
        };
        let i = match sel {
            Sel::First => {
                self.touched = false;
                0
            }
            Sel::Keep => keep(self),
            Sel::Refresh if self.touched => keep(self),
            Sel::Refresh => 0,
            Sel::Id(id) => {
                let found = id.as_deref().and_then(|id| self.position(id));
                self.touched = found.is_some();
                found.unwrap_or(0)
            }
        };
        self.selected = Some(i);
    }

    fn refresh_compat(&mut self) {
        self.rebuild(Sel::Keep);
    }

    fn thumbnails(&self) -> Vec<Command> {
        match self.view() {
            View::Items if !self.entries.is_empty() => vec![Command::LoadThumbnails {
                items: self.entries.iter().map(|e| e.item.clone()).collect(),
            }],
            View::Packs if !self.packs.shown.is_empty() => vec![Command::LoadPackCovers {
                packs: self.packs.shown.clone(),
            }],
            _ => Vec::new(),
        }
    }

    fn cycle_target(&mut self, backwards: bool) -> Vec<Command> {
        let all = TargetApp::ALL;
        let i = all.iter().position(|t| *t == self.target).unwrap_or(0);
        let n = all.len();
        let next = if backwards {
            all[(i + n - 1) % n]
        } else {
            all[(i + 1) % n]
        };
        self.set_target(next)
    }

    fn set_target(&mut self, t: TargetApp) -> Vec<Command> {
        let changed = t != self.target;
        self.target = t;
        self.target_source = TargetSource::Chosen;
        if !changed {
            return Vec::new();
        }
        self.refresh_compat();
        if !self.query.trim().is_empty() {
            // Re-rank with the new target's compatibility hints, right away.
            self.generation += 1;
            return vec![self.search_command(0)];
        }
        Vec::new()
    }

    fn toggle_sticker(&mut self) -> Vec<Command> {
        self.sticker = !self.sticker;
        self.refresh_compat();
        Vec::new()
    }

    fn goto(&mut self, i: usize) {
        let len = self.grid_len();
        if len > 0 {
            self.selected = Some(i.min(len - 1));
            self.touched = true;
        }
    }

    fn navigate(&mut self, nav: Nav) {
        let n = self.grid_len();
        if n == 0 {
            self.selected = None;
            return;
        }
        self.touched = true;
        let Some(i) = self.selected else {
            self.selected = Some(0);
            return;
        };
        let c = self.columns;
        let step = c * self.visible_rows;
        let last = n - 1;
        let row = i / c;
        let last_row = last / c;
        let next = match nav {
            Nav::Left => i.saturating_sub(1),
            Nav::Right => (i + 1).min(last),
            Nav::Up => {
                if i >= c {
                    i - c
                } else {
                    i
                }
            }
            Nav::Down => {
                if row < last_row {
                    (i + c).min(last)
                } else {
                    i
                }
            }
            Nav::Home => 0,
            Nav::End => last,
            Nav::PageUp => {
                if i >= step {
                    i - step
                } else {
                    i % c
                }
            }
            Nav::PageDown => {
                if i + step <= last {
                    i + step
                } else {
                    // Same column on the last row if it exists there, else the last item.
                    let candidate = last_row * c + i % c;
                    if candidate <= last && candidate >= i {
                        candidate
                    } else {
                        last
                    }
                }
            }
        };
        self.selected = Some(next);
    }

    // -- item actions --------------------------------------------------------------------

    fn copy_selected(&mut self, toggle_sticker: bool) -> Vec<Command> {
        match self.selected {
            Some(i) => self.copy_at(i, toggle_sticker),
            None => {
                if self.copying.is_none() {
                    self.status = Status::NothingSelected;
                }
                Vec::new()
            }
        }
    }

    fn copy_at(&mut self, index: usize, toggle_sticker: bool) -> Vec<Command> {
        if self.copying.is_some() {
            return Vec::new();
        }
        let Some(entry) = self.entries.get(index) else {
            return Vec::new();
        };
        let sticker = (self.sticker ^ toggle_sticker) && supports_stickers(self.target);
        let item = entry.item.clone();
        self.copying = Some(InFlight {
            item_id: item.id.clone(),
            name: item.name.clone(),
            target: self.target,
            sticker,
        });
        self.status = Status::Copying {
            name: item.name.clone(),
        };
        vec![Command::Copy {
            item: Box::new(item),
            target: self.target,
            sticker,
        }]
    }

    fn copy_finished(&mut self, item_id: String, warning: Option<String>) -> Vec<Command> {
        let Some(done) = self.copying.take_if(|c| c.item_id == item_id) else {
            return Vec::new();
        };
        self.status = Status::Copied {
            name: done.name,
            target: done.target,
            sticker: done.sticker,
            warning,
        };
        self.after_use(&item_id)
    }

    /// Remember the item for this window and hide (per `hide_on_copy`).
    fn after_use(&mut self, item_id: &str) -> Vec<Command> {
        let mut cmds = Vec::new();
        if let Some(w) = &self.window {
            self.history.record(&w.class, item_id);
            cmds.push(Command::SaveHistory(self.history.clone()));
        }
        if self.cfg.hide_on_copy && self.visible {
            cmds.extend(self.hide());
        }
        cmds
    }

    /// The selected item, or "Nothing selected".
    fn selected_item_id(&mut self) -> Option<String> {
        let id = self.selected_entry().map(|e| e.item.id.clone());
        if id.is_none() && self.copying.is_none() {
            self.status = Status::NothingSelected;
        }
        id
    }

    fn toggle_favorite(&mut self) -> Vec<Command> {
        self.selected_item_id()
            .map(|item_id| Command::ToggleFavorite { item_id })
            .into_iter()
            .collect()
    }

    fn favorite_toggled(&mut self, item_id: &str, outcome: Result<bool, String>) -> Vec<Command> {
        let status = match outcome {
            Ok(liked) => {
                if liked {
                    self.liked.insert(item_id.to_string());
                } else {
                    self.liked.remove(item_id);
                    self.favorites.items.retain(|i| i.id != item_id);
                }
                if self.view() == View::Items && (self.tiles.favorites || self.tab == Tab::Favorites) {
                    self.rebuild(Sel::Keep);
                }
                Status::Favorite { added: liked }
            }
            Err(e) => Status::Error(tf("Favorite failed: {error}", &[("error", &t_dyn(&e))])),
        };
        if self.copying.is_none() {
            self.status = status;
        }
        Vec::new()
    }

    fn open_in_browser(&mut self) -> Vec<Command> {
        self.selected_item_id()
            .map(|item_id| Command::OpenInBrowser { item_id })
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::focus::FocusBackend;
    use stickpile_api::{Variant, VariantInfo};

    pub(crate) fn item(id: &str) -> ItemSummary {
        ItemSummary {
            id: id.into(),
            set_id: "set1".into(),
            set_slug: "cats".into(),
            set_name: "Cats".into(),
            owner_username: "me".into(),
            name: format!("item-{id}"),
            keywords: vec!["cat".into()],
            emojis: vec!["😺".into()],
            vibe: None,
            tags: vec![],
            kind: MediaKind::Still,
            format: MediaFormat::Png,
            width: 512,
            height: 512,
            quality: Some(80),
            likes: 0,
            copies: 0,
            media_id: format!("m-{id}"),
            sha256: "00".into(),
            phash: "00".into(),
            variants: vec![
                VariantInfo {
                    variant: Variant::Original,
                    mime: "image/png".into(),
                    width: 512,
                    height: 512,
                    size_bytes: 1000,
                },
                VariantInfo {
                    variant: Variant::Thumb,
                    mime: "image/webp".into(),
                    width: 256,
                    height: 256,
                    size_bytes: 100,
                },
            ],
            updated_at: 0,
        }
    }

    pub(crate) fn items(n: usize) -> Vec<ItemSummary> {
        (0..n).map(|i| item(&i.to_string())).collect()
    }

    fn hit(id: &str) -> SearchHit {
        SearchHit {
            item: item(id),
            score: 1.0,
            matched: vec!["keyword".into()],
            incompatible: None,
        }
    }

    pub(crate) fn hits(n: usize) -> Vec<SearchHit> {
        (0..n).map(|i| hit(&format!("h{i}"))).collect()
    }

    pub(crate) fn window(class: &str) -> WindowInfo {
        WindowInfo {
            class: class.into(),
            title: "chat".into(),
            backend: FocusBackend::Hyprland,
        }
    }

    /// Favorite marks off, so an opening asks only for its own tab's data;
    /// `tiles_tests` covers them.
    pub(crate) fn cfg() -> ModelConfig {
        ModelConfig {
            columns: 4,
            visible_rows: 2,
            recent_count: 5,
            max_results: 10,
            tiles: Tiles {
                favorites: false,
                ..Tiles::default()
            },
            ..ModelConfig::default()
        }
    }

    pub(crate) fn opened(cfg: ModelConfig, detected: Option<TargetApp>) -> Model {
        let mut m = Model::new(cfg);
        let window = detected.map(|_| window("com.ayugram.desktop"));
        m.handle(Event::Open(OpenInfo { window, detected }));
        m
    }

    /// Model showing `n` search results for query "cat".
    pub(crate) fn with_results(n: usize) -> Model {
        let mut m = opened(cfg(), None);
        m.handle(Event::QueryChanged("cat".into()));
        let generation = m.generation();
        m.handle(Event::Results {
            generation,
            outcome: Ok(Results {
                hits: hits(n),
                offline: false,
            }),
        });
        m
    }

    pub(crate) fn search_of(cmds: &[Command]) -> Option<(String, TargetApp, u64, u64)> {
        cmds.iter().find_map(|c| match c {
            Command::Search {
                query,
                target,
                generation,
                debounce_ms,
                ..
            } => Some((query.clone(), *target, *generation, *debounce_ms)),
            _ => None,
        })
    }

    pub(crate) fn ids(m: &Model) -> Vec<String> {
        m.entries().iter().map(|e| e.item.id.clone()).collect()
    }

    // -- open / toggle / hide ---------------------------------------------------

    #[test]
    fn starts_hidden_with_defaults() {
        let m = Model::new(ModelConfig {
            default_target: TargetApp::Discord,
            prefer_stickers: true,
            ..cfg()
        });
        assert!(!m.is_visible());
        assert!(m.entries().is_empty());
        assert_eq!(m.selected(), None);
        assert_eq!(m.target(), TargetApp::Discord);
        assert_eq!(m.target_source(), &TargetSource::Default);
        assert!(m.sticker());
    }

    #[test]
    fn open_with_detected_target() {
        let mut m = Model::new(cfg());
        let cmds = m.handle(Event::Open(OpenInfo {
            window: Some(window("com.ayugram.desktop")),
            detected: Some(TargetApp::Telegram),
        }));
        assert_eq!(cmds[0], Command::Present);
        assert!(cmds.contains(&Command::LoadRecent { limit: 5 }));
        assert!(m.is_visible());
        assert_eq!(m.target(), TargetApp::Telegram);
        assert_eq!(
            m.target_source(),
            &TargetSource::Detected {
                class: "com.ayugram.desktop".into()
            }
        );
        assert_eq!(m.footer().target_hint, "auto: com.ayugram.desktop");
        assert_eq!(m.window().unwrap().class, "com.ayugram.desktop");
    }

    #[test]
    fn open_without_match_uses_default_target() {
        let mut m = Model::new(ModelConfig {
            default_target: TargetApp::Slack,
            ..cfg()
        });
        m.handle(Event::Open(OpenInfo {
            window: Some(window("firefox")),
            detected: None,
        }));
        assert_eq!(m.target(), TargetApp::Slack);
        assert_eq!(m.target_source(), &TargetSource::Default);
        assert_eq!(m.footer().target_hint, "default");
        m.handle(Event::Open(OpenInfo::default()));
        assert_eq!(m.target(), TargetApp::Slack);
    }

    #[test]
    fn reopen_resets_session_state() {
        let mut m = with_results(3);
        m.handle(Event::CycleTarget { backwards: false });
        m.handle(Event::ToggleSticker);
        m.handle(Event::Hide);
        let cmds = m.handle(Event::Open(OpenInfo::default()));
        assert!(cmds.contains(&Command::CancelSearch));
        assert_eq!(m.query(), "");
        assert!(m.entries().is_empty(), "recent list not loaded yet");
        assert_eq!(m.target(), TargetApp::Generic);
        assert!(m.sticker(), "the sticker preference is not reset on open");
        assert_eq!(m.status(), &Status::NoRecent);
    }

    #[test]
    fn toggle_hides_only_an_active_visible_popup() {
        let mut m = Model::new(cfg());
        let cmds = m.handle(Event::Toggle {
            popup_active: false,
            open: OpenInfo::default(),
        });
        assert_eq!(cmds[0], Command::Present);
        assert!(m.is_visible());
        // Visible but another window has focus: bring it back instead of hiding.
        let cmds = m.handle(Event::Toggle {
            popup_active: false,
            open: OpenInfo::default(),
        });
        assert_eq!(cmds[0], Command::Present);
        let cmds = m.handle(Event::Toggle {
            popup_active: true,
            open: OpenInfo::default(),
        });
        assert_eq!(cmds, vec![Command::Hide]);
        assert!(!m.is_visible());
        // Hidden: even a stale "active" flag opens it.
        let cmds = m.handle(Event::Toggle {
            popup_active: true,
            open: OpenInfo::default(),
        });
        assert_eq!(cmds[0], Command::Present);
    }

    #[test]
    fn hide_and_hidden() {
        let mut m = opened(cfg(), None);
        assert_eq!(m.handle(Event::Hide), vec![Command::Hide]);
        assert!(!m.is_visible());
        assert!(m.handle(Event::Hide).is_empty(), "already hidden");
        let mut m = opened(cfg(), None);
        assert!(m.handle(Event::Hidden).is_empty());
        assert!(!m.is_visible());
        // Without modes, Escape hides as it always did.
        let mut m = opened(ModelConfig { modal: false, ..cfg() }, None);
        assert_eq!(m.handle(Event::Escape), vec![Command::Hide]);
        assert!(!m.is_visible());
    }

    // -- recent -------------------------------------------------------------------

    #[test]
    fn recent_items_fill_the_grid() {
        let mut m = opened(cfg(), None);
        assert_eq!(m.status(), &Status::NoRecent);
        let cmds = m.handle(Event::RecentLoaded(items(8)));
        assert_eq!(ids(&m), ["0", "1", "2", "3", "4"], "capped at recent_count");
        assert_eq!(m.selected(), Some(0));
        assert_eq!(m.status(), &Status::Idle);
        match &cmds[..] {
            [Command::LoadThumbnails { items }] => assert_eq!(items.len(), 5),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn empty_recent_shows_hint() {
        let mut m = opened(cfg(), None);
        assert!(m.handle(Event::RecentLoaded(vec![])).is_empty());
        assert_eq!(m.status(), &Status::NoRecent);
        assert_eq!(m.footer().status, "Nothing used yet — type to search");
        assert_eq!(m.selected(), None);
    }

    #[test]
    fn recent_arriving_during_search_does_not_replace_results() {
        let mut m = with_results(2);
        assert!(m.handle(Event::RecentLoaded(items(3))).is_empty());
        assert_eq!(ids(&m), ["h0", "h1"]);
        m.handle(Event::QueryChanged(String::new()));
        assert_eq!(ids(&m), ["0", "1", "2"]);
    }

    #[test]
    fn recent_ordered_by_window_history() {
        let mut history = WindowHistory::default();
        history.record("com.ayugram.desktop", "3");
        let mut m = Model::with_history(cfg(), history);
        m.handle(Event::Open(OpenInfo {
            window: Some(window("com.ayugram.desktop")),
            detected: Some(TargetApp::Telegram),
        }));
        m.handle(Event::RecentLoaded(items(4)));
        assert_eq!(ids(&m), ["3", "0", "1", "2"]);
    }

    // -- search ---------------------------------------------------------------------

    #[test]
    fn typing_issues_debounced_search() {
        let mut m = opened(
            ModelConfig {
                debounce_ms: 120,
                ..cfg()
            },
            Some(TargetApp::Telegram),
        );
        let g0 = m.generation();
        let cmds = m.handle(Event::QueryChanged("  naahh ".into()));
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            Command::Search {
                query,
                target,
                limit,
                generation,
                debounce_ms,
            } => {
                assert_eq!(query, "naahh");
                assert_eq!(*target, TargetApp::Telegram);
                assert_eq!(*limit, 10);
                assert_eq!(*generation, g0 + 1);
                assert_eq!(*debounce_ms, 120);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(m.status(), &Status::Searching);
        assert_eq!(m.footer().level, Level::Busy);
        assert_eq!(m.query(), "  naahh ");
    }

    #[test]
    fn unchanged_query_is_a_no_op() {
        let mut m = opened(cfg(), None);
        m.handle(Event::QueryChanged("cat".into()));
        let g = m.generation();
        assert!(m.handle(Event::QueryChanged("cat".into())).is_empty());
        assert_eq!(m.generation(), g);
    }

    #[test]
    fn results_replace_entries_and_select_first() {
        let m = with_results(3);
        assert_eq!(ids(&m), ["h0", "h1", "h2"]);
        assert_eq!(m.selected(), Some(0));
        assert_eq!(m.status(), &Status::Idle);
        assert_eq!(m.entries()[0].matched, ["keyword"]);
        assert!(!m.is_offline());
    }

    #[test]
    fn results_emit_thumbnail_loads_and_bump_revision() {
        let mut m = opened(cfg(), None);
        m.handle(Event::QueryChanged("cat".into()));
        let rev = m.entries_revision();
        let g = m.generation();
        let cmds = m.handle(Event::Results {
            generation: g,
            outcome: Ok(Results {
                hits: hits(2),
                offline: false,
            }),
        });
        assert!(m.entries_revision() > rev);
        assert!(matches!(&cmds[..], [Command::LoadThumbnails { items }] if items.len() == 2));
    }

    #[test]
    fn stale_results_are_dropped() {
        let mut m = opened(cfg(), None);
        m.handle(Event::QueryChanged("ca".into()));
        let old = m.generation();
        m.handle(Event::QueryChanged("cat".into()));
        let rev = m.entries_revision();
        let cmds = m.handle(Event::Results {
            generation: old,
            outcome: Ok(Results {
                hits: hits(3),
                offline: false,
            }),
        });
        assert!(cmds.is_empty());
        assert_eq!(m.entries_revision(), rev);
        assert_eq!(m.status(), &Status::Searching);
        let cmds = m.handle(Event::Results {
            generation: old,
            outcome: Err("boom".into()),
        });
        assert!(cmds.is_empty());
        assert_eq!(m.status(), &Status::Searching);
    }

    #[test]
    fn results_after_clearing_query_are_dropped() {
        let mut m = opened(cfg(), None);
        m.handle(Event::RecentLoaded(items(2)));
        m.handle(Event::QueryChanged("cat".into()));
        let g = m.generation();
        let cmds = m.handle(Event::QueryChanged(" ".into()));
        assert!(cmds.contains(&Command::CancelSearch));
        assert!(search_of(&cmds).is_none());
        m.handle(Event::Results {
            generation: g,
            outcome: Ok(Results {
                hits: hits(3),
                offline: false,
            }),
        });
        assert_eq!(ids(&m), ["0", "1"]);
    }

    #[test]
    fn offline_results_are_flagged() {
        let mut m = opened(cfg(), None);
        m.handle(Event::QueryChanged("cat".into()));
        let g = m.generation();
        m.handle(Event::Results {
            generation: g,
            outcome: Ok(Results {
                hits: hits(1),
                offline: true,
            }),
        });
        assert_eq!(m.status(), &Status::Offline);
        let f = m.footer();
        assert!(f.offline);
        assert_eq!(f.level, Level::Warning);
        assert_eq!(f.status, "Offline — showing cached results");
        m.handle(Event::QueryChanged(String::new()));
        assert!(!m.is_offline(), "recent list is not an offline result");
    }

    #[test]
    fn no_results_status() {
        let mut m = opened(cfg(), None);
        m.handle(Event::QueryChanged("zzz".into()));
        let g = m.generation();
        m.handle(Event::Results {
            generation: g,
            outcome: Ok(Results::default()),
        });
        assert!(m.entries().is_empty());
        assert_eq!(m.selected(), None);
        assert_eq!(m.footer().status, "No matches for “zzz”");
    }

    #[test]
    fn search_error_keeps_entries() {
        let mut m = with_results(2);
        m.handle(Event::QueryChanged("cats".into()));
        let g = m.generation();
        m.handle(Event::Results {
            generation: g,
            outcome: Err("server said no".into()),
        });
        assert_eq!(ids(&m), ["h0", "h1"]);
        assert_eq!(m.status(), &Status::Error("Search failed: server said no".into()));
        assert_eq!(m.footer().level, Level::Error);
    }

    #[test]
    fn results_are_capped_at_max_results() {
        let m = with_results(25);
        assert_eq!(m.entries().len(), 10);
    }

    // -- navigation ---------------------------------------------------------------------

    fn nav(m: &mut Model, n: Nav) -> Option<usize> {
        assert!(m.handle(Event::Navigate(n)).is_empty());
        m.selected()
    }

    fn select(m: &mut Model, i: usize) {
        m.handle(Event::Select(i));
        assert_eq!(m.selected(), Some(i));
    }

    #[test]
    fn left_right_clamp() {
        let mut m = with_results(10); // 4 columns: rows [0..4) [4..8) [8..10)
        assert_eq!(nav(&mut m, Nav::Left), Some(0));
        assert_eq!(nav(&mut m, Nav::Right), Some(1));
        select(&mut m, 9);
        assert_eq!(nav(&mut m, Nav::Right), Some(9));
        assert_eq!(nav(&mut m, Nav::Left), Some(8));
    }

    #[test]
    fn up_down_by_rows() {
        let mut m = with_results(10);
        select(&mut m, 1);
        assert_eq!(nav(&mut m, Nav::Up), Some(1), "top row stays");
        assert_eq!(nav(&mut m, Nav::Down), Some(5));
        assert_eq!(nav(&mut m, Nav::Down), Some(9));
        assert_eq!(nav(&mut m, Nav::Down), Some(9), "last row stays");
        select(&mut m, 7);
        assert_eq!(nav(&mut m, Nav::Down), Some(9), "partial last row: clamp to last item");
        assert_eq!(nav(&mut m, Nav::Up), Some(5));
        assert_eq!(nav(&mut m, Nav::Up), Some(1));
    }

    #[test]
    fn home_end() {
        let mut m = with_results(10);
        assert_eq!(nav(&mut m, Nav::End), Some(9));
        assert_eq!(nav(&mut m, Nav::Home), Some(0));
    }

    #[test]
    fn page_up_down() {
        // 4 columns x 2 visible rows = 8 per page.
        let mut m = with_results(10);
        select(&mut m, 1);
        assert_eq!(nav(&mut m, Nav::PageDown), Some(9), "same column on the last row");
        select(&mut m, 2);
        assert_eq!(
            nav(&mut m, Nav::PageDown),
            Some(9),
            "column 2 missing on last row: last item"
        );
        assert_eq!(nav(&mut m, Nav::PageUp), Some(1));
        assert_eq!(nav(&mut m, Nav::PageUp), Some(1), "top row keeps its column");
        select(&mut m, 0);
        assert_eq!(nav(&mut m, Nav::PageDown), Some(8));
        assert_eq!(
            nav(&mut m, Nav::PageDown),
            Some(8),
            "already on the last row in this column"
        );
        select(&mut m, 9);
        assert_eq!(nav(&mut m, Nav::PageDown), Some(9));
    }

    #[test]
    fn layout_event_changes_geometry() {
        let mut m = with_results(10);
        m.handle(Event::Layout {
            columns: 3,
            visible_rows: 1,
        });
        assert_eq!(m.columns(), 3);
        assert_eq!(nav(&mut m, Nav::Down), Some(3));
        assert_eq!(nav(&mut m, Nav::PageDown), Some(6));
        m.handle(Event::Layout {
            columns: 0,
            visible_rows: 0,
        });
        assert_eq!(m.columns(), 1);
        assert_eq!(nav(&mut m, Nav::Down), Some(7));
    }

    #[test]
    fn navigation_without_entries_or_selection() {
        let mut m = opened(cfg(), None);
        assert_eq!(nav(&mut m, Nav::Down), None);
        m.handle(Event::RecentLoaded(items(3)));
        m.selected = None;
        assert_eq!(nav(&mut m, Nav::End), Some(0), "first key selects the first item");
    }

    #[test]
    fn pointer_selection_out_of_range_is_ignored() {
        let mut m = with_results(3);
        m.handle(Event::Select(2));
        m.handle(Event::Select(7));
        assert_eq!(m.selected(), Some(2));
    }

    // -- copy -------------------------------------------------------------------------

    #[test]
    fn enter_copies_selected_item() {
        let mut m = opened(cfg(), Some(TargetApp::Telegram));
        m.handle(Event::RecentLoaded(items(3)));
        m.handle(Event::Navigate(Nav::Right));
        let cmds = m.handle(Event::CopySelected { toggle_sticker: false });
        match &cmds[..] {
            [Command::Copy { item, target, sticker }] => {
                assert_eq!(item.id, "1");
                assert_eq!(*target, TargetApp::Telegram);
                assert!(!sticker);
            }
            other => panic!("{other:?}"),
        }
        assert!(m.is_copying());
        assert_eq!(m.footer().status, "Preparing item-1…");
        assert_eq!(m.footer().level, Level::Busy);
    }

    #[test]
    fn shift_enter_toggles_sticker_for_one_copy() {
        let mut m = opened(cfg(), Some(TargetApp::Telegram));
        m.handle(Event::RecentLoaded(items(2)));
        let cmds = m.handle(Event::CopySelected { toggle_sticker: true });
        assert!(matches!(&cmds[..], [Command::Copy { sticker: true, .. }]));
        assert!(!m.sticker(), "persistent preference unchanged");
        m.handle(Event::CopyFinished {
            item_id: "0".into(),
            warning: None,
        });
        assert_eq!(m.status().text(), "Copied item-0 as sticker for Telegram");

        // With prefer_stickers, Shift+Enter means "not as sticker".
        let mut m = opened(
            ModelConfig {
                prefer_stickers: true,
                ..cfg()
            },
            Some(TargetApp::Telegram),
        );
        m.handle(Event::RecentLoaded(items(2)));
        assert!(matches!(
            &m.handle(Event::CopySelected { toggle_sticker: false })[..],
            [Command::Copy { sticker: true, .. }]
        ));
        m.handle(Event::CopyFailed {
            item_id: "0".into(),
            error: "x".into(),
        });
        assert!(matches!(
            &m.handle(Event::CopySelected { toggle_sticker: true })[..],
            [Command::Copy { sticker: false, .. }]
        ));
    }

    #[test]
    fn sticker_never_requested_for_targets_without_stickers() {
        let mut m = opened(
            ModelConfig {
                prefer_stickers: true,
                ..cfg()
            },
            Some(TargetApp::Discord),
        );
        m.handle(Event::RecentLoaded(items(1)));
        assert_eq!(m.sticker_mode(), StickerMode::Unsupported);
        let cmds = m.handle(Event::CopySelected { toggle_sticker: false });
        assert!(matches!(
            &cmds[..],
            [Command::Copy {
                sticker: false,
                target: TargetApp::Discord,
                ..
            }]
        ));
    }

    #[test]
    fn copy_with_nothing_selected() {
        let mut m = opened(cfg(), None);
        assert!(m.handle(Event::CopySelected { toggle_sticker: false }).is_empty());
        assert_eq!(m.status(), &Status::NothingSelected);
        assert!(m
            .handle(Event::CopyAt {
                index: 3,
                toggle_sticker: false
            })
            .is_empty());
    }

    #[test]
    fn copy_at_selects_and_copies() {
        let mut m = with_results(5);
        let cmds = m.handle(Event::CopyAt {
            index: 3,
            toggle_sticker: false,
        });
        assert_eq!(m.selected(), Some(3));
        assert!(matches!(&cmds[..], [Command::Copy { item, .. }] if item.id == "h3"));
    }

    #[test]
    fn second_copy_while_busy_is_ignored() {
        let mut m = with_results(3);
        m.handle(Event::CopySelected { toggle_sticker: false });
        assert!(m.handle(Event::CopySelected { toggle_sticker: false }).is_empty());
        assert!(m
            .handle(Event::CopyAt {
                index: 2,
                toggle_sticker: false
            })
            .is_empty());
        assert_eq!(m.status(), &Status::Copying { name: "item-h0".into() });
    }

    #[test]
    fn copy_finished_hides_and_reports() {
        let mut m = opened(cfg(), Some(TargetApp::Telegram));
        m.handle(Event::RecentLoaded(items(2)));
        m.handle(Event::CopySelected { toggle_sticker: false });
        let cmds = m.handle(Event::CopyFinished {
            item_id: "0".into(),
            warning: None,
        });
        assert_eq!(cmds.last(), Some(&Command::Hide));
        assert!(!m.is_visible());
        assert!(!m.is_copying());
        assert_eq!(m.footer().status, "Copied item-0 for Telegram");
        assert_eq!(m.footer().level, Level::Info);
    }

    #[test]
    fn copy_finished_records_window_history() {
        let mut m = opened(cfg(), Some(TargetApp::Telegram));
        m.handle(Event::RecentLoaded(items(2)));
        m.handle(Event::Select(1));
        m.handle(Event::CopySelected { toggle_sticker: false });
        let cmds = m.handle(Event::CopyFinished {
            item_id: "1".into(),
            warning: None,
        });
        let saved = cmds.iter().find_map(|c| match c {
            Command::SaveHistory(h) => Some(h.clone()),
            _ => None,
        });
        assert_eq!(saved.unwrap().ids("com.ayugram.desktop"), ["1"]);
        assert_eq!(m.history().ids("com.ayugram.desktop"), ["1"]);

        // Unknown window: nothing to save.
        let mut m = opened(cfg(), None);
        m.handle(Event::RecentLoaded(items(1)));
        m.handle(Event::CopySelected { toggle_sticker: false });
        let cmds = m.handle(Event::CopyFinished {
            item_id: "0".into(),
            warning: None,
        });
        assert_eq!(cmds, vec![Command::Hide]);
    }

    #[test]
    fn copy_finished_without_hide_on_copy() {
        let mut m = opened(
            ModelConfig {
                hide_on_copy: false,
                ..cfg()
            },
            None,
        );
        m.handle(Event::RecentLoaded(items(2)));
        m.handle(Event::CopySelected { toggle_sticker: false });
        assert!(m
            .handle(Event::CopyFinished {
                item_id: "0".into(),
                warning: None
            })
            .is_empty());
        assert!(m.is_visible());
    }

    #[test]
    fn copy_warning_is_shown() {
        let mut m = opened(
            ModelConfig {
                hide_on_copy: false,
                ..cfg()
            },
            Some(TargetApp::Discord),
        );
        m.handle(Event::RecentLoaded(items(1)));
        m.handle(Event::CopySelected { toggle_sticker: false });
        m.handle(Event::CopyFinished {
            item_id: "0".into(),
            warning: Some("Discord limit is 10 MiB".into()),
        });
        let f = m.footer();
        assert_eq!(f.status, "Copied item-0 for Discord — Discord limit is 10 MiB");
        assert_eq!(f.level, Level::Warning);
    }

    #[test]
    fn copy_failure_keeps_popup_and_allows_retry() {
        let mut m = with_results(2);
        m.handle(Event::CopySelected { toggle_sticker: false });
        assert!(m
            .handle(Event::CopyFailed {
                item_id: "h0".into(),
                error: "offline and not cached".into()
            })
            .is_empty());
        assert!(m.is_visible());
        assert_eq!(m.footer().status, "Copy failed: offline and not cached");
        assert_eq!(m.footer().level, Level::Error);
        assert_eq!(m.handle(Event::CopySelected { toggle_sticker: false }).len(), 1);
    }

    #[test]
    fn answers_for_other_items_are_ignored() {
        let mut m = with_results(2);
        m.handle(Event::CopySelected { toggle_sticker: false });
        assert!(m
            .handle(Event::CopyFinished {
                item_id: "h1".into(),
                warning: None
            })
            .is_empty());
        assert!(m
            .handle(Event::CopyFailed {
                item_id: "h1".into(),
                error: "x".into()
            })
            .is_empty());
        assert!(m.is_copying());
        let mut idle = with_results(1);
        assert!(idle
            .handle(Event::CopyFinished {
                item_id: "h0".into(),
                warning: None
            })
            .is_empty());
        assert_eq!(idle.status(), &Status::Idle);
    }

    #[test]
    fn copy_status_survives_search_traffic() {
        let mut m = with_results(2);
        m.handle(Event::CopySelected { toggle_sticker: false });
        m.handle(Event::QueryChanged("dog".into()));
        assert_eq!(m.status(), &Status::Copying { name: "item-h0".into() });
        let g = m.generation();
        m.handle(Event::Results {
            generation: g,
            outcome: Ok(Results {
                hits: vec![],
                offline: false,
            }),
        });
        assert_eq!(m.status(), &Status::Copying { name: "item-h0".into() });
        // The copied item is no longer listed, but the answer still lands.
        m.handle(Event::CopyFinished {
            item_id: "h0".into(),
            warning: None,
        });
        assert_eq!(m.status().text(), "Copied item-h0 for Other / browser");
    }

    // -- target & sticker -----------------------------------------------------------------

    #[test]
    fn cycle_target_wraps_both_ways() {
        let mut m = opened(cfg(), None);
        assert_eq!(m.target(), TargetApp::Generic);
        m.handle(Event::CycleTarget { backwards: false });
        assert_eq!(m.target(), TargetApp::Telegram);
        assert_eq!(m.target_source(), &TargetSource::Chosen);
        assert_eq!(m.footer().target_hint, "chosen");
        m.handle(Event::CycleTarget { backwards: true });
        assert_eq!(m.target(), TargetApp::Generic);
        m.handle(Event::CycleTarget { backwards: true });
        assert_eq!(m.target(), TargetApp::Slack);
        for _ in 0..TargetApp::ALL.len() {
            m.handle(Event::CycleTarget { backwards: false });
        }
        assert_eq!(m.target(), TargetApp::Slack);
    }

    #[test]
    fn target_change_researches_immediately() {
        let mut m = with_results(2);
        let g = m.generation();
        let cmds = m.handle(Event::SetTarget(TargetApp::Discord));
        let (query, target, generation, debounce) = search_of(&cmds).unwrap();
        assert_eq!((query.as_str(), target, debounce), ("cat", TargetApp::Discord, 0));
        assert_eq!(generation, g + 1);
        assert!(
            m.handle(Event::SetTarget(TargetApp::Discord)).is_empty(),
            "same target: nothing to do"
        );
    }

    #[test]
    fn target_change_in_recent_mode_does_not_search() {
        let mut m = opened(cfg(), None);
        m.handle(Event::RecentLoaded(items(2)));
        let rev = m.entries_revision();
        let cmds = m.handle(Event::SetTarget(TargetApp::Telegram));
        assert!(cmds.is_empty());
        assert!(m.entries_revision() > rev, "compatibility recomputed");
        assert_eq!(m.selected(), Some(0));
    }

    #[test]
    fn target_change_keeps_selection() {
        let mut m = opened(cfg(), None);
        m.handle(Event::RecentLoaded(items(4)));
        m.handle(Event::Select(2));
        m.handle(Event::CycleTarget { backwards: false });
        assert_eq!(m.selected(), Some(2));
    }

    #[test]
    fn sticker_toggle_and_indicator() {
        let mut m = opened(cfg(), Some(TargetApp::Telegram));
        assert_eq!(m.sticker_mode(), StickerMode::Off);
        m.handle(Event::ToggleSticker);
        assert_eq!(m.sticker_mode(), StickerMode::On);
        assert_eq!(m.footer().sticker, StickerMode::On);
        m.handle(Event::SetTarget(TargetApp::Whatsapp));
        assert_eq!(m.sticker_mode(), StickerMode::Unsupported);
        assert!(m.sticker(), "preference kept for the next sticker-capable target");
    }

    #[test]
    fn sticker_preference_starts_from_prefer_stickers() {
        let m = opened(cfg(), Some(TargetApp::Telegram));
        assert!(!m.sticker());
        assert_eq!(m.footer().sticker, StickerMode::Off);
        let m = opened(
            ModelConfig {
                prefer_stickers: true,
                ..cfg()
            },
            Some(TargetApp::Telegram),
        );
        assert!(m.sticker());
        assert_eq!(m.footer().sticker, StickerMode::On);
    }

    #[test]
    fn sticker_toggle_survives_close_and_reopen() {
        let telegram = || OpenInfo {
            window: Some(window("com.ayugram.desktop")),
            detected: Some(TargetApp::Telegram),
        };
        let mut m = opened(cfg(), Some(TargetApp::Telegram));
        m.handle(Event::ToggleSticker);
        // Hiding, the shortcut again, and the WM close button all keep it.
        m.handle(Event::Hide);
        m.handle(Event::Open(telegram()));
        assert_eq!(m.sticker_mode(), StickerMode::On);
        m.handle(Event::Toggle {
            popup_active: true,
            open: OpenInfo::default(),
        });
        m.handle(Event::Toggle {
            popup_active: false,
            open: telegram(),
        });
        assert_eq!(m.sticker_mode(), StickerMode::On);
        m.handle(Event::Hidden);
        m.handle(Event::Open(telegram()));
        assert_eq!(m.footer().sticker, StickerMode::On);
        m.handle(Event::RecentLoaded(items(2)));
        assert!(matches!(
            &m.handle(Event::CopySelected { toggle_sticker: false })[..],
            [Command::Copy { sticker: true, .. }]
        ));

        // Turning it off again also sticks, even against prefer_stickers.
        let mut m = opened(
            ModelConfig {
                prefer_stickers: true,
                ..cfg()
            },
            Some(TargetApp::Telegram),
        );
        m.handle(Event::ToggleSticker);
        m.handle(Event::Hide);
        m.handle(Event::Open(telegram()));
        assert_eq!(m.sticker_mode(), StickerMode::Off);
    }

    #[test]
    fn shift_enter_does_not_carry_over_to_the_next_opening() {
        let mut m = opened(cfg(), Some(TargetApp::Telegram));
        m.handle(Event::RecentLoaded(items(2)));
        assert!(matches!(
            &m.handle(Event::CopySelected { toggle_sticker: true })[..],
            [Command::Copy { sticker: true, .. }]
        ));
        m.handle(Event::Hide);
        m.handle(Event::Open(OpenInfo {
            window: Some(window("com.ayugram.desktop")),
            detected: Some(TargetApp::Telegram),
        }));
        assert_eq!(m.sticker_mode(), StickerMode::Off);
    }

    // -- compatibility & entries ---------------------------------------------------------

    #[test]
    fn recent_compatibility_is_computed_locally() {
        let mut big = item("big");
        big.kind = MediaKind::Video;
        big.format = MediaFormat::Mp4;
        big.variants = vec![VariantInfo {
            variant: Variant::Original,
            mime: "video/mp4".into(),
            width: 1920,
            height: 1080,
            size_bytes: 50 * 1024 * 1024,
        }];
        let mut m = opened(cfg(), Some(TargetApp::Discord));
        m.handle(Event::RecentLoaded(vec![item("ok"), big]));
        assert_eq!(m.entries()[0].incompatible, None);
        let why = m.entries()[1].incompatible.clone().unwrap();
        assert!(why.contains("10 MiB"), "{why}");
        assert!(m.entries()[1].tooltip().contains("⚠"));
        m.handle(Event::SetTarget(TargetApp::Telegram));
        assert_eq!(m.entries()[1].incompatible, None);
    }

    #[test]
    fn sticker_mode_flags_items_without_sticker_variant() {
        let mut m = opened(cfg(), Some(TargetApp::Telegram));
        let mut sticker = item("s");
        sticker.variants.push(VariantInfo {
            variant: Variant::TgSticker,
            mime: "image/webp".into(),
            width: 512,
            height: 512,
            size_bytes: 40_000,
        });
        m.handle(Event::RecentLoaded(vec![item("plain"), sticker]));
        assert!(m.entries().iter().all(|e| e.incompatible.is_none()));
        m.handle(Event::ToggleSticker);
        assert!(m.entries()[0].incompatible.as_deref().unwrap().contains("sticker"));
        assert_eq!(m.entries()[1].incompatible, None);
    }

    #[test]
    fn server_incompatibility_applies_to_its_target_only() {
        let mut m = opened(cfg(), Some(TargetApp::Discord));
        m.handle(Event::QueryChanged("cat".into()));
        let g = m.generation();
        let mut h = hit("x");
        h.incompatible = Some("server says too big".into());
        m.handle(Event::Results {
            generation: g,
            outcome: Ok(Results {
                hits: vec![h],
                offline: false,
            }),
        });
        assert_eq!(m.entries()[0].incompatible.as_deref(), Some("server says too big"));
        m.handle(Event::SetTarget(TargetApp::Telegram));
        assert_eq!(
            m.entries()[0].incompatible,
            None,
            "stale server hint dropped until new results arrive"
        );
    }

    #[test]
    fn unknown_variants_are_not_flagged() {
        let mut it = item("bare");
        it.variants.clear();
        assert_eq!(incompatibility(&it, TargetApp::Telegram, true), None);
    }

    #[test]
    fn entry_badges_and_tooltip() {
        let mut e = Entry {
            item: item("a"),
            incompatible: None,
            matched: vec![],
            liked: false,
        };
        assert_eq!(e.badge(), None);
        assert!(!e.is_animated());
        assert_eq!(e.tooltip(), "item-a\nCats\n😺 cat");
        e.item.kind = MediaKind::Animation;
        e.item.format = MediaFormat::Gif;
        assert_eq!(e.badge(), Some("GIF"));
        assert!(e.is_animated());
        e.item.format = MediaFormat::Webp;
        assert_eq!(e.badge(), Some("WEBP"));
        e.item.format = MediaFormat::Mp4;
        assert_eq!(e.badge(), Some("ANIM"));
        e.item.kind = MediaKind::Video;
        assert_eq!(e.badge(), Some("VIDEO"));
        e.item.emojis.clear();
        e.item.keywords.clear();
        assert_eq!(e.tooltip(), "item-a\nCats");
    }

    #[test]
    fn drop_reports_records_and_hides() {
        let mut m = opened(cfg(), Some(TargetApp::Telegram));
        m.handle(Event::RecentLoaded(items(2)));
        let cmds = m.handle(Event::Dropped {
            item_id: "1".into(),
            name: "item-1".into(),
        });
        assert!(matches!(&cmds[..], [Command::SaveHistory(_), Command::Hide]));
        assert_eq!(m.footer().status, "Dropped item-1");
        assert_eq!(m.history().ids("com.ayugram.desktop"), ["1"]);
        // Already hidden: no second Hide.
        let cmds = m.handle(Event::Dropped {
            item_id: "0".into(),
            name: "item-0".into(),
        });
        assert!(matches!(&cmds[..], [Command::SaveHistory(_)]));
    }

    #[test]
    fn status_texts() {
        assert_eq!(Status::Idle.text(), "");
        assert_eq!(Status::Searching.text(), "Searching…");
        assert_eq!(Status::NothingSelected.level(), Level::Info);
        let copied = Status::Copied {
            name: "cat".into(),
            target: TargetApp::Telegram,
            sticker: false,
            warning: None,
        };
        assert_eq!(copied.text(), "Copied cat for Telegram");
        assert_eq!(Status::Error("x".into()).level(), Level::Error);
    }
}

#[cfg(test)]
mod modal_tests;

#[cfg(test)]
mod tiles_tests;
