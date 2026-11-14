//! What the popup needs from the outside world, as an object-safe trait so
//! the GTK layer can run against `stickpile_core::Session` or the
//! [`crate::demo`] directory backend. All futures are `Send + 'static` and run
//! on the tokio runtime; the GTK side awaits their `JoinHandle`s on the GLib
//! main loop.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use stickpile_api::{ItemSummary, Role, SetSummary, TargetApp, TokenRequest, User, UserStats};
use stickpile_core::session::{self, Listing};
use stickpile_core::{CoreError, PreparedCopy, Session};
use stickpile_i18n::global::t;
use stickpile_quick::model::Results;

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
    fn sync(self: Arc<Self>) -> BoxFut<Result<(), String>>;
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
}

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
        CoreError::Offline(_) => offline.into(),
        e if e.is_unauthorized() => REJECTED().into(),
        other => other.to_string(),
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
}

impl CoreBackend {
    /// `login_required`: carry over what the previous session learned (the
    /// shell re-opens the session to pick up a token from `stickpile login`).
    pub fn new(session: Session, login_required: bool) -> CoreBackend {
        CoreBackend {
            session: session.with_client_name(client_name()),
            login_required: AtomicBool::new(login_required),
        }
    }

    pub fn has_token(&self) -> bool {
        self.session.is_logged_in()
    }

    /// Track authorisation from a result: a rejection sets the flag, an
    /// answer from the server (`reached_server`) clears it. Errors become
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
                }
                Ok(v)
            }
            Err(e) => {
                if e.is_unauthorized() {
                    self.login_required.store(true, Ordering::Relaxed);
                }
                Err(describe_with(&e, offline))
            }
        }
    }

    /// [`CoreBackend::track`] for a list that may come from the offline cache.
    fn listing<T>(&self, r: Result<Listing<T>, CoreError>, offline: &str) -> Result<Vec<T>, String> {
        self.track(r, |l| !l.offline, offline).map(|l| l.items)
    }
}

impl Backend for CoreBackend {
    fn login_required(&self) -> bool {
        self.login_required.load(Ordering::Relaxed)
    }

    fn search(self: Arc<Self>, query: String, target: TargetApp, limit: usize) -> BoxFut<Result<Results, String>> {
        Box::pin(async move {
            let r = self.session.search(&query, Some(target), limit).await;
            let out = self.track(r, |o| !o.offline, ITEM_NOT_CACHED())?;
            Ok(Results {
                hits: out.hits,
                offline: out.offline,
            })
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

    fn sync(self: Arc<Self>) -> BoxFut<Result<(), String>> {
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
            b.clone().sync().await.unwrap();
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
}
