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
use stickpile_core::config::Theme;
use stickpile_core::settings as config_file;
use stickpile_core::{ClientConfig, PreparedCopy, Reach, Session};
use stickpile_i18n::global::{t, tf};
use stickpile_i18n::Lang;
use stickpile_quick::focus::FocusEnv;
use stickpile_quick::model::{
    self, safe_label, Command, Event, HelpPage, Level, Model, OpenInfo, StickerMode, SuggestionLine,
};
use stickpile_quick::offline::{Confirm, KeepScope, Menu};
use stickpile_quick::{Mode, Tab, View};

use crate::backend::{self, Backend, BoxFut, CoreBackend, LoginError};
use crate::demo::DemoBackend;
use crate::keys::{self, KeyPress, Mods};
use crate::offline::{remove_numbers, Downloads, Panel, PanelView};
use crate::paste::{self, PasteTarget};
use crate::ui::drag::{self, LazyFileProvider, PreparedFuture};
use crate::ui::item::StickerObject;
use crate::ui::item::{AVAILABLE_ICON, UNAVAILABLE_ICON};
use crate::ui::me::{MeAction, MePage};
use crate::ui::packs::{self, PackObject};
use crate::ui::settings::OfflineAction;
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
.stickpile-vibe { font-size: 0.7em; font-style: italic; padding: 1px 5px; }
.stickpile-text { font-size: 0.7em; padding: 1px 5px; margin: 3px 3px 0 3px; }
label.stickpile-heart { color: #ff5470; }
.stickpile-footer { padding: 4px 10px; }
.stickpile-footer .stickpile-sticker { min-height: 0; min-width: 0; padding: 0 6px; font-weight: normal; }
.stickpile-details { font-feature-settings: "tnum"; }
.stickpile-tabs { padding: 6px 8px 0 8px; }
.stickpile-tabs button.stickpile-tab { min-height: 26px; padding: 2px 9px; font-weight: normal; }
.stickpile-tab-number { font-size: 0.8em; font-feature-settings: "tnum"; opacity: 0.55; }
.stickpile-tabs button.stickpile-current { background-color: alpha(@accent_bg_color, 0.16); color: @accent_color; font-weight: bold; }
.stickpile-tabs button.stickpile-current .stickpile-tab-number { opacity: 0.9; }
.stickpile-tabs button.stickpile-version { min-height: 0; padding: 2px 6px; font-size: 0.8em; font-weight: normal; font-feature-settings: "tnum"; opacity: 0.55; }
.stickpile-tabs button.stickpile-version:hover { opacity: 1; }
.stickpile-header { padding: 6px 8px; min-height: 34px; }
.stickpile-header button.stickpile-syntax { min-height: 0; padding: 2px 8px; font-size: 0.9em; font-weight: normal; }
.stickpile-suggest { padding: 0 8px 4px 8px; }
.stickpile-suggest button { min-height: 0; padding: 1px 7px; font-weight: normal; }
.stickpile-suggest button.stickpile-chosen { background-color: alpha(@accent_bg_color, 0.2); color: @accent_color; font-weight: bold; }
.stickpile-suggest-key { font-family: monospace; }
.stickpile-suggest-glyph { font-size: 1.3em; }
.stickpile-suggest-name { font-size: 0.85em; opacity: 0.7; }
.stickpile-mode { font-size: 0.75em; font-weight: bold; padding: 1px 6px; border-radius: 4px; }
.stickpile-mode.insert { background-color: @accent_bg_color; color: @accent_fg_color; }
.stickpile-mode.normal { background-color: alpha(currentColor, 0.12); }
.stickpile-pending { font-family: monospace; font-weight: bold; }
.stickpile-help { font-size: 0.92em; background-color: @popover_bg_color; color: @popover_fg_color; border-radius: 12px; box-shadow: 0 2px 14px alpha(black, 0.35); }
.stickpile-help-key { font-family: monospace; font-weight: bold; }
.stickpile-pack-name { font-weight: bold; }
.stickpile-stat { padding: 10px 6px; }
.stickpile-offline { color: @success_color; padding: 2px 4px; }
.stickpile-unavailable { color: @warning_color; padding: 2px 4px; }
.stickpile-confirm { background-color: @popover_bg_color; color: @popover_fg_color; border-radius: 12px; box-shadow: 0 2px 14px alpha(black, 0.35); padding: 18px 22px; }
.stickpile-confirm-body { margin-top: 4px; }
.stickpile-menu button { font-weight: normal; }
.stickpile-menu-keys { font-family: monospace; opacity: 0.6; margin-left: 18px; }
row.stickpile-error-row .subtitle { color: @error_color; }
row.stickpile-cursor { background-color: alpha(@accent_bg_color, 0.14); box-shadow: inset 3px 0 @accent_bg_color; }
button.stickpile-cursor { outline: 2px solid alpha(@accent_bg_color, 0.8); outline-offset: 2px; }
window.stickpile-oled {
  --window-bg-color: #000000;
  --view-bg-color: #000000;
  --headerbar-bg-color: #000000;
  --headerbar-backdrop-color: #000000;
  --sidebar-bg-color: #000000;
  --secondary-sidebar-bg-color: #000000;
  --dialog-bg-color: #000000;
  --popover-bg-color: #141414;
  --thumbnail-bg-color: #000000;
}
"#;

/// `ui.theme`: libadwaita's colour scheme, and for OLED the dark one with
/// pure black behind everything (the `stickpile-oled` class above).
fn apply_theme(window: &adw::ApplicationWindow, theme: Theme) {
    let scheme = match theme {
        Theme::System => adw::ColorScheme::Default,
        Theme::Dark | Theme::Oled => adw::ColorScheme::ForceDark,
        Theme::White => adw::ColorScheme::ForceLight,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
    set_class(window, "stickpile-oled", theme == Theme::Oled);
}

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
    /// Next to the entry where it searches: shows the search syntax (F2).
    syntax: gtk::Button,
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
    /// Footer safe-mode toggle; clicking it is `S`.
    safe: gtk::ToggleButton,
    safe_label: gtk::Label,
    /// "paste: by hand" where this session cannot paste for us.
    paste_note: gtk::Label,
    status: gtk::Label,
    /// The selection's stats, above the footer row.
    details: gtk::Label,
    /// The values offered for the filter being typed, under the header;
    /// it clips what does not fit instead of widening the window, and
    /// scrolls to keep the chosen value in view.
    suggest: gtk::ScrolledWindow,
    suggest_view: gtk::Viewport,
    suggest_box: gtk::Box,
    /// The question over the popup (`Model::confirm`).
    confirm: ConfirmCard,
    /// Over the grids: the parent of the menu of a tile, which points into it.
    menu_anchor: gtk::Box,
}

/// The card of a question: title, text, the two buttons, the keys.
struct ConfirmCard {
    card: gtk::Box,
    title: gtk::Label,
    body: gtk::Label,
    accept: gtk::Button,
    cancel: gtk::Button,
    hint: gtk::Label,
}

impl ConfirmCard {
    fn build() -> ConfirmCard {
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .width_request(420)
            .visible(false)
            .build();
        card.add_css_class("stickpile-confirm");
        let title = gtk::Label::builder().xalign(0.0).wrap(true).build();
        title.add_css_class("title-4");
        let body = gtk::Label::builder().xalign(0.0).wrap(true).max_width_chars(52).build();
        body.add_css_class("stickpile-confirm-body");
        let cancel = gtk::Button::builder().can_focus(false).build();
        let accept = gtk::Button::builder().can_focus(false).build();
        let buttons = gtk::Box::builder()
            .spacing(8)
            .halign(gtk::Align::End)
            .margin_top(8)
            .build();
        buttons.append(&cancel);
        buttons.append(&accept);
        let hint = gtk::Label::builder().xalign(1.0).build();
        hint.add_css_class("dim-label");
        hint.add_css_class("caption");
        card.append(&title);
        card.append(&body);
        card.append(&buttons);
        card.append(&hint);
        ConfirmCard {
            card,
            title,
            body,
            accept,
            cancel,
            hint,
        }
    }

    fn show(&self, c: Option<&Confirm>) {
        self.card.set_visible(c.is_some());
        let Some(c) = c else { return };
        self.title.set_label(&c.title);
        self.body.set_label(&c.body);
        self.accept.set_label(&c.accept);
        self.cancel.set_label(&c.cancel);
        set_class(&self.accept, "destructive-action", c.destructive);
        set_class(&self.accept, "suggested-action", !c.destructive);
        self.hint.set_label(&c.hint);
    }
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
    /// The suggestion line on screen.
    suggest_shown: RefCell<Option<SuggestionLine>>,
    /// The help page on screen (`None`: hidden, or to be drawn again).
    help_shown: Cell<Option<HelpPage>>,
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
    /// The window the popup was last opened over, to paste into after a pick.
    paste_target: RefCell<Option<PasteTarget>>,
    /// The session the popup was last opened in can paste (`paste.auto`).
    paste_available: Cell<bool>,
    /// Keeping offline: the downloader's runs and the Settings block.
    downloads: RefCell<Downloads>,
    panel: RefCell<Panel>,
    panel_shown: RefCell<Option<PanelView>>,
    /// Which session's reachability is followed ([`Shell::watch_reach`]):
    /// counts the sessions opened, so the watch of an old one ends.
    reach_watch: Cell<u64>,
    /// Where the pointer was when the item tiles last changed: until it
    /// moves, GTK's hover does not select ([`Shell::resting_over_new_tiles`]).
    hover_lock: Cell<Option<(f64, f64)>>,
    /// The menu of a tile on screen, and its popover.
    menu: RefCell<Option<(Menu, gtk::Popover)>>,
    /// Where the last right click was: the cell and the point in it.
    menu_at: RefCell<Option<(gtk::Widget, f64, f64)>>,
    /// The question on screen.
    confirm_shown: RefCell<Option<Confirm>>,
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
            suggest_shown: RefCell::new(None),
            help_shown: Cell::new(None),
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
            paste_target: RefCell::new(None),
            paste_available: Cell::new(true),
            downloads: RefCell::new(Downloads::default()),
            panel: RefCell::new(Panel::default()),
            panel_shown: RefCell::new(None),
            reach_watch: Cell::new(0),
            hover_lock: Cell::new(None),
            menu: RefCell::new(None),
            menu_at: RefCell::new(None),
            confirm_shown: RefCell::new(None),
            cfg: RefCell::new(cfg),
        });
        if shell.config_error.is_none() && !shell.demo_mode {
            shell.open_session(false);
        }
        shell.connect_signals();
        shell.start_sync_timer();
        shell.watch_offline();
        shell.render();
        shell.refresh_account_lang();
        shell.refresh_keeps();
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
        self.help_shown.set(None);
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
    fn open_session(self: &Rc<Self>, login_required: bool) {
        // The old session's reachability is no longer followed.
        self.reach_watch.set(self.reach_watch.get() + 1);
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
                self.watch_reach();
            }
            Ok(Ok(Err(e))) => self.session_failed(crate::backend::describe(&e)),
            Ok(Err(e)) => self.session_failed(tf("stickpile-core failed: {error}", &[("error", &e)])),
            Err(_) => self.session_failed(t("opening the local cache timed out").into()),
        }
    }

    /// Follow what the session knows of the server (the core's `Reach`)
    /// and tell the model as it changes ([`backend::reach_event`]), from
    /// the session's runtime to the main loop, the way the downloader's
    /// status comes ([`Shell::watch_offline`]). Ends when another session
    /// is opened or the shell is gone.
    fn watch_reach(self: &Rc<Self>) {
        let watch = self.reach_watch.get();
        let Some(mut rx) = self.backend().and_then(|b| b.reach()) else {
            return;
        };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            loop {
                let reach = *rx.borrow_and_update();
                let Some(this) = weak.upgrade() else { break };
                if this.reach_watch.get() != watch {
                    break;
                }
                // Not inside a dispatch: the main loop runs it next.
                glib::idle_add_local_once(move || {
                    if this.reach_watch.get() == watch {
                        this.reach_changed(reach);
                    }
                });
                if rx.changed().await.is_err() {
                    break;
                }
            }
        });
    }

    /// Where the pointer is over the popup window, as GTK last knew it.
    fn pointer_at(&self) -> Option<(f64, f64)> {
        let surface = self.w.window.surface()?;
        let pointer = surface.display().default_seat()?.pointer()?;
        surface.device_position(&pointer).map(|(x, y, _)| (x, y))
    }

    /// The pointer has not moved since the item tiles last changed: a tile
    /// GTK selects now is the one that came under it (GTK selects on hover
    /// with single-click activation), not one the user went to. Once it
    /// moves, hovering selects again.
    fn resting_over_new_tiles(&self) -> bool {
        let Some(at) = self.hover_lock.get() else {
            return false;
        };
        if self.pointer_at() == Some(at) {
            return true;
        }
        self.hover_lock.set(None);
        false
    }

    fn reach_changed(self: &Rc<Self>, reach: Reach) {
        let Some(event) = backend::reach_event(reach) else {
            return;
        };
        tracing::debug!("server: {reach:?}");
        if event == Event::Reachable(true) && !self.model.borrow().reachable() {
            // Back after an outage: what failed to load meanwhile loads again.
            self.thumbs.borrow_mut().retain(|_, t| !matches!(t, Thumb::Failed));
            self.covers.borrow_mut().retain(|_, t| !matches!(t, Thumb::Failed));
        }
        self.dispatch(event);
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
        let info = self.capture(env);
        self.dispatch(Event::Open(info));
        self.login_prompt();
    }

    pub fn toggle(self: &Rc<Self>, env: &FocusEnv) {
        let active = self.w.window.is_visible() && self.w.window.is_active();
        let open = if active { OpenInfo::default() } else { self.capture(env) };
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

    /// Read the focused window before the popup shows: the target app for
    /// the model, the window to paste into for later.
    fn capture(&self, env: &FocusEnv) -> OpenInfo {
        let captured = glue::capture(&self.cfg.borrow(), env);
        *self.paste_target.borrow_mut() = captured.paste;
        self.paste_available.set(paste::available(env));
        captured.open
    }

    /// A pick was copied: paste it into the window the popup was opened
    /// over once that has the focus again (off the main thread).
    fn paste_after_pick(&self) {
        let hidden = !self.model.borrow().is_visible();
        let plan = paste::plan(&self.cfg.borrow().paste, self.paste_target.borrow().as_ref(), hidden);
        match plan {
            Ok(plan) => {
                self.paste_target.replace(None);
                runtime().spawn_blocking(move || paste::run_logged(&plan));
            }
            Err(skip) => tracing::debug!("not pasting: {skip}"),
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
        self.update_menu();
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
    /// Save a `z` key's or `S`'s change as its setting, unless the
    /// setting is locked by a read-only `config.toml`.
    fn save_bool(self: &Rc<Self>, key: &str, on: bool) {
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
            Command::SaveTile { tile, on } => self.save_bool(glue::tile_key(tile), on),
            Command::SaveSafeMode { on } => self.save_bool(glue::SAFE_MODE_KEY, on),
            Command::LoadVocabulary => {
                if let Some(b) = self.backend() {
                    self.spawn(b.vocabulary(), |this, vocab| {
                        if let Some(vocab) = vocab {
                            this.dispatch(Event::VocabularyLoaded(vocab));
                        }
                    });
                }
            }
            Command::Sync => self.sync_now(),
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
            Command::CheckOffline {
                items,
                target,
                sticker,
                generation,
            } => self.answer(
                move |b| b.offline_available(items, target, sticker),
                move |outcome| Event::OfflineChecked { generation, outcome },
            ),
            Command::LoadKeptReasons { item } => {
                let item_id = item.id.clone();
                self.answer(
                    move |b| b.offline_reasons(*item),
                    move |outcome| Event::KeptReasons { item_id, outcome },
                )
            }
            Command::EstimateKeep { scope } => {
                let s = scope.clone();
                self.answer(
                    move |b| b.offline_estimate(s),
                    move |outcome| Event::Estimated { scope, outcome },
                )
            }
            Command::SetKeep { scope, keep } => self.set_keep(scope, keep),
            Command::RemoveKept => {
                let Some(b) = self.backend() else {
                    self.dispatch(Event::KeptRemoved(Err(t("not connected").into())));
                    return;
                };
                self.spawn(b.offline_remove_all(), |this, out| {
                    let outcome = out
                        .unwrap_or_else(|| Err(t("internal error (see log)").into()))
                        .map(|r| (r.removed_files as u64, r.freed_bytes));
                    this.panel.borrow_mut().note = None;
                    this.dispatch(Event::KeptRemoved(outcome));
                    this.refresh_usage();
                });
            }
        }
    }

    // -- keeping offline ----------------------------------------------------------------

    /// Keep or stop keeping; after a keep the downloader runs.
    fn set_keep(self: &Rc<Self>, scope: KeepScope, keep: bool) {
        let Some(b) = self.backend() else {
            self.dispatch(Event::KeepDone {
                scope,
                keep,
                outcome: Err(t("not connected").into()),
            });
            return;
        };
        let s = scope.clone();
        self.spawn(b.offline_set(s, keep), move |this, out| {
            let outcome = out.unwrap_or_else(|| Err(t("internal error (see log)").into()));
            let ok = outcome.is_ok();
            this.dispatch(Event::KeepDone { scope, keep, outcome });
            if ok && keep {
                this.run_offline();
            }
            this.refresh_usage();
        });
    }

    /// Tell the model what is kept (another process, the CLI, may keep too).
    fn refresh_keeps(self: &Rc<Self>) {
        let Some(b) = self.backend() else { return };
        self.spawn(b.offline_keeps(), |this, out| match out {
            Some(Ok(keeps)) => this.dispatch(Event::OfflineKeeps(keeps)),
            Some(Err(e)) => tracing::debug!("what is kept: {e}"),
            None => {}
        });
    }

    /// Run the downloader, unless one runs (then once more after it).
    fn run_offline(self: &Rc<Self>) {
        if self.backend().is_none() || !self.downloads.borrow_mut().want() {
            return;
        }
        self.start_run();
    }

    /// One run of the downloader ([`Downloads`] says when).
    fn start_run(self: &Rc<Self>) {
        let Some(b) = self.backend() else {
            self.downloads.borrow_mut().finished(None);
            return;
        };
        self.panel.borrow_mut().running = true;
        self.render_panel();
        let control = self.downloads.borrow().control.clone();
        self.spawn(b.offline_run(control), |this, out| {
            let outcome = match out {
                Some(Ok(o)) => {
                    tracing::info!(
                        "offline: {:?}, {} of {} files on this device",
                        o.end,
                        o.status.files_done,
                        o.status.files_total
                    );
                    Some(o)
                }
                Some(Err(e)) => {
                    tracing::warn!("offline downloads: {e}");
                    None
                }
                None => None,
            };
            let again = this.downloads.borrow_mut().finished(outcome.as_ref());
            {
                let mut p = this.panel.borrow_mut();
                p.running = again;
                if let Some(o) = &outcome {
                    p.status = Some(o.status.clone());
                }
            }
            this.dispatch(Event::OfflineFiles);
            this.refresh_usage();
            if again {
                this.start_run();
            }
        });
    }

    /// Follow the downloader's status: the Settings block, and the marks
    /// as files arrive (at most a few times a second).
    fn watch_offline(self: &Rc<Self>) {
        let mut rx = self.downloads.borrow().control.subscribe();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let mut done = None;
            while rx.changed().await.is_ok() {
                let status = rx.borrow_and_update().clone();
                let Some(this) = weak.upgrade() else { break };
                let arrived = done != Some(status.files_done);
                done = Some(status.files_done);
                {
                    let mut p = this.panel.borrow_mut();
                    p.paused = this.downloads.borrow().control.is_paused();
                    p.status = Some(status);
                }
                this.render_panel();
                if arrived && this.model.try_borrow_mut().is_ok() {
                    this.dispatch(Event::OfflineFiles);
                }
                drop(this);
                glib::timeout_future(Duration::from_millis(400)).await;
            }
        });
    }

    /// What the Settings block says: the usage, and between runs what the
    /// plan says.
    fn refresh_usage(self: &Rc<Self>) {
        let Some(b) = self.backend() else { return };
        self.spawn(b.clone().offline_usage(), |this, out| {
            match out {
                Some(Ok(u)) => this.panel.borrow_mut().usage = Some(u),
                Some(Err(e)) => tracing::debug!("offline usage: {e}"),
                None => {}
            }
            this.render_panel();
        });
        self.spawn(b.offline_plan_status(), |this, out| {
            match out {
                Some(Ok((s, skips))) => this.panel.borrow_mut().planned(s, skips),
                Some(Err(e)) => tracing::debug!("offline plan: {e}"),
                None => {}
            }
            this.render_panel();
        });
    }

    /// Draw the Settings page's "On this device" block, when it changed.
    fn render_panel(&self) {
        let Ok(m) = self.model.try_borrow() else { return };
        let mut p = self.panel.borrow_mut();
        p.paused = self.downloads.borrow().control.is_paused();
        let view = p.view(m.library_kept());
        drop(p);
        drop(m);
        if self.panel_shown.borrow().as_ref() != Some(&view) {
            self.settings.show_offline(&view);
            *self.panel_shown.borrow_mut() = Some(view);
        }
    }

    /// A control of the "On this device" block.
    fn offline_action(self: &Rc<Self>, action: OfflineAction) {
        let control = self.downloads.borrow().control.clone();
        match action {
            OfflineAction::KeepLibrary(keep) => {
                if keep != self.model.borrow().library_kept() {
                    self.dispatch(Event::Keep {
                        scope: KeepScope::Library,
                        keep,
                    });
                }
            }
            OfflineAction::Pause => control.pause(),
            OfflineAction::Resume => {
                control.resume();
                self.run_offline();
            }
            OfflineAction::Cancel => {
                // A paused run waits: it has to go on to see the cancel.
                control.cancel();
                control.resume();
            }
            OfflineAction::ClearCache => {
                let Some(b) = self.backend() else { return };
                self.spawn(b.clear_cache(), |this, out| {
                    match out {
                        Some(Ok(r)) => this.panel.borrow_mut().cleared(&r),
                        Some(Err(e)) => this.panel.borrow_mut().failed(e),
                        None => {}
                    }
                    // Thumbnails decoded before stay drawn; files go.
                    this.dispatch(Event::OfflineFiles);
                    this.refresh_usage();
                });
            }
            OfflineAction::RemoveKept => {
                let (files, bytes) = remove_numbers(self.panel.borrow().usage.as_ref());
                self.dispatch(Event::AskRemoveKept { files, bytes });
            }
        }
        // The switch shows what is kept, whatever was clicked.
        *self.panel_shown.borrow_mut() = None;
        self.render_panel();
    }

    // -- the menu of a tile --------------------------------------------------------------

    /// A right click on a cell of the pack list.
    fn pack_menu_hook(self: &Rc<Self>) -> packs::MenuHook {
        let weak = Rc::downgrade(self);
        Rc::new(move |position, cell, x, y| {
            if let Some(this) = weak.upgrade() {
                this.open_menu_at(position, cell, x, y);
            }
        })
    }

    /// A right click on a cell of the shown grid.
    fn open_menu_at(self: &Rc<Self>, position: u32, cell: &gtk::Widget, x: f64, y: f64) {
        *self.menu_at.borrow_mut() = Some((cell.clone(), x, y));
        self.dispatch(Event::OpenMenu(Some(position as usize)));
    }

    /// Show, change or close the menu as the model says. Called after a
    /// render, with the model free.
    fn update_menu(self: &Rc<Self>) {
        let want = self.model.borrow().menu();
        let same = match (&*self.menu.borrow(), &want) {
            (Some((shown, _)), Some(m)) => shown == m,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        if let Some((_, popover)) = self.menu.borrow_mut().take() {
            popover.popdown();
            popover.unparent();
        }
        let Some(menu) = want else { return };
        let popover = self.build_menu(&menu);
        popover.set_parent(&self.w.menu_anchor);
        // Point at the cell clicked, else (the Menu key) the middle.
        let at = self.menu_at.borrow_mut().take().and_then(|(cell, x, y)| {
            cell.compute_point(&self.w.menu_anchor, &gtk::graphene::Point::new(x as f32, y as f32))
        });
        let (x, y) = match at {
            Some(p) => (p.x() as i32, p.y() as i32),
            None => (self.w.menu_anchor.width() / 2, self.w.menu_anchor.height() / 3),
        };
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x, y, 1, 1)));
        *self.menu.borrow_mut() = Some((menu, popover.clone()));
        popover.popup();
    }

    fn build_menu(self: &Rc<Self>, menu: &Menu) -> gtk::Popover {
        let list = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        list.add_css_class("stickpile-menu");
        let popover = gtk::Popover::builder()
            .child(&list)
            .has_arrow(false)
            .halign(gtk::Align::Start)
            .build();
        popover.add_css_class("menu");
        for entry in &menu.entries {
            let label = gtk::Label::builder()
                .label(&entry.label)
                .xalign(0.0)
                .hexpand(true)
                .build();
            let Some(action) = entry.action else {
                label.add_css_class("dim-label");
                label.set_margin_start(12);
                label.set_margin_end(12);
                label.set_margin_top(4);
                label.set_margin_bottom(4);
                list.append(&label);
                continue;
            };
            let row = gtk::Box::builder().build();
            row.append(&label);
            if !entry.keys.is_empty() {
                let keys = gtk::Label::new(Some(&entry.keys));
                keys.add_css_class("stickpile-menu-keys");
                row.append(&keys);
            }
            let button = gtk::Button::builder().child(&row).build();
            button.add_css_class("flat");
            let this = Rc::downgrade(self);
            let pop = popover.clone();
            button.connect_clicked(move |_| {
                let Some(this) = this.upgrade() else { return };
                // Chosen: the model closes the menu, so no "closed" for it.
                this.menu.borrow_mut().take();
                pop.popdown();
                let this = this.clone();
                glib::idle_add_local_once(move || this.dispatch(Event::MenuChosen(action)));
            });
            list.append(&button);
        }
        let this = Rc::downgrade(self);
        popover.connect_closed(move |p| {
            let p = p.clone();
            let this = this.clone();
            glib::idle_add_local_once(move || {
                p.unparent();
                let Some(this) = this.upgrade() else { return };
                let mine = this.menu.borrow().as_ref().is_some_and(|(_, q)| *q == p);
                if mine {
                    this.menu.borrow_mut().take();
                    this.dispatch(Event::CloseMenu);
                }
            });
        });
        popover
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

    /// The line of values offered under the search field.
    fn show_suggestions(&self, line: Option<&SuggestionLine>) {
        let row = &self.w.suggest_box;
        while let Some(child) = row.first_child() {
            row.remove(&child);
        }
        self.w.suggest.set_visible(line.is_some());
        let Some(line) = line else {
            return;
        };
        let key = gtk::Label::new(Some(&line.key));
        key.add_css_class("dim-label");
        key.add_css_class("caption");
        key.add_css_class("stickpile-suggest-key");
        row.append(&key);
        let mut chosen = None;
        for (i, word) in line.words.iter().enumerate() {
            let b = gtk::Button::builder()
                .can_focus(false)
                .action_name("win.suggest")
                .action_target(&(i as u32).to_variant())
                .build();
            match line.labels.get(i).filter(|l| !l.is_empty()) {
                // An emoji of `e:` with its name: the glyph, then the name,
                // smaller and dimmer. No font is set, so fontconfig falls
                // back to a colour emoji font for the glyph.
                Some(name) => {
                    let content = gtk::Box::builder().spacing(4).build();
                    let glyph = gtk::Label::new(Some(word));
                    glyph.add_css_class("stickpile-suggest-glyph");
                    content.append(&glyph);
                    let label = gtk::Label::new(Some(name));
                    label.add_css_class("stickpile-suggest-name");
                    content.append(&label);
                    b.set_child(Some(&content));
                }
                None => b.set_label(word),
            }
            b.add_css_class("flat");
            b.add_css_class("caption");
            row.append(&b);
            if line.chosen == Some(i) {
                b.add_css_class("stickpile-chosen");
                chosen = Some(b);
            }
        }
        let hint = gtk::Label::new(Some(t("Tab")));
        hint.add_css_class("dim-label");
        hint.add_css_class("caption");
        hint.set_margin_start(4);
        hint.set_tooltip_text(Some(t("Tab / Shift+Tab: put in the next / previous value")));
        row.append(&hint);
        // A long line (the words written on the items, `x:un`) runs past
        // the right edge, and Tab may choose a value there: once the line
        // is laid out, scroll the chosen value into view. A line without
        // one starts at its beginning.
        let Some(adj) = self.w.suggest_view.hadjustment() else {
            return;
        };
        let Some(chosen) = chosen else {
            adj.set_value(0.0);
            return;
        };
        // The first value with the key before it, the last with the hint.
        let first = line.chosen == Some(0);
        let last = line.chosen == Some(line.words.len() - 1);
        let line_box = row.clone();
        row.add_tick_callback(move |_, _| {
            if chosen.parent().as_ref() != Some(line_box.upcast_ref()) {
                // Drawn again since: that line scrolls itself.
                return glib::ControlFlow::Break;
            }
            // Not laid out yet: the next frame.
            if chosen.width() == 0 {
                return glib::ControlFlow::Continue;
            }
            if let Some(b) = chosen.compute_bounds(&line_box) {
                let start = if first { 0.0 } else { f64::from(b.x()) };
                let end = if last {
                    adj.upper()
                } else {
                    f64::from(b.x() + b.width())
                };
                adj.clamp_page(start, end);
            }
            glib::ControlFlow::Break
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
        w.syntax.set_visible(m.searches());
        w.title.set_hexpand(!field);
        let placeholder = m.placeholder();
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
            self.hover_lock.set(self.pointer_at());
            let covers = self.covers.borrow();
            let packs: Vec<PackObject> = m
                .packs()
                .iter()
                .map(|s| {
                    let tex = match covers.get(&cover_key(s)) {
                        Some(Thumb::Ready(t)) => Some(t),
                        _ => None,
                    };
                    PackObject::new(s, tex, m.pack_kept(&s.id))
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

        let page = m.help_page();
        if let Some(p) = page.filter(|p| self.help_shown.get() != Some(*p)) {
            // Drawn when it opens: the count keys follow the setting.
            help::show(&w.help, p, m.config().counts);
        }
        self.help_shown.set(page);
        w.help.set_visible(page.is_some());

        let line = m.suggestions();
        if *self.suggest_shown.borrow() != line {
            self.show_suggestions(line.as_ref());
            *self.suggest_shown.borrow_mut() = line;
        }

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
        // Shown even where the target has no sticker route (dimmed): the
        // preference holds for the next target.
        let on = m.sticker();
        w.sticker.set_active(on);
        w.sticker_label
            .set_label(if on { t("sticker: on") } else { t("sticker: off") });
        set_class(&w.sticker_label, "accent", on && f.sticker == StickerMode::On);
        set_class(&w.sticker_label, "dim-label", f.sticker == StickerMode::Unsupported);
        w.safe.set_active(f.safe);
        w.safe_label.set_label(safe_label(f.safe));
        set_class(&w.safe_label, "accent", f.safe);
        w.paste_note
            .set_visible(self.cfg.borrow().paste.auto && !self.paste_available.get());
        w.details.set_visible(f.details_row);
        w.details.set_label(&f.details);
        w.details
            .set_tooltip_text((!f.details.is_empty()).then_some(f.details.as_str()));
        // Kept when empty, so the grid above does not change height.
        w.status.set_label(&f.status);
        // A long message (why a sticker does not copy) is cut: all of it on hover.
        w.status
            .set_tooltip_text((!f.status.is_empty()).then_some(f.status.as_str()));
        set_class(&w.status, "error", f.level == Level::Error);
        set_class(&w.status, "warning", f.level == Level::Warning);
        set_class(&w.status, "dim-label", matches!(f.level, Level::Info | Level::Busy));

        self.apply_focus(&m);
        let cursor = m.config().modal;
        self.me.show_cursor(cursor && tab == Tab::Me);
        self.settings.show_cursor(cursor && tab == Tab::Settings);

        let confirm = m.confirm();
        if *self.confirm_shown.borrow() != confirm {
            w.confirm.show(confirm.as_ref());
            *self.confirm_shown.borrow_mut() = confirm;
        }

        self.rendering.set(was_rendering);
        drop(m);
        self.update_page();
        if tab == Tab::Settings {
            self.render_panel();
        }
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
        // Whether the server answers is checked now, in the background:
        // searches do not wait for it (docs/offline.md).
        if let Some(b) = self.backend() {
            b.check_server();
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
        self.refresh_keeps();
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
        // What was kept and downloaded belonged to the old account.
        self.downloads.borrow_mut().forget();
        *self.panel.borrow_mut() = Panel::default();
        self.dispatch(Event::Reload);
        *self.sync.borrow_mut() = sync::SyncPolicy::default();
        self.maybe_sync();
        self.refresh_keeps();
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
            SettingsAction::Offline(a) => return self.offline_action(a),
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
                self.refresh_usage();
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
                w.pack_grid
                    .set_factory(Some(&packs::factory(px, self.pack_menu_hook())));
            }
            let (width, height) = glue::window_size(&new);
            w.window.set_default_size(width, height);
            self.layout.set(None);
        }
        if old.ui.language != new.ui.language {
            self.apply_language();
        }
        if old.ui.theme != new.ui.theme {
            apply_theme(&w.window, new.ui.theme);
        }
        let account = pages::account_changed(&old, &new);
        let offline_changed = old.offline != new.offline;
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
        // The running session takes the new options; what they want more
        // is downloaded, what they no longer want is released.
        if offline_changed {
            if let Some(b) = self.backend() {
                b.set_offline_options(new.offline.clone());
                self.run_offline();
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

    /// The search in two steps (docs/offline.md): what to show now, then
    /// maybe the server's results, both for `generation` (the model drops
    /// them once another search started).
    fn run_search(self: &Rc<Self>, query: String, target: TargetApp, limit: usize, generation: u64) {
        let Some(b) = self.backend() else {
            self.dispatch(Event::Results {
                generation,
                outcome: Err(t("not connected").into()),
            });
            return;
        };
        let start = Instant::now();
        self.spawn(b.search(query, target, limit), move |this, out| {
            let (outcome, then) = match out {
                Some(Ok(s)) => (Ok(s.first), s.then),
                Some(Err(e)) => (Err(e), None),
                None => (Err(t("internal error (see log)").into()), None),
            };
            if let Ok(r) = &outcome {
                tracing::debug!(
                    "search {generation}: {} results after {:.1?} (offline: {}; the server's to come: {})",
                    r.hits.len(),
                    start.elapsed(),
                    r.offline,
                    then.is_some()
                );
            }
            this.dispatch(Event::Results { generation, outcome });
            let Some(then) = then else { return };
            this.spawn(then, move |this, out| {
                let outcome = out.unwrap_or_else(|| Err(t("internal error (see log)").into()));
                tracing::debug!(
                    "search {generation}: the server's answer after {:.1?}: {}",
                    start.elapsed(),
                    match &outcome {
                        Ok(Some(r)) => format!("{} results", r.hits.len()),
                        Ok(None) => "none".into(),
                        Err(e) => e.clone(),
                    }
                );
                this.dispatch(Event::ServerResults { generation, outcome });
            });
        });
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
                    this.paste_after_pick();
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

    /// A sync when the schedule says one is due (`SyncPolicy`).
    fn maybe_sync(self: &Rc<Self>) {
        if self.sync.borrow().due(Instant::now()) {
            self.start_sync();
        }
    }

    /// The server is back ([`Command::Sync`]): a sync now, unless one is
    /// running; the schedule goes on from it.
    fn sync_now(self: &Rc<Self>) {
        if !self.sync.borrow().is_running() {
            self.start_sync();
        }
    }

    fn start_sync(self: &Rc<Self>) {
        let Some(b) = self.backend() else { return };
        self.sync.borrow_mut().started(Instant::now());
        self.spawn(b.sync(), |this, out| {
            let ok = matches!(out, Some(Ok(_)));
            if let Some(Err(e)) = &out {
                tracing::debug!("sync: {e}");
            }
            this.sync.borrow_mut().finished(Instant::now(), ok);
            if matches!(out, Some(Ok(true))) {
                this.dispatch(Event::LibraryChanged);
            }
            this.update_page();
            // After each successful sync: what is kept and missing.
            if ok {
                this.refresh_keeps();
                this.run_offline();
            }
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
        w.syntax.connect_clicked(move |_| this.dispatch(Event::ToggleSyntax));
        let this = self.clone();
        w.tabs
            .version
            .connect_clicked(move |_| this.open_uri(stickpile_quick::PROJECT_URL));
        // The suggestion buttons are made per line: they name this action
        // with their index.
        let accept = gio::SimpleAction::new("suggest", Some(glib::VariantTy::UINT32));
        let this = self.clone();
        accept.connect_activate(move |_, index| {
            if let Some(i) = index.and_then(|v| v.get::<u32>()) {
                this.dispatch(Event::AcceptSuggestion(i as usize));
            }
        });
        w.window.add_action(&accept);
        let this = self.clone();
        w.back.connect_clicked(move |_| this.dispatch(Event::PackBack));
        let click = gtk::GestureClick::new();
        let this = self.clone();
        click.connect_released(move |_, _, _, _| this.dispatch(Event::ToggleHelp));
        w.help.add_controller(click);
        for (button, yes) in [(&w.confirm.accept, true), (&w.confirm.cancel, false)] {
            let this = self.clone();
            button.connect_clicked(move |_| this.dispatch(Event::Confirm(yes)));
        }

        for (selection, items) in [(&w.selection, true), (&w.pack_selection, false)] {
            let this = self.clone();
            selection.connect_selected_notify(move |sel| {
                let i = sel.selected();
                if this.rendering.get() || i == gtk::INVALID_LIST_POSITION {
                    return;
                }
                if items && this.resting_over_new_tiles() {
                    // GTK selects the tile under a resting pointer once the
                    // tiles changed under it (the server's results in
                    // place): not the user's choice, so the model's stays.
                    let kept = this.model.borrow().selected();
                    let was = this.rendering.replace(true);
                    sel.set_selected(kept.map_or(gtk::INVALID_LIST_POSITION, |k| k as u32));
                    this.rendering.set(was);
                    return;
                }
                this.dispatch(Event::Select(i as usize));
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
        // Same as `S`.
        let this = self.clone();
        w.safe.connect_toggled(move |_| {
            if !this.rendering.get() {
                this.dispatch(Event::ToggleSafe);
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
        // A press on the item grid holds the search's results (the
        // server's, a search typed further), so the tile under it does not
        // change before the release activates it. Watched before the cells
        // (capture) and never claimed; the release is told once the click
        // it makes (on the release) is handled. The end comes on a
        // release, and when a drag takes the press over (the dragged
        // sticker is taken then).
        let press = gtk::GestureClick::builder()
            .button(0)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let this = self.clone();
        press.connect_pressed(move |_, _, _, _| this.dispatch(Event::Pressing(true)));
        let this = self.clone();
        press.connect_end(move |_, _| {
            let this = this.clone();
            glib::idle_add_local_once(move || this.dispatch(Event::Pressing(false)));
        });
        w.grid.add_controller(press);

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
        w.pack_grid
            .set_factory(Some(&packs::factory(px, self.pack_menu_hook())));
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
            let vibe = gtk::Label::builder()
                .max_width_chars(12)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            vibe.add_css_class("osd");
            vibe.add_css_class("stickpile-pill");
            vibe.add_css_class("stickpile-vibe");
            // The words in the picture, one line (off by default).
            let text = gtk::Label::builder()
                .halign(gtk::Align::Start)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .single_line_mode(true)
                .build();
            text.add_css_class("osd");
            text.add_css_class("stickpile-pill");
            text.add_css_class("stickpile-text");
            // On the device (ui.show_offline), or not with the server gone.
            let available = gtk::Image::from_icon_name(AVAILABLE_ICON);
            available.add_css_class("osd");
            available.add_css_class("stickpile-pill");
            available.add_css_class("stickpile-offline");
            let unavailable = gtk::Image::from_icon_name(UNAVAILABLE_ICON);
            unavailable.add_css_class("osd");
            unavailable.add_css_class("stickpile-pill");
            unavailable.add_css_class("stickpile-unavailable");
            // Emojis, the vibe and the heart along the top, the text above
            // the warning, tags, the offline mark and the GIF badge along
            // the bottom. Clicks go through to the cell.
            let row = |valign, parts: &[&gtk::Widget]| {
                let row = gtk::Box::builder().spacing(3).valign(valign).can_target(false).build();
                row.add_css_class("stickpile-tile-row");
                for p in parts {
                    row.append(*p);
                }
                row
            };
            let spacer = || gtk::Box::builder().hexpand(true).build().upcast::<gtk::Widget>();
            let top = row(
                gtk::Align::Start,
                &[emojis.upcast_ref(), &spacer(), vibe.upcast_ref(), heart.upcast_ref()],
            );
            let bottom = row(
                gtk::Align::End,
                &[
                    warn.upcast_ref(),
                    tags.upcast_ref(),
                    &spacer(),
                    available.upcast_ref(),
                    unavailable.upcast_ref(),
                    badge.upcast_ref(),
                ],
            );
            let foot = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .valign(gtk::Align::End)
                .can_target(false)
                .build();
            foot.append(&text);
            foot.append(&bottom);
            let cell = gtk::Overlay::new();
            cell.set_child(Some(&picture));
            cell.add_overlay(&top);
            cell.add_overlay(&foot);
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
            item.chain_property::<StickerObject>("vibe")
                .bind(&vibe, "label", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("has-vibe")
                .bind(&vibe, "visible", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("text")
                .bind(&text, "label", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("has-text")
                .bind(&text, "visible", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("available")
                .bind(&available, "visible", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("unavailable")
                .bind(&unavailable, "visible", gtk::Widget::NONE);
            item.chain_property::<StickerObject>("opacity")
                .bind(&picture, "opacity", gtk::Widget::NONE);

            // A right click: the menu of the tile.
            let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
            let (shell, li2) = (this.clone(), li.clone());
            click.connect_pressed(move |g, _, x, y| {
                if let Some(w) = g.widget() {
                    shell.open_menu_at(li2.position(), &w, x, y);
                }
            });
            cell.add_controller(click);

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
    // Not focusable, like the tabs: the entry keeps the focus.
    let syntax = gtk::Button::builder()
        .can_focus(false)
        .valign(gtk::Align::Center)
        .build();
    syntax.add_css_class("flat");
    syntax.add_css_class("stickpile-syntax");
    syntax.update_property(&[gtk::accessible::Property::KeyShortcuts("F2")]);
    let count = gtk::Label::builder().xalign(1.0).build();
    count.add_css_class("dim-label");
    count.add_css_class("caption");
    let header = gtk::Box::builder().spacing(8).build();
    header.add_css_class("stickpile-header");
    header.append(&back);
    header.append(&title);
    header.append(&entry);
    header.append(&syntax);
    header.append(&count);
    let suggest_box = gtk::Box::builder().spacing(4).build();
    suggest_box.add_css_class("stickpile-suggest");
    let suggest_view = gtk::Viewport::builder().child(&suggest_box).build();
    let suggest = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&suggest_view)
        .visible(false)
        .build();
    let top = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    top.append(&tabs.root);
    top.append(&header);
    top.append(&suggest);

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
    let confirm = ConfirmCard::build();
    // Fills the main area without taking clicks: the menu of a tile hangs on it.
    let menu_anchor = gtk::Box::builder().can_target(false).build();
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&content));
    overlay.add_overlay(&menu_anchor);
    overlay.add_overlay(&help);
    overlay.add_overlay(&confirm.card);

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
    let safe_label = gtk::Label::new(None);
    safe_label.add_css_class("caption");
    let safe = gtk::ToggleButton::builder()
        .child(&safe_label)
        .can_focus(false)
        .valign(gtk::Align::Center)
        .build();
    safe.add_css_class("flat");
    safe.add_css_class("stickpile-sticker");
    safe.update_property(&[gtk::accessible::Property::KeyShortcuts("Control+Shift+S")]);
    let paste_note = gtk::Label::builder().visible(false).build();
    paste_note.add_css_class("caption");
    paste_note.add_css_class("dim-label");
    let status = gtk::Label::builder()
        .hexpand(true)
        .xalign(1.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    status.add_css_class("caption");
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    row.append(&mode);
    row.append(&pending);
    row.append(&target);
    row.append(&target_hint);
    row.append(&sticker);
    row.append(&safe);
    row.append(&paste_note);
    row.append(&status);
    let details = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    details.add_css_class("caption");
    details.add_css_class("dim-label");
    details.add_css_class("stickpile-details");
    let footer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    footer.add_css_class("stickpile-footer");
    footer.append(&details);
    footer.append(&row);

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
    apply_theme(&window, cfg.ui.theme);

    let w = Widgets {
        window,
        stack,
        error,
        tabs,
        back,
        title,
        entry,
        syntax,
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
        safe,
        safe_label,
        paste_note,
        status,
        details,
        suggest,
        suggest_view,
        suggest_box,
        confirm,
        menu_anchor,
    };
    relabel_widgets(&w);
    w
}

/// The fixed texts of the window, in the current language (the tab bar,
/// the help and the pages have their own).
fn relabel_widgets(w: &Widgets) {
    w.back.set_tooltip_text(Some(t("Back to the packs (-, Backspace)")));
    w.syntax.set_label(t("Syntax"));
    w.syntax.set_tooltip_text(Some(t("Search syntax (F2)")));
    w.syntax
        .update_property(&[gtk::accessible::Property::Label(t("Search syntax"))]);
    w.target.set_tooltip_text(Some(t(
        "Target app — Ctrl+T / Ctrl+Shift+T to change\nCtrl+S: sticker mode · Shift+Enter: copy with sticker mode flipped",
    )));
    w.sticker.set_tooltip_text(Some(t("Send as sticker (Ctrl+S)")));
    w.sticker
        .update_property(&[gtk::accessible::Property::Label(t("Send as sticker"))]);
    w.safe
        .set_tooltip_text(Some(t("Safe mode: hide items tagged nsfw (S, Ctrl+Shift+S)")));
    w.safe
        .update_property(&[gtk::accessible::Property::Label(t("Safe mode"))]);
    w.paste_note.set_label(t("paste: by hand"));
    w.paste_note.set_tooltip_text(Some(t(
        "Automatic paste needs Hyprland, Sway or X11: this session only copies, so paste with Ctrl+V. Settings, Paste after a pick, turns this off.",
    )));
    w.error.set_title(t("stickpile cannot start"));
}
