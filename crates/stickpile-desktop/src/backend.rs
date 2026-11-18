//! What the popup needs from the outside world, as an object-safe trait so
//! the GTK layer can run against `stickpile_core::Session` or the
//! [`crate::demo`] directory backend. All futures are `Send + 'static` and run
//! on the tokio runtime; the GTK side awaits their `JoinHandle`s on the GLib
//! main loop.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

use stickpile_api::{ItemSummary, Role, SetSummary, TargetApp, TokenRequest, User, UserStats, Variant};
use stickpile_core::cache::EvictReport;
use stickpile_core::offline::{
    OfflineControl, OfflineOptions, OfflineOutcome, OfflineState, OfflineStatus, OfflineUsage, RunEnd,
};
use stickpile_core::session::{self, Listing};
use stickpile_core::{CoreError, PreparedCopy, Session};
use stickpile_i18n::global::{t, tf};
use stickpile_quick::model::{target_name, Results};
use stickpile_quick::offline::{Estimate, KeepScope, KeptFor, Skips};
use stickpile_quick::suggest::Vocabulary;

use crate::offline::{estimate_from_core, reason_from_core, scope_from_core, scope_to_core, skips_from_core};

pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// A password login.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    /// Code for an account with TOTP (see [`LoginError::TotpRequired`]).
    pub totp: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("totp", &self.totp.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    /// The account has TOTP: ask for a code and log in again with it.
    TotpRequired,
    /// User-facing text.
    Failed(String),
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginError::TotpRequired => f.write_str(t("A TOTP code is required")),
            LoginError::Failed(text) => f.write_str(text),
        }
    }
}

/// How requests are authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    /// A stored API token.
    Token,
    /// No token: the server knows us by tailnet identity (`tailscale serve`).
    Tailnet,
    /// Signed out: the server rejects our requests.
    None,
}

/// What the "Me" page shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub server_url: String,
    /// How requests are authenticated right now.
    pub auth: AuthKind,
    pub username: Option<String>,
    pub display_name: Option<String>,
    /// Copies by the viewer on all devices.
    pub copies: Option<u64>,
    /// Items the viewer liked.
    pub likes: Option<u64>,
    /// The viewer's own sets.
    pub packs: Option<u64>,
    /// The server identifies us without a token, so `login(None)` works.
    pub tailnet_login_available: bool,
    /// Items in the viewer's own sets.
    pub items: Option<u64>,
    pub copies_last_7d: Option<u64>,
    /// Likes other users gave the viewer's items.
    pub likes_received: Option<u64>,
    /// Storage taken by the viewer's library.
    pub storage_bytes: Option<u64>,
    pub admin: bool,
    pub totp_enabled: bool,
    /// Tailnet login the account is bound to.
    pub tailscale_login: Option<String>,
    /// Copies on this device not yet reported to the server.
    pub pending_usage: usize,
    /// The account's language (`en`, `uk`); `None`: automatic.
    pub language: Option<String>,
}

impl Profile {
    pub fn signed_out(server_url: impl Into<String>) -> Profile {
        Profile {
            server_url: server_url.into(),
            auth: AuthKind::None,
            username: None,
            display_name: None,
            copies: None,
            likes: None,
            packs: None,
            tailnet_login_available: false,
            items: None,
            copies_last_7d: None,
            likes_received: None,
            storage_bytes: None,
            admin: false,
            totp_enabled: false,
            tailscale_login: None,
            pending_usage: 0,
            language: None,
        }
    }

    pub fn for_user(server_url: impl Into<String>, auth: AuthKind, user: &User) -> Profile {
        Profile {
            auth,
            username: Some(user.username.clone()),
            display_name: Some(user.display_name.clone()).filter(|n| !n.is_empty()),
            admin: user.role == Role::Admin,
            totp_enabled: user.totp_enabled,
            tailscale_login: user.tailscale_login.clone(),
            language: user.language.clone(),
            ..Profile::signed_out(server_url)
        }
    }

    pub fn with_stats(self, stats: &UserStats) -> Profile {
        Profile {
            copies: Some(stats.copies_total),
            likes: Some(stats.likes_given),
            packs: Some(stats.sets),
            items: Some(stats.items),
            copies_last_7d: Some(stats.copies_last_7d),
            likes_received: Some(stats.likes_received),
            storage_bytes: Some(stats.storage_bytes),
            ..self
        }
    }
}

pub trait Backend: Send + Sync + 'static {
    /// The server rejected our credentials on the last request that reached
    /// it (no token and no tailnet identity, or a revoked token). Having no
    /// token is not enough: tailnet users work without one.
    fn login_required(&self) -> bool;
    fn search(self: Arc<Self>, query: String, target: TargetApp, limit: usize) -> BoxFut<Result<Results, String>>;
    fn recent(self: Arc<Self>, limit: usize) -> BoxFut<Vec<ItemSummary>>;
    fn thumbnail(self: Arc<Self>, item: ItemSummary) -> BoxFut<Result<PathBuf, String>>;
    fn prepare_copy(
        self: Arc<Self>,
        item: ItemSummary,
        target: TargetApp,
        sticker: bool,
    ) -> BoxFut<Result<PreparedCopy, String>>;
    fn record_copy(self: Arc<Self>, item: ItemSummary, prepared: PreparedCopy) -> BoxFut<Result<(), String>>;
    /// Pull the library's changes into the offline cache; `true` when the
    /// cache changed (the popup then loads its suggestions again, see
    /// [`stickpile_quick::model::Event::LibraryChanged`]).
    fn sync(self: Arc<Self>) -> BoxFut<Result<bool, String>>;
    /// The viewer's most copied items, most first.
    fn most_used(self: Arc<Self>, limit: usize) -> BoxFut<Result<Vec<ItemSummary>, String>>;
    /// Items the viewer liked.
    fn favorites(self: Arc<Self>, limit: usize) -> BoxFut<Result<Vec<ItemSummary>, String>>;
    /// The viewer's own sets, then other users' public sets.
    fn packs(self: Arc<Self>) -> BoxFut<Result<Vec<SetSummary>, String>>;
    /// A set's items in set order.
    fn pack_items(self: Arc<Self>, set_id: String) -> BoxFut<Result<Vec<ItemSummary>, String>>;
    /// Local thumbnail file of the set's cover, None when the set has no cover.
    fn pack_cover(self: Arc<Self>, set: SetSummary) -> BoxFut<Result<Option<PathBuf>, String>>;
    /// Flips the viewer's like; returns the new state (true = liked).
    fn toggle_favorite(self: Arc<Self>, item_id: String) -> BoxFut<Result<bool, String>>;
    /// Who we are on the server. Being signed out is not an error; an
    /// unreachable server is.
    fn profile(self: Arc<Self>) -> BoxFut<Result<Profile, String>>;
    /// None: log in with the tailnet identity (no password).
    fn login(self: Arc<Self>, credentials: Option<Credentials>) -> BoxFut<Result<Profile, LoginError>>;
    /// Revokes the current token on the server (best effort) and deletes the token file.
    fn logout(self: Arc<Self>) -> BoxFut<Result<(), String>>;
    /// Where the browser shows the item (`gx`).
    fn web_url(&self, item_id: &str) -> Option<String>;
    /// Set the account's language (`None`: automatic), so the web UI and
    /// other clients follow the one chosen here.
    fn set_account_language(self: Arc<Self>, code: Option<String>) -> BoxFut<Result<(), String>>;
    /// The values the search suggests: the vibes, tags, packs and owners of
    /// the items the viewer can see (the offline cache).
    fn vocabulary(self: Arc<Self>) -> BoxFut<Vocabulary>;

    // -- keeping offline (docs/offline.md); the demo has none ------------------------

    /// Whether the server answered the last request that tried it; `None`
    /// before one did.
    fn reachable(&self) -> Option<bool> {
        None
    }
    /// What the user keeps on this device.
    fn offline_keeps(self: Arc<Self>) -> BoxFut<Result<Vec<KeepScope>, String>> {
        no_offline()
    }
    /// Per item id, whether the file a copy to `target` needs is on the device.
    fn offline_available(
        self: Arc<Self>,
        _items: Vec<ItemSummary>,
        _target: TargetApp,
        _sticker: bool,
    ) -> BoxFut<Result<Vec<(String, bool)>, String>> {
        no_offline()
    }
    /// Why the item is kept (empty: it is not).
    fn offline_reasons(self: Arc<Self>, _item: ItemSummary) -> BoxFut<Result<Vec<KeptFor>, String>> {
        no_offline()
    }
    /// What keeping `scope` would take.
    fn offline_estimate(self: Arc<Self>, _scope: KeepScope) -> BoxFut<Result<Estimate, String>> {
        no_offline()
    }
    /// Keep `scope` (`keep`) or stop keeping it.
    fn offline_set(self: Arc<Self>, _scope: KeepScope, _keep: bool) -> BoxFut<Result<(), String>> {
        no_offline()
    }
    /// Download what is kept and missing, until done, cancelled, the
    /// server unreachable, the limit reached or the disk full.
    fn offline_run(self: Arc<Self>, _control: OfflineControl) -> BoxFut<Result<OfflineOutcome, String>> {
        no_offline()
    }
    /// Kept files and the others on the device.
    fn offline_usage(self: Arc<Self>) -> BoxFut<Result<OfflineUsage, String>> {
        no_offline()
    }
    /// What the plan says now, as a status of no run (`Idle`): how much of
    /// what is wanted is on the device, skipped, failed. For the Settings
    /// block between runs (after a keep, a release, a removal), with what
    /// it leaves out by reason.
    fn offline_plan_status(self: Arc<Self>) -> BoxFut<Result<(OfflineStatus, Skips), String>> {
        no_offline()
    }
    /// "Clear cache": the files that are not kept.
    fn clear_cache(self: Arc<Self>) -> BoxFut<Result<EvictReport, String>> {
        no_offline()
    }
    /// "Remove kept files": every kept file, and what to keep.
    fn offline_remove_all(self: Arc<Self>) -> BoxFut<Result<EvictReport, String>> {
        no_offline()
    }
    /// The `[offline]` options changed on the Settings page.
    fn set_offline_options(&self, _opts: OfflineOptions) {}
}

fn no_offline<T: Send + 'static>() -> BoxFut<Result<T, String>> {
    Box::pin(async { Err(t("Keeping offline needs a server (not in demo mode)").to_string()) })
}

/// [`Backend::reachable`], kept in an atomic.
const REACH_UNKNOWN: u8 = 0;
const REACH_YES: u8 = 1;
const REACH_NO: u8 = 2;

/// The item's page in the web UI of the server at `server_url`.
pub fn item_page_url(server_url: &str, item_id: &str) -> Option<String> {
    let mut url = url::Url::parse(server_url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.path_segments_mut().ok()?.pop_if_empty().extend(["items", item_id]);
    Some(url.into())
}

/// User-facing text for a core error.
pub fn describe(e: &CoreError) -> String {
    describe_with(e, ITEM_NOT_CACHED())
}

/// [`describe`] with the text for an unreachable server.
fn describe_with(e: &CoreError, offline: &str) -> String {
    match e {
        CoreError::NotLoggedIn => NOT_LOGGED_IN().into(),
        // Not a network error to the user: the sticker is not kept here.
        CoreError::NotOnDevice { item, .. } => tf(
            "{name} is not on this device and the server is unreachable; keep it offline (K) to copy it without the server",
            &[("name", item)],
        ),
        CoreError::Offline(_) => offline.into(),
        e if e.is_unauthorized() => REJECTED().into(),
        other => other.to_string(),
    }
}

/// A copy of a kept sticker, with the server unreachable, needs a rendition
/// that is not on the device: not downloaded yet, or not kept for this app
/// or as a sticker. Unlike an unkept sticker's message, it does not send
/// the user to `K`, which would release the sticker.
pub fn kept_not_here(name: &str, variant: Variant, target: TargetApp, sticker: bool) -> String {
    let args: [(&str, &dyn std::fmt::Display); 3] = [
        ("name", &name),
        ("rendition", &variant.as_str()),
        ("app", &target_name(target)),
    ];
    if sticker {
        tf(
            "{name} is kept offline, but its {rendition} file for {app} as a sticker is not on this device: it is not downloaded yet, or not kept for stickers to {app} (offline.targets, offline.sticker_renditions); the server is unreachable",
            &args,
        )
    } else {
        tf(
            "{name} is kept offline, but its {rendition} file for {app} is not on this device: it is not downloaded yet, or not kept for {app} (offline.targets); the server is unreachable",
            &args,
        )
    }
}

// The messages, in the current language when they are made.
#[allow(non_snake_case)]
fn NOT_LOGGED_IN() -> &'static str {
    t("Not logged in; log in on the Me tab (6)")
}
#[allow(non_snake_case)]
fn REJECTED() -> &'static str {
    t("The server did not accept this client; log in on the Me tab (6)")
}
#[allow(non_snake_case)]
fn ITEM_NOT_CACHED() -> &'static str {
    t("Server unreachable and the item is not cached")
}
#[allow(non_snake_case)]
fn UNREACHABLE() -> &'static str {
    t("Server unreachable")
}
#[allow(non_snake_case)]
fn NO_TAILNET() -> &'static str {
    t("The server does not know this device by its tailnet identity; sign in with a username and password")
}

/// User-facing text for a failed login: the server's own message (wrong
/// password, wrong code) rather than the token hints of [`describe`].
fn login_failure(e: &CoreError) -> String {
    let text = match e {
        CoreError::Api { error, .. } => error.message.clone(),
        CoreError::Offline(_) => UNREACHABLE().into(),
        other => other.to_string(),
    };
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => text,
    }
}

/// Usage events from this client are reported as `desktop/<version>`.
pub fn client_name() -> String {
    format!("desktop/{}", env!("CARGO_PKG_VERSION"))
}

/// The real backend.
pub struct CoreBackend {
    session: Session,
    login_required: AtomicBool,
    /// [`Backend::reachable`]: `REACH_*`.
    reach: AtomicU8,
}

impl CoreBackend {
    /// `login_required`: carry over what the previous session learned (the
    /// shell re-opens the session to pick up a token from `stickpile login`).
    pub fn new(session: Session, login_required: bool) -> CoreBackend {
        CoreBackend {
            session: session.with_client_name(client_name()),
            login_required: AtomicBool::new(login_required),
            reach: AtomicU8::new(REACH_UNKNOWN),
        }
    }

    pub fn has_token(&self) -> bool {
        self.session.is_logged_in()
    }

    fn reached(&self, yes: bool) {
        self.reach
            .store(if yes { REACH_YES } else { REACH_NO }, Ordering::Relaxed);
    }

    /// Track authorisation and reachability from a result: a rejection
    /// sets the login flag, an answer from the server (`reached_server`)
    /// clears it, and an unreachable server is noted. Errors become
    /// user-facing text, `offline` for an unreachable server.
    fn track<T>(
        &self,
        r: Result<T, CoreError>,
        reached_server: impl FnOnce(&T) -> bool,
        offline: &str,
    ) -> Result<T, String> {
        match r {
            Ok(v) => {
                if reached_server(&v) {
                    self.login_required.store(false, Ordering::Relaxed);
                    self.reached(true);
                }
                Ok(v)
            }
            Err(e) => {
                if e.is_unauthorized() {
                    self.login_required.store(true, Ordering::Relaxed);
                }
                if e.is_offline() {
                    self.reached(false);
                } else if e.status().is_some() {
                    self.reached(true);
                }
                Err(describe_with(&e, offline))
            }
        }
    }

    /// [`CoreBackend::track`] for a list that may come from the offline cache.
    fn listing<T>(&self, r: Result<Listing<T>, CoreError>, offline: &str) -> Result<Vec<T>, String> {
        if r.as_ref().is_ok_and(|l| l.offline) {
            self.reached(false);
        }
        self.track(r, |l| !l.offline, offline).map(|l| l.items)
    }

    /// Run blocking cache work off the async workers.
    fn blocking<T: Send + 'static>(
        self: Arc<Self>,
        f: impl FnOnce(&Session) -> Result<T, CoreError> + Send + 'static,
    ) -> BoxFut<Result<T, String>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || f(&self.session).map_err(|e| describe(&e)))
                .await
                .map_err(|e| e.to_string())?
        })
    }
}

impl Backend for CoreBackend {
    fn login_required(&self) -> bool {
        self.login_required.load(Ordering::Relaxed)
    }

    fn search(self: Arc<Self>, query: String, target: TargetApp, limit: usize) -> BoxFut<Result<Results, String>> {
        Box::pin(async move {
            let r = self.session.search(&query, Some(target), limit).await;
            if r.as_ref().is_ok_and(|o| o.offline) {
                self.reached(false);
            }
            let out = self.track(r, |o| !o.offline, ITEM_NOT_CACHED())?;
            Ok(Results {
                hits: out.hits,
                offline: out.offline,
            })
        })
    }

    fn vocabulary(self: Arc<Self>) -> BoxFut<Vocabulary> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                // Whose emojis `e:` offers, who `is:mine` is inside an
                // opened pack.
                let me = self.session.cache().me().ok().flatten().map(|u| u.username);
                match self.session.offline_index() {
                    Ok(index) => Vocabulary::for_viewer(index.items(), me.as_deref()),
                    Err(e) => {
                        tracing::debug!("no suggestions from the cache: {e}");
                        Vocabulary {
                            me,
                            ..Vocabulary::default()
                        }
                    }
                }
            })
            .await
            .unwrap_or_default()
        })
    }

    fn recent(self: Arc<Self>, limit: usize) -> BoxFut<Vec<ItemSummary>> {
        // Session::recent reads the local SQLite log: keep it off the async workers.
        Box::pin(async move {
            tokio::task::spawn_blocking(move || self.session.recent(limit))
                .await
                .unwrap_or_default()
        })
    }

    fn thumbnail(self: Arc<Self>, item: ItemSummary) -> BoxFut<Result<PathBuf, String>> {
        Box::pin(async move {
            let r = self.session.thumbnail(&item).await;
            self.track(r, |_| false, ITEM_NOT_CACHED())
        })
    }

    fn prepare_copy(
        self: Arc<Self>,
        item: ItemSummary,
        target: TargetApp,
        sticker: bool,
    ) -> BoxFut<Result<PreparedCopy, String>> {
        Box::pin(async move {
            let r = self.session.prepare_copy(&item, target, sticker).await;
            if let Err(CoreError::NotOnDevice {
                item: name, variant, ..
            }) = &r
            {
                // Kept already: `K` would release it, so say what is missing.
                if self.session.offline_reasons(&item).is_ok_and(|r| !r.is_empty()) {
                    self.reached(false);
                    return Err(kept_not_here(name, *variant, target, sticker));
                }
            }
            // The core copies another kept file only when the server did not
            // answer, so the tiles dim as after a failed copy.
            if r.as_ref()
                .is_ok_and(|p| p.warning.as_deref() == Some(stickpile_core::session::COPIED_ANOTHER_FILE))
            {
                self.reached(false);
            }
            self.track(r, |_| false, ITEM_NOT_CACHED())
        })
    }

    fn record_copy(self: Arc<Self>, item: ItemSummary, prepared: PreparedCopy) -> BoxFut<Result<(), String>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || self.session.record_copy(&item, &prepared).map_err(|e| describe(&e)))
                .await
                .map_err(|e| e.to_string())?
        })
    }

    fn sync(self: Arc<Self>) -> BoxFut<Result<bool, String>> {
        Box::pin(async move {
            let r = self.session.sync().await;
            self.track(r, |_| true, UNREACHABLE())
        })
    }

    fn most_used(self: Arc<Self>, limit: usize) -> BoxFut<Result<Vec<ItemSummary>, String>> {
        Box::pin(async move {
            let r = self.session.most_used(limit).await;
            self.listing(r, UNREACHABLE())
        })
    }

    fn favorites(self: Arc<Self>, limit: usize) -> BoxFut<Result<Vec<ItemSummary>, String>> {
        Box::pin(async move {
            let r = self.session.favorites(limit).await;
            self.track(r, |_| true, t("Server unreachable; favorites need the server"))
        })
    }

    fn packs(self: Arc<Self>) -> BoxFut<Result<Vec<SetSummary>, String>> {
        Box::pin(async move {
            let r = self.session.packs().await;
            self.listing(r, t("Server unreachable and no packs are cached"))
        })
    }

    fn pack_items(self: Arc<Self>, set_id: String) -> BoxFut<Result<Vec<ItemSummary>, String>> {
        Box::pin(async move {
            let r = self.session.pack_items(&set_id).await;
            self.listing(r, t("Server unreachable and the pack is not cached"))
        })
    }

    fn pack_cover(self: Arc<Self>, set: SetSummary) -> BoxFut<Result<Option<PathBuf>, String>> {
        Box::pin(async move {
            let r = self.session.pack_cover(&set).await;
            self.track(r, |_| false, t("Server unreachable and the cover is not cached"))
        })
    }

    fn toggle_favorite(self: Arc<Self>, item_id: String) -> BoxFut<Result<bool, String>> {
        Box::pin(async move {
            let r = self.session.toggle_like(&item_id).await;
            self.track(r, |_| true, UNREACHABLE())
        })
    }

    fn profile(self: Arc<Self>) -> BoxFut<Result<Profile, String>> {
        Box::pin(async move {
            let client = self.session.client();
            let server_url = self.session.config().server_url.clone();
            let has_token = client.has_token();
            let user = match client.me().await {
                Ok(user) => Some(user),
                Err(e) if e.is_unauthorized() => None,
                Err(e) => return Err(describe_with(&e, UNREACHABLE())),
            };
            self.login_required.store(user.is_none(), Ordering::Relaxed);
            let auth = match (&user, has_token) {
                (None, _) => AuthKind::None,
                (Some(_), true) => AuthKind::Token,
                (Some(_), false) => AuthKind::Tailnet,
            };
            let signed_in = user.is_some();
            let stats = async {
                match signed_in {
                    true => client.stats_me().await.ok(),
                    false => None,
                }
            };
            let tailnet = async {
                match auth {
                    AuthKind::Tailnet => true,
                    _ if !has_token => false,
                    // The token answers for us; ask what the server makes of us without it.
                    _ => client.anonymous().me().await.is_ok(),
                }
            };
            let (stats, tailnet_login_available) = tokio::join!(stats, tailnet);
            let mut profile = match &user {
                Some(user) => Profile::for_user(server_url, auth, user),
                None => Profile::signed_out(server_url),
            };
            if let Some(stats) = &stats {
                profile = profile.with_stats(stats);
            }
            profile.tailnet_login_available = tailnet_login_available;
            profile.pending_usage = self.session.pending_usage();
            Ok(profile)
        })
    }

    fn login(self: Arc<Self>, credentials: Option<Credentials>) -> BoxFut<Result<Profile, LoginError>> {
        Box::pin(async move {
            let token_name = session::token_name("stickpile-desktop");
            let tailnet = credentials.is_none();
            let r = match credentials {
                None => self.session.login_tailnet(&token_name).await,
                Some(c) => {
                    let req = TokenRequest {
                        username: c.username.trim().to_string(),
                        password: c.password,
                        totp: c.totp.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
                        token_name,
                    };
                    self.session.login_password(&req).await.map(Some)
                }
            };
            let user = match r {
                Ok(Some(user)) => user,
                Ok(None) => return Err(LoginError::Failed(NO_TAILNET().into())),
                Err(e) if e.is_totp_required() => return Err(LoginError::TotpRequired),
                Err(e) => return Err(LoginError::Failed(login_failure(&e))),
            };
            self.login_required.store(false, Ordering::Relaxed);
            let server_url = self.session.config().server_url.clone();
            match self.clone().profile().await {
                Ok(profile) => Ok(profile),
                Err(e) => {
                    // Logged in all the same; the page fills in on the next look.
                    tracing::warn!("profile after login: {e}");
                    let mut profile = Profile::for_user(server_url, AuthKind::Token, &user);
                    profile.tailnet_login_available = tailnet;
                    Ok(profile)
                }
            }
        })
    }

    fn logout(self: Arc<Self>) -> BoxFut<Result<(), String>> {
        Box::pin(async move {
            let out = self.session.logout().await.map_err(|e| describe(&e))?;
            if !out.had_token {
                return Ok(());
            }
            match &out.revoked {
                Ok(true) => {}
                Ok(false) => tracing::warn!("the token was not found on the server; deleted it locally"),
                Err(e) if e.is_unauthorized() => {}
                Err(e) => tracing::warn!("could not revoke the token on the server ({e}); revoke it in the web UI"),
            }
            // Whether the server still knows us (tailnet) decides the login prompt.
            if !matches!(&out.revoked, Err(e) if e.is_offline()) {
                let r = self.session.client().me().await;
                let _ = self.track(r, |_| true, UNREACHABLE());
            }
            Ok(())
        })
    }

    fn web_url(&self, item_id: &str) -> Option<String> {
        item_page_url(&self.session.config().server_url, item_id)
    }

    fn set_account_language(self: Arc<Self>, code: Option<String>) -> BoxFut<Result<(), String>> {
        Box::pin(async move {
            let req = stickpile_api::UpdateMe {
                language: Some(code),
                ..Default::default()
            };
            self.session
                .client()
                .update_me(&req)
                .await
                .map(|_| ())
                .map_err(|e| describe_with(&e, UNREACHABLE()))
        })
    }

    fn reachable(&self) -> Option<bool> {
        match self.reach.load(Ordering::Relaxed) {
            REACH_YES => Some(true),
            REACH_NO => Some(false),
            _ => None,
        }
    }

    fn offline_keeps(self: Arc<Self>) -> BoxFut<Result<Vec<KeepScope>, String>> {
        self.blocking(|s| Ok(s.offline_keeps()?.iter().map(scope_from_core).collect()))
    }

    fn offline_available(
        self: Arc<Self>,
        items: Vec<ItemSummary>,
        target: TargetApp,
        sticker: bool,
    ) -> BoxFut<Result<Vec<(String, bool)>, String>> {
        self.blocking(move |s| {
            let here = s.available_offline(&items, target, sticker);
            Ok(items.into_iter().map(|i| i.id).zip(here).collect())
        })
    }

    fn offline_reasons(self: Arc<Self>, item: ItemSummary) -> BoxFut<Result<Vec<KeptFor>, String>> {
        self.blocking(move |s| Ok(s.offline_reasons(&item)?.into_iter().map(reason_from_core).collect()))
    }

    fn offline_estimate(self: Arc<Self>, scope: KeepScope) -> BoxFut<Result<Estimate, String>> {
        self.blocking(move |s| Ok(estimate_from_core(&s.offline_estimate(&scope_to_core(&scope))?)))
    }

    fn offline_set(self: Arc<Self>, scope: KeepScope, keep: bool) -> BoxFut<Result<(), String>> {
        self.blocking(move |s| {
            let scope = scope_to_core(&scope);
            if keep {
                s.offline_keep(&scope)
            } else {
                s.offline_release(&scope)
            }
        })
    }

    fn offline_run(self: Arc<Self>, control: OfflineControl) -> BoxFut<Result<OfflineOutcome, String>> {
        Box::pin(async move {
            let out = self.session.offline_run(&control).await.map_err(|e| describe(&e))?;
            match out.end {
                RunEnd::WaitingForNetwork => self.reached(false),
                RunEnd::Unauthorized => self.login_required.store(true, Ordering::Relaxed),
                _ => {}
            }
            Ok(out)
        })
    }

    fn offline_usage(self: Arc<Self>) -> BoxFut<Result<OfflineUsage, String>> {
        self.blocking(|s| s.offline_usage())
    }

    fn offline_plan_status(self: Arc<Self>) -> BoxFut<Result<(OfflineStatus, Skips), String>> {
        self.blocking(|s| {
            let plan = s.offline_plan()?;
            let status = OfflineStatus {
                state: OfflineState::Idle,
                files_done: plan.totals.present_files,
                files_total: plan.totals.files,
                bytes_done: plan.totals.present_bytes,
                bytes_total: plan.totals.bytes,
                skipped: plan.skipped.len() as u64,
                failed: plan.failures.len() as u64,
                scopes: Vec::new(),
                message: None,
            };
            Ok((status, skips_from_core(&plan.skipped)))
        })
    }

    fn clear_cache(self: Arc<Self>) -> BoxFut<Result<EvictReport, String>> {
        self.blocking(|s| s.clear_cache())
    }

    fn offline_remove_all(self: Arc<Self>) -> BoxFut<Result<EvictReport, String>> {
        self.blocking(|s| s.offline_remove_all())
    }

    fn set_offline_options(&self, opts: OfflineOptions) {
        self.session.set_offline_options(opts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thumbs::tests::encoded;
    use image::ImageFormat;
    use stickpile_api::{MediaKind, Visibility};
    use stickpile_core::mock::{dead_url, MockServer};
    use stickpile_core::ClientConfig;

    fn config_for(url: &str, dir: &std::path::Path) -> ClientConfig {
        ClientConfig {
            server_url: url.to_string(),
            token_file: Some(dir.join("state/token")),
            cache_dir: Some(dir.join("cache")),
            timeout_secs: 2,
            ..ClientConfig::default()
        }
    }

    fn animated_gif() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
            enc.set_repeat(image::codecs::gif::Repeat::Infinite).unwrap();
            for i in 0..3u8 {
                let img = image::RgbaImage::from_pixel(12, 12, image::Rgba([i * 80, 0, 0, 255]));
                enc.encode_frame(image::Frame::new(img)).unwrap();
            }
        }
        out
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    fn backend(rt: &tokio::runtime::Runtime, cfg: ClientConfig) -> Arc<CoreBackend> {
        Arc::new(CoreBackend::new(rt.block_on(Session::open(cfg)).unwrap(), false))
    }

    #[test]
    fn error_texts() {
        assert_eq!(
            describe(&CoreError::NotLoggedIn),
            "Not logged in; log in on the Me tab (6)"
        );
        assert_eq!(
            describe(&CoreError::Offline("timeout".into())),
            "Server unreachable and the item is not cached"
        );
        assert_eq!(describe(&CoreError::Other("boom".into())), "boom");
        assert!(client_name().starts_with("desktop/"));
    }

    #[test]
    fn item_pages() {
        assert_eq!(
            item_page_url("http://localhost:8080", "01ABC").as_deref(),
            Some("http://localhost:8080/items/01ABC")
        );
        assert_eq!(
            item_page_url("https://stickers.example/base/", "x y").as_deref(),
            Some("https://stickers.example/base/items/x%20y")
        );
        assert_eq!(item_page_url("demo:/tmp/sets", "a"), None);
        assert_eq!(item_page_url("not a url", "a"), None);
    }

    /// The whole popup flow through the real core against its mock server.
    #[test]
    fn core_backend_against_mock_server() {
        let server = MockServer::start();
        let (still, gif) = {
            let mut st = server.state();
            let set = st.add_set("alice", "cats", "Cats", Visibility::Public);
            (
                st.add_item(&set.id, "grumpy cat", &["cat"], encoded(40, 30, ImageFormat::Png)),
                st.add_item(&set.id, "dancing cat", &["cat", "dance"], animated_gif()),
            )
        };
        let token = server.state().issue_token("alice", "desktop test");
        let dir = tempfile::tempdir().unwrap();
        stickpile_core::token::write_token(&dir.path().join("state/token"), &token).unwrap();
        let rt = rt();
        let b = backend(&rt, config_for(&server.url, dir.path()));
        assert!(b.has_token());
        rt.block_on(async {
            assert!(b.clone().vocabulary().await.sets.is_empty(), "nothing cached yet");
            assert!(b.clone().sync().await.unwrap(), "the first sync fills the cache");
            assert_eq!(b.clone().vocabulary().await.sets, ["cats"]);
            assert!(!b.clone().sync().await.unwrap(), "nothing new on the server");
            let r = b.clone().search("cat".into(), TargetApp::Discord, 10).await.unwrap();
            assert!(!r.offline);
            let ids: Vec<_> = r.hits.iter().map(|h| h.item.id.as_str()).collect();
            assert!(
                ids.contains(&still.id.as_str()) && ids.contains(&gif.id.as_str()),
                "{ids:?}"
            );
            assert!(!b.login_required());

            let thumb = b.clone().thumbnail(still.clone()).await.unwrap();
            let rgba = crate::thumbs::decode_file(&thumb, 256).unwrap();
            assert!(rgba.width > 0);

            let p = b
                .clone()
                .prepare_copy(still.clone(), TargetApp::Discord, false)
                .await
                .unwrap();
            assert!(p.file.exists());
            let specs = crate::offers::provider_specs(&p.offers);
            let mimes: Vec<_> = specs.iter().map(|s| s.mime.as_str()).collect();
            assert_eq!(mimes, ["image/png", "text/uri-list", "x-special/gnome-copied-files"]);

            let g = b
                .clone()
                .prepare_copy(gif.clone(), TargetApp::Discord, false)
                .await
                .unwrap();
            assert_eq!(gif.kind, MediaKind::Animation);
            assert_eq!(g.offers[0].mime, "image/gif");

            b.clone().record_copy(gif.clone(), g).await.unwrap();
            let recent = b.clone().recent(5).await;
            assert_eq!(recent.first().map(|i| i.id.as_str()), Some(gif.id.as_str()));
        });
    }

    #[test]
    fn rejected_credentials_set_login_required() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = backend(&rt, config_for(&server.url, dir.path()));
        assert!(!b.has_token());
        assert!(!b.login_required(), "no token alone does not mean logged out (tailnet)");
        let err = rt
            .block_on(b.clone().search("cat".into(), TargetApp::Generic, 5))
            .unwrap_err();
        assert!(err.contains("the Me tab"), "{err}");
        assert!(b.login_required());
        assert!(rt.block_on(b.clone().sync()).is_err());
        assert!(b.login_required());
    }

    #[test]
    fn offline_search_falls_back_without_login_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = Arc::new(CoreBackend::new(
            rt.block_on(Session::open(config_for(&dead_url(), dir.path()))).unwrap(),
            true,
        ));
        let r = rt
            .block_on(b.clone().search("cat".into(), TargetApp::Generic, 5))
            .unwrap();
        assert!(r.offline);
        assert!(b.login_required(), "an offline answer says nothing about credentials");
        assert!(rt.block_on(b.clone().sync()).is_err());
    }

    /// alice: public "cats" (3 items) and an empty set; bob: public "dogs"
    /// (1 item) and a private set (1 item).
    fn world() -> (MockServer, Vec<ItemSummary>) {
        let server = MockServer::start();
        let items = {
            let mut st = server.state();
            st.add_user("bob", "pw", stickpile_api::Role::User);
            let cats = st.add_set("alice", "cats", "Cats", Visibility::Public);
            st.add_set("alice", "empty", "Empty", Visibility::Private);
            let dogs = st.add_set("bob", "dogs", "Dogs", Visibility::Public);
            let secret = st.add_set("bob", "secret", "Secret", Visibility::Private);
            vec![
                st.add_item(&cats.id, "grumpy cat", &["cat"], encoded(40, 30, ImageFormat::Png)),
                st.add_item(&cats.id, "dancing cat", &["cat", "dance"], animated_gif()),
                st.add_item(&cats.id, "sleepy cat", &["cat"], encoded(20, 20, ImageFormat::Png)),
                st.add_item(&dogs.id, "good dog", &["dog"], encoded(30, 30, ImageFormat::Png)),
                st.add_item(&secret.id, "hidden dog", &["dog"], encoded(10, 10, ImageFormat::Png)),
            ]
        };
        (server, items)
    }

    fn token_file(dir: &std::path::Path) -> PathBuf {
        dir.join("state/token")
    }

    /// A backend with a token for alice.
    fn alice(rt: &tokio::runtime::Runtime, server: &MockServer, dir: &std::path::Path) -> Arc<CoreBackend> {
        let token = server.state().issue_token("alice", "desktop test");
        stickpile_core::token::write_token(&token_file(dir), &token).unwrap();
        backend(rt, config_for(&server.url, dir))
    }

    fn ids(items: &[ItemSummary]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    fn prepared(item: &ItemSummary) -> PreparedCopy {
        PreparedCopy {
            item_id: item.id.clone(),
            target: TargetApp::Generic,
            variant: stickpile_api::Variant::Original,
            sticker: false,
            file: PathBuf::new(),
            offers: vec![],
            warning: None,
        }
    }

    #[test]
    fn packs_items_and_covers() {
        let (server, items) = world();
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = alice(&rt, &server, dir.path());
        rt.block_on(async {
            let packs = b.clone().packs().await.unwrap();
            let names: Vec<_> = packs.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(names, ["Cats", "Dogs"], "own first; no empty or private sets");

            let cats = b.clone().pack_items(packs[0].id.clone()).await.unwrap();
            assert_eq!(ids(&cats), ids(&items[..3]), "set order, not by name");
            assert!(b.clone().pack_items(items[4].set_id.clone()).await.is_err());

            // The cover item's thumbnail is the grid's cache file.
            let cover = b.clone().pack_cover(packs[0].clone()).await.unwrap().unwrap();
            assert_eq!(cover, b.clone().thumbnail(items[0].clone()).await.unwrap());

            // A cover image wins; it is cached under its URL.
            let image = encoded(24, 24, ImageFormat::Png);
            server.state().set_cover_image(&packs[1].id, image.clone());
            let dogs = b.clone().packs().await.unwrap().remove(1);
            let path = b.clone().pack_cover(dogs.clone()).await.unwrap().unwrap();
            assert!(path.starts_with(dir.path().join("cache")), "{path:?}");
            assert_eq!(std::fs::read(&path).unwrap(), image);
            let requests = server.state().requests.len();
            assert_eq!(b.clone().pack_cover(dogs).await.unwrap(), Some(path));
            assert_eq!(server.state().requests.len(), requests);

            let mut empty = packs[0].clone();
            empty.cover_item_id = None;
            empty.cover_url = None;
            assert_eq!(b.clone().pack_cover(empty).await.unwrap(), None);
            assert!(!b.login_required());
        });
    }

    #[test]
    fn favorites_toggle_twice() {
        let (server, items) = world();
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = alice(&rt, &server, dir.path());
        rt.block_on(async {
            assert!(b.clone().favorites(10).await.unwrap().is_empty());
            assert!(b.clone().toggle_favorite(items[1].id.clone()).await.unwrap());
            assert!(b.clone().toggle_favorite(items[3].id.clone()).await.unwrap());
            let mut favs = b.clone().favorites(10).await.unwrap();
            favs.sort_by(|a, b| a.name.cmp(&b.name));
            assert_eq!(ids(&favs), [items[1].id.as_str(), items[3].id.as_str()]);
            assert_eq!(b.clone().favorites(1).await.unwrap().len(), 1);

            assert!(!b.clone().toggle_favorite(items[1].id.clone()).await.unwrap());
            assert_eq!(ids(&b.clone().favorites(10).await.unwrap()), [items[3].id.as_str()]);
            assert_eq!(server.state().likes.len(), 1);
            // Another user's private item cannot be liked.
            assert!(b.clone().toggle_favorite(items[4].id.clone()).await.is_err());
        });
    }

    #[test]
    fn most_used_counts_the_viewers_copies() {
        let (server, items) = world();
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = alice(&rt, &server, dir.path());
        // bob copies his dog a lot: not alice's usage.
        let bob = server.state().issue_token("bob", "bob");
        let events = (0..3)
            .map(|_| stickpile_api::UsageEvent {
                id: stickpile_core::new_id(),
                item_id: items[3].id.clone(),
                copied_at: 1,
                target: None,
                variant: None,
                client: None,
            })
            .collect();
        rt.block_on(async {
            let client = stickpile_core::ApiClient::new(&server.url, std::time::Duration::from_secs(2))
                .unwrap()
                .with_token(Some(bob));
            client.usage(&stickpile_api::UsageBatch { events }).await.unwrap();

            assert!(b.clone().most_used(10).await.unwrap().is_empty());
            for item in [&items[2], &items[0], &items[2]] {
                b.clone().record_copy(item.clone(), prepared(item)).await.unwrap();
            }
            b.session.flush_usage().await.unwrap();
            let used = b.clone().most_used(10).await.unwrap();
            assert_eq!(ids(&used), [items[2].id.as_str(), items[0].id.as_str()]);
            assert_eq!(ids(&b.clone().most_used(1).await.unwrap()), [items[2].id.as_str()]);

            // A copy the server has not heard of yet comes from the local history.
            server.state().fail_usage = Some(500);
            b.clone()
                .record_copy(items[1].clone(), prepared(&items[1]))
                .await
                .unwrap();
            let used = b.clone().most_used(10).await.unwrap();
            assert_eq!(ids(&used), ids(&[items[2].clone(), items[0].clone(), items[1].clone()]));
            assert!(!b.login_required());
        });
    }

    #[test]
    fn offline_lists_fall_back_to_the_cache() {
        let (mut server, items) = world();
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = alice(&rt, &server, dir.path());
        let packs = rt.block_on(async {
            b.clone().sync().await.unwrap();
            for item in [&items[1], &items[3], &items[1]] {
                b.clone().record_copy(item.clone(), prepared(item)).await.unwrap();
            }
            let packs = b.clone().packs().await.unwrap();
            b.clone().pack_cover(packs[0].clone()).await.unwrap().unwrap();
            packs
        });
        server.stop();
        rt.block_on(async {
            let used = b.clone().most_used(5).await.unwrap();
            assert_eq!(ids(&used), [items[1].id.as_str(), items[3].id.as_str()]);

            let offline = b.clone().packs().await.unwrap();
            let names: Vec<_> = offline.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(names, ["Cats", "Dogs"]);
            assert_eq!(offline[0].item_count, 3);
            let cats = b.clone().pack_items(packs[0].id.clone()).await.unwrap();
            let names: Vec<_> = cats.iter().map(|i| i.name.as_str()).collect();
            assert_eq!(names, ["dancing cat", "grumpy cat", "sleepy cat"], "by name offline");
            assert!(b.clone().pack_cover(packs[0].clone()).await.unwrap().is_some());

            let err = b.clone().pack_items("unknown".into()).await.unwrap_err();
            assert_eq!(err, "Server unreachable and the pack is not cached");
            let err = b.clone().pack_cover(packs[1].clone()).await.unwrap_err();
            assert_eq!(err, "Server unreachable and the cover is not cached");
            let err = b.clone().favorites(10).await.unwrap_err();
            assert_eq!(err, "Server unreachable; favorites need the server");
            let err = b.clone().toggle_favorite(items[0].id.clone()).await.unwrap_err();
            assert_eq!(err, "Server unreachable");
            assert_eq!(b.clone().profile().await.unwrap_err(), "Server unreachable");
            assert!(!b.login_required());
        });
    }

    #[test]
    fn unreachable_server_without_cache() {
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = backend(&rt, config_for(&dead_url(), dir.path()));
        rt.block_on(async {
            assert_eq!(b.clone().profile().await.unwrap_err(), "Server unreachable");
            assert_eq!(
                b.clone().packs().await.unwrap_err(),
                "Server unreachable and no packs are cached"
            );
            assert!(b.clone().most_used(5).await.unwrap().is_empty());
            let unreachable = LoginError::Failed("Server unreachable".into());
            assert_eq!(b.clone().login(None).await.unwrap_err(), unreachable);
            let creds = Credentials {
                username: "alice".into(),
                password: "secret".into(),
                totp: None,
            };
            assert_eq!(b.clone().login(Some(creds)).await.unwrap_err(), unreachable);
            b.clone().logout().await.unwrap();
            assert!(!b.login_required());
        });
    }

    /// Keep a pack, run the downloader, then with the server gone: the kept
    /// sticker copies, another one says why it does not.
    #[test]
    fn keeping_offline_through_the_backend() {
        let (mut server, items) = world();
        let (cat, dog) = (items[0].clone(), items[3].clone());
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = alice(&rt, &server, dir.path());
        assert_eq!(b.reachable(), None);
        let cats = KeepScope::Set(cat.set_id.clone());
        rt.block_on(async {
            b.clone().sync().await.unwrap();
            assert_eq!(b.reachable(), Some(true));
            assert!(b.clone().offline_keeps().await.unwrap().is_empty());
            let e = b.clone().offline_estimate(cats.clone()).await.unwrap();
            assert_eq!(e.items, 3);
            assert!(e.files >= 3 && e.bytes > 0, "{e:?}");
            assert!(e.fits());
            b.clone().offline_set(cats.clone(), true).await.unwrap();
            assert_eq!(b.clone().offline_keeps().await.unwrap(), std::slice::from_ref(&cats));
            assert_eq!(b.clone().offline_reasons(cat.clone()).await.unwrap(), [KeptFor::Set]);
            assert!(b.clone().offline_reasons(dog.clone()).await.unwrap().is_empty());

            let control = OfflineControl::new();
            let out = b.clone().offline_run(control.clone()).await.unwrap();
            assert!(out.is_complete(), "{out:?}");
            assert_eq!(control.status().files_done, control.status().files_total);
            let here = b
                .clone()
                .offline_available(vec![cat.clone(), dog.clone()], TargetApp::Generic, false)
                .await
                .unwrap();
            assert_eq!(here, [(cat.id.clone(), true), (dog.id.clone(), false)]);
            let usage = b.clone().offline_usage().await.unwrap();
            assert!(usage.kept_files >= 3, "{usage:?}");
            assert_eq!(usage.budget, Some(1024 * 1024 * 1024));
        });

        server.stop();
        rt.block_on(async {
            let copied = b.clone().prepare_copy(cat.clone(), TargetApp::Generic, false).await;
            assert!(copied.is_ok(), "{copied:?}");
            let err = b
                .clone()
                .prepare_copy(dog.clone(), TargetApp::Generic, false)
                .await
                .unwrap_err();
            assert_eq!(
                err,
                "good dog is not on this device and the server is unreachable; keep it offline (K) to copy it without the server"
            );
            assert_eq!(b.reachable(), Some(false));
            let out = b.clone().offline_run(OfflineControl::new()).await.unwrap();
            assert_eq!(out.end, RunEnd::WaitingForNetwork);

            let cleared = b.clone().clear_cache().await.unwrap();
            assert!(cleared.kept_files >= 3, "{cleared:?}");
            let removed = b.clone().offline_remove_all().await.unwrap();
            assert!(removed.removed_files >= 3, "{removed:?}");
            assert!(b.clone().offline_keeps().await.unwrap().is_empty());
            let here = b
                .clone()
                .offline_available(vec![cat.clone()], TargetApp::Generic, false)
                .await
                .unwrap();
            assert_eq!(here, [(cat.id.clone(), false)]);
        });
    }

    // -- ported from the behaviour review of the offline round (f398c3e) ------------------

    /// A file the mock takes for an MP4 video.
    fn mp4_bytes(len: usize) -> Vec<u8> {
        let mut v = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom".to_vec();
        v.resize(len.max(v.len()), 7);
        v
    }

    /// A file the mock takes for a WebM video.
    fn webm_bytes(len: usize) -> Vec<u8> {
        let mut v = vec![0x1A, 0x45, 0xDF, 0xA3];
        v.resize(len.max(v.len()), 9);
        v
    }

    /// alice's public pack `mixed`: a PNG still with its Telegram sticker,
    /// a GIF, an MP4 video and a WebM video, as the server makes them.
    fn mixed(server: &MockServer) -> (String, Vec<ItemSummary>) {
        let mut st = server.state();
        let set = st.add_set("alice", "mixed", "Mixed", Visibility::Public);
        let still = st.add_item(&set.id, "still png", &["still"], encoded(20, 20, ImageFormat::Png));
        st.add_variant(
            &still.id,
            stickpile_api::Variant::TgSticker,
            "image/webp",
            encoded(16, 16, ImageFormat::Png),
        );
        let gif = st.add_item(&set.id, "gif animation", &["gif"], animated_gif());
        st.add_variant(&gif.id, stickpile_api::Variant::Mp4, "video/mp4", mp4_bytes(1500));
        let mp4 = st.add_item(&set.id, "mp4 video", &["video"], mp4_bytes(4000));
        st.add_variant(
            &mp4.id,
            stickpile_api::Variant::TgVideoSticker,
            "video/webm",
            webm_bytes(800),
        );
        let webm = st.add_item(&set.id, "webm video", &["video"], webm_bytes(3000));
        st.add_variant(&webm.id, stickpile_api::Variant::Mp4, "video/mp4", mp4_bytes(2500));
        let items = [still, gif, mp4, webm]
            .iter()
            .map(|i| {
                let item = st.items.iter().find(|m| m.item.id == i.id).unwrap();
                st.summary(&item.item)
            })
            .collect();
        (set.id, items)
    }

    /// The question before keeping a pack counts what it leaves out by
    /// reason: the core lists a sticker of a kind switched off once (the
    /// two videos, videos being off by default), and the question says two
    /// stickers, not two files.
    #[test]
    fn the_popup_counts_what_it_leaves_out() {
        let server = MockServer::start();
        let (set_id, items) = mixed(&server);
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = alice(&rt, &server, dir.path());
        let (core, popup) = rt.block_on(async {
            b.clone().sync().await.unwrap();
            let popup = b
                .clone()
                .offline_estimate(KeepScope::Set(set_id.clone()))
                .await
                .unwrap();
            let core = b
                .session
                .offline_estimate(&stickpile_core::offline::Scope::Set(set_id.clone()))
                .unwrap();
            (core, popup)
        });
        let videos = items.iter().filter(|i| i.kind == MediaKind::Video).count() as u64;
        assert_eq!(videos, 2);
        assert_eq!(core.skipped.len() as u64, videos, "{:?}", core.skipped);
        assert_eq!(
            popup.skips,
            stickpile_quick::offline::Skips {
                kind_off: videos,
                ..Default::default()
            }
        );
        let body = stickpile_quick::offline::Ask::Keep {
            scope: KeepScope::Set(set_id),
            name: "Mixed".into(),
            estimate: popup,
        }
        .view()
        .body;
        assert!(
            body.contains("2 stickers are left out: their kind is off in the Offline settings."),
            "{body}"
        );
        assert!(!body.contains("files are left out"), "{body}");
    }

    /// A sticker kept by itself, the server unreachable, copied to an app
    /// or in a sticker mode whose rendition is not kept here (Telegram's
    /// sticker WebP, on a device whose default target is Generic): the
    /// popup says what is missing and does not send the user to `K`, which
    /// releases a kept sticker. A sticker that is not kept gets the `K`
    /// advice.
    #[test]
    fn a_kept_sticker_offline_is_not_told_to_keep_it() {
        let mut server = MockServer::start();
        let (_set_id, items) = mixed(&server);
        let (still, gif) = (items[0].clone(), items[1].clone());
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = alice(&rt, &server, dir.path());
        rt.block_on(async {
            b.clone().sync().await.unwrap();
            b.clone()
                .offline_set(KeepScope::Item(still.id.clone()), true)
                .await
                .unwrap();
            let out = b.clone().offline_run(OfflineControl::new()).await.unwrap();
            assert!(out.is_complete(), "{out:?}");
        });
        server.stop();
        rt.block_on(async {
            assert_eq!(
                b.clone().offline_reasons(still.clone()).await.unwrap(),
                [KeptFor::Item]
            );
            match b.clone().prepare_copy(still.clone(), TargetApp::Telegram, true).await {
                // A core that falls back to another kept rendition the app takes.
                Ok(p) => assert_ne!(p.variant, stickpile_api::Variant::TgSticker),
                Err(e) => {
                    assert!(!e.contains("(K)"), "K would release it: {e}");
                    assert_eq!(
                        e,
                        "still png is kept offline, but its tg_sticker file for Telegram as a sticker is not on this device: it is not downloaded yet, or not kept for stickers to Telegram (offline.targets, offline.sticker_renditions); the server is unreachable"
                    );
                }
            }
            assert_eq!(b.reachable(), Some(false));
            let err = b
                .clone()
                .prepare_copy(gif.clone(), TargetApp::Generic, false)
                .await
                .unwrap_err();
            assert_eq!(
                err,
                "gif animation is not on this device and the server is unreachable; keep it offline (K) to copy it without the server"
            );
        });
    }

    #[test]
    fn the_message_of_a_kept_sticker_names_the_missing_file() {
        assert_eq!(
            kept_not_here("cat", stickpile_api::Variant::Original, TargetApp::Discord, false),
            "cat is kept offline, but its original file for Discord is not on this device: it is not downloaded yet, or not kept for Discord (offline.targets); the server is unreachable"
        );
    }

    #[test]
    fn the_demo_keeps_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.png"), encoded(8, 8, ImageFormat::Png)).unwrap();
        let demo = Arc::new(crate::demo::DemoBackend::scan(dir.path(), &ClientConfig::default()).unwrap());
        let rt = rt();
        assert_eq!(demo.reachable(), None);
        let err = rt.block_on(demo.offline_keeps()).unwrap_err();
        assert!(err.contains("demo"), "{err}");
    }

    #[test]
    fn signed_out_lists_ask_for_login() {
        let (server, items) = world();
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = backend(&rt, config_for(&server.url, dir.path()));
        rt.block_on(async {
            let p = b.clone().profile().await.unwrap();
            assert_eq!(p, Profile::signed_out(server.url.clone()));
            assert!(b.login_required());
            b.login_required.store(false, Ordering::Relaxed);
            let err = b.clone().favorites(5).await.unwrap_err();
            assert!(err.contains("the Me tab"), "{err}");
            assert!(b.login_required());
            assert!(b.clone().packs().await.is_err());
            assert!(b.clone().most_used(5).await.is_err());
            assert!(b.clone().toggle_favorite(items[0].id.clone()).await.is_err());
        });
    }

    #[test]
    fn profile_then_logout() {
        let (server, _items) = world();
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = alice(&rt, &server, dir.path());
        rt.block_on(async {
            let p = b.clone().profile().await.unwrap();
            assert_eq!(p.server_url, server.url);
            assert_eq!(p.auth, AuthKind::Token);
            assert_eq!(p.username.as_deref(), Some("alice"));
            assert_eq!(p.display_name.as_deref(), Some("alice"));
            assert_eq!(
                (p.packs, p.items, p.copies, p.likes),
                (Some(2), Some(3), Some(0), Some(0))
            );
            assert!(!p.tailnet_login_available);
            assert!(!p.admin);

            server.state().tailnet_user = Some("alice".into());
            assert!(b.clone().profile().await.unwrap().tailnet_login_available);

            b.clone().logout().await.unwrap();
            assert!(!token_file(dir.path()).exists());
            assert!(!b.has_token());
            assert!(server.state().tokens.iter().all(|t| t.info.revoked));
            // The tailnet identity still answers without a token.
            let p = b.clone().profile().await.unwrap();
            assert_eq!(p.auth, AuthKind::Tailnet);
            assert_eq!(p.username.as_deref(), Some("alice"));
            assert!(p.tailnet_login_available);
            assert!(!b.login_required());

            server.state().tailnet_user = None;
            let p = b.clone().profile().await.unwrap();
            assert_eq!(p, Profile::signed_out(server.url.clone()));
            assert!(b.login_required());
            b.clone().logout().await.unwrap();
        });
    }

    #[test]
    fn logout_without_tailnet_asks_for_login() {
        let (server, _items) = world();
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = alice(&rt, &server, dir.path());
        rt.block_on(async {
            b.clone().sync().await.unwrap();
            assert!(!b.login_required());
            b.clone().logout().await.unwrap();
            assert!(b.login_required());
            assert!(b.session.cache().me().unwrap().is_none(), "account data dropped");
            assert_eq!(b.clone().profile().await.unwrap().auth, AuthKind::None);
        });
    }

    #[test]
    fn revoked_token_reads_as_signed_out() {
        let (server, _items) = world();
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = alice(&rt, &server, dir.path());
        server.state().tokens[0].info.revoked = true;
        server.state().tailnet_user = Some("alice".into());
        rt.block_on(async {
            let p = b.clone().profile().await.unwrap();
            assert_eq!(p.auth, AuthKind::None);
            assert!(p.tailnet_login_available);
            assert!(b.login_required());
            b.clone().logout().await.unwrap();
            assert!(!token_file(dir.path()).exists());
            assert!(!b.login_required(), "the tailnet identity works again");
            assert_eq!(b.clone().profile().await.unwrap().auth, AuthKind::Tailnet);
        });
    }

    #[test]
    fn password_login_with_totp() {
        let (server, _items) = world();
        server.state().users[0].totp = Some("123456".into());
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = backend(&rt, config_for(&server.url, dir.path()));
        let creds = |password: &str, totp: Option<&str>| {
            Some(Credentials {
                username: "alice".into(),
                password: password.into(),
                totp: totp.map(Into::into),
            })
        };
        rt.block_on(async {
            let login = |c| b.clone().login(c);
            assert_eq!(
                login(creds("secret", None)).await.unwrap_err(),
                LoginError::TotpRequired
            );
            assert_eq!(
                login(creds("secret", Some(" "))).await.unwrap_err(),
                LoginError::TotpRequired
            );
            assert_eq!(
                login(creds("secret", Some("000000"))).await.unwrap_err(),
                LoginError::Failed("Invalid TOTP code".into())
            );
            assert_eq!(
                login(creds("nope", None)).await.unwrap_err(),
                LoginError::Failed("Invalid username or password".into())
            );
            assert!(!token_file(dir.path()).exists());

            let p = login(creds("secret", Some("123456"))).await.unwrap();
            assert_eq!(p.auth, AuthKind::Token);
            assert_eq!(p.username.as_deref(), Some("alice"));
            assert!(b.has_token());
            assert!(!b.login_required());
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(token_file(dir.path())).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o600);
            }
            assert!(server.state().tokens[0].info.name.starts_with("stickpile-desktop@"));
            // The same backend uses the new token right away.
            let r = b.clone().search("cat".into(), TargetApp::Generic, 5).await.unwrap();
            assert_eq!(r.hits.len(), 3);
            assert_eq!(b.session.cache().me().unwrap().unwrap().username, "alice");
        });
    }

    #[test]
    fn tailnet_login_turns_the_identity_into_a_token() {
        let (server, _items) = world();
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = backend(&rt, config_for(&server.url, dir.path()));
        rt.block_on(async {
            let LoginError::Failed(text) = b.clone().login(None).await.unwrap_err() else {
                panic!("expected a failure");
            };
            assert!(text.contains("tailnet identity"), "{text}");

            server.state().tailnet_user = Some("alice".into());
            let p = b.clone().profile().await.unwrap();
            assert_eq!(p.auth, AuthKind::Tailnet);
            assert!(p.tailnet_login_available);

            let p = b.clone().login(None).await.unwrap();
            assert_eq!(p.auth, AuthKind::Token);
            assert!(p.tailnet_login_available);
            assert!(token_file(dir.path()).exists());
            assert!(server.state().tokens[0].info.name.starts_with("stickpile-desktop@"));

            // From now on the token answers.
            server.state().tailnet_user = None;
            assert_eq!(b.clone().profile().await.unwrap().auth, AuthKind::Token);
            b.clone().logout().await.unwrap();
            assert_eq!(b.clone().profile().await.unwrap().auth, AuthKind::None);
            assert!(b.login_required());
        });
    }

    #[test]
    fn login_failure_texts() {
        let api = |message: &str| CoreError::Api {
            status: 401,
            error: stickpile_api::ApiError {
                code: "unauthorized".into(),
                message: message.into(),
            },
        };
        assert_eq!(login_failure(&api("invalid TOTP code")), "Invalid TOTP code");
        assert_eq!(login_failure(&api("")), "");
        assert_eq!(
            login_failure(&CoreError::Offline("refused".into())),
            "Server unreachable"
        );
        assert_eq!(LoginError::TotpRequired.to_string(), "A TOTP code is required");
        let creds = Credentials {
            username: "alice".into(),
            password: "hunter2".into(),
            totp: Some("123456".into()),
        };
        let shown = format!("{creds:?}");
        assert!(!shown.contains("hunter2") && !shown.contains("123456"), "{shown}");
    }

    /// The popup's suggestion line per keystroke on 50,000 cached items
    /// (rule 8 of the text-search round), the vocabulary loaded as the
    /// backend loads it (`CoreBackend::vocabulary`): the query changes
    /// (its search command resolved) and the line is drawn.
    /// `cargo test -p stickpile-desktop --release --lib -- --ignored --nocapture perf_popup`
    #[test]
    #[ignore]
    fn perf_popup_suggestions() {
        use std::time::Instant;
        use stickpile_quick::{Event, Model, ModelConfig, OpenInfo};
        use stickpile_search::synthetic;

        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(&dead_url(), dir.path());
        let items: Vec<ItemSummary> = synthetic::library(50_000, 42).iter().map(synthetic::summary).collect();
        {
            let cache = stickpile_core::cache::Cache::open(&cfg.cache_path(), cfg.cache_max_mb).unwrap();
            cache.bind_server(&cfg.server_url).unwrap();
            cache
                .set_me(&User {
                    id: "user0".into(),
                    username: "User0".into(),
                    display_name: "User0".into(),
                    role: stickpile_api::Role::User,
                    totp_enabled: false,
                    tailscale_login: None,
                    created_at: 0,
                    telegram_user_id: None,
                    theme: Default::default(),
                    language: None,
                })
                .unwrap();
            cache.upsert_items(&items).unwrap();
        }
        let rt = rt();
        let b = backend(&rt, cfg);
        let start = Instant::now();
        let vocab = rt.block_on(b.clone().vocabulary());
        println!("vocabulary, cache index built: {:.2?}", start.elapsed());
        let start = Instant::now();
        let vocab2 = rt.block_on(b.clone().vocabulary());
        println!("vocabulary again (once per cache change): {:.2?}", start.elapsed());
        assert_eq!(vocab.tags, vocab2.tags);
        let mut m = Model::new(ModelConfig::default());
        m.handle(Event::Open(OpenInfo::default()));
        m.handle(Event::VocabularyLoaded(vocab));
        for typed in [
            "vibe:",
            "vibe:sa",
            "t:",
            "t:ko",
            "e:",
            "e:sk",
            "x:",
            "x:l",
            "x:lo",
            "x:lol",
            "x:lool",
            "x:ко",
            "x:оли",
            "x:п'ят",
        ] {
            let runs = 20;
            let other = format!("a {typed}");
            let start = Instant::now();
            let mut line = None;
            for i in 0..runs {
                let q = if i % 2 == 0 { typed } else { other.as_str() };
                m.handle(Event::QueryChanged(q.into()));
                line = m.suggestions();
            }
            let per = start.elapsed() / runs;
            let words = line.map(|l| l.words.join(" ")).unwrap_or_else(|| "none".into());
            println!("  {typed:<10} {per:>10.2?}  {words}");
        }
    }
}
