//! The GTK shell: builds the popup window, renders the `stickpile-quick`
//! model and executes its commands on the tokio runtime. It holds no popup
//! logic of its own; anything decidable without a display lives in the
//! model or in the display-free modules of this crate.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use adw::prelude::*;
use futures::FutureExt;
use gtk::{gdk, gio, glib};
use stickpile_api::{ItemSummary, TargetApp};
use stickpile_core::{ClientConfig, PreparedCopy, Session};
use stickpile_quick::focus::FocusEnv;
use stickpile_quick::model::{self, Command, Event, Level, Model, OpenInfo, StickerMode};

use crate::backend::{Backend, BoxFut, CoreBackend};
use crate::demo::DemoBackend;
use crate::keys::{self, KeyPress, Mods};
use crate::ui::drag::{self, LazyFileProvider, PreparedFuture};
use crate::ui::item::StickerObject;
use crate::{glue, offers, sync, thumbs};

/// Concurrent thumbnail downloads/decodes.
const THUMB_CONCURRENCY: usize = 6;
/// Textures kept beyond the ones on screen.
const THUMB_CACHE: usize = 600;
/// Prepared copies remembered for drag-and-drop.
const PREPARED_CACHE: usize = 16;
const SESSION_OPEN_TIMEOUT: Duration = Duration::from_secs(3);

const CSS: &str = r#"
gridview.stickpile-grid > child { padding: 4px; border-radius: 10px; }
.stickpile-badge { font-size: 0.7em; font-weight: bold; padding: 1px 5px; border-radius: 5px; margin: 6px; }
.stickpile-warn { margin: 6px; color: @warning_color; }
.stickpile-footer { padding: 4px 10px; }
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
    login: adw::StatusPage,
    error: adw::StatusPage,
    entry: gtk::SearchEntry,
    grid: gtk::GridView,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    target: gtk::Label,
    target_hint: gtk::Label,
    sticker: gtk::Label,
    status: gtk::Label,
}

pub struct Shell {
    cfg: ClientConfig,
    config_path: PathBuf,
    config_error: Option<String>,
    demo: bool,
    backend: RefCell<Option<Arc<dyn Backend>>>,
    backend_error: RefCell<Option<String>>,
    model: RefCell<Model>,
    w: Widgets,
    rendering: Cell<bool>,
    rendered_revision: Cell<u64>,
    scrolled_to: Cell<Option<usize>>,
    thumbs: RefCell<HashMap<String, Thumb>>,
    thumb_permits: Arc<tokio::sync::Semaphore>,
    prepared: RefCell<VecDeque<PreparedCopy>>,
    drag: RefCell<Option<DragState>>,
    search_timer: RefCell<Option<glib::SourceId>>,
    sync: RefCell<sync::SyncPolicy>,
    history_path: PathBuf,
}

impl Shell {
    pub fn start(app: &adw::Application, opts: StartOptions) -> Rc<Shell> {
        let (cfg, config_error) = match ClientConfig::load(&opts.config_path) {
            Ok(c) => (c, None),
            Err(e) => (ClientConfig::default(), Some(e.to_string())),
        };
        let history_path = glue::history_path(&cfg);
        let history = glue::load_history(&history_path);
        let model = Model::with_history(glue::model_config(&cfg), history);
        let w = build_widgets(app, &cfg);
        let shell = Rc::new(Shell {
            config_path: opts.config_path,
            config_error,
            demo: opts.demo.is_some(),
            backend: RefCell::new(None),
            backend_error: RefCell::new(None),
            model: RefCell::new(model),
            w,
            rendering: Cell::new(false),
            rendered_revision: Cell::new(u64::MAX),
            scrolled_to: Cell::new(None),
            thumbs: RefCell::new(HashMap::new()),
            thumb_permits: Arc::new(tokio::sync::Semaphore::new(THUMB_CONCURRENCY)),
            prepared: RefCell::new(VecDeque::new()),
            drag: RefCell::new(None),
            search_timer: RefCell::new(None),
            sync: RefCell::new(sync::SyncPolicy::default()),
            history_path,
            cfg,
        });
        if shell.config_error.is_none() {
            match &opts.demo {
                Some(dir) => match DemoBackend::scan(dir, shell.cfg.clipboard.clone()) {
                    Ok(b) => {
                        tracing::info!("demo mode: {} files from {}", b.len(), dir.display());
                        *shell.backend.borrow_mut() = Some(Arc::new(b));
                    }
                    Err(e) => *shell.backend_error.borrow_mut() = Some(format!("{}: {e}", dir.display())),
                },
                None => shell.open_session(),
            }
        }
        shell.connect_signals();
        shell.start_sync_timer();
        shell.render();
        shell
    }

    /// (Re)open the core session. Runs inside the runtime so a panic in the
    /// core cannot take the resident process down.
    fn open_session(&self) {
        let cfg = self.cfg.clone();
        let rt = runtime();
        let task = rt.spawn(Session::open(cfg));
        // Opening only loads the token and the cache; never let it freeze the UI.
        match rt.block_on(async { tokio::time::timeout(SESSION_OPEN_TIMEOUT, task).await }) {
            Ok(Ok(Ok(session))) => {
                // Still no token: whatever the old session learned still holds.
                let carried = self.login_required() && !session.is_logged_in();
                *self.backend.borrow_mut() = Some(Arc::new(CoreBackend::new(session, carried)));
                *self.backend_error.borrow_mut() = None;
            }
            Ok(Ok(Err(e))) => *self.backend_error.borrow_mut() = Some(crate::backend::describe(&e)),
            Ok(Err(e)) => *self.backend_error.borrow_mut() = Some(format!("stickpile-core failed: {e}")),
            Err(_) => *self.backend_error.borrow_mut() = Some("opening the local cache timed out".into()),
        }
    }

    fn backend(&self) -> Option<Arc<dyn Backend>> {
        self.backend.borrow().clone()
    }

    fn login_required(&self) -> bool {
        self.backend().is_some_and(|b| b.login_required())
    }

    pub fn config(&self) -> &ClientConfig {
        &self.cfg
    }

    // -- entry points from the application -------------------------------------

    /// Show the popup; the focused window is captured first.
    pub fn open(self: &Rc<Self>, env: &FocusEnv) {
        let info = glue::capture_focus(&self.cfg, env);
        self.dispatch(Event::Open(info));
    }

    pub fn toggle(self: &Rc<Self>, env: &FocusEnv) {
        let active = self.w.window.is_visible() && self.w.window.is_active();
        let open = if active {
            OpenInfo::default()
        } else {
            glue::capture_focus(&self.cfg, env)
        };
        self.dispatch(Event::Toggle {
            popup_active: active,
            open,
        });
    }

    pub fn hide(self: &Rc<Self>) {
        if self.model.borrow().is_visible() {
            self.dispatch(Event::Escape);
        }
    }

    // -- model loop --------------------------------------------------------------------

    pub fn dispatch(self: &Rc<Self>, event: Event) {
        let commands = self.model.borrow_mut().handle(event);
        self.render();
        for c in commands {
            self.execute(c);
        }
    }

    fn execute(self: &Rc<Self>, command: Command) {
        match command {
            Command::Present => self.present(),
            Command::Hide => self.w.window.set_visible(false),
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
            Command::LoadThumbnails { items } => self.load_thumbnails(items),
            Command::Copy { item, target, sticker } => self.copy(*item, target, sticker),
            Command::SaveHistory(history) => {
                let path = self.history_path.clone();
                runtime().spawn_blocking(move || {
                    if let Err(e) = glue::save_history(&path, &history) {
                        tracing::warn!("saving {}: {e}", path.display());
                    }
                });
            }
        }
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

    // -- rendering -------------------------------------------------------------------------

    fn render(&self) {
        let m = self.model.borrow();
        let w = &self.w;
        self.rendering.set(true);

        if m.entries_revision() != self.rendered_revision.get() {
            let thumbs = self.thumbs.borrow();
            let objects: Vec<StickerObject> = m
                .entries()
                .iter()
                .map(|e| {
                    let tex = match thumbs.get(&e.item.id) {
                        Some(Thumb::Ready(t)) => Some(t),
                        _ => None,
                    };
                    StickerObject::new(e, tex)
                })
                .collect();
            w.store.splice(0, w.store.n_items(), &objects);
            self.rendered_revision.set(m.entries_revision());
            self.scrolled_to.set(None);
        }

        let selected = m.selected().map(|i| i as u32).unwrap_or(gtk::INVALID_LIST_POSITION);
        if w.selection.selected() != selected {
            w.selection.set_selected(selected);
        }
        if m.is_visible() && w.window.is_visible() && m.selected() != self.scrolled_to.get() {
            if let Some(i) = m.selected() {
                w.grid.scroll_to(i as u32, gtk::ListScrollFlags::NONE, None);
            }
            self.scrolled_to.set(m.selected());
        }
        if w.entry.text() != m.query() {
            w.entry.set_text(m.query());
            w.entry.set_position(-1);
        }

        let f = m.footer();
        w.target.set_label(&f.target);
        w.target_hint.set_label(&f.target_hint);
        match f.sticker {
            StickerMode::Unsupported => w.sticker.set_visible(false),
            mode => {
                w.sticker.set_visible(true);
                w.sticker.set_label(if mode == StickerMode::On {
                    "sticker: on"
                } else {
                    "sticker: off"
                });
                set_class(&w.sticker, "accent", mode == StickerMode::On);
            }
        }
        w.status.set_label(&f.status);
        set_class(&w.status, "error", f.level == Level::Error);
        set_class(&w.status, "warning", f.level == Level::Warning);
        set_class(&w.status, "dim-label", matches!(f.level, Level::Info | Level::Busy));

        self.rendering.set(false);
        drop(m);
        self.update_page();
    }

    /// Main grid, "Not logged in" (the server rejected us) or "cannot start".
    fn update_page(&self) {
        let page = if self.config_error.is_some() || self.backend().is_none() {
            "error"
        } else if self.login_required() {
            "login"
        } else {
            "main"
        };
        if self.w.stack.visible_child_name().as_deref() == Some(page) {
            return;
        }
        let config = glib::markup_escape_text(&self.config_path.display().to_string());
        match page {
            "error" => {
                let msg = self
                    .config_error
                    .clone()
                    .or_else(|| self.backend_error.borrow().clone())
                    .unwrap_or_else(|| "unknown error".into());
                self.w.error.set_description(Some(&format!(
                    "{}\n\nConfig: <tt>{config}</tt>",
                    glib::markup_escape_text(&msg)
                )));
            }
            "login" => self.w.login.set_description(Some(&format!(
                "The server did not accept this client. Run <tt>stickpile login</tt> in a terminal \
                 (on the tailnet it needs no password), then open the popup again.\n\n\
                 Server: <tt>{}</tt>\nConfig: <tt>{config}</tt>",
                glib::markup_escape_text(&self.cfg.server_url)
            ))),
            _ => {}
        }
        self.w.stack.set_visible_child_name(page);
        if page == "main" {
            self.w.entry.grab_focus();
        }
    }

    fn present(self: &Rc<Self>) {
        // Pick up a token written by `stickpile login` since the server said no.
        if !self.demo && self.config_error.is_none() && (self.backend().is_none() || self.login_required()) {
            self.open_session();
        }
        self.update_page();
        // Failed thumbnails get another chance on every open.
        self.thumbs.borrow_mut().retain(|_, t| !matches!(t, Thumb::Failed));
        self.w.window.present();
        self.w.entry.grab_focus();
        self.maybe_sync();
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
        let Some(b) = self.backend() else {
            self.dispatch(Event::Results {
                generation,
                outcome: Err("not connected".into()),
            });
            return;
        };
        self.spawn(b.search(query, target, limit), move |this, out| {
            let outcome = out.unwrap_or_else(|| Err("internal error (see log)".into()));
            this.dispatch(Event::Results { generation, outcome });
        });
    }

    // -- thumbnails -----------------------------------------------------------------------------

    fn load_thumbnails(self: &Rc<Self>, items: Vec<ItemSummary>) {
        let Some(backend) = self.backend() else { return };
        let px = thumbs::decode_px(self.cfg.ui.thumb_px);
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
                let stride = rgba.stride();
                let tex = gdk::MemoryTexture::new(
                    rgba.width as i32,
                    rgba.height as i32,
                    gdk::MemoryFormat::R8g8b8a8,
                    &glib::Bytes::from_owned(rgba.pixels),
                    stride,
                )
                .upcast::<gdk::Texture>();
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

    // -- copy ---------------------------------------------------------------------------------------

    fn copy(self: &Rc<Self>, item: ItemSummary, target: TargetApp, sticker: bool) {
        let id = item.id.clone();
        let Some(b) = self.backend() else {
            self.dispatch(Event::CopyFailed {
                item_id: id,
                error: "not connected".into(),
            });
            return;
        };
        self.spawn(b.prepare_copy(item.clone(), target, sticker), move |this, out| {
            let result = out.unwrap_or_else(|| Err("internal error (see log)".into()));
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
            return Err("the popup was closed before the item was ready".into());
        }
        if !window.is_active() {
            return Err("the popup lost focus before the item was ready — press Enter again".into());
        }
        let specs = offers::provider_specs(&prepared.offers);
        if specs.is_empty() {
            return Err("nothing to offer (check [clipboard] in config.toml)".into());
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
        w.selection.connect_selected_notify(move |sel| {
            let i = sel.selected();
            if !this.rendering.get() && i != gtk::INVALID_LIST_POSITION {
                this.dispatch(Event::Select(i as usize));
            }
        });

        let this = self.clone();
        w.grid.connect_activate(move |_, pos| {
            this.dispatch(Event::CopyAt {
                index: pos as usize,
                toggle_sticker: false,
            });
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let this = self.clone();
        keys.connect_key_pressed(move |ctl, keyval, keycode, state| {
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
            match keys::map_key(&KeyPress {
                name: &name,
                latin: latin.as_deref(),
                mods,
            }) {
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
            glib::Propagation::Proceed
        });

        // Grid cells: picture + badge + warning icon, bound to the item's
        // properties; each cell is also a drag source for the item's file.
        let factory = gtk::SignalListItemFactory::new();
        let thumb_px = self.cfg.ui.thumb_px as i32;
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
            let badge = gtk::Label::builder()
                .halign(gtk::Align::End)
                .valign(gtk::Align::End)
                .build();
            badge.add_css_class("osd");
            badge.add_css_class("stickpile-badge");
            let warn = gtk::Image::builder()
                .icon_name("dialog-warning-symbolic")
                .halign(gtk::Align::Start)
                .valign(gtk::Align::End)
                .build();
            warn.add_css_class("stickpile-warn");
            let cell = gtk::Overlay::new();
            cell.set_child(Some(&picture));
            cell.add_overlay(&badge);
            cell.add_overlay(&warn);
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
        w.grid.set_factory(Some(&factory));
    }
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}

fn build_widgets(app: &adw::Application, cfg: &ClientConfig) -> Widgets {
    let css = gtk::CssProvider::new();
    css.load_from_string(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let entry = gtk::SearchEntry::builder()
        .hexpand(true)
        .placeholder_text("Search — e:😂 set:cats type:gif vibe:tired")
        .build();
    let top = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .build();
    top.append(&entry);

    let store = gio::ListStore::new::<StickerObject>();
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    let columns = cfg.ui.columns.max(1);
    let grid = gtk::GridView::builder()
        .model(&selection)
        .min_columns(columns)
        .max_columns(columns)
        .single_click_activate(true)
        .can_focus(false)
        .build();
    grid.add_css_class("stickpile-grid");
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&grid)
        .build();

    let target = gtk::Label::new(None);
    target.add_css_class("heading");
    target.set_tooltip_text(Some(
        "Target app — Ctrl+T / Ctrl+Shift+T to change\nCtrl+S: sticker mode · Shift+Enter: copy with sticker mode flipped",
    ));
    let target_hint = gtk::Label::new(None);
    target_hint.add_css_class("dim-label");
    target_hint.add_css_class("caption");
    let sticker = gtk::Label::new(None);
    sticker.add_css_class("caption");
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
    footer.append(&target);
    footer.append(&target_hint);
    footer.append(&sticker);
    footer.append(&status);

    let main = adw::ToolbarView::new();
    main.add_top_bar(&top);
    main.set_content(Some(&scroller));
    main.add_bottom_bar(&footer);

    let login = adw::StatusPage::builder()
        .icon_name("system-lock-screen-symbolic")
        .title("Not logged in")
        .description(format!(
            "Run <tt>stickpile login</tt> in a terminal, then open the popup again.\n\nServer: <tt>{}</tt>",
            glib::markup_escape_text(&cfg.server_url)
        ))
        .build();
    let error = adw::StatusPage::builder()
        .icon_name("dialog-error-symbolic")
        .title("stickpile cannot start")
        .build();

    let stack = gtk::Stack::new();
    stack.add_named(&main, Some("main"));
    stack.add_named(&login, Some("login"));
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

    Widgets {
        window,
        stack,
        login,
        error,
        entry,
        grid,
        store,
        selection,
        target,
        target_hint,
        sticker,
        status,
    }
}
