//! The [`StickpileLibrary`] object: one [`Session`] on a Tokio runtime of its own,
//! plus what the app needs around it (the sync schedule, the config file,
//! the items it has shown).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use stickpile_api::{
    pairing, ItemSummary, PairRequest, SetSummary, TokenCreated, TokenRequest, UploadMeta, User, Variant,
};
use stickpile_core::session::{save_login, tailnet_login};
use stickpile_core::{settings, token, ApiClient, ClientConfig, CoreError, Session, SyncPolicy};
use stickpile_quick::suggest::{self, Vocabulary};
use stickpile_quick::within::Within;

use crate::hints::{self, Words};
use crate::motion;

use crate::types::*;
use crate::{device, LibraryError, LibraryErrorKind};

/// Worker threads of the library's runtime (network and files only; the
/// work is small).
const WORKER_THREADS: usize = 2;

/// Items kept by id after a list showed them, so that `thumbnail`,
/// `prepare` and `record_use` need no request for them. Beyond this the
/// memory is cleared and refills from the next lists.
const SEEN_MAX: usize = 4096;

/// Cache `meta` key prefix of the favorites last read from the server, per
/// user id: their ids, one per line. The cache keeps no likes, so this is
/// what [`StickpileLibrary::favorites`] answers with offline.
const FAVORITES_KEY: &str = "android_favorites:";

/// Favorites read (and saved for offline) by every online
/// [`StickpileLibrary::favorites`], however few the caller asks for: the
/// server's most for a search.
const FAVORITES_SAVED: usize = 200;

/// How long a server found unreachable is believed so: meanwhile the reads
/// answer from the offline cache at once, without each waiting for the
/// timeout, and while they come in the server is checked in the background
/// every 15 s, so the lists are live again as soon as it is back
/// ([`ApiClient::with_fail_fast`]). Only a library left alone longer tries
/// the server in the foreground (waiting for the timeout once).
const FAIL_FAST: Duration = Duration::from_secs(5 * 60);

/// The stickpile library: an account on one server, its offline cache, and
/// the sync. Create one per process and share it; every method may be
/// called from any thread. `suspend` methods run on the library's own Tokio
/// runtime; the others only read local state (the config, the offline
/// cache) and return quickly.
///
/// docs/android.md calls it `Library`; that name clashes in Kotlin with
/// `com.sun.jna.Library`, which every UniFFI Kotlin file imports.
#[derive(uniffi::Object)]
pub struct StickpileLibrary {
    /// `Some` until the library is dropped.
    runtime: Option<tokio::runtime::Runtime>,
    handle: tokio::runtime::Handle,
    inner: Arc<Inner>,
}

impl std::fmt::Debug for StickpileLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StickpileLibrary")
            .field("dirs", &self.inner.dirs)
            .finish()
    }
}

impl Drop for StickpileLibrary {
    fn drop(&mut self) {
        // Never blocks, so it is fine wherever the last reference goes.
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

#[derive(Debug)]
struct Dirs {
    config_file: PathBuf,
    cache_dir: PathBuf,
    token_file: PathBuf,
}

impl Dirs {
    /// The config file (layered as on the desktop, see
    /// `stickpile_core::settings`) with the library's own cache and token.
    fn load_config(&self) -> Result<ClientConfig, LibraryError> {
        let layered = settings::load_layered(&self.config_file)?;
        for w in &layered.warnings {
            tracing::warn!("{w}");
        }
        let mut config = layered.config;
        config.cache_dir = Some(self.cache_dir.clone());
        config.token_file = Some(self.token_file.clone());
        Ok(config)
    }

    /// Save `url` as the config file's `server_url`, keeping the rest of it.
    fn save_server_url(&self, url: &str) -> Result<(), LibraryError> {
        let layered = settings::load_layered(&self.config_file)?;
        settings::set_value(&layered.write_path, "server_url", toml::Value::String(url.to_string()))?;
        Ok(())
    }
}

struct Inner {
    dirs: Dirs,
    /// Replaced when the server changes.
    session: RwLock<Arc<Session>>,
    sync: Mutex<SyncStatus>,
    /// Held while a sync runs and while the account or server changes, so
    /// the two never overlap.
    busy: tokio::sync::Mutex<()>,
    seen: Mutex<HashMap<String, ItemSummary>>,
    /// The packs listed last (`packs`, `own_packs`) by id, for `pack_cover`.
    packs_seen: Mutex<HashMap<String, SetSummary>>,
    /// The words of the suggestions and hints, and the cache generation
    /// they were read at.
    vocabulary: Mutex<Option<(u64, Arc<Words>)>>,
    generation: Mutex<Generation>,
}

/// Counts the account and server changes (logins, logouts, another
/// server), so that what a read learnt is kept only when no change came in
/// between: the items it showed (`seen`) and the favorites it saved for
/// offline. A read that finishes after a change would otherwise write the
/// old account's or server's data into the new one's. A read takes the
/// count with its session ([`Inner::current`]) and writes through
/// [`Inner::if_current`]; a change runs inside [`Inner::begin_change`].
#[derive(Debug, Default)]
struct Generation {
    count: u64,
    /// A change is under way.
    changing: bool,
}

/// An account or server change under way; ends when dropped (also when
/// the change fails).
struct Change<'a>(&'a Inner);

impl Drop for Change<'_> {
    fn drop(&mut self) {
        let mut g = lock(&self.0.generation);
        // Reads that began during the change have the count of its start.
        g.count += 1;
        g.changing = false;
    }
}

/// The sync schedule of the current session and account.
struct SyncStatus {
    policy: SyncPolicy,
    /// Unix time of the last sync that went through in this process.
    last_ok: Option<i64>,
    last_error: Option<LibraryErrorKind>,
}

impl SyncStatus {
    /// A fresh schedule, as if the last sync had been at the cache's sync
    /// cursor (the server's time of it), so a restarted app does not sync
    /// again at once. No cursor (a new account or server): due now.
    fn new(session: &Session) -> SyncStatus {
        let mut policy = SyncPolicy::default();
        if let Some(cursor) = session.cache().sync_cursor().ok().flatten() {
            let age = Duration::from_secs(u64::try_from(now_unix() - cursor).unwrap_or(0));
            if let Some(at) = Instant::now().checked_sub(age) {
                policy.started(at);
                policy.finished(at, true);
            }
        }
        SyncStatus {
            policy,
            last_ok: None,
            last_error: None,
        }
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Errors after which the offline cache answers instead (as in `Session`):
/// the server is unreachable or failing.
fn falls_back(e: &CoreError) -> bool {
    e.is_offline() || e.status().is_some_and(|s| s >= 500)
}

/// A server URL as typed, checked and normalised as a pairing link's is
/// ([`pairing::server_url`]): `http` or `https` with a host, no user name,
/// password, query or fragment; the host as it is contacted (lower case,
/// IDN in `xn--` form), no default port, no trailing `/`. The input is not
/// repeated in the error: it may hold a password.
fn server_url_from(input: &str) -> Result<String, LibraryError> {
    pairing::server_url(input).map_err(|e| LibraryError::invalid("server_url", format!("server URL refused: {e}")))
}

/// Whether the configured `current` URL names the server at `url` (a
/// result of [`server_url_from`]). A configured URL that does not normalise
/// (written by hand) is compared without its trailing `/`.
fn same_server(current: &str, url: &str) -> bool {
    match pairing::server_url(current) {
        Ok(current) => current == url,
        Err(_) => current.trim().trim_end_matches('/') == url,
    }
}

/// Aborts a task when the caller stops waiting for it (the Kotlin coroutine
/// was cancelled). Aborting a finished task does nothing.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Inner {
    fn session(&self) -> Arc<Session> {
        self.session.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The session, with the generation to give [`Inner::if_current`] for
    /// what is read from it.
    fn current(&self) -> (u64, Arc<Session>) {
        // The count first: a change replaces the session before it ends, so
        // the session is never older than the count.
        let generation = lock(&self.generation).count;
        (generation, self.session())
    }

    /// Run `write` (keeping what a read learnt) unless an account or server
    /// change began since `generation` was taken ([`Inner::current`]). A
    /// change does not begin while `write` runs.
    fn if_current(&self, generation: u64, write: impl FnOnce()) {
        let g = lock(&self.generation);
        if g.changing || g.count != generation {
            tracing::debug!("a read ended after an account or server change; its results are not kept");
            return;
        }
        write();
    }

    /// An account or server change begins; it ends when the result is
    /// dropped. Until then no read keeps what it learnt, and none that
    /// began before keeps it after. The caller holds `busy`.
    fn begin_change(&self) -> Change<'_> {
        let mut g = lock(&self.generation);
        g.count += 1;
        g.changing = true;
        Change(self)
    }

    /// Use `session` from now on (the server changed).
    fn replace_session(&self, session: Session) {
        let session = Arc::new(session);
        *lock(&self.sync) = SyncStatus::new(&session);
        lock(&self.seen).clear();
        lock(&self.packs_seen).clear();
        *lock(&self.vocabulary) = None;
        *self.session.write().unwrap_or_else(|e| e.into_inner()) = session;
    }

    /// Sign out of `old`'s server before going to another one, so that its
    /// token never reaches the new URL: `Session::logout` (send the unsent
    /// uses, revoke the token, delete the token file, drop the account's
    /// cached data), waiting for the old server at most its `timeout_secs`.
    /// When that fails or the old server does not answer in time, the token
    /// file is deleted here all the same. Only failing to delete it is an
    /// error, and then nothing else may change.
    async fn sign_out_for_new_server(&self, old: &Session) -> Result<(), LibraryError> {
        let wait = Duration::from_secs(old.config().timeout_secs.max(1));
        match tokio::time::timeout(wait, old.logout()).await {
            Ok(Ok(out)) => {
                if let Err(e) = &out.revoked {
                    tracing::warn!("leaving the server: the token was deleted but not revoked: {e}");
                }
            }
            Ok(Err(e)) => tracing::warn!("leaving the server: signing out failed ({e}); deleting the token here"),
            Err(_) => tracing::warn!("leaving the server: it did not answer in {wait:?}; deleting the token here"),
        }
        token::delete_token(&self.dirs.token_file)?;
        Ok(())
    }

    /// Before a new token for `session`'s server takes the place of the one
    /// stored, revoke the stored one there, so that it does not stay valid
    /// unused: the unsent uses go out with it first (another account's login
    /// would drop them), then it is revoked. Best effort, as in
    /// [`Inner::sign_out_for_new_server`]: the server is waited for at most
    /// its `timeout_secs`, and a failure is only logged; the login goes on
    /// either way. Nothing happens when no token is stored.
    async fn revoke_replaced_token(&self, session: &Session) {
        let stored = match session.client().token() {
            Some(t) => Some(t),
            None => token::read_token(&session.config().token_path()).ok().flatten(),
        };
        let Some(stored) = stored else {
            return;
        };
        // A client of its own: the session's may get the new token meanwhile.
        let client = session.client().anonymous().with_token(Some(stored));
        let wait = Duration::from_secs(session.config().timeout_secs.max(1));
        let revoke = async {
            if session.is_logged_in() {
                if let Err(e) = session.flush_usage().await {
                    tracing::info!("replacing the token: the unsent uses stay queued: {e}");
                }
            }
            match client.current_token_id().await? {
                Some(id) => client.revoke_token(&id).await.map(|()| true),
                None => Ok(false),
            }
        };
        match tokio::time::timeout(wait, revoke).await {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => tracing::info!("replacing the token: the server does not know the old one"),
            Ok(Err(e)) => tracing::warn!("replacing the token: the old one was not revoked: {e}"),
            Err(_) => tracing::warn!(
                "replacing the token: the server did not answer in {wait:?}; the old one was not revoked"
            ),
        }
    }

    /// Store `created`, a new token from `session`'s server, as the
    /// session's (as `Session`'s logins do, [`save_login`]), after revoking
    /// the one it replaces ([`Inner::revoke_replaced_token`]).
    async fn adopt(&self, session: &Session, created: TokenCreated) -> Result<User, LibraryError> {
        self.revoke_replaced_token(session).await;
        let _change = self.begin_change();
        save_login(session.config(), Some(session.cache()), &created)?;
        session.client().set_token(Some(created.token));
        self.account_changed();
        Ok(created.user)
    }

    /// The account changed on the same server (a login or logout).
    fn account_changed(&self) {
        *lock(&self.sync) = SyncStatus::new(&self.session());
        lock(&self.seen).clear();
        lock(&self.packs_seen).clear();
    }

    /// Keep `packs`, listed at `generation`, by id for `pack_cover`.
    fn remember_packs(&self, generation: u64, packs: &[SetSummary]) {
        self.if_current(generation, || {
            let mut seen = lock(&self.packs_seen);
            if seen.len() + packs.len() > SEEN_MAX {
                seen.clear();
            }
            for pack in packs {
                seen.insert(pack.id.clone(), pack.clone());
            }
        });
    }

    /// Keep `items`, read at `generation`, by id for `thumbnail`, `prepare`
    /// and `record_use`.
    fn remember(&self, generation: u64, items: &[ItemSummary]) {
        self.if_current(generation, || {
            let mut seen = lock(&self.seen);
            if seen.len() + items.len() > SEEN_MAX {
                seen.clear();
            }
            for item in items {
                seen.insert(item.id.clone(), item.clone());
            }
        });
    }

    fn list(&self, generation: u64, session: &Session, items: Vec<ItemSummary>, offline: bool) -> ItemList {
        self.remember(generation, &items);
        ItemList {
            items: self.items(session, &items),
            offline,
        }
    }

    /// `items` for the app, marked liked as far as the saved favorites say.
    fn items(&self, session: &Session, items: &[ItemSummary]) -> Vec<Item> {
        let liked = Self::liked_ids(session);
        items.iter().map(|i| Item::new(i, liked.contains(&i.id))).collect()
    }

    /// An item shown before, else the cached one; no request.
    fn local_item(&self, session: &Session, id: &str) -> Result<Option<ItemSummary>, LibraryError> {
        if let Some(item) = lock(&self.seen).get(id) {
            return Ok(Some(item.clone()));
        }
        Ok(session.cache().item(id)?)
    }

    /// An item shown before, else the cached one, else from the server
    /// (`session`, taken at `generation`).
    async fn item_local_first(
        &self,
        generation: u64,
        session: &Session,
        id: &str,
    ) -> Result<ItemSummary, LibraryError> {
        if let Some(item) = self.local_item(session, id)? {
            return Ok(item);
        }
        let item = session.item_summary(id).await?;
        self.remember(generation, std::slice::from_ref(&item));
        Ok(item)
    }

    /// An item shown before, else from the server (with its current
    /// renditions), else the cached one when the server is unreachable.
    async fn item_fresh_first(
        &self,
        generation: u64,
        session: &Session,
        id: &str,
    ) -> Result<ItemSummary, LibraryError> {
        if let Some(item) = lock(&self.seen).get(id) {
            return Ok(item.clone());
        }
        let item = session.item_summary(id).await?;
        self.remember(generation, std::slice::from_ref(&item));
        Ok(item)
    }

    fn vocabulary(&self, session: &Session) -> Arc<Words> {
        let generation = session.cache().generation().unwrap_or(0);
        let mut slot = lock(&self.vocabulary);
        if let Some((g, v)) = slot.as_ref() {
            if *g == generation {
                return v.clone();
            }
        }
        let vocabulary = match session.offline_index() {
            Ok(index) => {
                let items: Vec<&ItemSummary> = index.items().collect();
                Arc::new(Words::from_items(items.iter().copied()))
            }
            Err(e) => {
                tracing::debug!("no suggestions from the cache: {e}");
                Arc::new(Words::default())
            }
        };
        *slot = Some((generation, vocabulary.clone()));
        vocabulary
    }

    /// `items`, the pack `pack_id`'s, that `query` finds
    /// ([`StickpileLibrary::pack_search`]); reads the cache.
    fn search_pack(&self, session: &Session, pack_id: &str, query: &str, items: Vec<ItemSummary>) -> Vec<ItemSummary> {
        if query.is_empty() {
            return items;
        }
        let words = Vocabulary::from_items(&items).then(&self.vocabulary(session).vocab);
        let query = suggest::resolve(query, &words).0;
        let visibility = lock(&self.packs_seen).get(pack_id).map(|s| s.visibility);
        let me = match session.cache().me() {
            Ok(user) => user.map(|u| u.username),
            Err(e) => {
                tracing::debug!("who is mine: {e}");
                None
            }
        };
        let within = Within::new(&items, visibility, me.as_deref());
        let found = within.search(&query, me.as_deref(), &Self::liked_ids(session));
        found.into_iter().filter_map(|f| items.get(f.index).cloned()).collect()
    }

    fn favorites_key(session: &Session) -> Option<String> {
        let me = session.cache().me().ok().flatten()?;
        Some(format!("{FAVORITES_KEY}{}", me.id))
    }

    /// The ids of the favorites last read from the server, with the likes
    /// changed here since; empty when they were never read.
    fn liked_ids(session: &Session) -> HashSet<String> {
        let Some(key) = Self::favorites_key(session) else {
            return HashSet::new();
        };
        match session.cache().meta_get(&key) {
            Ok(Some(ids)) => ids.lines().filter(|l| !l.is_empty()).map(str::to_string).collect(),
            Ok(None) => HashSet::new(),
            Err(e) => {
                tracing::debug!("reading the favorites: {e}");
                HashSet::new()
            }
        }
    }

    /// Save `ids`, the favorites read online at `generation`, for offline,
    /// under the account the cache names.
    fn save_favorites(&self, generation: u64, session: &Session, ids: &[String]) {
        self.if_current(generation, || Self::write_favorites(session, ids));
    }

    /// [`Inner::save_favorites`] without the generation check.
    fn write_favorites(session: &Session, ids: &[String]) {
        let Some(key) = Self::favorites_key(session) else {
            return;
        };
        if let Err(e) = session.cache().meta_set(&key, &ids.join("\n")) {
            tracing::debug!("remembering the favorites: {e}");
        }
    }

    /// The favorites last read from the server, as far as they are cached;
    /// `None` when there are none.
    fn saved_favorites(&self, session: &Session) -> Result<Option<Vec<ItemSummary>>, LibraryError> {
        let Some(key) = Self::favorites_key(session) else {
            return Ok(None);
        };
        let Some(ids) = session.cache().meta_get(&key)? else {
            return Ok(None);
        };
        let mut items = Vec::new();
        for id in ids.lines().filter(|l| !l.is_empty()) {
            if let Some(item) = session.cache().item(id)? {
                items.push(item);
            }
        }
        Ok(Some(items))
    }

    /// Keep the saved favorites in step with a like that went through (on
    /// `session`, taken at `generation`).
    fn favorite_changed(&self, generation: u64, session: &Session, item_id: &str, liked: bool) {
        self.if_current(generation, || {
            let Some(key) = Self::favorites_key(session) else {
                return;
            };
            let Ok(Some(saved)) = session.cache().meta_get(&key) else {
                return;
            };
            let mut ids: Vec<String> = saved
                .lines()
                .filter(|l| !l.is_empty() && *l != item_id)
                .map(str::to_string)
                .collect();
            if liked {
                ids.insert(0, item_id.to_string());
            }
            Self::write_favorites(session, &ids);
        });
    }

    fn sync_state(&self) -> SyncState {
        let session = self.session();
        let status = lock(&self.sync);
        SyncState {
            last_sync: status.last_ok.or_else(|| session.cache().sync_cursor().ok().flatten()),
            last_error: status.last_error,
            pending_usage: to_u32(session.pending_usage()),
            due: status.policy.due(Instant::now()),
        }
    }

    /// Sync now; the caller holds `busy`.
    async fn run_sync(&self) -> Result<SyncState, LibraryError> {
        let session = self.session();
        lock(&self.sync).policy.started(Instant::now());
        let result = session.sync().await.map_err(LibraryError::from);
        {
            let mut status = lock(&self.sync);
            status.policy.finished(Instant::now(), result.is_ok());
            match &result {
                Ok(()) => {
                    status.last_ok = Some(now_unix());
                    status.last_error = None;
                }
                Err(e) => status.last_error = Some(e.kind()),
            }
        }
        result.map(|()| self.sync_state())
    }
}

async fn open_session(config: ClientConfig) -> Result<Session, LibraryError> {
    Ok(Session::open(config)
        .await?
        .with_client_name(crate::client_name())
        .with_fail_fast(FAIL_FAST))
}

impl StickpileLibrary {
    /// Run `f` on the runtime. When the caller stops waiting (the Kotlin
    /// coroutine was cancelled), the work is aborted: for calls that only
    /// read.
    async fn read<T, F, Fut>(&self, f: F) -> Result<T, LibraryError>
    where
        F: FnOnce(Arc<Inner>) -> Fut,
        Fut: Future<Output = Result<T, LibraryError>> + Send + 'static,
        T: Send + 'static,
    {
        let task = self.handle.spawn(f(self.inner.clone()));
        let _abort = AbortOnDrop(task.abort_handle());
        task.await?
    }

    /// Run `f` on the runtime to its end, even when the caller stops waiting:
    /// for calls that change something or write files.
    async fn write<T, F, Fut>(&self, f: F) -> Result<T, LibraryError>
    where
        F: FnOnce(Arc<Inner>) -> Fut,
        Fut: Future<Output = Result<T, LibraryError>> + Send + 'static,
        T: Send + 'static,
    {
        self.handle.spawn(f(self.inner.clone())).await?
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl StickpileLibrary {
    /// Open the library: read `dirs.config_file` if it exists (else the
    /// defaults), keep the cache and the token in `dirs.cache_dir` and
    /// `dirs.token_file`, and open the offline cache. Nothing goes to the
    /// network, so this works offline and signed out.
    ///
    /// Throws `InvalidInput` (field `config_file`, `cache_dir` or
    /// `token_file`) for a path that is not absolute or a config file that
    /// cannot be read as a stickpile config (deleting it gives the
    /// defaults), `Io` when the cache cannot be opened, `Other` for a token
    /// file readable by others.
    #[uniffi::constructor]
    pub fn new(dirs: LibraryDirs) -> Result<Arc<StickpileLibrary>, LibraryError> {
        let path = |field: &str, p: &str| {
            let p = PathBuf::from(p);
            if p.is_absolute() {
                Ok(p)
            } else {
                Err(LibraryError::invalid(field, format!("{p:?} is not an absolute path")))
            }
        };
        let dirs = Dirs {
            config_file: path("config_file", &dirs.config_file)?,
            cache_dir: path("cache_dir", &dirs.cache_dir)?,
            token_file: path("token_file", &dirs.token_file)?,
        };
        let config = dirs.load_config()?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKER_THREADS)
            .thread_name("stickpile-rt")
            .enable_all()
            .build()
            .map_err(|e| LibraryError::Io {
                detail: format!("starting the runtime: {e}"),
            })?;
        let handle = runtime.handle().clone();
        // On a thread of its own: `block_on` must not run inside another
        // runtime, and the caller may be in one.
        let opened = std::thread::scope(|s| {
            s.spawn(|| handle.block_on(open_session(config)))
                .join()
                .unwrap_or_else(|_| Err(LibraryError::other("opening the session panicked")))
        });
        let session = match opened {
            Ok(session) => session,
            Err(e) => {
                // Dropping a runtime blocks, which panics in async code.
                runtime.shutdown_background();
                return Err(e);
            }
        };
        let inner = Inner {
            dirs,
            sync: Mutex::new(SyncStatus::new(&session)),
            session: RwLock::new(Arc::new(session)),
            busy: tokio::sync::Mutex::new(()),
            seen: Mutex::new(HashMap::new()),
            packs_seen: Mutex::new(HashMap::new()),
            vocabulary: Mutex::new(None),
            generation: Mutex::new(Generation::default()),
        };
        tracing::info!(
            "stickpile {} (Android library), server {}",
            crate::version(),
            inner.session().config().server_url
        );
        Ok(Arc::new(StickpileLibrary {
            runtime: Some(runtime),
            handle,
            inner: Arc::new(inner),
        }))
    }

    // -- account ----------------------------------------------------------------

    /// The server's URL, as configured (`http://localhost:8080` until one
    /// is set).
    pub fn server_url(&self) -> String {
        self.inner.session().config().server_url.clone()
    }

    /// Use another server: `url` is checked and normalised as a pairing
    /// link's is (`http` or `https` with a host, no user name, password,
    /// query or fragment; the host lower case and an IDN host in `xn--`
    /// form, no default port, no trailing `/`). The same server as now
    /// changes nothing. Another URL signs out first, so the stored token is never
    /// sent to the new URL: the unsent uses go to the old server and the
    /// token is revoked there (best effort, waiting at most the configured
    /// timeout, 5 s by default), and the token file is deleted even when the
    /// old server cannot be reached. Then `url` is saved as `server_url` in
    /// the config file and the library reopened for it, signed out
    /// (`account` returns `None`; sign in again, or pair). The cache belongs
    /// to one server: the old one's items and history are dropped. Throws
    /// `InvalidInput` (field `server_url`) for a bad URL, nothing changed;
    /// `Io` when the token file cannot be deleted (nothing changed) or the
    /// config cannot be written (signed out, the URL unchanged).
    pub async fn set_server_url(&self, url: String) -> Result<(), LibraryError> {
        self.write(move |inner| async move {
            let url = server_url_from(&url)?;
            let _busy = inner.busy.lock().await;
            let old = inner.session();
            if same_server(&old.config().server_url, &url) {
                return Ok(());
            }
            let _change = inner.begin_change();
            inner.sign_out_for_new_server(&old).await?;
            inner.dirs.save_server_url(&url)?;
            let session = open_session(inner.dirs.load_config()?).await?;
            inner.replace_session(session);
            Ok(())
        })
        .await
    }

    /// Who is signed in, asking the server; `None` when nobody is (no token
    /// and no tailnet identity, or the server refused the token). When the
    /// server cannot be reached, the account the cache last saw; with none
    /// cached, `None` when no token is stored either (a fresh install, so
    /// the app shows onboarding) and `Unreachable` when one is.
    pub async fn account(&self) -> Result<Option<Account>, LibraryError> {
        self.read(|inner| async move {
            let session = inner.session();
            let server_url = session.config().server_url.clone();
            let auth = if session.is_logged_in() {
                AuthKind::Token
            } else {
                AuthKind::Tailnet
            };
            match session.client().me().await {
                Ok(user) => Ok(Some(Account::new(&user, &server_url, auth))),
                Err(e) if e.is_unauthorized() => Ok(None),
                Err(e) if falls_back(&e) => match session.cache().me()? {
                    Some(user) => Ok(Some(Account::new(&user, &server_url, auth))),
                    // No token and nothing cached: signed out, whatever the
                    // server would say about a tailnet identity.
                    None if !session.is_logged_in() => Ok(None),
                    None => Err(e.into()),
                },
                Err(e) => Err(e.into()),
            }
        })
        .await
    }

    /// Sign in without a password where the server knows this device by its
    /// tailnet identity (behind `tailscale serve`): a token is made and
    /// stored; a token stored before is revoked on the server first (best
    /// effort, as by `logout`). `None`: the server does not know this
    /// device that way.
    pub async fn login_tailnet(&self) -> Result<Option<Account>, LibraryError> {
        self.write(|inner| async move {
            let _busy = inner.busy.lock().await;
            let session = inner.session();
            session.client().forget_unreachable();
            let Some(created) = tailnet_login(session.client(), &device::token_name()).await? else {
                return Ok(None);
            };
            let user = inner.adopt(&session, created).await?;
            Ok(Some(Account::new(&user, &session.config().server_url, AuthKind::Token)))
        })
        .await
    }

    /// Sign in with a username and password (and the two-factor code, if the
    /// account has two-factor authentication on); the token is stored, and
    /// a token stored before is revoked on the server first (best effort,
    /// as by `logout`). Throws `Unauthorized` for a wrong password or code
    /// (the stored token is kept), and
    /// `InvalidInput` with field `totp` when the account needs a code and
    /// none was given (ask for it and call again).
    pub async fn login_password(
        &self,
        username: String,
        password: String,
        totp: Option<String>,
    ) -> Result<Account, LibraryError> {
        self.write(move |inner| async move {
            let _busy = inner.busy.lock().await;
            let session = inner.session();
            let req = TokenRequest {
                username: username.trim().to_string(),
                password,
                totp: totp.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
                token_name: device::token_name(),
            };
            let created = match session.client().login(&req).await {
                Ok(created) => created,
                Err(e) if e.is_totp_required() => return Err(LibraryError::invalid("totp", e.to_string())),
                Err(e) => return Err(LibraryError::input(e, "username")),
            };
            let user = inner.adopt(&session, created).await?;
            Ok(Account::new(&user, &session.config().server_url, AuthKind::Token))
        })
        .await
    }

    /// Sign in with a one-time pairing code made on a signed-in device, for
    /// the server at `server_url` (both from a scanned pairing link, or
    /// typed). The code is exchanged first, without the stored token; only
    /// when that works is `server_url` saved (as by `set_server_url`) and the
    /// new token stored in place of the old one, so a wrong code changes
    /// nothing. Pairing with another server than the current one first
    /// signs out of the current one as `set_server_url` does (its token is
    /// revoked there, best effort); pairing with the current one revokes
    /// the token stored for it, as `login_password` does. The token is
    /// named after the device,
    /// `Android (<model>)`. Throws `InvalidInput` (field `code` or
    /// `server_url`) for input that cannot be a code or URL, `Unauthorized`
    /// for a wrong, expired or used code, `InvalidInput` (field
    /// `token_name`) if the server refuses the token name.
    pub async fn login_pairing(&self, server_url: String, code: String) -> Result<Account, LibraryError> {
        self.write(move |inner| async move {
            let code = pairing::normalize(&code).ok_or_else(|| LibraryError::invalid("code", "not a pairing code"))?;
            let url = server_url_from(&server_url)?;
            let _busy = inner.busy.lock().await;
            let mut config = inner.dirs.load_config()?;
            config.server_url = url.clone();
            // A client of its own, without a token: the stored one belongs to
            // the current server and never goes to `url`.
            let client = ApiClient::from_config(&config)?;
            let req = PairRequest {
                code,
                token_name: device::token_name(),
            };
            let created = client
                .pair(&req)
                .await
                .map_err(|e| LibraryError::input(e, "token_name"))?;
            let _change = inner.begin_change();
            let old = inner.session();
            if same_server(&old.config().server_url, &url) {
                inner.revoke_replaced_token(&old).await;
            } else {
                inner.sign_out_for_new_server(&old).await?;
            }
            // Stored like `stickpile login` (`session::save_login`): the
            // token, the cache bound to the server (by `Session::open`) and
            // to the account.
            inner.dirs.save_server_url(&url)?;
            token::write_token(&config.token_path(), &created.token)?;
            let session = open_session(config).await?;
            session.cache().set_me(&created.user)?;
            inner.replace_session(session);
            Ok(Account::new(&created.user, &url, AuthKind::Token))
        })
        .await
    }

    /// Sign out: send the unsent uses, revoke the token on the server (best
    /// effort: also when that fails, the token is deleted here), and drop
    /// the account's data from the cache. Signed out already: nothing
    /// happens.
    pub async fn logout(&self) -> Result<(), LibraryError> {
        self.write(|inner| async move {
            let _busy = inner.busy.lock().await;
            let _change = inner.begin_change();
            let out = inner.session().logout().await?;
            if let Err(e) = &out.revoked {
                tracing::warn!("the token was deleted but not revoked on the server: {e}");
            }
            inner.account_changed();
            Ok(())
        })
        .await
    }

    // -- reading ----------------------------------------------------------------

    /// Items matching `query` (the search syntax of the other clients:
    /// words, `vibe:`, `tag:`, `set:`, `is:liked`, ...), best first, with
    /// hints for `target`. A `key:value` filter whose value is half typed or
    /// misspelled searches for its best suggestion (`suggestions`).
    /// When the server is unreachable the offline cache answers
    /// (`offline = true`, names and keywords only).
    pub async fn search(&self, query: String, target: Target, limit: u32) -> Result<ItemList, LibraryError> {
        self.read(move |inner| async move {
            let (generation, session) = inner.current();
            let query = if query.contains(':') {
                let (inner2, session2) = (inner.clone(), session.clone());
                let vocabulary = tokio::task::spawn_blocking(move || inner2.vocabulary(&session2)).await?;
                suggest::resolve(query.trim(), &vocabulary.vocab).0
            } else {
                query.trim().to_string()
            };
            let out = session.search(&query, Some(target.into()), limit as usize).await?;
            let items = out.hits.into_iter().map(|h| h.item).collect();
            Ok(inner.list(generation, &session, items, out.offline))
        })
        .await
    }

    /// The items used last on this device, newest first (no network).
    pub fn recent(&self, limit: u32) -> Vec<Item> {
        let (generation, session) = self.inner.current();
        let items = session.recent(limit as usize);
        self.inner.remember(generation, &items);
        self.inner.items(&session, &items)
    }

    /// The items the account liked, most used first. Offline: the
    /// favorites last read from the server, as far as the cache has them
    /// (`offline = true`); throws `Unreachable` if they were never read.
    /// Online it reads (and remembers for offline) up to 200 favorites, or
    /// `limit` if more, and returns the first `limit`: asking for a few
    /// does not shorten the list kept for offline.
    pub async fn favorites(&self, limit: u32) -> Result<ItemList, LibraryError> {
        self.read(move |inner| async move {
            let (generation, session) = inner.current();
            let limit = limit as usize;
            match session.favorites(limit.max(FAVORITES_SAVED)).await {
                Ok(mut items) => {
                    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
                    inner.save_favorites(generation, &session, &ids);
                    items.truncate(limit);
                    Ok(inner.list(generation, &session, items, false))
                }
                Err(e) if falls_back(&e) => match inner.saved_favorites(&session)? {
                    Some(mut items) => {
                        items.truncate(limit);
                        Ok(inner.list(generation, &session, items, true))
                    }
                    None => Err(e.into()),
                },
                Err(e) => Err(e.into()),
            }
        })
        .await
    }

    /// The account's most used items: the server's count over all its
    /// devices, then this device's history. Offline: this device's history
    /// (`offline = true`).
    pub async fn most_used(&self, limit: u32) -> Result<ItemList, LibraryError> {
        self.read(move |inner| async move {
            let (generation, session) = inner.current();
            let listing = session.most_used(limit as usize).await?;
            Ok(inner.list(generation, &session, listing.items, listing.offline))
        })
        .await
    }

    /// The packs to pick from: the account's own, most recently changed
    /// first, then other users' public ones; empty packs are left out.
    /// Offline: the packs of the cached items (`offline = true`); throws
    /// `Unreachable` if there are none.
    pub async fn packs(&self) -> Result<PackList, LibraryError> {
        self.read(|inner| async move {
            let (generation, session) = inner.current();
            let listing = session.packs().await?;
            inner.remember_packs(generation, &listing.items);
            let me = match session.cache().me()? {
                Some(user) => Some(user.username),
                None if !listing.offline => session.client().me().await.ok().map(|u| u.username),
                None => None,
            };
            Ok(PackList {
                packs: listing.items.iter().map(|s| Pack::new(s, me.as_deref())).collect(),
                offline: listing.offline,
            })
        })
        .await
    }

    /// The signed-in user's own packs, empty ones included: where an
    /// upload can go (`packs` leaves empty packs out). Offline: the own
    /// packs known from the cached items (`offline = true`), so not the
    /// empty ones; throws `Unreachable` if none are cached.
    pub async fn own_packs(&self) -> Result<PackList, LibraryError> {
        self.read(|inner| async move {
            let (generation, session) = inner.current();
            let listing = session.own_packs().await?;
            inner.remember_packs(generation, &listing.items);
            Ok(PackList {
                packs: listing
                    .items
                    .iter()
                    .map(|s| Pack::new(s, Some(s.owner_username.as_str())))
                    .collect(),
                offline: listing.offline,
            })
        })
        .await
    }

    /// One pack's items in pack order. Offline: its cached items by name
    /// (`offline = true`); throws `Unreachable` if none are cached.
    pub async fn pack_items(&self, pack_id: String) -> Result<ItemList, LibraryError> {
        self.read(move |inner| async move {
            let (generation, session) = inner.current();
            let listing = session.pack_items(&pack_id).await?;
            Ok(inner.list(generation, &session, listing.items, listing.offline))
        })
        .await
    }

    /// The items of one pack that `query` finds, with the whole search
    /// syntax of `search` (`vibe:tired`, `t:x`, `type:gif`, `-tag:nsfw`,
    /// `is:liked`, `q>70`, `sort:`, words): words best first, filters
    /// alone in pack order. The pack's items are read as `pack_items`
    /// reads them (offline: the cached ones, `offline = true`; the same
    /// errors) and searched here, not by the server
    /// (`stickpile_quick::within`). A half-typed filter value searches for
    /// its best suggestion, the pack's values first. A blank query gives
    /// the whole pack, as `pack_items`.
    pub async fn pack_search(&self, pack_id: String, query: String) -> Result<ItemList, LibraryError> {
        self.read(move |inner| async move {
            let (generation, session) = inner.current();
            let listing = session.pack_items(&pack_id).await?;
            let offline = listing.offline;
            let (inner2, session2) = (inner.clone(), session.clone());
            let items = tokio::task::spawn_blocking(move || {
                inner2.search_pack(&session2, &pack_id, query.trim(), listing.items)
            })
            .await?;
            Ok(inner.list(generation, &session, items, offline))
        })
        .await
    }

    /// The values to offer while a `key:value` filter is typed at the end of
    /// `query` (`vibe:sar`, `-tag:`, `set:ca`): the vibes, tags, packs and
    /// owners of the cached items, and the fixed words of `type:`, `is:` and
    /// `sort:`. `None` when the end of `query` is no such filter (no
    /// network).
    pub fn suggestions(&self, query: String) -> Option<Suggestions> {
        let session = self.inner.session();
        suggest::suggest(&query, &self.inner.vocabulary(&session).vocab).map(Suggestions::from)
    }

    /// What to offer under a search field while `query` is typed, each
    /// offer with the whole query it makes (no network; the words are those
    /// of the cached items):
    ///
    /// - a `key:value` filter being typed (`vibe:sar`, `-tag:`): its values,
    ///   as `suggestions` ranks them, the one the search uses `chosen`;
    /// - `e:` (or `emoji:`): the emojis of the cached items, most used first;
    /// - a bare word (`rea`): the tags, vibes and packs containing it as
    ///   filters (`tag:reaction`), those starting with it first, then the
    ///   filter keys the word starts (`t`: `tag:`, `type:`);
    /// - nothing, or a space at the end: the filter keys `tag:`, `vibe:`,
    ///   `e:`, `set:`, `by:`, `type:`, `is:`, `sort:`, labelled with their
    ///   one-letter forms (`tag: (t:)`).
    ///
    /// The one-letter keys work as the long ones (`v:sar` as `vibe:sar`).
    /// A leading `-` (leave out) stays on what is offered. Inside an
    /// unfinished phrase (an odd number of `"`) nothing is offered. At most
    /// 12 offers.
    pub fn hints(&self, query: String) -> Vec<SearchHint> {
        let session = self.inner.session();
        hints::hints(&query, &self.inner.vocabulary(&session))
    }

    /// The item's page in the server's web UI (`<server>/items/<id>`);
    /// `None` when the configured server URL is not an `http` or `https`
    /// URL (no network).
    pub fn item_url(&self, item_id: String) -> Option<String> {
        let mut url = url::Url::parse(&self.inner.session().config().server_url).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        url.path_segments_mut().ok()?.pop_if_empty().extend(["items", &item_id]);
        Some(url.into())
    }

    // -- files ------------------------------------------------------------------

    /// The path of a pack's cover (WebP, at most 256 px): the pack's own
    /// cover image, else its cover item's thumbnail (the first item unless
    /// one was chosen), downloaded into the cache unless it is there.
    /// `None` for a pack with neither, and for one that `packs` or
    /// `own_packs` has not listed since the account last changed. Offline:
    /// the cached file; throws `Unreachable` if it is not cached (offline
    /// lists name the first cached item by name as the cover).
    pub async fn pack_cover(&self, pack_id: String) -> Result<Option<String>, LibraryError> {
        self.write(move |inner| async move {
            let session = inner.session();
            let Some(pack) = lock(&inner.packs_seen).get(&pack_id).cloned() else {
                return Ok(None);
            };
            Ok(session.pack_cover(&pack).await?.map(|p| path_string(&p)))
        })
        .await
    }

    /// The path of the item's thumbnail (WebP, at most 256 px), downloaded
    /// into the cache unless it is there. Offline: the cached file; throws
    /// `Unreachable` if it is not cached.
    pub async fn thumbnail(&self, item_id: String) -> Result<String, LibraryError> {
        self.write(move |inner| async move {
            let (generation, session) = inner.current();
            let item = inner.item_local_first(generation, &session, &item_id).await?;
            Ok(path_string(&session.thumbnail(&item).await?))
        })
        .await
    }

    /// The rendition of a moving item to play in place of its still
    /// thumbnail: an animated GIF or WebP when there is one (the smaller),
    /// else an MP4 or WebM, fetched into the cache unless it is there. With
    /// `download` false nothing is downloaded: the cached file or `None`.
    /// `None` also for a still and for an item without a playable
    /// rendition. Offline: the cached file; throws `Unreachable` if it is
    /// not cached. Runs to its end once started, like `prepare`.
    pub async fn motion(&self, item_id: String, download: bool) -> Result<Option<Motion>, LibraryError> {
        self.write(move |inner| async move {
            let (generation, session) = inner.current();
            let item = inner.item_local_first(generation, &session, &item_id).await?;
            let Some((info, kind)) = motion::choose(&item) else {
                return Ok(None);
            };
            let path = if download {
                Some(session.fetch_variant(&item, info.variant).await?)
            } else {
                session.cached_variant(&item, info.variant)?
            };
            Ok(path.map(|p| Motion {
                path: path_string(&p),
                kind,
                width: info.width,
                height: info.height,
            }))
        })
        .await
    }

    /// The file to insert into a text field of `editor_package` that accepts
    /// `accepted_mime_types` (as the field lists them, wildcards included): the best
    /// rendition for the chat app behind the package (the sticker rendition
    /// first when `sticker` is set and the app has stickers), never of a type
    /// the field did not list. The file is fetched into the cache and named
    /// `<item name>.<extension>`; offline the cached file is used. Throws
    /// `NothingFits` when no rendition fits the field, `Unreachable` when the
    /// file is not cached and the server cannot be reached.
    pub async fn prepare(
        &self,
        item_id: String,
        editor_package: String,
        accepted_mime_types: Vec<String>,
        sticker: bool,
    ) -> Result<Prepared, LibraryError> {
        self.write(move |inner| async move {
            let (generation, session) = inner.current();
            // Asked for by the user: tried even if the server was away a moment ago.
            session.client().forget_unreachable();
            let item = inner.item_fresh_first(generation, &session, &item_id).await?;
            let target = stickpile_core::target_for_package(&editor_package);
            let choice =
                stickpile_core::variant_for(&item, target, &accepted_mime_types, sticker).ok_or_else(|| {
                    LibraryError::NothingFits {
                        detail: format!(
                            "{}: no rendition of [{}] for {}",
                            item.name,
                            accepted_mime_types.join(", "),
                            target.as_str()
                        ),
                    }
                })?;
            let path = session.fetch_variant(&item, choice.variant.variant).await?;
            Ok(Prepared {
                display_name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: path_string(&path),
                mime_type: choice.mime_type.to_string(),
                target: target.into(),
                variant: choice.variant.variant.as_str().to_string(),
                sticker: choice.sticker,
            })
        })
        .await
    }

    // -- writing ----------------------------------------------------------------

    /// Record that the item went into a field of `editor_package` as
    /// `variant` (`Prepared.variant`): it heads the recent items, and the
    /// use is sent to the server in the background, or with the next sync
    /// when it cannot be reached (no network here). Throws `NotFound` for
    /// an item neither shown nor cached, `InvalidInput` (field `variant`)
    /// for an unknown variant.
    pub fn record_use(&self, item_id: String, variant: String, editor_package: String) -> Result<(), LibraryError> {
        let variant = Variant::parse(&variant)
            .ok_or_else(|| LibraryError::invalid("variant", format!("unknown variant {variant:?}")))?;
        let session = self.inner.session();
        let item = self
            .inner
            .local_item(&session, &item_id)?
            .ok_or_else(|| LibraryError::not_found(format!("item {item_id} is not cached")))?;
        let target = stickpile_core::target_for_package(&editor_package);
        session.record_use(&item, target, variant)?;
        Ok(())
    }

    /// Like the item, or take the like back; returns the new state (true:
    /// liked). Needs the server.
    pub async fn toggle_like(&self, item_id: String) -> Result<bool, LibraryError> {
        self.write(move |inner| async move {
            let (generation, session) = inner.current();
            // Asked for by the user: tried even if the server was away a moment ago.
            session.client().forget_unreachable();
            let liked = session.toggle_like(&item_id).await?;
            inner.favorite_changed(generation, &session, &item_id, liked);
            Ok(liked)
        })
        .await
    }

    /// Upload one file into the pack `set_id` (one of the account's own) and
    /// return the new item, or the one already in the pack with the same
    /// content. To upload several, call this once per file. Throws
    /// `InvalidInput` (field `file`) when the server refuses the file (not
    /// a picture or video it takes, too large), `NotFound` for a pack that
    /// is not the account's, `Io` when the file cannot be read. The file is
    /// streamed from disk, never read into memory whole.
    pub async fn upload(&self, set_id: String, file: UploadFile) -> Result<Item, LibraryError> {
        self.write(move |inner| async move {
            let (generation, session) = inner.current();
            // Asked for by the user: tried even if the server was away a moment ago.
            session.client().forget_unreachable();
            let path = PathBuf::from(&file.path);
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "upload".into());
            let listed = |v: Vec<String>| {
                let v: Vec<String> = v
                    .into_iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                (!v.is_empty()).then_some(v)
            };
            let meta = UploadMeta {
                name: file.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
                keywords: listed(file.keywords),
                emojis: listed(file.emojis),
                ..UploadMeta::default()
            };
            let results = session
                .client()
                .upload_path(&set_id, &path, &filename, Some(&meta))
                .await
                .map_err(|e| LibraryError::input(e, "file"))?;
            let result = results
                .into_iter()
                .next()
                .ok_or_else(|| LibraryError::other("the server answered for no file"))?;
            if let Some(error) = result.error {
                return Err(LibraryError::invalid("file", error));
            }
            let item = result
                .item
                .ok_or_else(|| LibraryError::other("the server answered with no item"))?;
            let summary = session.item_summary(&item.id).await?;
            inner.remember(generation, std::slice::from_ref(&summary));
            Ok(Item::new(&summary, false))
        })
        .await
    }

    // -- pairing input ----------------------------------------------------------

    /// Read a scanned pairing link (`stickpile://pair?url=...&code=...`);
    /// `None` when `text` is not one, or its server URL is refused (as by
    /// `set_server_url`, e.g. one with a user name). The server URL comes
    /// back normalised, so what the app shows is the host that
    /// `login_pairing` contacts (no network).
    pub fn parse_pairing_link(&self, text: String) -> Option<PairingLink> {
        pairing::parse_link(&text).map(|l| PairingLink {
            server_url: l.server_url,
            code: pairing::display(&l.code),
        })
    }

    /// The display form of a typed pairing code (`7KQ2-M9XA-4FTR`); `None`
    /// while `input` is not a whole code yet. Case, spaces and dashes do not
    /// matter; `O`, `I` and `L` read as `0`, `1`, `1` (no network).
    pub fn normalize_pairing_code(&self, input: String) -> Option<String> {
        pairing::normalize(&input).map(|c| pairing::display(&c))
    }

    // -- sync -------------------------------------------------------------------

    /// Sync if one is due: the last one is older than 10 minutes (or there
    /// was none since the account or server changed) and the last failed
    /// attempt is older than 60 s. A sync pulls the item changes into the
    /// offline cache and sends the unsent uses. Returns the state after it;
    /// throws what made the sync fail (`Unreachable`, `NotSignedIn`, ...).
    /// When none is due, or one is running, returns at once.
    pub async fn sync_if_due(&self) -> Result<SyncState, LibraryError> {
        self.write(|inner| async move {
            let Ok(_busy) = inner.busy.try_lock() else {
                return Ok(inner.sync_state());
            };
            if !lock(&inner.sync).policy.due(Instant::now()) {
                return Ok(inner.sync_state());
            }
            inner.run_sync().await
        })
        .await
    }

    /// Sync now, whether or not one is due (after one that is running).
    /// Returns the state after it; throws what made it fail.
    pub async fn sync_now(&self) -> Result<SyncState, LibraryError> {
        self.write(|inner| async move {
            let _busy = inner.busy.lock().await;
            // Asked for: the server is tried even if it was away a moment ago.
            inner.session().client().forget_unreachable();
            inner.run_sync().await
        })
        .await
    }

    /// Where the sync stands (no network).
    pub fn sync_state(&self) -> SyncState {
        self.inner.sync_state()
    }

    /// Uses recorded on this device and not yet sent (no network).
    pub fn pending_usage(&self) -> u32 {
        to_u32(self.inner.session().pending_usage())
    }

    /// Bytes of files in the offline cache: thumbnails and prepared
    /// renditions (no network). The item list is not counted.
    pub fn cache_bytes(&self) -> Result<u64, LibraryError> {
        Ok(self.inner.session().cache().files_total_bytes()?)
    }

    /// Deletes every cached file (thumbnails and prepared renditions) and
    /// returns the bytes freed. The item list stays, so search still works
    /// offline; files are fetched again when needed.
    pub fn clear_cache(&self) -> Result<u64, LibraryError> {
        Ok(self.inner.session().cache().clear_files()?.freed_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls_are_checked() {
        for (input, want) in [
            (" https://s.example/ ", "https://s.example"),
            ("http://10.0.0.2:8080", "http://10.0.0.2:8080"),
            ("https://example.org/stickers/", "https://example.org/stickers"),
            ("HTTPS://S.Example:443/", "https://s.example"),
            ("http://s.example:80/a/", "http://s.example/a"),
            ("https://b\u{fc}cher.example", "https://xn--bcher-kva.example"),
        ] {
            assert_eq!(server_url_from(input).unwrap(), want, "{input:?}");
        }
        for bad in [
            "",
            "example.org",
            "ftp://example.org",
            "http://",
            "https://x?a=1",
            "https://x#f",
            "https://my.server@evil.example",
            "https://alice:hunter2@s.example",
        ] {
            let e = server_url_from(bad).unwrap_err();
            assert!(
                matches!(&e, LibraryError::InvalidInput { field, .. } if field == "server_url"),
                "{bad:?}: {e:?}"
            );
            assert!(!e.to_string().contains("hunter2"), "{e}");
        }
    }

    /// What `parse_pairing_link` shows is what `login_pairing` and
    /// `set_server_url` contact, for every URL either accepts.
    #[test]
    fn pairing_links_and_typed_urls_agree() {
        for input in [
            "https://s.example",
            "https://s.example/",
            "HTTPS://S.Example:443/stickers/",
            "http://10.0.0.2:8080",
            "http://[::1]:8080/",
            "https://b\u{fc}cher.example",
            "http://h/stick pile",
            "https://my.server@evil.example",
            "https://:pw@s.example",
            "https://s.example?a=1",
            "https://s.example#f",
            "ftp://s.example",
            "https://",
            "s.example",
        ] {
            let linked = pairing::parse_link(&pairing::link(input, "7KQ2-M9XA-4FTR")).map(|l| l.server_url);
            assert_eq!(linked, server_url_from(input).ok(), "{input:?}");
        }
        assert!(same_server("https://s.example/", "https://s.example"));
        assert!(same_server("HTTPS://S.EXAMPLE:443", "https://s.example"));
        assert!(!same_server("https://s.example", "https://t.example"));
    }

    #[test]
    fn library_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<StickpileLibrary>();
    }
}
