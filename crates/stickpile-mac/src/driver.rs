//! The popup's driver: the GTK shell (`stickpile-desktop/src/ui/shell.rs`)
//! without GTK. It feeds events into the `stickpile-quick` model, runs the
//! commands it returns (network and files on a tokio runtime), and hands the
//! app a [`ViewState`] to draw.
//!
//! Everything here runs on the app's main thread: the app calls the
//! [`crate::bridge::Popup`] methods there, and background work comes back
//! through [`PopupUi::post`]. The state sits behind a mutex only because
//! UniFFI objects must be `Sync`; it is never held while the app is called.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use stickpile_api::{ItemSummary, SetSummary, TargetApp};
use stickpile_core::settings as config_file;
use stickpile_core::{ClientConfig, PreparedCopy, Session};
use stickpile_desktop::backend::{self, Backend, BoxFut, CoreBackend, Credentials, LoginError, Profile};
use stickpile_desktop::demo::DemoBackend;
use stickpile_desktop::{glue, pages, sync};
use stickpile_i18n::global::{t, tf, tp};
use stickpile_i18n::Lang;
use stickpile_quick::focus::{FocusBackend, WindowInfo};
use stickpile_quick::model::{self, Command, Event, Model, OpenInfo, StickerMode, Tile};
use stickpile_quick::{Mode, Tab, View, HELP};

use crate::bridge::*;
use crate::keys::{mac_key_names, MacKey};
use crate::me::MeState;
use crate::pasteboard;
use crate::settings_page::{to_toml, SettingsPage};

/// Concurrent thumbnail downloads.
const THUMB_CONCURRENCY: usize = 6;
const SESSION_OPEN_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Thumb {
    Loading,
    Ready,
    Failed,
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

struct State {
    cfg: ClientConfig,
    config_path: PathBuf,
    config_error: Option<String>,
    demo_mode: bool,
    demo: Option<Arc<DemoBackend>>,
    backend: Option<Arc<dyn Backend>>,
    backend_error: Option<String>,
    model: Model,
    /// The tab on screen at the last render (`None`: hidden).
    shown: Option<Tab>,
    /// The grid geometry last given to the model.
    layout: Option<(usize, usize)>,
    /// The grid's visible size in points, as the app reported it.
    grid_px: Option<(f64, f64)>,
    last_tab_path: PathBuf,
    saved_tab: Option<Tab>,
    account_lang: Option<Lang>,
    account_lang_path: PathBuf,
    system_languages: Vec<String>,
    thumbs: HashMap<String, Thumb>,
    covers: HashMap<String, Thumb>,
    /// A debounced search waiting for its time.
    search: Option<tokio::task::AbortHandle>,
    sync: sync::SyncPolicy,
    history_path: PathBuf,
    me: MeState,
    settings: SettingsPage,
    /// `entries_revision` of the lists last sent to the app.
    sent_revision: Option<u64>,
}

pub struct Driver {
    ui: Arc<dyn PopupUi>,
    rt: tokio::runtime::Runtime,
    weak: Weak<Driver>,
    thumb_permits: Arc<tokio::sync::Semaphore>,
    s: Mutex<State>,
}

fn init_logging(log_file: Option<&str>) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let file = log_file.and_then(|path| {
        let path = PathBuf::from(path);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()
    });
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false);
    let _ = match file {
        Some(f) => builder.with_writer(Mutex::new(f)).try_init(),
        None => builder.with_writer(std::io::stderr).try_init(),
    };
}

impl Driver {
    pub fn new(options: StartOptions, ui: Arc<dyn PopupUi>) -> Arc<Driver> {
        init_logging(options.log_file.as_deref());
        let config_path = options
            .config_path
            .map(PathBuf::from)
            .unwrap_or_else(ClientConfig::default_path);
        tracing::info!(
            "stickpile {} (macOS app), config {}",
            env!("CARGO_PKG_VERSION"),
            config_path.display()
        );
        let (cfg, config_error) = match ClientConfig::load(&config_path) {
            Ok(c) => (c, None),
            Err(e) => {
                tracing::warn!("{e}");
                (ClientConfig::default(), Some(e.to_string()))
            }
        };
        let account_lang_path = glue::account_lang_path(&cfg);
        let account_lang = glue::load_account_lang(&account_lang_path);
        let system_languages = options.system_languages;
        stickpile_i18n::global::set(effective_lang(&cfg, account_lang, &system_languages));
        let history_path = glue::history_path(&cfg);
        let mut model = Model::with_history(glue::model_config(&cfg), glue::load_history(&history_path));
        let last_tab_path = glue::last_tab_path(&cfg);
        let saved_tab = glue::load_last_tab(&last_tab_path);
        if let Some(tab) = saved_tab {
            model.restore_tab(tab);
        }
        let mut backend_error = None;
        let demo_dir = options.demo_dir.map(PathBuf::from);
        let demo = match (&demo_dir, &config_error) {
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
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("stickpile-rt")
            .enable_all()
            .build()
            .expect("tokio runtime");
        let state = State {
            config_path,
            config_error,
            demo_mode: demo_dir.is_some(),
            backend: demo.clone().map(|d| d as Arc<dyn Backend>),
            demo,
            backend_error,
            model,
            shown: None,
            layout: None,
            grid_px: None,
            last_tab_path,
            saved_tab,
            account_lang,
            account_lang_path,
            system_languages,
            thumbs: HashMap::new(),
            covers: HashMap::new(),
            search: None,
            sync: sync::SyncPolicy::default(),
            history_path,
            me: MeState::default(),
            settings: SettingsPage::default(),
            sent_revision: None,
            cfg,
        };
        Arc::new_cyclic(|weak| Driver {
            ui,
            rt,
            weak: weak.clone(),
            thumb_permits: Arc::new(tokio::sync::Semaphore::new(THUMB_CONCURRENCY)),
            s: Mutex::new(state),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.s.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn start(&self) {
        let open = {
            let s = self.lock();
            s.config_error.is_none() && !s.demo_mode
        };
        if open {
            self.open_session(false);
        }
        self.start_sync_timer();
        self.render();
        self.refresh_account_lang();
    }

    // -- background work ------------------------------------------------------------

    /// Run `fut` on the runtime and hand its output to `done` on the main
    /// thread (`None` if the task panicked).
    fn spawn<T: Send + 'static>(&self, fut: BoxFut<T>, done: impl FnOnce(&Driver, Option<T>) + Send + 'static) {
        let handle = self.rt.spawn(fut);
        let weak = self.weak.clone();
        let ui = self.ui.clone();
        self.rt.spawn(async move {
            let out = match handle.await {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::error!("background task failed: {e}");
                    None
                }
            };
            ui.post(MainTask::new(move || {
                if let Some(d) = weak.upgrade() {
                    done(&d, out);
                }
            }));
        });
    }

    /// Run `f` on the main thread after `delay`; the handle cancels it.
    fn after(&self, delay: Duration, f: impl FnOnce(&Driver) + Send + 'static) -> tokio::task::AbortHandle {
        let weak = self.weak.clone();
        let ui = self.ui.clone();
        self.rt
            .spawn(async move {
                tokio::time::sleep(delay).await;
                ui.post(MainTask::new(move || {
                    if let Some(d) = weak.upgrade() {
                        f(&d);
                    }
                }));
            })
            .abort_handle()
    }

    fn backend(&self) -> Option<Arc<dyn Backend>> {
        self.lock().backend.clone()
    }

    fn login_required(&self) -> bool {
        self.backend().is_some_and(|b| b.login_required())
    }

    /// (Re)open the core session. `login_required`: what the old session
    /// learned about the login (same server and token file).
    fn open_session(&self, login_required: bool) {
        let cfg = self.lock().cfg.clone();
        let task = self.rt.spawn(Session::open(cfg));
        // Opening only loads the token and the cache; never let it freeze the UI.
        let outcome = self
            .rt
            .block_on(async { tokio::time::timeout(SESSION_OPEN_TIMEOUT, task).await });
        let mut s = self.lock();
        match outcome {
            Ok(Ok(Ok(session))) => {
                let carried = login_required && !session.is_logged_in();
                s.backend = Some(Arc::new(CoreBackend::new(session, carried)));
                s.backend_error = None;
            }
            Ok(Ok(Err(e))) => s.backend_error = Some(backend::describe(&e)),
            Ok(Err(e)) => s.backend_error = Some(tf("stickpile-core failed: {error}", &[("error", &e)])),
            Err(_) => s.backend_error = Some(t("opening the local cache timed out").into()),
        }
        if let Some(e) = &s.backend_error {
            tracing::warn!("opening the session: {e}");
        }
    }

    /// Run a backend request and hand its outcome to the model; every
    /// request is answered, with an error when there is no backend.
    fn answer<T: Send + 'static>(
        &self,
        request: impl FnOnce(Arc<dyn Backend>) -> BoxFut<Result<T, String>>,
        event: impl FnOnce(Result<T, String>) -> Event + Send + 'static,
    ) {
        let Some(b) = self.backend() else {
            self.dispatch(event(Err(t("not connected").into())));
            return;
        };
        self.spawn(request(b), move |d, out| {
            let outcome = out.unwrap_or_else(|| Err(t("internal error (see log)").into()));
            d.dispatch(event(outcome));
        });
    }

    // -- language -------------------------------------------------------------------

    fn refresh_account_lang(&self) {
        let Some(b) = self.backend() else { return };
        self.spawn(b.profile(), |d, out| {
            if let Some(Ok(p)) = out {
                d.account_lang_known(&p);
            }
        });
    }

    /// A profile arrived: follow its language when it changed.
    fn account_lang_known(&self, p: &Profile) {
        if p.auth == backend::AuthKind::None {
            return;
        }
        let lang = p.language.as_deref().and_then(Lang::parse);
        let changed = {
            let mut s = self.lock();
            let changed = s.account_lang != lang;
            s.account_lang = lang;
            if changed {
                if let Err(e) = glue::save_account_lang(&s.account_lang_path, lang) {
                    tracing::debug!("saving the account's language: {e}");
                }
            }
            changed
        };
        if changed {
            self.apply_language();
        }
    }

    fn apply_language(&self) {
        let lang = {
            let s = self.lock();
            effective_lang(&s.cfg, s.account_lang, &s.system_languages)
        };
        if lang == stickpile_i18n::global::get() {
            return;
        }
        tracing::info!("language: {lang}");
        stickpile_i18n::global::set(lang);
        if self.lock().model.tab() == Tab::Settings {
            self.reload_settings();
        }
        self.render();
    }

    // -- entry points ------------------------------------------------------------------

    fn capture(&self, front: Option<FrontApp>) -> OpenInfo {
        let s = self.lock();
        if !s.cfg.focus.detect {
            return OpenInfo::default();
        }
        let window = front.map(|f| WindowInfo {
            class: f.bundle_id,
            title: f.name,
            backend: FocusBackend::MacOs,
        });
        glue::open_info(&s.cfg, Ok(window))
    }

    pub fn toggle(&self, front: Option<FrontApp>, popup_active: bool) {
        let open = if popup_active {
            OpenInfo::default()
        } else {
            self.capture(front)
        };
        self.dispatch(Event::Toggle { popup_active, open });
        self.login_prompt();
    }

    pub fn open(&self, front: Option<FrontApp>, tab: Option<TabId>) {
        let info = self.capture(front);
        self.dispatch(Event::Open(info));
        match tab {
            Some(tab) => self.dispatch(Event::ShowTab(tab.into())),
            None => self.login_prompt(),
        }
    }

    pub fn hide(&self) {
        if self.lock().model.is_visible() {
            self.dispatch(Event::Hide);
        }
    }

    pub fn hidden(&self) {
        if self.lock().model.is_visible() {
            self.dispatch(Event::Hidden);
            self.remember_tab();
        }
    }

    /// The server rejected us: nothing works until a login, so the popup
    /// opens on the Me tab with its form.
    fn login_prompt(&self) {
        if self.lock().model.is_visible() && self.login_required() {
            self.dispatch(Event::ShowTab(Tab::Me));
        }
    }

    pub fn key(&self, input: KeyInput) -> bool {
        let key = MacKey::from_input(&input);
        let (mode, tab) = {
            let s = self.lock();
            (s.model.mode(), s.model.tab())
        };
        match stickpile_desktop::keys::map_key(&key.press(), mode, tab) {
            Some(event) => {
                self.dispatch(event);
                true
            }
            None => false,
        }
    }

    pub fn query_changed(&self, text: String) {
        if self.lock().model.query() != text {
            self.dispatch(Event::QueryChanged(text));
        }
    }

    pub fn select(&self, index: u32) {
        self.dispatch(Event::Select(index as usize));
    }

    pub fn activate(&self, index: u32) {
        self.dispatch(Event::CopyAt {
            index: index as usize,
            toggle_sticker: false,
        });
    }

    pub fn show_tab(&self, tab: TabId) {
        self.dispatch(Event::ShowTab(tab.into()));
    }

    pub fn toggle_help(&self) {
        self.dispatch(Event::ToggleHelp);
    }

    pub fn toggle_sticker(&self) {
        self.dispatch(Event::ToggleSticker);
    }

    pub fn cycle_target(&self) {
        self.dispatch(Event::CycleTarget { backwards: false });
    }

    pub fn pack_back(&self) {
        self.dispatch(Event::PackBack);
    }

    pub fn text_focus(&self, focused: bool) {
        let modal = self.lock().model.config().modal;
        if modal {
            self.dispatch(Event::TextFocus(focused));
        }
    }

    pub fn grid_size(&self, width: f64, height: f64) {
        self.lock().grid_px = Some((width, height));
        if self.update_layout() {
            self.render();
        }
    }

    pub fn window_size(&self) -> Size {
        let (w, h) = glue::window_size(&self.lock().cfg);
        Size {
            width: w as f64,
            height: h as f64,
        }
    }

    // -- model loop --------------------------------------------------------------------

    pub fn dispatch(&self, event: Event) {
        let commands = self.lock().model.handle(event);
        self.update_layout();
        self.render();
        self.page_shown();
        for c in commands {
            self.execute(c);
        }
    }

    /// Columns and visible rows of the grid shown, from the size the app
    /// reported; before it has one, what the config asks for. Tells the
    /// model when they changed; `true` then.
    fn update_layout(&self) -> bool {
        let mut s = self.lock();
        let cell = (s.cfg.ui.thumb_px + CELL_EXTRA) as i32;
        let view = s.model.view();
        let layout = match (s.grid_px, view) {
            (Some((w, h)), View::Packs) => glue::grid_layout(
                w as i32,
                h as i32,
                cell,
                cell + stickpile_desktop::glue::PACK_LABELS_PX as i32,
            ),
            (Some((w, h)), _) => glue::grid_layout(w as i32, h as i32, cell, cell),
            (None, View::Packs) => (s.cfg.ui.columns.max(1) as usize, glue::pack_rows(&s.cfg)),
            (None, _) => (s.cfg.ui.columns.max(1) as usize, glue::VISIBLE_ROWS as usize),
        };
        if s.layout == Some(layout) {
            return false;
        }
        s.layout = Some(layout);
        // Only geometry: nothing to run.
        let _ = s.model.handle(Event::Layout {
            columns: layout.0,
            visible_rows: layout.1,
        });
        true
    }

    /// A page that came into view loads its data.
    fn page_shown(&self) {
        let shown = {
            let mut s = self.lock();
            let shown = s.model.is_visible().then_some(s.model.tab());
            if shown == s.shown {
                return;
            }
            s.shown = shown;
            shown
        };
        match shown {
            Some(Tab::Me) => self.load_profile(),
            Some(Tab::Settings) => self.reload_settings(),
            _ => {}
        }
    }

    /// Keep the tab the popup closed on, for the next start.
    fn remember_tab(&self) {
        let (tab, path) = {
            let mut s = self.lock();
            let tab = s.model.tab();
            if s.saved_tab == Some(tab) {
                return;
            }
            s.saved_tab = Some(tab);
            (tab, s.last_tab_path.clone())
        };
        self.rt.spawn_blocking(move || {
            if let Err(e) = glue::save_last_tab(&path, tab) {
                tracing::warn!("saving {}: {e}", path.display());
            }
        });
    }

    /// A `z` key showed or hid something on the tiles: save it like the
    /// Settings tab would, unless `config.toml` sets that key.
    fn save_tile(&self, tile: Tile, on: bool) {
        let key = glue::tile_key(tile);
        let config_path = self.lock().config_path.clone();
        let layered = match config_file::load_layered(&config_path) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("saving {key}: {e}");
                return;
            }
        };
        if layered.locked.contains(key) {
            tracing::info!("{key} is set in {}; not saved", config_path.display());
            return;
        }
        if let Err(e) = config_file::set_value(&layered.write_path, key, toml::Value::Boolean(on)) {
            tracing::warn!("saving {key}: {e}");
            return;
        }
        self.reload_settings();
    }

    fn execute(&self, command: Command) {
        match command {
            Command::Present => self.present(),
            Command::Hide => {
                self.remember_tab();
                self.ui.hide();
            }
            Command::Search {
                query,
                target,
                limit,
                generation,
                debounce_ms,
            } => self.schedule_search(query, target, limit, generation, debounce_ms),
            Command::CancelSearch => self.cancel_search(),
            Command::LoadRecent { limit } => {
                let Some(b) = self.backend() else { return };
                self.spawn(b.recent(limit), |d, items| {
                    d.dispatch(Event::RecentLoaded(items.unwrap_or_default()));
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
                let path = self.lock().history_path.clone();
                self.rt.spawn_blocking(move || {
                    if let Err(e) = glue::save_history(&path, &history) {
                        tracing::warn!("saving {}: {e}", path.display());
                    }
                });
            }
            Command::OpenInBrowser { item_id } => match self.backend().and_then(|b| b.web_url(&item_id)) {
                Some(url) => self.ui.open_url(url),
                None => tracing::warn!("no web page for {item_id}"),
            },
            Command::Page(key) => self.ui.page_key(key.into()),
            // A stale timer is ignored by the model, so none is cancelled.
            Command::KeyTimeout { after_ms, seq } => {
                self.after(Duration::from_millis(after_ms), move |d| {
                    d.dispatch(Event::KeyTimeout { seq })
                });
            }
        }
    }

    fn present(&self) {
        let reopen = {
            let s = self.lock();
            !s.demo_mode && s.config_error.is_none()
        } && (self.backend().is_none() || self.login_required());
        // Pick up a token written by `stickpile login` since the server said no.
        if reopen {
            let required = self.login_required();
            self.open_session(required);
            self.render();
        }
        {
            // Failed thumbnails and covers get another chance on every open.
            let mut s = self.lock();
            s.thumbs.retain(|_, t| *t != Thumb::Failed);
            s.covers.retain(|_, t| *t != Thumb::Failed);
        }
        self.ui.present();
        self.maybe_sync();
    }

    // -- rendering -------------------------------------------------------------------------

    fn render(&self) {
        let view = self.view_state();
        self.ui.render(view);
    }

    fn view_state(&self) -> ViewState {
        let mut s = self.lock();
        let s = &mut *s;
        let m = &s.model;
        let tab = m.tab();
        let view = m.view();
        let field = tab.has_text_field();
        let placeholder = match (tab, m.open_pack()) {
            (Tab::Packs, Some(p)) => tf("Filter {pack} by name, keyword or emoji", &[("pack", &p.name)]),
            (Tab::Packs, None) => t("Filter packs by name").to_string(),
            _ => t("Search, or try e:😂 set:cats type:gif vibe:tired").to_string(),
        };
        let count = match view {
            View::Items => pages::item_count(m.entries().len() as u64),
            View::Packs => tp(m.packs().len() as u64, "{n} pack", "{n} packs"),
            View::Page => String::new(),
        };
        let revision = m.entries_revision();
        let lists_changed = s.sent_revision != Some(revision);
        let tiles = m.tiles();
        let entries = lists_changed.then(|| {
            m.entries()
                .iter()
                .map(|e| EntryView {
                    id: e.item.id.clone(),
                    name: e.item.name.clone(),
                    badge: e.badge().map(str::to_string),
                    tooltip: e.tooltip(),
                    incompatible: e.incompatible.is_some(),
                    liked: tiles.favorites && e.liked,
                    emojis: if tiles.emojis {
                        e.item.emojis.concat()
                    } else {
                        String::new()
                    },
                    tags: if tiles.tags {
                        e.item.tags.join(", ")
                    } else {
                        String::new()
                    },
                })
                .collect()
        });
        let packs = lists_changed.then(|| {
            m.packs()
                .iter()
                .map(|set| PackView {
                    id: set.id.clone(),
                    cover_key: cover_key(set),
                    name: set.name.clone(),
                    detail: pages::item_count(set.item_count),
                })
                .collect()
        });
        s.sent_revision = Some(revision);
        let f = m.footer();
        let footer = Footer {
            mode: f.mode.map(|mode| mode.label().to_string()),
            insert: f.mode == Some(Mode::Insert),
            pending: f.pending,
            target: f.target,
            target_hint: f.target_hint,
            sticker: match f.sticker {
                StickerMode::On => StickerState::On,
                StickerMode::Off => StickerState::Off,
                StickerMode::Unsupported => StickerState::Unsupported,
            },
            sticker_label: if f.sticker == StickerMode::On {
                t("sticker: on")
            } else {
                t("sticker: off")
            }
            .to_string(),
            status: f.status,
            level: match f.level {
                model::Level::Info => Level::Info,
                model::Level::Busy => Level::Busy,
                model::Level::Warning => Level::Warning,
                model::Level::Error => Level::Error,
            },
        };
        let focus = match m.mode() {
            _ if !m.is_visible() => FocusWish::Keep,
            Mode::Insert if field => FocusWish::Field,
            Mode::Insert => FocusWish::Keep,
            Mode::Normal => FocusWish::NoField,
        };
        let error = if s.config_error.is_some() || s.backend.is_none() {
            Some(
                s.config_error
                    .clone()
                    .or_else(|| s.backend_error.clone())
                    .unwrap_or_else(|| t("unknown error").into()),
            )
        } else {
            None
        };
        ViewState {
            visible: m.is_visible(),
            error,
            config_path: s.config_path.display().to_string(),
            language: stickpile_i18n::global::get().code().to_string(),
            tabs: Tab::ALL
                .iter()
                .map(|t| TabLabel {
                    tab: (*t).into(),
                    label: t.label().to_string(),
                    number: t.number(),
                })
                .collect(),
            tab: tab.into(),
            view: match view {
                View::Items => ViewKind::Items,
                View::Packs => ViewKind::Packs,
                View::Page if tab == Tab::Me => ViewKind::Me,
                View::Page => ViewKind::Settings,
            },
            focus,
            title: m.title(),
            has_field: field,
            query: m.query().to_string(),
            placeholder,
            count,
            back: tab == Tab::Packs && m.open_pack().is_some(),
            entries_revision: revision,
            entries,
            packs,
            selected: m.selected().filter(|_| view != View::Page).map(|i| i as u32),
            columns: m.columns() as u32,
            thumb_px: s.cfg.ui.thumb_px,
            tiles: crate::bridge::Tiles {
                favorites: tiles.favorites,
                emojis: tiles.emojis,
                tags: tiles.tags,
            },
            help: m.help_visible().then(help),
            footer,
            me: s.me.view(),
            settings: s.settings.view(&s.config_path),
        }
    }

    // -- Me and Settings --------------------------------------------------------------------

    fn load_profile(&self) {
        let Some(b) = self.backend() else {
            self.lock().me.profile_loaded(Err(t("not connected").into()));
            self.render();
            return;
        };
        self.lock().me.loading();
        self.render();
        self.spawn(b.profile(), |d, out| {
            if let Some(Ok(p)) = &out {
                d.account_lang_known(p);
            }
            d.lock()
                .me
                .profile_loaded(out.unwrap_or_else(|| Err(t("internal error (see log)").into())));
            d.render();
        });
    }

    pub fn retry_profile(&self) {
        self.load_profile();
    }

    pub fn login(&self, username: String, password: String, totp: String) {
        let username = username.trim().to_string();
        if username.is_empty() || password.is_empty() {
            self.lock().me.missing_credentials();
            self.render();
            return;
        }
        let totp = Some(totp.trim().to_string()).filter(|c| !c.is_empty() && self.lock().me.wants_totp());
        self.run_login(Some(Credentials {
            username,
            password,
            totp,
        }));
    }

    pub fn login_tailnet(&self) {
        self.run_login(None);
    }

    fn run_login(&self, credentials: Option<Credentials>) {
        let Some(b) = self.backend() else {
            self.lock()
                .me
                .login_finished(Err(LoginError::Failed(t("not connected").into())));
            self.render();
            return;
        };
        if !self.lock().me.start(false) {
            return;
        }
        self.render();
        self.spawn(b.login(credentials), |d, out| {
            let outcome = out.unwrap_or_else(|| Err(LoginError::Failed(t("internal error (see log)").into())));
            let ok = outcome.is_ok();
            if let Ok(p) = &outcome {
                d.account_lang_known(p);
            }
            d.lock().me.login_finished(outcome);
            d.render();
            if ok {
                d.account_changed();
            }
        });
    }

    pub fn logout(&self) {
        let Some(b) = self.backend() else { return };
        if !self.lock().me.start(true) {
            return;
        }
        self.render();
        self.spawn(b.logout(), |d, out| {
            d.lock()
                .me
                .logout_finished(out.unwrap_or_else(|| Err(t("internal error (see log)").into())));
            d.account_changed();
            d.load_profile();
        });
    }

    pub fn open_server(&self) {
        let url = self.lock().cfg.server_url.clone();
        if url.starts_with("http://") || url.starts_with("https://") {
            self.ui.open_url(url);
        }
    }

    /// Logged in or out, or another server: what the item tabs show
    /// belongs to the old account. Fetch it again and sync the new library.
    fn account_changed(&self) {
        {
            let mut s = self.lock();
            s.thumbs.retain(|_, t| *t != Thumb::Failed);
            s.covers.clear();
            s.sync = sync::SyncPolicy::default();
        }
        self.dispatch(Event::Reload);
        self.maybe_sync();
    }

    pub fn set_setting(&self, key: String, value: SettingValue) {
        let value = to_toml(value);
        self.settings_action(&key, Some(value));
    }

    pub fn reset_setting(&self, key: String) {
        self.settings_action(&key, None);
    }

    /// Save a Settings change (`None`: back to the default) and apply it.
    fn settings_action(&self, key: &str, value: Option<toml::Value>) {
        let path = {
            let s = self.lock();
            s.settings.write_path(&s.config_path)
        };
        // A language chosen here becomes the account's too, so the web UI
        // follows (automatic leaves the account alone).
        let chosen_lang = match (key, &value) {
            ("ui.language", Some(toml::Value::String(v))) => Lang::parse(v),
            _ => None,
        };
        let result = match value {
            Some(v) => config_file::set_value(&path, key, v),
            None => config_file::unset_value(&path, key),
        };
        let error = result.as_ref().err().map(|e| {
            tracing::warn!("saving a setting: {e}");
            tf("Not saved: {error}", &[("error", &e)])
        });
        self.lock().settings.save_failed(error);
        if let (Ok(()), Some(lang), Some(b)) = (&result, chosen_lang, self.backend()) {
            self.spawn(
                b.set_account_language(Some(lang.code().to_string())),
                move |d, out| match out {
                    Some(Ok(())) => {
                        let mut s = d.lock();
                        s.account_lang = Some(lang);
                        let _ = glue::save_account_lang(&s.account_lang_path, Some(lang));
                    }
                    Some(Err(e)) => tracing::info!("the account's language stays: {e}"),
                    None => {}
                },
            );
        }
        self.reload_settings();
        self.render();
    }

    /// Read the config again for the Settings page, and apply it if it changed.
    fn reload_settings(&self) {
        let config_path = self.lock().config_path.clone();
        match config_file::load_layered(&config_path) {
            Ok(layered) => {
                let new = layered.config.clone();
                let changed = {
                    let mut s = self.lock();
                    s.settings.show(layered);
                    new != s.cfg
                };
                if changed {
                    self.apply_config(new);
                } else {
                    self.render();
                }
            }
            Err(e) => {
                self.lock().settings.set_error(Some(e.to_string()));
                self.render();
            }
        }
    }

    /// Use a new config without a restart.
    fn apply_config(&self, new: ClientConfig) {
        let old = std::mem::replace(&mut self.lock().cfg, new.clone());
        if old.ui.columns != new.ui.columns || old.ui.thumb_px != new.ui.thumb_px {
            self.lock().layout = None;
        }
        if old.ui.language != new.ui.language {
            self.apply_language();
        }
        let account = pages::account_changed(&old, &new);
        if pages::needs_new_session(&old, &new) {
            let (demo, demo_mode, config_error) = {
                let s = self.lock();
                (s.demo.clone(), s.demo_mode, s.config_error.is_some())
            };
            match demo {
                Some(demo) => demo.configure(&new),
                None if !demo_mode && !config_error => {
                    // Release the old session's cache first; if the new one
                    // cannot open, the error page says so rather than the
                    // old server answering on.
                    let login_required = !account && self.login_required();
                    self.lock().backend = None;
                    self.open_session(login_required);
                }
                None => {}
            }
        }
        self.dispatch(Event::Configure(glue::model_config(&new)));
        if account {
            self.lock().me.forget();
            self.account_changed();
            if self.lock().model.tab() == Tab::Me {
                self.load_profile();
            }
        }
    }

    // -- search ------------------------------------------------------------------------------

    fn cancel_search(&self) {
        if let Some(h) = self.lock().search.take() {
            h.abort();
        }
    }

    fn schedule_search(&self, query: String, target: TargetApp, limit: usize, generation: u64, debounce_ms: u64) {
        self.cancel_search();
        if debounce_ms == 0 {
            self.run_search(query, target, limit, generation);
            return;
        }
        let handle = self.after(Duration::from_millis(debounce_ms), move |d| {
            d.lock().search = None;
            d.run_search(query, target, limit, generation);
        });
        self.lock().search = Some(handle);
    }

    fn run_search(&self, query: String, target: TargetApp, limit: usize, generation: u64) {
        self.answer(
            move |b| b.search(query, target, limit),
            move |outcome| Event::Results { generation, outcome },
        );
    }

    // -- thumbnails and covers -------------------------------------------------------------------

    fn load_thumbnails(&self, items: Vec<ItemSummary>) {
        let Some(backend) = self.backend() else { return };
        for item in items {
            let id = item.id.clone();
            {
                let mut s = self.lock();
                if s.thumbs.contains_key(&id) {
                    continue;
                }
                s.thumbs.insert(id.clone(), Thumb::Loading);
            }
            let b = backend.clone();
            let permits = self.thumb_permits.clone();
            let fut: BoxFut<Result<PathBuf, String>> = Box::pin(async move {
                let _permit = permits.acquire_owned().await.map_err(|e| e.to_string())?;
                b.thumbnail(item).await
            });
            self.spawn(fut, move |d, out| d.image_done(id, None, out.map(|r| r.map(Some))));
        }
    }

    fn load_covers(&self, packs: Vec<SetSummary>) {
        let Some(backend) = self.backend() else { return };
        for set in packs {
            let key = cover_key(&set);
            {
                let mut s = self.lock();
                if s.covers.contains_key(&key) {
                    continue;
                }
                s.covers.insert(key.clone(), Thumb::Loading);
            }
            let b = backend.clone();
            let permits = self.thumb_permits.clone();
            let fut: BoxFut<Result<Option<PathBuf>, String>> = Box::pin(async move {
                let _permit = permits.acquire_owned().await.map_err(|e| e.to_string())?;
                b.pack_cover(set).await
            });
            let set_key = key.clone();
            self.spawn(fut, move |d, out| d.image_done(set_key, Some(()), out));
        }
    }

    /// A thumbnail (`cover: None`) or a pack cover finished loading.
    fn image_done(&self, key: String, cover: Option<()>, out: Option<Result<Option<PathBuf>, String>>) {
        let state = match out {
            Some(Ok(Some(path))) => {
                self.ui.thumbnail(key.clone(), path.display().to_string());
                Thumb::Ready
            }
            Some(Err(e)) => {
                tracing::debug!("image {key:?}: {e}");
                Thumb::Failed
            }
            // No cover: the placeholder stays.
            Some(Ok(None)) | None => Thumb::Failed,
        };
        let mut s = self.lock();
        let map = if cover.is_some() { &mut s.covers } else { &mut s.thumbs };
        map.insert(key, state);
    }

    // -- copy ---------------------------------------------------------------------------------------

    fn copy(&self, item: ItemSummary, target: TargetApp, sticker: bool) {
        let id = item.id.clone();
        let Some(b) = self.backend() else {
            self.dispatch(Event::CopyFailed {
                item_id: id,
                error: t("not connected").into(),
            });
            return;
        };
        self.spawn(b.prepare_copy(item.clone(), target, sticker), move |d, out| {
            let result = out.unwrap_or_else(|| Err(t("internal error (see log)").into()));
            match result.and_then(|p| d.set_clipboard(&p).map(|()| p)) {
                Ok(prepared) => {
                    let warning = prepared.warning.clone();
                    d.dispatch(Event::CopyFinished { item_id: id, warning });
                    d.record(item, prepared);
                }
                Err(error) => d.dispatch(Event::CopyFailed { item_id: id, error }),
            }
        });
    }

    fn set_clipboard(&self, prepared: &PreparedCopy) -> Result<(), String> {
        let items = pasteboard::items(&prepared.offers);
        if items.is_empty() {
            return Err(t("nothing to offer (check [clipboard] in config.toml)").into());
        }
        let summary = pasteboard::summary(&items);
        if let Some(e) = self.ui.set_clipboard(items) {
            return Err(e);
        }
        tracing::info!("pasteboard: {} [{summary}]", prepared.file.display());
        Ok(())
    }

    fn record(&self, item: ItemSummary, prepared: PreparedCopy) {
        let Some(b) = self.backend() else { return };
        self.spawn(b.record_copy(item, prepared), |_, out| {
            if let Some(Err(e)) = out {
                tracing::warn!("recording usage: {e}");
            }
        });
    }

    // -- sync ------------------------------------------------------------------------------------------------

    fn maybe_sync(&self) {
        let now = Instant::now();
        if !self.lock().sync.due(now) {
            return;
        }
        let Some(b) = self.backend() else { return };
        self.lock().sync.started(now);
        self.spawn(b.sync(), |d, out| {
            let ok = matches!(out, Some(Ok(())));
            if let Some(Err(e)) = out {
                tracing::debug!("sync: {e}");
            }
            d.lock().sync.finished(Instant::now(), ok);
        });
    }

    fn start_sync_timer(&self) {
        self.after(sync::TICK, |d| {
            d.maybe_sync();
            d.start_sync_timer();
        });
    }
}

/// `ui.language` for this config and account; `auto` without an account
/// language follows the environment, then the system's preferred languages.
fn effective_lang(cfg: &ClientConfig, account: Option<Lang>, system: &[String]) -> Lang {
    let preferred = system.join(":");
    glue::effective_lang(cfg, account, |k| {
        std::env::var(k)
            .ok()
            .or_else(|| (k == "LANGUAGE" && !preferred.is_empty()).then(|| preferred.clone()))
    })
}

/// The key help, translated, with Mac key names.
fn help() -> Vec<HelpSection> {
    let mut sections: Vec<HelpSection> = HELP
        .iter()
        .map(|s| HelpSection {
            title: stickpile_i18n::global::t_dyn(s.title),
            keys: s
                .keys
                .iter()
                .map(|(k, what)| HelpKey {
                    keys: mac_key_names(&stickpile_i18n::global::t_dyn(k)),
                    action: stickpile_i18n::global::t_dyn(what),
                })
                .collect(),
        })
        .collect();
    sections.retain(|s| !s.keys.is_empty());
    sections
}
