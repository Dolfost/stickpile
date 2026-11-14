//! The GTK shell: builds the popup window, renders the `stickpile-quick`
//! model and executes its commands on the tokio runtime. It holds no popup
//! logic of its own; anything decidable without a display lives in the
//! model or in the display-free modules of this crate.
//!
//! Focus follows the model's mode: in Insert the text field of the tab (or
//! the page field the user went into) has the keyboard focus, in Normal no
//! text field has it. Focus changes the user causes are reported back as
//! `Event::TextFocus`, always from an idle callback, never while the model
//! is borrowed for rendering.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use adw::prelude::*;
use futures::FutureExt;
use gtk::{gdk, gio, glib};
use stickpile_api::{ItemSummary, SetSummary, TargetApp};
use stickpile_core::settings as config_file;
use stickpile_core::{ClientConfig, PreparedCopy, Session};
use stickpile_i18n::global::{t, tf};
use stickpile_i18n::Lang;
use stickpile_quick::focus::FocusEnv;
use stickpile_quick::model::{self, Command, Event, Level, Model, OpenInfo, StickerMode, Tile};
use stickpile_quick::{Mode, Tab, View};

use crate::backend::{Backend, BoxFut, CoreBackend, LoginError};
use crate::demo::DemoBackend;
use crate::keys::{self, KeyPress, Mods};
use crate::ui::drag::{self, LazyFileProvider, PreparedFuture};
use crate::ui::item::StickerObject;
use crate::ui::me::{MeAction, MePage};
use crate::ui::packs::{self, PackObject};
use crate::ui::settings::{SettingsAction, SettingsPage};
use crate::ui::tabbar::TabBar;
use crate::ui::{help, set_class};
use crate::{glue, offers, pages, sync, thumbs};

/// Concurrent thumbnail downloads/decodes.
const THUMB_CONCURRENCY: usize = 6;
/// Textures kept beyond the ones on screen.
const THUMB_CACHE: usize = 600;
/// Prepared copies remembered for drag-and-drop.
const PREPARED_CACHE: usize = 16;
const SESSION_OPEN_TIMEOUT: Duration = Duration::from_secs(3);

/// Cells are `thumb_px` plus [`glue::CELL_EXTRA_PX`]: the child padding here.
const CSS: &str = r#"
gridview.stickpile-grid { border-spacing: 0; }
gridview.stickpile-grid > child { padding: 8px; border-radius: 10px; }
gridview.stickpile-grid > child:selected { background-color: alpha(@accent_bg_color, 0.45); }
gridview.stickpile-grid > child:selected:hover { background-color: alpha(@accent_bg_color, 0.55); }
.stickpile-tile-row { margin: 3px; }
.stickpile-badge { font-size: 0.7em; font-weight: bold; padding: 1px 5px; border-radius: 5px; }
.stickpile-warn { color: @warning_color; }
.stickpile-pill { font-size: 0.9em; padding: 0 4px; border-radius: 6px; }
.stickpile-tags { font-size: 0.7em; padding: 1px 5px; }
label.stickpile-heart { color: #ff5470; }
.stickpile-footer { padding: 4px 10px; }
.stickpile-footer .stickpile-sticker { min-height: 0; min-width: 0; padding: 0 6px; font-weight: normal; }
.stickpile-tabs { padding: 6px 8px 0 8px; }
.stickpile-tabs button.stickpile-tab { min-height: 26px; padding: 2px 9px; font-weight: normal; }
.stickpile-tab-number { font-size: 0.8em; font-feature-settings: "tnum"; opacity: 0.55; }
.stickpile-tabs button.stickpile-current { background-color: alpha(@accent_bg_color, 0.16); color: @accent_color; font-weight: bold; }
.stickpile-tabs button.stickpile-current .stickpile-tab-number { opacity: 0.9; }
.stickpile-header { padding: 6px 8px; min-height: 34px; }
.stickpile-mode { font-size: 0.75em; font-weight: bold; padding: 1px 6px; border-radius: 4px; }
.stickpile-mode.insert { background-color: @accent_bg_color; color: @accent_fg_color; }
.stickpile-mode.normal { background-color: alpha(currentColor, 0.12); }
.stickpile-pending { font-family: monospace; font-weight: bold; }
.stickpile-help { font-size: 0.92em; background-color: @popover_bg_color; color: @popover_fg_color; border-radius: 12px; box-shadow: 0 2px 14px alpha(black, 0.35); }
.stickpile-help-key { font-family: monospace; font-weight: bold; }
.stickpile-pack-name { font-weight: bold; }
.stickpile-stat { padding: 10px 6px; }
row.stickpile-cursor { background-color: alpha(@accent_bg_color, 0.14); box-shadow: inset 3px 0 @accent_bg_color; }
button.stickpile-cursor { outline: 2px solid alpha(@accent_bg_color, 0.8); outline-offset: 2px; }
"#;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("stickpile-rt")
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

/// Startup options from the first invocation.
pub struct StartOptions {
    pub config_path: PathBuf,
    pub demo: Option<PathBuf>,
}

enum Thumb {
    Loading,
    Ready(gdk::Texture),
    Failed,
}

struct DragState {
    item: ItemSummary,
    prepared: PreparedFuture,
    cancelled: bool,
}

struct Widgets {
    window: adw::ApplicationWindow,
    stack: gtk::Stack,
    error: adw::StatusPage,
    tabs: TabBar,
    back: gtk::Button,
    title: gtk::Label,
    entry: gtk::SearchEntry,
    count: gtk::Label,
    /// items, packs, me, settings
    content: gtk::Stack,
    help: gtk::Widget,
    grid: gtk::GridView,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    pack_grid: gtk::GridView,
    pack_store: gio::ListStore,
    pack_selection: gtk::SingleSelection,
    mode: gtk::Label,
    pending: gtk::Label,
    target: gtk::Label,
    target_hint: gtk::Label,
    /// Footer sticker-mode toggle; clicking it is Ctrl+S.
    sticker: gtk::ToggleButton,
    sticker_label: gtk::Label,
    status: gtk::Label,
}

pub struct Shell {
    cfg: RefCell<ClientConfig>,
    config_path: PathBuf,
    config_error: Option<String>,
    demo_mode: bool,
    demo: Option<Arc<DemoBackend>>,
    backend: RefCell<Option<Arc<dyn Backend>>>,
    backend_error: RefCell<Option<String>>,
    model: RefCell<Model>,
    w: Widgets,
    me: Rc<MePage>,
    settings: Rc<SettingsPage>,
    /// Widgets are being updated from the model: their signals are not input.
    rendering: Cell<bool>,
    rendered_revision: Cell<u64>,
    scrolled_to: Cell<Option<(View, usize)>>,
    /// The tab on screen at the last render (`None`: hidden), to notice a
    /// page coming into view.
    shown: Cell<Option<Tab>>,
    /// The grid geometry last reported to the model.
    layout: Cell<Option<(usize, usize)>>,
    /// A layout check is queued for the next idle moment (a grid resized).
    layout_queued: Cell<bool>,
    /// Where the tab the popup closed on is kept (`ui.start_tab = "last"`),
    /// and the tab last written there.
    last_tab_path: PathBuf,
    saved_tab: Cell<Option<Tab>>,
    /// The account's language (from the server, kept in the cache for the
    /// next start), and where it is kept.
    account_lang: Cell<Option<Lang>>,
    account_lang_path: PathBuf,
    thumbs: RefCell<HashMap<String, Thumb>>,
    /// Pack covers by [`cover_key`].
    covers: RefCell<HashMap<String, Thumb>>,
    thumb_permits: Arc<tokio::sync::Semaphore>,
    prepared: RefCell<VecDeque<PreparedCopy>>,
    drag: RefCell<Option<DragState>>,
    search_timer: RefCell<Option<glib::SourceId>>,
    sync: RefCell<sync::SyncPolicy>,
    history_path: PathBuf,
}

/// A cover changes with the set's cover item or image.
fn cover_key(set: &SetSummary) -> String {
    format!(
        "{}\n{}\n{}",
        set.id,
        set.cover_item_id.as_deref().unwrap_or_default(),
        set.cover_url.as_deref().unwrap_or_default()
    )
}

fn texture(rgba: thumbs::Rgba) -> gdk::Texture {
    let stride = rgba.stride();
    gdk::MemoryTexture::new(
        rgba.width as i32,
        rgba.height as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(rgba.pixels),
        stride,
    )
    .upcast()
}

impl Shell {
    pub fn start(app: &adw::Application, opts: StartOptions) -> Rc<Shell> {
        let (cfg, config_error) = match ClientConfig::load(&opts.config_path) {
            Ok(c) => (c, None),
            Err(e) => (ClientConfig::default(), Some(e.to_string())),
        };
        let account_lang_path = glue::account_lang_path(&cfg);
        let account_lang = glue::load_account_lang(&account_lang_path);
        stickpile_i18n::global::set(glue::effective_lang(&cfg, account_lang, |k| std::env::var(k).ok()));
        let history_path = glue::history_path(&cfg);
        let history = glue::load_history(&history_path);
        let mut model = Model::with_history(glue::model_config(&cfg), history);
        let last_tab_path = glue::last_tab_path(&cfg);
        let saved_tab = glue::load_last_tab(&last_tab_path);
        if let Some(tab) = saved_tab {
            model.restore_tab(tab);
        }
        let me = MePage::new();
        let settings = SettingsPage::new();
        let w = build_widgets(app, &cfg, &me, &settings);
        let mut backend_error = None;
        let demo = match (&opts.demo, &config_error) {
            (Some(dir), None) => match DemoBackend::scan(dir, &cfg) {
                Ok(b) => {
                    tracing::info!("demo mode: {} files from {}", b.len(), dir.display());
                    Some(Arc::new(b))
                }
                Err(e) => {
                    backend_error = Some(format!("{}: {e}", dir.display()));
                    None
                }
            },
            _ => None,
        };
        let shell = Rc::new(Shell {
            config_path: opts.config_path,
            config_error,
            demo_mode: opts.demo.is_some(),
            backend: RefCell::new(demo.clone().map(|d| d as Arc<dyn Backend>)),
            demo,
            backend_error: RefCell::new(backend_error),
            model: RefCell::new(model),
            w,
            me,
            settings,
            rendering: Cell::new(false),
            rendered_revision: Cell::new(u64::MAX),
            scrolled_to: Cell::new(None),
            shown: Cell::new(None),
            layout: Cell::new(None),
            layout_queued: Cell::new(false),
            last_tab_path,
            saved_tab: Cell::new(saved_tab),
            account_lang: Cell::new(account_lang),
            account_lang_path,
            thumbs: RefCell::new(HashMap::new()),
            covers: RefCell::new(HashMap::new()),
            thumb_permits: Arc::new(tokio::sync::Semaphore::new(THUMB_CONCURRENCY)),
            prepared: RefCell::new(VecDeque::new()),
            drag: RefCell::new(None),
            search_timer: RefCell::new(None),
            sync: RefCell::new(sync::SyncPolicy::default()),
            history_path,
            cfg: RefCell::new(cfg),
        });
        if shell.config_error.is_none() && !shell.demo_mode {
            shell.open_session(false);
        }
        shell.connect_signals();
        shell.start_sync_timer();
        shell.render();
        shell.refresh_account_lang();
        shell
    }

    // -- language ----------------------------------------------------------------------

    /// Ask the server for the account's language (the web UI's setting).
    fn refresh_account_lang(self: &Rc<Self>) {
        let Some(b) = self.backend() else { return };
        self.spawn(b.profile(), |this, out| {
            if let Some(Ok(p)) = out {
                this.account_lang_known(&p);
            }
        });
    }

    /// A profile arrived: follow its language when it changed.
    fn account_lang_known(self: &Rc<Self>, p: &crate::backend::Profile) {
        if p.auth == crate::backend::AuthKind::None {
            return;
        }
        let lang = p.language.as_deref().and_then(Lang::parse);
        if self.account_lang.replace(lang) != lang {
            if let Err(e) = glue::save_account_lang(&self.account_lang_path, lang) {
                tracing::debug!("saving the account's language: {e}");
            }
            self.apply_language();
        }
    }

    /// Show everything in the language the config and the account ask for.
    fn apply_language(self: &Rc<Self>) {
        let lang = glue::effective_lang(&self.cfg.borrow(), self.account_lang.get(), |k| std::env::var(k).ok());
        if lang == stickpile_i18n::global::get() {
            return;
        }
        tracing::info!("language: {lang}");
        stickpile_i18n::global::set(lang);
        self.w.tabs.relabel();
        help::relabel(&self.w.help);
        self.me.relabel();
        self.settings.relabel();
        relabel_widgets(&self.w);
        if self.model.borrow().tab() == Tab::Settings {
            self.reload_settings();
        }
        // Model texts (status, titles) are made when shown.
        self.rendered_revision.set(u64::MAX);
        self.render();
    }

    /// (Re)open the core session. Runs inside the runtime so a panic in the
    /// core cannot take the resident process down. `login_required`: what
    /// the old session learned about the login (same server and token file).
    fn open_session(&self, login_required: bool) {
        let cfg = self.cfg.borrow().clone();
        let rt = runtime();
        let task = rt.spawn(Session::open(cfg));
        // Opening only loads the token and the cache; never let it freeze the UI.
        match rt.block_on(async { tokio::time::timeout(SESSION_OPEN_TIMEOUT, task).await }) {
            Ok(Ok(Ok(session))) => {
                // Still no token: whatever the old session learned still holds.
                let carried = login_required && !session.is_logged_in();
                *self.backend.borrow_mut() = Some(Arc::new(CoreBackend::new(session, carried)));
                *self.backend_error.borrow_mut() = None;
            }
            Ok(Ok(Err(e))) => self.session_failed(crate::backend::describe(&e)),
            Ok(Err(e)) => self.session_failed(tf("stickpile-core failed: {error}", &[("error", &e)])),
            Err(_) => self.session_failed(t("opening the local cache timed out").into()),
        }
    }

    fn session_failed(&self, error: String) {
        tracing::warn!("opening the session: {error}");
        *self.backend_error.borrow_mut() = Some(error);
    }

    fn backend(&self) -> Option<Arc<dyn Backend>> {
        self.backend.borrow().clone()
    }

    fn login_required(&self) -> bool {
        self.backend().is_some_and(|b| b.login_required())
    }

    /// `ui.start_hidden` of the current config.
    pub fn start_hidden(&self) -> bool {
        self.cfg.borrow().ui.start_hidden
    }

    // -- entry points from the application -------------------------------------

    /// Show the popup; the focused window is captured first.
    pub fn open(self: &Rc<Self>, env: &FocusEnv) {
        let info = glue::capture_focus(&self.cfg.borrow(), env);
        self.dispatch(Event::Open(info));
        self.login_prompt();
    }

    pub fn toggle(self: &Rc<Self>, env: &FocusEnv) {
        let active = self.w.window.is_visible() && self.w.window.is_active();
        let open = if active {
            OpenInfo::default()
        } else {
            glue::capture_focus(&self.cfg.borrow(), env)
        };
        self.dispatch(Event::Toggle {
            popup_active: active,
            open,
        });
        self.login_prompt();
    }

    pub fn hide(self: &Rc<Self>) {
        if self.model.borrow().is_visible() {
            self.dispatch(Event::Hide);
        }
    }

    /// The server rejected us: nothing works until a login, so the popup
    /// opens on the Me tab with its form.
    fn login_prompt(self: &Rc<Self>) {
        if self.model.borrow().is_visible() && self.login_required() {
            self.dispatch(Event::ShowTab(Tab::Me));
        }
    }

    // -- model loop --------------------------------------------------------------------

    pub fn dispatch(self: &Rc<Self>, event: Event) {
        let commands = self.model.borrow_mut().handle(event);
        self.render();
        self.after_render();
        for c in commands {
            self.execute(c);
        }
    }

    /// What a render calls for that must not run inside it: the grid
    /// geometry for the model, and loading a page that came into view.
    fn after_render(self: &Rc<Self>) {
        let (tab, view, visible) = {
            let m = self.model.borrow();
            (m.tab(), m.view(), m.is_visible())
        };
        let layout = self.grid_layout(view);
        if self.layout.get() != Some(layout) {
            self.layout.set(Some(layout));
            self.model.borrow_mut().handle(Event::Layout {
                columns: layout.0,
                visible_rows: layout.1,
            });
        }
        let shown = visible.then_some(tab);
        if shown != self.shown.get() {
            self.shown.set(shown);
            match shown {
                Some(Tab::Me) => self.load_profile(),
                Some(Tab::Settings) => self.reload_settings(),
                _ => {}
            }
        }
    }

    /// Columns and visible rows of the grid `view` shows, as GTK lays it
    /// out now; before it has a size, what the config asks for.
    fn grid_layout(&self, view: View) -> (usize, usize) {
        let cfg = self.cfg.borrow();
        let cell = (cfg.ui.thumb_px + glue::CELL_EXTRA_PX) as i32;
        let (grid, fallback) = match view {
            View::Packs => (&self.w.pack_grid, (cell, cell + glue::PACK_LABELS_PX as i32)),
            _ => (&self.w.grid, (cell, cell)),
        };
        measure_grid(grid, fallback).unwrap_or_else(|| {
            let rows = match view {
                View::Packs => glue::pack_rows(&cfg),
                _ => glue::VISIBLE_ROWS as usize,
            };
            (cfg.ui.columns.max(1) as usize, rows)
        })
    }

    /// A grid got a new size: tell the model its geometry once GTK is done
    /// allocating.
    fn queue_layout(self: &Rc<Self>) {
        if self.layout_queued.replace(true) {
            return;
        }
        let this = self.clone();
        glib::idle_add_local_once(move || {
            this.layout_queued.set(false);
            if this.model.try_borrow_mut().is_ok() {
                this.after_render();
            }
        });
    }

    /// Keep the tab the popup closed on, for the next start.
    fn remember_tab(&self) {
        let tab = self.model.borrow().tab();
        if self.saved_tab.replace(Some(tab)) == Some(tab) {
            return;
        }
        let path = self.last_tab_path.clone();
        runtime().spawn_blocking(move || {
            if let Err(e) = glue::save_last_tab(&path, tab) {
                tracing::warn!("saving {}: {e}", path.display());
            }
        });
    }

    /// A `z` key showed or hid something on the tiles: save it like the
    /// Settings tab would. A key `config.toml` sets stays as it is there;
    /// the change then lasts until the popup restarts.
    fn save_tile(self: &Rc<Self>, tile: Tile, on: bool) {
        let key = glue::tile_key(tile);
        let layered = match config_file::load_layered(&self.config_path) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("saving {key}: {e}");
                return;
            }
        };
        if layered.locked.contains(key) {
            tracing::info!("{key} is set in {}; not saved", self.config_path.display());
            return;
        }
        if let Err(e) = config_file::set_value(&layered.write_path, key, toml::Value::Boolean(on)) {
            tracing::warn!("saving {key}: {e}");
            return;
        }
        self.reload_settings();
    }

    fn execute(self: &Rc<Self>, command: Command) {
        match command {
            Command::Present => self.present(),
            Command::Hide => {
                self.remember_tab();
                self.w.window.set_visible(false)
            }
            Command::Search {
                query,
                target,
                limit,
                generation,
                debounce_ms,
            } => self.schedule_search(query, target, limit, generation, debounce_ms),
            Command::CancelSearch => self.cancel_search_timer(),
            Command::LoadRecent { limit } => {
                let Some(b) = self.backend() else { return };
                self.spawn(b.recent(limit), |this, items| {
                    this.dispatch(Event::RecentLoaded(items.unwrap_or_default()));
                });
            }
            Command::LoadMostUsed { limit, generation } => self.answer(
                move |b| b.most_used(limit),
                move |outcome| Event::MostUsedLoaded { generation, outcome },
            ),
            Command::LoadFavorites { limit, generation } => self.answer(
                move |b| b.favorites(limit),
                move |outcome| Event::FavoritesLoaded { generation, outcome },
            ),
            Command::LoadPacks { generation } => {
                self.answer(|b| b.packs(), move |outcome| Event::PacksLoaded { generation, outcome })
            }
            Command::LoadPackItems { set_id, generation } => {
                let id = set_id.clone();
                self.answer(
                    move |b| b.pack_items(id),
                    move |outcome| Event::PackItemsLoaded {
                        set_id,
                        generation,
                        outcome,
                    },
                )
            }
            Command::ToggleFavorite { item_id } => {
                let id = item_id.clone();
                self.answer(
                    move |b| b.toggle_favorite(id),
                    move |outcome| Event::FavoriteToggled { item_id, outcome },
                )
            }
            Command::LoadThumbnails { items } => self.load_thumbnails(items),
            Command::LoadPackCovers { packs } => self.load_covers(packs),
            Command::Copy { item, target, sticker } => self.copy(*item, target, sticker),
            Command::SaveTile { tile, on } => self.save_tile(tile, on),
            Command::SaveHistory(history) => {
                let path = self.history_path.clone();
                runtime().spawn_blocking(move || {
                    if let Err(e) = glue::save_history(&path, &history) {
                        tracing::warn!("saving {}: {e}", path.display());
                    }
                });
            }
            Command::OpenInBrowser { item_id } => match self.backend().and_then(|b| b.web_url(&item_id)) {
                Some(url) => self.open_uri(&url),
                None => tracing::warn!("no web page for {item_id}"),
            },
            Command::Page(key) => {
                let tab = self.model.borrow().tab();
                tracing::debug!("page key {key:?} on {tab:?}");
                match tab {
                    Tab::Me => self.me.key(key),
                    Tab::Settings => self.settings.key(key),
                    _ => {}
                }
            }
            // A stale timer is ignored by the model, so none is cancelled.
            Command::KeyTimeout { after_ms, seq } => {
                let this = self.clone();
                glib::timeout_add_local_once(Duration::from_millis(after_ms), move || {
                    this.dispatch(Event::KeyTimeout { seq });
                });
            }
        }
    }

    /// Run a backend request and hand its outcome to the model; every
    /// request is answered, with an error when there is no backend.
    fn answer<T: Send + 'static>(
        self: &Rc<Self>,
        request: impl FnOnce(Arc<dyn Backend>) -> BoxFut<Result<T, String>>,
        event: impl FnOnce(Result<T, String>) -> Event + 'static,
    ) {
        let Some(b) = self.backend() else {
            self.dispatch(event(Err(t("not connected").into())));
            return;
        };
        self.spawn(request(b), move |this, out| {
            let outcome = out.unwrap_or_else(|| Err(t("internal error (see log)").into()));
            this.dispatch(event(outcome));
        });
    }

    /// Run `fut` on the runtime and hand its output to `done` on the GTK main
    /// loop (`None` if the task panicked).
    fn spawn<T: Send + 'static>(self: &Rc<Self>, fut: BoxFut<T>, done: impl FnOnce(&Rc<Shell>, Option<T>) + 'static) {
        let handle = runtime().spawn(fut);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let out = match handle.await {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::error!("background task failed: {e}");
                    None
                }
            };
            done(&this, out);
        });
    }

    fn open_uri(&self, uri: &str) {
        let uri = uri.to_string();
        gtk::UriLauncher::new(&uri).launch(Some(&self.w.window), gio::Cancellable::NONE, move |r| {
            if let Err(e) = r {
                tracing::warn!("opening {uri}: {e}");
            }
        });
    }

    // -- rendering -------------------------------------------------------------------------

    fn render(&self) {
        let m = self.model.borrow();
        let w = &self.w;
        let was_rendering = self.rendering.replace(true);
        let tab = m.tab();
        let view = m.view();

        w.tabs.render(tab);
        w.title.set_label(&m.title());
        w.back.set_visible(tab == Tab::Packs && m.open_pack().is_some());
        let field = tab.has_text_field();
        w.entry.set_visible(field);
        w.title.set_hexpand(!field);
        let placeholder = match (tab, m.open_pack()) {
            (Tab::Packs, Some(p)) => tf("Filter {pack} by name, keyword or emoji", &[("pack", &p.name)]),
            (Tab::Packs, None) => t("Filter packs by name").to_string(),
            _ => t("Search, or try e:😂 set:cats type:gif vibe:tired").to_string(),
        };
        if w.entry.placeholder_text().as_deref() != Some(placeholder.as_str()) {
            w.entry.set_placeholder_text(Some(&placeholder));
        }
        if w.entry.text() != m.query() {
            w.entry.set_text(m.query());
            w.entry.set_position(-1);
        }
        let count = match view {
            View::Items => pages::item_count(m.entries().len() as u64),
            View::Packs if m.packs().len() == 1 => "1 pack".to_string(),
            View::Packs => format!("{} packs", m.packs().len()),
            View::Page => String::new(),
        };
        w.count.set_label(&count);
        w.count.set_visible(!count.is_empty() && !field);

        let page = match view {
            View::Items => "items",
            View::Packs => "packs",
            View::Page if tab == Tab::Me => "me",
            View::Page => "settings",
        };
        if w.content.visible_child_name().as_deref() != Some(page) {
            w.content.set_visible_child_name(page);
        }

        if m.entries_revision() != self.rendered_revision.get() {
            let tiles = m.tiles();
            let thumbs = self.thumbs.borrow();
            let objects: Vec<StickerObject> = m
                .entries()
                .iter()
                .map(|e| {
                    let tex = match thumbs.get(&e.item.id) {
                        Some(Thumb::Ready(t)) => Some(t),
                        _ => None,
                    };
                    StickerObject::new(e, tex, tiles)
                })
                .collect();
            w.store.splice(0, w.store.n_items(), &objects);
            let covers = self.covers.borrow();
            let packs: Vec<PackObject> = m
                .packs()
                .iter()
                .map(|s| {
                    let tex = match covers.get(&cover_key(s)) {
                        Some(Thumb::Ready(t)) => Some(t),
                        _ => None,
                    };
                    PackObject::new(s, tex)
                })
                .collect();
            w.pack_store.splice(0, w.pack_store.n_items(), &packs);
            self.rendered_revision.set(m.entries_revision());
            self.scrolled_to.set(None);
        }

        let (selection, grid) = match view {
            View::Packs => (&w.pack_selection, &w.pack_grid),
            _ => (&w.selection, &w.grid),
        };
        let selected = m.selected().map(|i| i as u32).unwrap_or(gtk::INVALID_LIST_POSITION);
        if view != View::Page && selection.selected() != selected {
            selection.set_selected(selected);
        }
        let at = m.selected().filter(|_| view != View::Page).map(|i| (view, i));
        if m.is_visible() && w.window.is_visible() && at != self.scrolled_to.get() {
            if let Some((_, i)) = at {
                grid.scroll_to(i as u32, gtk::ListScrollFlags::NONE, None);
            }
            self.scrolled_to.set(at);
        }

        w.help.set_visible(m.help_visible());

        let f = m.footer();
        match f.mode {
            Some(mode) => {
                w.mode.set_visible(true);
                w.mode.set_label(mode.label());
                set_class(&w.mode, "insert", mode == Mode::Insert);
                set_class(&w.mode, "normal", mode == Mode::Normal);
            }
            None => w.mode.set_visible(false),
        }
        w.pending.set_label(&f.pending);
        w.pending.set_visible(!f.pending.is_empty());
        w.target.set_label(&f.target);
        w.target_hint.set_label(&f.target_hint);
        match f.sticker {
            StickerMode::Unsupported => w.sticker.set_visible(false),
            mode => {
                let on = mode == StickerMode::On;
                w.sticker.set_visible(true);
                w.sticker.set_active(on);
                let label = if on { t("sticker: on") } else { t("sticker: off") };
                w.sticker_label.set_label(label);
                set_class(&w.sticker_label, "accent", on);
            }
        }
        w.status.set_label(&f.status);
        set_class(&w.status, "error", f.level == Level::Error);
        set_class(&w.status, "warning", f.level == Level::Warning);
        set_class(&w.status, "dim-label", matches!(f.level, Level::Info | Level::Busy));

        self.apply_focus(&m);
        let cursor = m.config().modal;
        self.me.show_cursor(cursor && tab == Tab::Me);
        self.settings.show_cursor(cursor && tab == Tab::Settings);

        self.rendering.set(was_rendering);
        drop(m);
        self.update_page();
    }

    /// Move the keyboard focus to match the mode. Callers set `rendering`,
    /// so the focus changes made here are not reported back.
    fn apply_focus(&self, m: &Model) {
        let window = &self.w.window;
        if !m.is_visible() || !window.is_visible() {
            return;
        }
        let focus = GtkWindowExt::focus(window);
        // An open drop-down list of the Settings page keeps the keys.
        if focus
            .as_ref()
            .is_some_and(|f| f.ancestor(gtk::Popover::static_type()).is_some())
        {
            return;
        }
        match m.mode() {
            Mode::Insert if m.tab().has_text_field() => {
                let entry: &gtk::Widget = self.w.entry.upcast_ref();
                if !focus.as_ref().is_some_and(|f| f == entry || f.is_ancestor(entry)) {
                    self.w.entry.grab_focus();
                }
            }
            // A page field the user went into; or no modes, where nothing is forced.
            Mode::Insert => {}
            Mode::Normal => {
                if focus.is_some_and(|f| f.is::<gtk::Text>()) {
                    GtkWindowExt::set_focus(window, None::<&gtk::Widget>);
                }
            }
        }
    }

    /// The popup, or "stickpile cannot start" (bad config, no cache).
    fn update_page(&self) {
        let page = if self.config_error.is_some() || self.backend().is_none() {
            "error"
        } else {
            "main"
        };
        if self.w.stack.visible_child_name().as_deref() == Some(page) {
            return;
        }
        if page == "error" {
            let config = glib::markup_escape_text(&self.config_path.display().to_string());
            let msg = self
                .config_error
                .clone()
                .or_else(|| self.backend_error.borrow().clone())
                .unwrap_or_else(|| t("unknown error").into());
            self.w.error.set_description(Some(&format!(
                "{}\n\n{}",
                glib::markup_escape_text(&msg),
                tf("Config: {file}", &[("file", &format!("<tt>{config}</tt>"))])
            )));
        }
        self.w.stack.set_visible_child_name(page);
    }

    fn present(self: &Rc<Self>) {
        // Pick up a token written by `stickpile login` since the server said no.
        if !self.demo_mode && self.config_error.is_none() && (self.backend().is_none() || self.login_required()) {
            self.open_session(self.login_required());
        }
        self.update_page();
        // Failed thumbnails and covers get another chance on every open.
        self.thumbs.borrow_mut().retain(|_, t| !matches!(t, Thumb::Failed));
        self.covers.borrow_mut().retain(|_, t| !matches!(t, Thumb::Failed));
        let was = self.rendering.replace(true);
        self.w.window.present();
        self.apply_focus(&self.model.borrow());
        self.rendering.set(was);
        self.maybe_sync();
    }

    // -- Me and Settings --------------------------------------------------------------------

    fn load_profile(self: &Rc<Self>) {
        let Some(b) = self.backend() else {
            self.me.profile_loaded(Err(t("not connected").into()));
            return;
        };
        self.me.loading();
        self.spawn(b.profile(), |this, out| {
            if let Some(Ok(p)) = &out {
                this.account_lang_known(p);
            }
            this.me
                .profile_loaded(out.unwrap_or_else(|| Err(t("internal error (see log)").into())));
        });
    }

    fn me_action(self: &Rc<Self>, action: MeAction) {
        let Some(b) = self.backend() else {
            self.me
                .login_finished(Err(LoginError::Failed(t("not connected").into())));
            return;
        };
        match action {
            MeAction::Login(credentials) => self.spawn(b.login(credentials), |this, out| {
                let outcome = out.unwrap_or_else(|| Err(LoginError::Failed(t("internal error (see log)").into())));
                let ok = outcome.is_ok();
                this.me.login_finished(outcome);
                if ok {
                    this.account_changed();
                }
            }),
            MeAction::Logout => self.spawn(b.logout(), |this, out| {
                this.me
                    .logout_finished(out.unwrap_or_else(|| Err(t("internal error (see log)").into())));
                this.account_changed();
                this.load_profile();
            }),
            MeAction::Retry => self.load_profile(),
            MeAction::Open(url) => self.open_uri(&url),
        }
    }

    /// Logged in or out, or another server: what the item tabs show
    /// belongs to the old account. Fetch it again and sync the new library.
    fn account_changed(self: &Rc<Self>) {
        self.thumbs.borrow_mut().retain(|_, t| !matches!(t, Thumb::Failed));
        self.covers.borrow_mut().clear();
        self.dispatch(Event::Reload);
        *self.sync.borrow_mut() = sync::SyncPolicy::default();
        self.maybe_sync();
    }

    fn settings_action(self: &Rc<Self>, action: SettingsAction) {
        let path = self.settings.write_path();
        // A language chosen here becomes the account's too, so the web UI
        // follows (automatic leaves the account alone).
        let chosen_lang = match &action {
            SettingsAction::Set("ui.language", toml::Value::String(v)) => Lang::parse(v),
            _ => None,
        };
        let result = match action {
            SettingsAction::Set(key, value) => config_file::set_value(&path, key, value),
            SettingsAction::Reset(key) => config_file::unset_value(&path, key),
        };
        if let Err(e) = &result {
            tracing::warn!("saving a setting: {e}");
            self.settings.save_failed(&tf("Not saved: {error}", &[("error", &e)]));
        }
        if let (Ok(()), Some(lang), Some(b)) = (&result, chosen_lang, self.backend()) {
            self.spawn(
                b.set_account_language(Some(lang.code().to_string())),
                move |this, out| match out {
                    Some(Ok(())) => {
                        this.account_lang.set(Some(lang));
                        let _ = glue::save_account_lang(&this.account_lang_path, Some(lang));
                    }
                    Some(Err(e)) => tracing::info!("the account's language stays: {e}"),
                    None => {}
                },
            );
        }
        self.reload_settings();
    }

    /// Read the config again for the Settings page, and apply it if it changed.
    fn reload_settings(self: &Rc<Self>) {
        match config_file::load_layered(&self.config_path) {
            Ok(layered) => {
                self.settings.set_error(None);
                self.settings.show(&layered, &self.config_path);
                let changed = layered.config != *self.cfg.borrow();
                if changed {
                    self.apply_config(layered.config);
                }
            }
            Err(e) => self.settings.set_error(Some(&e.to_string())),
        }
    }

    /// Use a new config without a restart.
    fn apply_config(self: &Rc<Self>, new: ClientConfig) {
        let old = self.cfg.replace(new.clone());
        let w = &self.w;
        if old.ui.columns != new.ui.columns || old.ui.thumb_px != new.ui.thumb_px {
            if old.ui.thumb_px != new.ui.thumb_px {
                let px = new.ui.thumb_px as i32;
                w.grid.set_factory(Some(&self.item_factory(px)));
                w.pack_grid.set_factory(Some(&packs::factory(px)));
            }
            let (width, height) = glue::window_size(&new);
            w.window.set_default_size(width, height);
            self.layout.set(None);
        }
        if old.ui.language != new.ui.language {
            self.apply_language();
        }
        let account = pages::account_changed(&old, &new);
        if pages::needs_new_session(&old, &new) {
            match &self.demo {
                Some(demo) => demo.configure(&new),
                None if !self.demo_mode && self.config_error.is_none() => {
                    // Release the old session's cache first; if the new one
                    // cannot open, the error page says so rather than the
                    // old server answering on.
                    let login_required = !account && self.login_required();
                    self.backend.replace(None);
                    self.open_session(login_required);
                }
                None => {}
            }
            self.update_page();
        }
        self.dispatch(Event::Configure(glue::model_config(&new)));
        if account {
            self.me.forget();
            self.account_changed();
            if self.model.borrow().tab() == Tab::Me {
                self.load_profile();
            }
        }
    }

    // -- search ------------------------------------------------------------------------------

    fn cancel_search_timer(&self) {
        if let Some(id) = self.search_timer.take() {
            id.remove();
        }
    }

    fn schedule_search(
        self: &Rc<Self>,
        query: String,
        target: TargetApp,
        limit: usize,
        generation: u64,
        debounce_ms: u64,
    ) {
        self.cancel_search_timer();
        if debounce_ms == 0 {
            self.run_search(query, target, limit, generation);
            return;
        }
        let this = self.clone();
        let id = glib::timeout_add_local_once(Duration::from_millis(debounce_ms), move || {
            // The source is finished: forget its id without removing it.
            this.search_timer.borrow_mut().take();
            this.run_search(query, target, limit, generation);
        });
        *self.search_timer.borrow_mut() = Some(id);
    }

    fn run_search(self: &Rc<Self>, query: String, target: TargetApp, limit: usize, generation: u64) {
        self.answer(
            move |b| b.search(query, target, limit),
            move |outcome| Event::Results { generation, outcome },
        );
    }

    // -- thumbnails and covers -------------------------------------------------------------------

    fn load_thumbnails(self: &Rc<Self>, items: Vec<ItemSummary>) {
        let Some(backend) = self.backend() else { return };
        let px = thumbs::decode_px(self.cfg.borrow().ui.thumb_px);
        for item in items {
            if self.thumbs.borrow().contains_key(&item.id) {
                continue;
            }
            self.thumbs.borrow_mut().insert(item.id.clone(), Thumb::Loading);
            let id = item.id.clone();
            let b = backend.clone();
            let permits = self.thumb_permits.clone();
            let fut: BoxFut<Result<thumbs::Rgba, String>> = Box::pin(async move {
                let _permit = permits.acquire_owned().await.map_err(|e| e.to_string())?;
                let path = b.thumbnail(item).await?;
                tokio::task::spawn_blocking(move || thumbs::decode_file(&path, px))
                    .await
                    .map_err(|e| e.to_string())?
            });
            self.spawn(fut, move |this, out| this.thumbnail_done(id, out));
        }
    }

    fn thumbnail_done(&self, id: String, out: Option<Result<thumbs::Rgba, String>>) {
        let state = match out {
            Some(Ok(rgba)) => {
                let tex = texture(rgba);
                for obj in self.w.store.iter::<StickerObject>().flatten() {
                    if obj.item_id() == id {
                        obj.set_texture(Some(&tex));
                    }
                }
                Thumb::Ready(tex)
            }
            Some(Err(e)) => {
                tracing::debug!("thumbnail {id}: {e}");
                Thumb::Failed
            }
            None => Thumb::Failed,
        };
        let mut thumbs = self.thumbs.borrow_mut();
        thumbs.insert(id, state);
        if thumbs.len() > THUMB_CACHE {
            let shown: std::collections::HashSet<String> = self
                .w
                .store
                .iter::<StickerObject>()
                .flatten()
                .map(|o| o.item_id())
                .collect();
            thumbs.retain(|k, t| matches!(t, Thumb::Loading) || shown.contains(k));
        }
    }

    fn load_covers(self: &Rc<Self>, packs: Vec<SetSummary>) {
        let Some(backend) = self.backend() else { return };
        let px = thumbs::decode_px(self.cfg.borrow().ui.thumb_px);
        for set in packs {
            let key = cover_key(&set);
            if self.covers.borrow().contains_key(&key) {
                continue;
            }
            self.covers.borrow_mut().insert(key.clone(), Thumb::Loading);
            let set_id = set.id.clone();
            let b = backend.clone();
            let permits = self.thumb_permits.clone();
            let fut: BoxFut<Result<Option<thumbs::Rgba>, String>> = Box::pin(async move {
                let _permit = permits.acquire_owned().await.map_err(|e| e.to_string())?;
                let Some(path) = b.pack_cover(set).await? else {
                    return Ok(None);
                };
                tokio::task::spawn_blocking(move || thumbs::decode_file(&path, px))
                    .await
                    .map_err(|e| e.to_string())?
                    .map(Some)
            });
            self.spawn(fut, move |this, out| this.cover_done(key, set_id, out));
        }
    }

    fn cover_done(&self, key: String, set_id: String, out: Option<Result<Option<thumbs::Rgba>, String>>) {
        let state = match out {
            Some(Ok(Some(rgba))) => {
                let tex = texture(rgba);
                for obj in self.w.pack_store.iter::<PackObject>().flatten() {
                    if obj.set_id() == set_id {
                        obj.show_cover(&tex);
                    }
                }
                Thumb::Ready(tex)
            }
            Some(Err(e)) => {
                tracing::debug!("cover of {set_id}: {e}");
                Thumb::Failed
            }
            // No cover: the placeholder stays.
            Some(Ok(None)) | None => Thumb::Failed,
        };
        self.covers.borrow_mut().insert(key, state);
    }

    // -- copy ---------------------------------------------------------------------------------------

    fn copy(self: &Rc<Self>, item: ItemSummary, target: TargetApp, sticker: bool) {
        let id = item.id.clone();
        let Some(b) = self.backend() else {
            self.dispatch(Event::CopyFailed {
                item_id: id,
                error: t("not connected").into(),
            });
            return;
        };
        self.spawn(b.prepare_copy(item.clone(), target, sticker), move |this, out| {
            let result = out.unwrap_or_else(|| Err(t("internal error (see log)").into()));
            match result.and_then(|p| this.set_clipboard(&p).map(|()| p)) {
                Ok(prepared) => {
                    let warning = prepared.warning.clone();
                    this.remember_prepared(prepared.clone());
                    this.dispatch(Event::CopyFinished { item_id: id, warning });
                    this.record(item, prepared);
                }
                Err(error) => this.dispatch(Event::CopyFailed { item_id: id, error }),
            }
        });
    }

    /// Put the offers on the clipboard. Must happen while the popup still has
    /// keyboard focus: Wayland only accepts a selection from the focused
    /// client (GDK uses the serial of our last input event).
    fn set_clipboard(&self, prepared: &PreparedCopy) -> Result<(), String> {
        let window = &self.w.window;
        if !window.is_visible() {
            return Err(t("the popup was closed before the item was ready").into());
        }
        if !window.is_active() {
            return Err(t("the popup lost focus before the item was ready — press Enter again").into());
        }
        let specs = offers::provider_specs(&prepared.offers);
        if specs.is_empty() {
            return Err(t("nothing to offer (check [clipboard] in config.toml)").into());
        }
        window
            .clipboard()
            .set_content(Some(&drag::clipboard_provider(&specs)))
            .map_err(|e| e.to_string())?;
        tracing::info!("clipboard: {} [{}]", prepared.file.display(), offers::summary(&specs));
        Ok(())
    }

    fn record(self: &Rc<Self>, item: ItemSummary, prepared: PreparedCopy) {
        let Some(b) = self.backend() else { return };
        self.spawn(b.record_copy(item, prepared), |_, out| {
            if let Some(Err(e)) = out {
                tracing::warn!("recording usage: {e}");
            }
        });
    }

    fn remember_prepared(&self, p: PreparedCopy) {
        let mut cache = self.prepared.borrow_mut();
        cache.retain(|q| !(q.item_id == p.item_id && q.target == p.target && q.sticker == p.sticker));
        cache.push_front(p);
        cache.truncate(PREPARED_CACHE);
    }

    fn find_prepared(&self, item_id: &str, target: TargetApp, sticker: bool) -> Option<PreparedCopy> {
        self.prepared
            .borrow()
            .iter()
            .find(|p| p.item_id == item_id && p.target == target && p.sticker == sticker && p.file.exists())
            .cloned()
    }

    // -- drag and drop ----------------------------------------------------------------------------------

    fn drag_content(self: &Rc<Self>, item_id: &str) -> Option<gdk::ContentProvider> {
        let (item, target, sticker) = {
            let m = self.model.borrow();
            let entry = m.entries().iter().find(|e| e.item.id == item_id)?;
            (
                entry.item.clone(),
                m.target(),
                m.sticker() && model::supports_stickers(m.target()),
            )
        };
        let (provider, prepared): (gdk::ContentProvider, PreparedFuture) =
            match self.find_prepared(item_id, target, sticker) {
                Some(p) => {
                    let provider = drag::file_provider(&p.file);
                    (provider, futures::future::ready(Ok(p)).boxed_local().shared())
                }
                None => {
                    let b = self.backend()?;
                    let handle = runtime().spawn(b.prepare_copy(item.clone(), target, sticker));
                    let fut = async move { handle.await.map_err(|e| e.to_string())? }
                        .boxed_local()
                        .shared();
                    let this = self.clone();
                    let remember = fut.clone();
                    glib::spawn_future_local(async move {
                        match remember.await {
                            Ok(p) => this.remember_prepared(p),
                            Err(e) => tracing::warn!("preparing drag: {e}"),
                        }
                    });
                    (LazyFileProvider::new(fut.clone()).upcast(), fut)
                }
            };
        *self.drag.borrow_mut() = Some(DragState {
            item,
            prepared,
            cancelled: false,
        });
        Some(provider)
    }

    fn drag_cancelled(&self) {
        if let Some(d) = self.drag.borrow_mut().as_mut() {
            d.cancelled = true;
        }
    }

    fn drag_ended(self: &Rc<Self>) {
        let Some(d) = self.drag.take() else { return };
        if d.cancelled {
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(prepared) = d.prepared.await {
                let (id, name) = (d.item.id.clone(), d.item.name.clone());
                this.record(d.item, prepared);
                this.dispatch(Event::Dropped { item_id: id, name });
            }
        });
    }

    // -- sync ------------------------------------------------------------------------------------------------

    fn maybe_sync(self: &Rc<Self>) {
        let now = Instant::now();
        if !self.sync.borrow().due(now) {
            return;
        }
        let Some(b) = self.backend() else { return };
        self.sync.borrow_mut().started(now);
        self.spawn(b.sync(), |this, out| {
            let ok = matches!(out, Some(Ok(())));
            if let Some(Err(e)) = out {
                tracing::debug!("sync: {e}");
            }
            this.sync.borrow_mut().finished(Instant::now(), ok);
            this.update_page();
        });
    }

    fn start_sync_timer(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(sync::TICK.as_secs() as u32, move || match weak.upgrade() {
            Some(this) => {
                this.maybe_sync();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
    }

    // -- signals -------------------------------------------------------------------------------------------

    fn connect_signals(self: &Rc<Self>) {
        let w = &self.w;

        let this = self.clone();
        w.entry.connect_changed(move |e| {
            if !this.rendering.get() {
                this.dispatch(Event::QueryChanged(e.text().to_string()));
            }
        });

        let this = self.clone();
        w.tabs.connect(move |tab| this.dispatch(Event::ShowTab(tab)));
        let this = self.clone();
        w.tabs.help.connect_clicked(move |_| this.dispatch(Event::ToggleHelp));
        let this = self.clone();
        w.back.connect_clicked(move |_| this.dispatch(Event::PackBack));
        let click = gtk::GestureClick::new();
        let this = self.clone();
        click.connect_released(move |_, _, _, _| this.dispatch(Event::ToggleHelp));
        w.help.add_controller(click);

        for selection in [&w.selection, &w.pack_selection] {
            let this = self.clone();
            selection.connect_selected_notify(move |sel| {
                let i = sel.selected();
                if !this.rendering.get() && i != gtk::INVALID_LIST_POSITION {
                    this.dispatch(Event::Select(i as usize));
                }
            });
        }

        // Same as Ctrl+S. The button never takes the focus, so typing goes on
        // in the search entry after a click.
        let this = self.clone();
        w.sticker.connect_toggled(move |_| {
            if !this.rendering.get() {
                this.dispatch(Event::ToggleSticker);
            }
        });

        for grid in [&w.grid, &w.pack_grid] {
            let this = self.clone();
            grid.connect_activate(move |_, pos| {
                this.dispatch(Event::CopyAt {
                    index: pos as usize,
                    toggle_sticker: false,
                });
            });
        }

        // Page actions run from the main loop: some come from signals that
        // fire while a render holds the model (a field losing the focus).
        let this = self.clone();
        self.me.connect_action(move |action| {
            let this = this.clone();
            glib::idle_add_local_once(move || this.me_action(action));
        });
        let this = self.clone();
        self.settings.connect_action(move |action| {
            let this = this.clone();
            glib::idle_add_local_once(move || this.settings_action(action));
        });

        // Report focus changes the user made; a click into a text field is Insert.
        let this = self.clone();
        w.window.connect_notify_local(Some("focus-widget"), move |win, _| {
            let focus = GtkWindowExt::focus(win);
            if let Some(f) = &focus {
                this.me.follow(f);
                this.settings.follow(f);
            }
            // GTK's own initial focus while mapping an inactive window is
            // not the user's doing.
            if this.rendering.get() || !win.is_active() {
                return;
            }
            let text = focus.is_some_and(|f| f.is::<gtk::Text>());
            let this = this.clone();
            glib::idle_add_local_once(move || {
                let still = GtkWindowExt::focus(&this.w.window).is_some_and(|f| f.is::<gtk::Text>());
                if still == text {
                    this.dispatch(Event::TextFocus(text));
                }
            });
        });
        // Whatever GTK focused while mapping the window, the mode decides.
        let this = self.clone();
        w.window.connect_is_active_notify(move |win| {
            if !win.is_active() {
                return;
            }
            let Ok(m) = this.model.try_borrow() else { return };
            let was = this.rendering.replace(true);
            this.apply_focus(&m);
            this.rendering.set(was);
            // A page field that went away while the window was inactive
            // (the login form after a login) left Insert mode behind.
            let text = GtkWindowExt::focus(win).is_some_and(|f| f.is::<gtk::Text>());
            let stale = m.config().modal && m.mode() == Mode::Insert && m.tab().is_page() && !text;
            drop(m);
            if stale {
                let this = this.clone();
                glib::idle_add_local_once(move || this.dispatch(Event::TextFocus(false)));
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let this = self.clone();
        keys.connect_key_pressed(move |ctl, keyval, keycode, state| {
            // An open drop-down list handles its own keys.
            let in_popover =
                GtkWindowExt::focus(&this.w.window).is_some_and(|f| f.ancestor(gtk::Popover::static_type()).is_some());
            if in_popover {
                return glib::Propagation::Proceed;
            }
            let name = keyval.name().map(|n| n.to_string()).unwrap_or_default();
            let latin = ctl
                .widget()
                .and_then(|w| w.display().translate_key(keycode, gdk::ModifierType::empty(), 0))
                .and_then(|(k, ..)| k.name())
                .map(|n| n.to_string());
            let mods = Mods {
                ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
                shift: state.contains(gdk::ModifierType::SHIFT_MASK),
                alt: state.contains(gdk::ModifierType::ALT_MASK),
                logo: state.intersects(gdk::ModifierType::SUPER_MASK | gdk::ModifierType::META_MASK),
            };
            let (mode, tab) = {
                let m = this.model.borrow();
                (m.mode(), m.tab())
            };
            let press = KeyPress {
                name: &name,
                latin: latin.as_deref(),
                mods,
            };
            match keys::map_key(&press, mode, tab) {
                Some(event) => {
                    this.dispatch(event);
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
        w.window.add_controller(keys);

        let this = self.clone();
        w.window.connect_close_request(move |_| {
            this.dispatch(Event::Hidden);
            this.remember_tab();
            glib::Propagation::Proceed
        });

        // A resized window reflows the grids; the model needs their geometry
        // for the keys that move by rows and pages.
        for grid in [&w.grid, &w.pack_grid] {
            for adj in [grid.hadjustment(), grid.vadjustment()].into_iter().flatten() {
                let this = self.clone();
                adj.connect_page_size_notify(move |_| this.queue_layout());
            }
        }

        let px = self.cfg.borrow().ui.thumb_px as i32;
        w.grid.set_factory(Some(&self.item_factory(px)));
        w.pack_grid.set_factory(Some(&packs::factory(px)));
    }

    /// Grid cells: picture + badge + warning icon, bound to the item's
    /// properties; each cell is also a drag source for the item's file.
    fn item_factory(self: &Rc<Self>, thumb_px: i32) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let this = self.clone();
        factory.connect_setup(move |_, obj| {
            let Some(li) = obj.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Contain)
                .can_shrink(true)
                .width_request(thumb_px)
                .height_request(thumb_px)
                .build();
            let badge = gtk::Label::new(None);
            badge.add_css_class("osd");
            badge.add_css_class("stickpile-badge");
            let warn = gtk::Image::from_icon_name("dialog-warning-symbolic");
            warn.add_css_class("stickpile-warn");
            let emojis = gtk::Label::builder()
                .max_width_chars(6)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            emojis.add_css_class("osd");
            emojis.add_css_class("stickpile-pill");
            let heart = gtk::Label::new(Some("\u{2665}"));
            heart.add_css_class("osd");
            heart.add_css_class("stickpile-pill");
            heart.add_css_class("stickpile-heart");
            let tags = gtk::Label::builder()
                .halign(gtk::Align::Start)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            tags.add_css_class("osd");
            tags.add_css_class("stickpile-pill");
            tags.add_css_class("stickpile-tags");
            // Emojis and the heart along the top, the warning, tags and the
            // GIF badge along the bottom. Clicks go through to the cell.
            let row = |valign, parts: &[&gtk::Widget]| {
                let row = gtk::Box::builder().spacing(3).valign(valign).can_target(false).build();
                row.add_css_class("stickpile-tile-row");
                for p in parts {
                    row.append(*p);
                }
                row
            };
            let spacer = || gtk::Box::builder().hexpand(true).build().upcast::<gtk::Widget>();
            let top = row(gtk::Align::Start, &[emojis.upcast_ref(), &spacer(), heart.upcast_ref()]);
            let bottom = row(
                gtk::Align::End,
                &[warn.upcast_ref(), tags.upcast_ref(), &spacer(), badge.upcast_ref()],
            );
            let cell = gtk::Overlay::new();
            cell.set_child(Some(&picture));
            cell.add_overlay(&top);
            cell.add_overlay(&bottom);
            li.set_child(Some(&cell));

            let item = li.property_expression("item");
            item.chain_property::<StickerObject>("texture")
                .bind(&picture, "paintable", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("tooltip")
                .bind(&cell, "tooltip-text", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("badge")
                .bind(&badge, "label", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("has-badge")
                .bind(&badge, "visible", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("incompatible")
                .bind(&warn, "visible", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("liked")
                .bind(&heart, "visible", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("emojis")
                .bind(&emojis, "label", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("has-emojis")
                .bind(&emojis, "visible", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("tags")
                .bind(&tags, "label", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("has-tags")
                .bind(&tags, "visible", gtk::Widget::NONE);

            let source = gtk::DragSource::new();
            source.set_actions(gdk::DragAction::COPY);
            let (shell, li2) = (this.clone(), li.clone());
            source.connect_prepare(move |src, _, _| {
                let obj = li2.item().and_downcast::<StickerObject>()?;
                let provider = shell.drag_content(&obj.item_id())?;
                if let Some(tex) = obj.texture() {
                    src.set_icon(Some(&tex), 0, 0);
                }
                Some(provider)
            });
            let shell = this.clone();
            source.connect_drag_cancel(move |_, _, _| {
                shell.drag_cancelled();
                false
            });
            let shell = this.clone();
            source.connect_drag_end(move |_, _, _| shell.drag_ended());
            cell.add_controller(source);
        });
        factory
    }
}

/// A grid of as many columns as fit (it reflows with the window).
fn grid_view(selection: &gtk::SingleSelection) -> gtk::GridView {
    let grid = gtk::GridView::builder()
        .model(selection)
        .min_columns(1)
        .max_columns(glue::MAX_COLUMNS)
        .single_click_activate(true)
        .can_focus(false)
        .build();
    grid.add_css_class("stickpile-grid");
    grid
}

/// Columns and fully visible rows of `grid` as allocated now, counted the
/// way GtkGridView does: its widest cell and the first cell's height for
/// that width (`fallback` while it has no cells). `None` before it has a
/// size or while hidden.
fn measure_grid(grid: &gtk::GridView, fallback: (i32, i32)) -> Option<(usize, usize)> {
    if !grid.is_mapped() {
        return None;
    }
    let width = grid.hadjustment()?.page_size() as i32;
    let height = grid.vadjustment()?.page_size() as i32;
    if width <= 0 || height <= 0 {
        return None;
    }
    let mut cell_w = 0;
    let mut child = grid.first_child();
    let first = child.clone();
    while let Some(c) = child {
        cell_w = cell_w.max(c.measure(gtk::Orientation::Horizontal, -1).0);
        child = c.next_sibling();
    }
    let (cell_w, cell_h) = match first {
        Some(c) if cell_w > 0 => {
            let (columns, _) = glue::grid_layout(width, height, cell_w, 1);
            let column = width / columns as i32;
            (cell_w, c.measure(gtk::Orientation::Vertical, column).0)
        }
        _ => fallback,
    };
    Some(glue::grid_layout(width, height, cell_w, cell_h))
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(child)
        .build()
}

fn single_selection(store: &gio::ListStore) -> gtk::SingleSelection {
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    selection
}

fn build_widgets(app: &adw::Application, cfg: &ClientConfig, me: &MePage, settings: &SettingsPage) -> Widgets {
    let css = gtk::CssProvider::new();
    css.load_from_string(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let tabs = TabBar::build();
    let back = gtk::Button::builder()
        .icon_name("go-previous-symbolic")
        .can_focus(false)
        .build();
    back.add_css_class("flat");
    let title = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("heading");
    let entry = gtk::SearchEntry::builder().hexpand(true).build();
    let count = gtk::Label::builder().xalign(1.0).build();
    count.add_css_class("dim-label");
    count.add_css_class("caption");
    let header = gtk::Box::builder().spacing(8).build();
    header.add_css_class("stickpile-header");
    header.append(&back);
    header.append(&title);
    header.append(&entry);
    header.append(&count);
    let top = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    top.append(&tabs.root);
    top.append(&header);

    let store = gio::ListStore::new::<StickerObject>();
    let selection = single_selection(&store);
    let grid = grid_view(&selection);
    let pack_store = gio::ListStore::new::<PackObject>();
    let pack_selection = single_selection(&pack_store);
    let pack_grid = grid_view(&pack_selection);

    let content = gtk::Stack::builder().vexpand(true).build();
    content.add_named(&scrolled(&grid), Some("items"));
    content.add_named(&scrolled(&pack_grid), Some("packs"));
    content.add_named(&me.root, Some("me"));
    content.add_named(&settings.root, Some("settings"));
    let help = help::build();
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&content));
    overlay.add_overlay(&help);

    let mode = gtk::Label::new(None);
    mode.add_css_class("stickpile-mode");
    mode.set_valign(gtk::Align::Center);
    let pending = gtk::Label::new(None);
    pending.add_css_class("stickpile-pending");
    let target = gtk::Label::new(None);
    target.add_css_class("heading");
    let target_hint = gtk::Label::new(None);
    target_hint.add_css_class("dim-label");
    target_hint.add_css_class("caption");
    let sticker_label = gtk::Label::new(None);
    sticker_label.add_css_class("caption");
    // Not focusable, like the grid: the search entry keeps the focus and
    // Ctrl+S is the keyboard route (advertised in the tooltip and to ATs).
    let sticker = gtk::ToggleButton::builder()
        .child(&sticker_label)
        .can_focus(false)
        .valign(gtk::Align::Center)
        .build();
    sticker.add_css_class("flat");
    sticker.add_css_class("stickpile-sticker");
    sticker.update_property(&[gtk::accessible::Property::KeyShortcuts("Control+S")]);
    let status = gtk::Label::builder()
        .hexpand(true)
        .xalign(1.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    status.add_css_class("caption");
    let footer = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    footer.add_css_class("stickpile-footer");
    footer.append(&mode);
    footer.append(&pending);
    footer.append(&target);
    footer.append(&target_hint);
    footer.append(&sticker);
    footer.append(&status);

    let main = adw::ToolbarView::new();
    main.add_top_bar(&top);
    main.set_content(Some(&overlay));
    main.add_bottom_bar(&footer);

    let error = adw::StatusPage::builder().icon_name("dialog-error-symbolic").build();

    let stack = gtk::Stack::new();
    stack.add_named(&main, Some("main"));
    stack.add_named(&error, Some("error"));

    let (width, height) = glue::window_size(cfg);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("stickpile")
        .default_width(width)
        .default_height(height)
        .hide_on_close(true)
        .content(&stack)
        .build();

    let w = Widgets {
        window,
        stack,
        error,
        tabs,
        back,
        title,
        entry,
        count,
        content,
        help,
        grid,
        store,
        selection,
        pack_grid,
        pack_store,
        pack_selection,
        mode,
        pending,
        target,
        target_hint,
        sticker,
        sticker_label,
        status,
    };
    relabel_widgets(&w);
    w
}

/// The fixed texts of the window, in the current language (the tab bar,
/// the help and the pages have their own).
fn relabel_widgets(w: &Widgets) {
    w.back.set_tooltip_text(Some(t("Back to the packs (-, Backspace)")));
    w.target.set_tooltip_text(Some(t(
        "Target app — Ctrl+T / Ctrl+Shift+T to change\nCtrl+S: sticker mode · Shift+Enter: copy with sticker mode flipped",
    )));
    w.sticker.set_tooltip_text(Some(t("Send as sticker (Ctrl+S)")));
    w.sticker
        .update_property(&[gtk::accessible::Property::Label(t("Send as sticker"))]);
    w.error.set_title(t("stickpile cannot start"));
}
