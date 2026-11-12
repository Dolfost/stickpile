//! Popup state machine.
//!
//! The shell feeds [`Event`]s into [`Model::handle`] and executes the returned
//! [`Command`]s (search, copy, hide, load thumbnails); it then re-renders from
//! the model's read-only view ([`Model::entries`], [`Model::selected`],
//! [`Model::footer`]). Nothing here knows about a toolkit, a clock or the
//! network, so every behaviour is unit-tested below.
//!
//! Async results are tagged: every [`Command::Search`] carries a generation
//! number and [`Event::Results`] for any other generation are dropped, so a
//! slow answer to an old query can never overwrite a newer one. Debouncing is
//! the shell's job; the command carries a `debounce_ms` hint (0 when the user
//! did not type, e.g. after switching the target app).

use stickpile_api::profiles::{self, ItemShape};
use stickpile_api::{ItemSummary, MediaFormat, MediaKind, SearchHit, TargetApp};

use crate::focus::WindowInfo;
use crate::history::WindowHistory;

/// Default delay between the last keystroke and the search request.
pub const DEFAULT_DEBOUNCE_MS: u64 = 120;

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
    /// The window was hidden or closed by something else (WM close button).
    Hidden,
    QueryChanged(String),
    Results {
        generation: u64,
        outcome: Result<Results, String>,
    },
    /// Most recently used items, newest first (answer to [`Command::LoadRecent`]).
    RecentLoaded(Vec<ItemSummary>),
    /// The grid's real geometry, if it differs from the configured one.
    Layout {
        columns: usize,
        visible_rows: usize,
    },
    Navigate(Nav),
    /// Pointer selection.
    Select(usize),
    /// Enter (`toggle_sticker: false`) or Shift+Enter (`true`).
    CopySelected {
        toggle_sticker: bool,
    },
    /// Double click / activation of a grid cell.
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
    Escape,
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
    /// Make sure these items' thumbnails are loaded (the shell skips ones it
    /// already has or is loading). Sent whenever the visible entries change.
    LoadThumbnails {
        items: Vec<ItemSummary>,
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
}

impl Status {
    pub fn level(&self) -> Level {
        match self {
            Status::Idle
            | Status::NoResults { .. }
            | Status::NoRecent
            | Status::NothingSelected
            | Status::Dropped { .. } => Level::Info,
            Status::Searching | Status::Copying { .. } => Level::Busy,
            Status::Offline => Level::Warning,
            Status::Copied { warning, .. } => {
                if warning.is_some() {
                    Level::Warning
                } else {
                    Level::Info
                }
            }
            Status::Error(_) => Level::Error,
        }
    }

    pub fn text(&self) -> String {
        match self {
            Status::Idle => String::new(),
            Status::Searching => "Searching…".into(),
            Status::NoResults { query } => format!("No matches for “{query}”"),
            Status::NoRecent => "Nothing used yet — type to search".into(),
            Status::Offline => "Offline — showing cached results".into(),
            Status::NothingSelected => "Nothing selected".into(),
            Status::Copying { name } => format!("Preparing {name}…"),
            Status::Copied {
                name,
                target,
                sticker,
                warning,
            } => {
                let what = if *sticker { " as sticker" } else { "" };
                let mut s = format!("Copied {name}{what} for {}", target_name(*target));
                if let Some(w) = warning {
                    s.push_str(" — ");
                    s.push_str(w);
                }
                s
            }
            Status::Dropped { name } => format!("Dropped {name}"),
            Status::Error(e) => e.clone(),
        }
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
            s.push_str(why);
        }
        s
    }
}

pub fn target_name(t: TargetApp) -> String {
    profiles::profile(t).display_name
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Showing {
    Recent,
    Results,
}

#[derive(Debug, Clone)]
pub struct Model {
    cfg: ModelConfig,
    visible: bool,
    query: String,
    recent: Vec<ItemSummary>,
    results: Vec<SearchHit>,
    offline: bool,
    /// Target the current results were searched for (server hints apply to it only).
    results_target: TargetApp,
    showing: Showing,
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
}

/// The copy currently being prepared.
#[derive(Debug, Clone, PartialEq)]
struct InFlight {
    item_id: String,
    name: String,
    target: TargetApp,
    sticker: bool,
}

impl Model {
    pub fn new(cfg: ModelConfig) -> Model {
        Model::with_history(cfg, WindowHistory::default())
    }

    /// Start with a per-window history loaded by the shell.
    pub fn with_history(cfg: ModelConfig, history: WindowHistory) -> Model {
        Model {
            visible: false,
            query: String::new(),
            recent: Vec::new(),
            results: Vec::new(),
            offline: false,
            results_target: cfg.default_target,
            showing: Showing::Recent,
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
            cfg,
        }
    }

    // -- read-only view ------------------------------------------------------

    pub fn config(&self) -> &ModelConfig {
        &self.cfg
    }
    pub fn is_visible(&self) -> bool {
        self.visible
    }
    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    /// Bumped whenever [`Model::entries`] changes, so the shell can skip
    /// rebuilding its list model.
    pub fn entries_revision(&self) -> u64 {
        self.entries_revision
    }
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }
    pub fn selected_entry(&self) -> Option<&Entry> {
        self.selected.and_then(|i| self.entries.get(i))
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
        self.showing == Showing::Results && self.offline
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
            TargetSource::Default => "default".to_string(),
            TargetSource::Detected { class } => format!("auto: {class}"),
            TargetSource::Chosen => "chosen".to_string(),
        };
        Footer {
            target: target_name(self.target),
            target_hint,
            sticker: self.sticker_mode(),
            status: self.status.text(),
            level: self.status.level(),
            offline: self.is_offline(),
        }
    }

    // -- events ----------------------------------------------------------------

    pub fn handle(&mut self, event: Event) -> Vec<Command> {
        match event {
            Event::Open(info) => self.open(info),
            Event::Toggle { popup_active, open } => {
                if self.visible && popup_active {
                    self.hide()
                } else {
                    self.open(open)
                }
            }
            Event::Hidden => {
                self.visible = false;
                Vec::new()
            }
            Event::QueryChanged(q) => self.query_changed(q),
            Event::Results { generation, outcome } => self.results(generation, outcome),
            Event::RecentLoaded(items) => self.recent_loaded(items),
            Event::Layout { columns, visible_rows } => {
                self.columns = columns.max(1);
                self.visible_rows = visible_rows.max(1);
                Vec::new()
            }
            Event::Navigate(nav) => {
                self.navigate(nav);
                Vec::new()
            }
            Event::Select(i) => {
                if i < self.entries.len() {
                    self.selected = Some(i);
                }
                Vec::new()
            }
            Event::CopySelected { toggle_sticker } => match self.selected {
                Some(i) => self.copy_at(i, toggle_sticker),
                None => {
                    if self.copying.is_none() {
                        self.status = Status::NothingSelected;
                    }
                    Vec::new()
                }
            },
            Event::CopyAt { index, toggle_sticker } => {
                if index < self.entries.len() {
                    self.selected = Some(index);
                    self.copy_at(index, toggle_sticker)
                } else {
                    Vec::new()
                }
            }
            Event::CycleTarget { backwards } => {
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
            Event::SetTarget(t) => self.set_target(t),
            Event::ToggleSticker => {
                self.sticker = !self.sticker;
                self.refresh_compat();
                Vec::new()
            }
            Event::Escape => self.hide(),
            Event::CopyFinished { item_id, warning } => self.copy_finished(item_id, warning),
            Event::CopyFailed { item_id, error } => {
                if self.copying.as_ref().is_none_or(|c| c.item_id != item_id) {
                    return Vec::new();
                }
                self.copying = None;
                self.status = Status::Error(format!("Copy failed: {error}"));
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
        self.show_recent();
        let mut cmds = vec![Command::Present, Command::CancelSearch];
        cmds.push(Command::LoadRecent {
            limit: self.cfg.recent_count,
        });
        cmds.extend(self.thumbnails());
        cmds
    }

    fn hide(&mut self) -> Vec<Command> {
        self.visible = false;
        vec![Command::Hide]
    }

    fn query_changed(&mut self, q: String) -> Vec<Command> {
        if q == self.query {
            return Vec::new();
        }
        self.query = q;
        self.generation += 1;
        if self.query.trim().is_empty() {
            self.results.clear();
            self.offline = false;
            if self.copying.is_none() {
                self.status = Status::Idle;
            }
            self.show_recent();
            let mut cmds = vec![Command::CancelSearch];
            cmds.extend(self.thumbnails());
            return cmds;
        }
        vec![self.search_command(self.cfg.debounce_ms)]
    }

    fn search_command(&mut self, debounce_ms: u64) -> Command {
        if self.copying.is_none() {
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
        match outcome {
            Ok(mut r) => {
                r.hits.truncate(self.cfg.max_results);
                self.offline = r.offline;
                self.results = r.hits;
                self.results_target = self.target;
                self.showing = Showing::Results;
                self.rebuild_entries(true);
                if self.copying.is_none() {
                    self.status = if self.results.is_empty() {
                        Status::NoResults {
                            query: self.query.trim().to_string(),
                        }
                    } else if self.offline {
                        Status::Offline
                    } else {
                        Status::Idle
                    };
                }
                self.thumbnails()
            }
            Err(e) => {
                if self.copying.is_none() {
                    self.status = Status::Error(format!("Search failed: {e}"));
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
        if self.showing != Showing::Recent {
            return Vec::new();
        }
        self.show_recent();
        self.thumbnails()
    }

    fn show_recent(&mut self) {
        self.showing = Showing::Recent;
        self.rebuild_entries(true);
        if self.recent.is_empty() && self.status == Status::Idle {
            self.status = Status::NoRecent;
        } else if !self.recent.is_empty() && self.status == Status::NoRecent {
            self.status = Status::Idle;
        }
    }

    fn rebuild_entries(&mut self, reset_selection: bool) {
        let sticker = self.sticker && supports_stickers(self.target);
        self.entries = match self.showing {
            Showing::Recent => self
                .recent
                .iter()
                .map(|item| Entry {
                    incompatible: incompatibility(item, self.target, sticker),
                    item: item.clone(),
                    matched: Vec::new(),
                })
                .collect(),
            Showing::Results => self
                .results
                .iter()
                .map(|hit| {
                    let server = if self.results_target == self.target && !sticker {
                        hit.incompatible.clone()
                    } else {
                        None
                    };
                    Entry {
                        incompatible: server.or_else(|| incompatibility(&hit.item, self.target, sticker)),
                        item: hit.item.clone(),
                        matched: hit.matched.clone(),
                    }
                })
                .collect(),
        };
        self.entries_revision += 1;
        if reset_selection || self.selected.is_some_and(|i| i >= self.entries.len()) {
            self.selected = if self.entries.is_empty() { None } else { Some(0) };
        }
    }

    fn refresh_compat(&mut self) {
        self.rebuild_entries(false);
    }

    fn thumbnails(&self) -> Vec<Command> {
        if self.entries.is_empty() {
            return Vec::new();
        }
        vec![Command::LoadThumbnails {
            items: self.entries.iter().map(|e| e.item.clone()).collect(),
        }]
    }

    fn set_target(&mut self, t: TargetApp) -> Vec<Command> {
        let changed = t != self.target;
        self.target = t;
        self.target_source = TargetSource::Chosen;
        if !changed {
            return Vec::new();
        }
        self.refresh_compat();
        if self.showing == Showing::Results || !self.query.trim().is_empty() {
            // Re-rank with the new target's compatibility hints, right away.
            self.generation += 1;
            return vec![self.search_command(0)];
        }
        Vec::new()
    }

    fn navigate(&mut self, nav: Nav) {
        let n = self.entries.len();
        if n == 0 {
            self.selected = None;
            return;
        }
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

    fn items(n: usize) -> Vec<ItemSummary> {
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

    fn hits(n: usize) -> Vec<SearchHit> {
        (0..n).map(|i| hit(&format!("h{i}"))).collect()
    }

    fn window(class: &str) -> WindowInfo {
        WindowInfo {
            class: class.into(),
            title: "chat".into(),
            backend: FocusBackend::Hyprland,
        }
    }

    fn cfg() -> ModelConfig {
        ModelConfig {
            columns: 4,
            visible_rows: 2,
            recent_count: 5,
            max_results: 10,
            ..ModelConfig::default()
        }
    }

    fn opened(cfg: ModelConfig, detected: Option<TargetApp>) -> Model {
        let mut m = Model::new(cfg);
        let window = detected.map(|_| window("com.ayugram.desktop"));
        m.handle(Event::Open(OpenInfo { window, detected }));
        m
    }

    /// Model showing `n` search results for query "cat".
    fn with_results(n: usize) -> Model {
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

    fn search_of(cmds: &[Command]) -> Option<(String, TargetApp, u64, u64)> {
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

    fn ids(m: &Model) -> Vec<String> {
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
        m.handle(Event::Escape);
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
    fn escape_and_hidden() {
        let mut m = opened(cfg(), None);
        assert_eq!(m.handle(Event::Escape), vec![Command::Hide]);
        assert!(!m.is_visible());
        let mut m = opened(cfg(), None);
        assert!(m.handle(Event::Hidden).is_empty());
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
        // Escape, the shortcut again, and the WM close button all keep it.
        m.handle(Event::Escape);
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
        m.handle(Event::Escape);
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
        m.handle(Event::Escape);
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
