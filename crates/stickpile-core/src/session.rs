//! High-level facade for UI shells. Owned by the core agent; consumed by the
//! desktop client (and later the UniFFI bindings).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use stickpile_api::profiles::{choose_variant, ItemShape};
use stickpile_api::routes::{self, path};
use stickpile_api::{
    CreateToken, ItemSummary, MediaKind, SearchHit, SetSource, SetSummary, TargetApp, TokenCreated, TokenRequest, User,
    Variant, VariantInfo, Visibility,
};

use crate::cache::{self, safe_component, Cache, OfflineIndex, SyncReport};
use crate::client::{summary_from_detail, summary_from_set_item, ApiClient, SearchParams};
use crate::clipboard::{build_offers, ClipOffer};
use crate::usage::{copy_event, FlushReport, UsageQueue};
use crate::{token, ClientConfig, CoreError, Result};

/// Results of [`Session::search`].
#[derive(Debug, Clone, Default)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    /// True if the server was unreachable and the offline cache answered
    /// (metadata-only ranking, no image-content matches).
    pub offline: bool,
}

/// A list from the server, or from the offline cache when the server could
/// not be reached.
#[derive(Debug, Clone)]
pub struct Listing<T> {
    pub items: Vec<T>,
    /// True if the offline cache answered.
    pub offline: bool,
}

impl<T> Listing<T> {
    fn online(items: Vec<T>) -> Listing<T> {
        Listing { items, offline: false }
    }

    fn offline(items: Vec<T>) -> Listing<T> {
        Listing { items, offline: true }
    }
}

/// What [`Session::logout`] did.
#[derive(Debug)]
pub struct Logout {
    /// A token was stored; it is gone now.
    pub had_token: bool,
    /// `Ok(true)`: revoked on the server; `Ok(false)`: the server does not
    /// know the token (or there was none); `Err`: not revoked, e.g. the
    /// server is unreachable or already rejects the token.
    pub revoked: Result<bool>,
}

/// Everything needed to put one item on the clipboard.
#[derive(Debug, Clone)]
pub struct PreparedCopy {
    pub item_id: String,
    pub target: TargetApp,
    pub variant: Variant,
    pub sticker: bool,
    /// Materialised file in the cache, named after the item (`cat.gif`), used
    /// for file-reference offers and drag-and-drop.
    pub file: PathBuf,
    /// In offer order, per `clipboard` config for this target and kind.
    pub offers: Vec<ClipOffer>,
    /// E.g. "Discord limit is 10 MiB" when no variant fits; the copy still
    /// proceeds with the best available variant.
    pub warning: Option<String>,
}

/// Thread-safe (`Send + Sync`); all methods take `&self`.
pub struct Session {
    config: ClientConfig,
    client: ApiClient,
    cache: Arc<Cache>,
    usage: Arc<UsageQueue>,
    offline: Mutex<Option<Arc<OfflineIndex>>>,
    runtime: Option<tokio::runtime::Handle>,
    client_name: String,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("client", &self.client)
            .field("cache", &self.cache)
            .finish()
    }
}

impl Session {
    /// Load the token (if any) and open the cache. Never fails because the
    /// server is down.
    pub async fn open(config: ClientConfig) -> Result<Session> {
        config.validate().map_err(|e| CoreError::Other(e.to_string()))?;
        let token = token::read_token(&config.token_path())?;
        let client = ApiClient::from_config(&config)?.with_token(token);
        let cache_dir = config.cache_path();
        let max_mb = config.cache_max_mb;
        let cache = tokio::task::spawn_blocking(move || Cache::open(&cache_dir, max_mb))
            .await
            .map_err(|e| CoreError::Other(format!("opening cache: {e}")))??;
        cache.bind_server(&config.server_url)?;
        let cache = Arc::new(cache);
        let usage = Arc::new(UsageQueue::new(cache.clone()));
        Ok(Session {
            config,
            client,
            cache,
            usage,
            offline: Mutex::new(None),
            runtime: tokio::runtime::Handle::try_current().ok(),
            client_name: format!("core/{}", env!("CARGO_PKG_VERSION")),
        })
    }

    /// Name reported in usage events (`desktop/0.1.0`, `cli/0.1.0`).
    pub fn with_client_name(mut self, name: impl Into<String>) -> Session {
        self.client_name = name.into();
        self
    }

    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// True if an API token is stored. (Tokenless requests still work for
    /// tailnet users behind `tailscale serve`; `stickpile login` turns that
    /// identity into a token without a password.)
    pub fn is_logged_in(&self) -> bool {
        self.client.has_token()
    }

    /// The REST client (authenticated with the session's token).
    pub fn client(&self) -> &ApiClient {
        &self.client
    }

    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    /// Pull item metadata changes into the cache and flush queued usage events.
    pub async fn sync(&self) -> Result<()> {
        self.sync_report().await.map(|_| ())
    }

    /// [`Session::sync`] with the item sync report.
    pub async fn sync_report(&self) -> Result<SyncReport> {
        let me = self.client.me().await?;
        self.cache.set_me(&me)?;
        let report = cache::sync(&self.cache, &self.client, cache::SYNC_PAGE_LIMIT).await?;
        if let Err(e) = self.usage.flush(&self.client).await {
            tracing::debug!("usage flush after sync: {e}");
        }
        Ok(report)
    }

    /// Server search with `target` compatibility hints; on [`crate::CoreError::Offline`]
    /// falls back to the offline cache.
    pub async fn search(&self, query: &str, target: Option<TargetApp>, limit: usize) -> Result<SearchOutcome> {
        let params = SearchParams {
            q: query.to_string(),
            limit: Some(limit),
            offset: None,
            target,
        };
        match self.client.search(&params).await {
            Ok(res) => Ok(SearchOutcome {
                hits: res.hits,
                offline: false,
            }),
            Err(e) if falls_back(&e) => {
                tracing::debug!("search falling back to the offline cache: {e}");
                self.search_offline(query, target, limit)
            }
            Err(e) => Err(e),
        }
    }

    /// Search the offline cache only.
    pub fn search_offline(&self, query: &str, target: Option<TargetApp>, limit: usize) -> Result<SearchOutcome> {
        let index = self.offline_index()?;
        Ok(SearchOutcome {
            hits: index.search(query, target, limit),
            offline: true,
        })
    }

    fn offline_index(&self) -> Result<Arc<OfflineIndex>> {
        let generation = self.cache.generation()?;
        let mut slot = self.offline.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(idx) = slot.as_ref().filter(|i| i.generation() == generation) {
            return Ok(idx.clone());
        }
        let idx = Arc::new(self.cache.offline_index()?);
        *slot = Some(idx.clone());
        Ok(idx)
    }

    /// Most recently copied items (from the local usage log), newest first.
    pub fn recent(&self, limit: usize) -> Vec<ItemSummary> {
        self.cache.recent(limit).unwrap_or_else(|e| {
            tracing::warn!("reading copy history: {e}");
            Vec::new()
        })
    }

    /// An item by id: from the server, else from the offline cache.
    pub async fn item_summary(&self, id: &str) -> Result<ItemSummary> {
        match self.client.item(id).await {
            Ok(detail) => Ok(summary_from_detail(&detail)),
            Err(e) if e.is_offline() => self.cache.item(id)?.ok_or(e),
            Err(e) => Err(e),
        }
    }

    /// Path of the cached thumbnail, downloading it if needed.
    pub async fn thumbnail(&self, item: &ItemSummary) -> Result<PathBuf> {
        let rel = format!("files/thumb/{}.webp", safe_component(&item.media_id));
        let expect = item
            .variants
            .iter()
            .find(|v| v.variant == Variant::Thumb)
            .map(|v| v.size_bytes);
        if let Some(p) = self.cache.lookup_file(&rel, expect)? {
            return Ok(p);
        }
        let abs = self.cache.abs(&rel);
        self.client.download_item_file(&item.id, Variant::Thumb, &abs).await?;
        self.cache.register_file(&rel)?;
        Ok(abs)
    }

    /// Choose the variant (profiles::choose_variant), materialise it in the
    /// cache and build the clipboard offers.
    pub async fn prepare_copy(&self, item: &ItemSummary, target: TargetApp, sticker: bool) -> Result<PreparedCopy> {
        let (variant, is_sticker, warning) = pick_variant(item, target, sticker)?;
        let ext = variant_extension(item, variant.variant);
        let rel = format!(
            "clip/{}.{}/{}.{ext}",
            safe_component(&item.media_id),
            variant.variant.as_str(),
            clip_file_stem(&item.name, &item.id)
        );
        let file = match self.cache.lookup_file(&rel, Some(variant.size_bytes))? {
            Some(p) => p,
            None => {
                let abs = self.cache.abs(&rel);
                self.client.download_item_file(&item.id, variant.variant, &abs).await?;
                self.cache.register_file(&rel)?;
                abs
            }
        };
        let mime = variant_mime(item, &variant);
        let offers_cfg = self
            .config
            .clipboard
            .offers(target)
            .for_kind(item.kind, is_sticker)
            .to_vec();
        let file_for_offers = file.clone();
        let offers = tokio::task::spawn_blocking(move || build_offers(&offers_cfg, &file_for_offers, &mime))
            .await
            .map_err(|e| CoreError::Other(format!("building clipboard offers: {e}")))??;
        Ok(PreparedCopy {
            item_id: item.id.clone(),
            target,
            variant: variant.variant,
            sticker: is_sticker,
            file,
            offers,
            warning,
        })
    }

    /// Queue a usage event and try to flush in the background. Works offline.
    pub fn record_copy(&self, item: &ItemSummary, prepared: &PreparedCopy) -> Result<()> {
        let event = copy_event(item, Some(prepared.target), Some(prepared.variant), &self.client_name);
        self.cache.add_history(item, &event)?;
        self.usage.push(&event)?;
        if self.is_logged_in() {
            let handle = self
                .runtime
                .clone()
                .or_else(|| tokio::runtime::Handle::try_current().ok());
            if let Some(handle) = handle {
                let usage = self.usage.clone();
                let client = self.client.clone();
                handle.spawn(async move {
                    if let Err(e) = usage.flush(&client).await {
                        tracing::debug!("background usage flush: {e}");
                    }
                });
            }
        }
        Ok(())
    }

    /// Upload queued usage events now (waits for a running flush first).
    pub async fn flush_usage(&self) -> Result<FlushReport> {
        self.usage.flush(&self.client).await
    }

    /// Number of usage events waiting for upload.
    pub fn pending_usage(&self) -> usize {
        self.usage.len().unwrap_or(0)
    }

    // -- browsing ---------------------------------------------------------------

    /// The viewer's most copied items: the server's count over all their
    /// devices first (`/stats/me` lists the top 10), then this device's copy
    /// history. The history alone when the server is unreachable.
    pub async fn most_used(&self, limit: usize) -> Result<Listing<ItemSummary>> {
        match self.client.stats_me().await {
            Ok(stats) => {
                let mut items: Vec<ItemSummary> = stats.top_items.into_iter().map(|e| e.item).collect();
                if items.len() < limit {
                    let seen: HashSet<String> = items.iter().map(|i| i.id.clone()).collect();
                    let local = self.cache.most_copied(limit)?;
                    items.extend(local.into_iter().filter(|i| !seen.contains(&i.id)));
                }
                items.truncate(limit);
                Ok(Listing::online(items))
            }
            Err(e) if falls_back(&e) => {
                tracing::debug!("most used from the local copy history: {e}");
                Ok(Listing::offline(self.cache.most_copied(limit)?))
            }
            Err(e) => Err(e),
        }
    }

    /// Items the viewer liked (`is:liked`, at most 200), most copied first:
    /// the API keeps no like time to sort by. Likes are not cached, so this
    /// needs the server.
    pub async fn favorites(&self, limit: usize) -> Result<Vec<ItemSummary>> {
        let params = SearchParams {
            q: "is:liked".into(),
            limit: Some(limit),
            offset: None,
            target: None,
        };
        let res = self.client.search(&params).await?;
        Ok(res.hits.into_iter().take(limit).map(|h| h.item).collect())
    }

    /// The sets a picker offers: the viewer's own sets, most recently updated
    /// first like on the web home page, then other users' public sets. Empty
    /// sets are left out. Offline: the sets of the cached items.
    pub async fn packs(&self) -> Result<Listing<SetSummary>> {
        let (mine, all) = tokio::join!(self.client.sets(Some("mine")), self.client.sets(Some("all")));
        match mine.and_then(|m| all.map(|a| (m, a))) {
            Ok((mine, all)) => {
                let own: HashSet<String> = mine.iter().map(|s| s.id.clone()).collect();
                let sets = mine
                    .into_iter()
                    .chain(all.into_iter().filter(|s| !own.contains(&s.id)))
                    .filter(|s| s.item_count > 0)
                    .collect();
                Ok(Listing::online(sets))
            }
            Err(e) if falls_back(&e) => {
                tracing::debug!("packs from the offline cache: {e}");
                let sets = self.cached_packs()?;
                if sets.is_empty() {
                    return Err(e);
                }
                Ok(Listing::offline(sets))
            }
            Err(e) => Err(e),
        }
    }

    /// Sets made up from the cached items, ordered like [`Session::packs`].
    /// The cache keeps no set records: visibility follows the offline
    /// index's rule (own private, the rest public) and the cover is the
    /// first item by name.
    fn cached_packs(&self) -> Result<Vec<SetSummary>> {
        let me = self.cache.me()?.map(|u| u.username);
        let mine = |owner: &str| me.as_deref() == Some(owner);
        let mut items = self.cache.items()?;
        items.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        let mut sets: Vec<SetSummary> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for item in &items {
            let i = *index.entry(item.set_id.clone()).or_insert_with(|| {
                sets.push(SetSummary {
                    id: item.set_id.clone(),
                    slug: item.set_slug.clone(),
                    name: item.set_name.clone(),
                    description: String::new(),
                    owner_username: item.owner_username.clone(),
                    visibility: if mine(&item.owner_username) {
                        Visibility::Private
                    } else {
                        Visibility::Public
                    },
                    source: SetSource::Local,
                    item_count: 0,
                    cover_item_id: Some(item.id.clone()),
                    cover_url: None,
                    updated_at: 0,
                });
                sets.len() - 1
            });
            sets[i].item_count += 1;
            sets[i].updated_at = sets[i].updated_at.max(item.updated_at);
        }
        sets.sort_by(|a, b| {
            mine(&b.owner_username)
                .cmp(&mine(&a.owner_username))
                .then(b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(sets)
    }

    /// A set's items in set order. Offline: its cached items by name (the
    /// cache does not keep the set order).
    pub async fn pack_items(&self, set_id: &str) -> Result<Listing<ItemSummary>> {
        match self.client.set(set_id).await {
            Ok(d) => Ok(Listing::online(
                d.items.iter().map(|i| summary_from_set_item(i, &d.set)).collect(),
            )),
            Err(e) if falls_back(&e) => {
                let mut items: Vec<ItemSummary> =
                    self.cache.items()?.into_iter().filter(|i| i.set_id == set_id).collect();
                if items.is_empty() {
                    return Err(e);
                }
                items.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
                Ok(Listing::offline(items))
            }
            Err(e) => Err(e),
        }
    }

    /// Local thumbnail of a set's cover: its own cover image, else the cover
    /// item's thumbnail (the same cache file as [`Session::thumbnail`]).
    /// `None` for an empty set without a cover image.
    pub async fn pack_cover(&self, set: &SetSummary) -> Result<Option<PathBuf>> {
        let cover_image = format!("{}/", path(routes::SET_COVER, &[&set.id]));
        if let Some(url) = set.cover_url.as_deref().filter(|u| u.starts_with(&cover_image)) {
            // The URL changes with the image, so it can name the cache file.
            let rel = format!("files/cover/{}.webp", safe_component(url));
            if let Some(p) = self.cache.lookup_file(&rel, None)? {
                return Ok(Some(p));
            }
            let abs = self.cache.abs(&rel);
            self.client.download(url, &abs).await?;
            self.cache.register_file(&rel)?;
            return Ok(Some(abs));
        }
        let Some(id) = set.cover_item_id.as_deref() else {
            return Ok(None);
        };
        let item = match self.cache.item(id)? {
            Some(item) => item,
            None => self.item_summary(id).await?,
        };
        self.thumbnail(&item).await.map(Some)
    }

    /// Flip the viewer's like of an item; returns the new state (true: liked).
    pub async fn toggle_like(&self, item_id: &str) -> Result<bool> {
        let liked = self.client.item(item_id).await?.item.liked_by_me;
        let state = if liked {
            self.client.unlike(item_id).await?
        } else {
            self.client.like(item_id).await?
        };
        Ok(state.liked)
    }

    // -- account ----------------------------------------------------------------

    /// [`tailnet_login`] for this session: the new token is stored (see
    /// [`save_login`]) and used from now on. `Ok(None)`: the server does not
    /// know us by tailnet identity.
    pub async fn login_tailnet(&self, token_name: &str) -> Result<Option<User>> {
        match tailnet_login(&self.client, token_name).await? {
            Some(created) => self.adopt(created).map(Some),
            None => Ok(None),
        }
    }

    /// Password (+TOTP) login for this session, stored and used like
    /// [`Session::login_tailnet`]. [`CoreError::is_totp_required`] tells that
    /// the account wants a code and `req.totp` is `None`.
    pub async fn login_password(&self, req: &TokenRequest) -> Result<User> {
        let created = self.client.login(req).await?;
        self.adopt(created)
    }

    fn adopt(&self, created: TokenCreated) -> Result<User> {
        save_login(&self.config, Some(&self.cache), &created)?;
        self.client.set_token(Some(created.token));
        Ok(created.user)
    }

    /// Log out like `stickpile logout`: upload queued usage, revoke the token
    /// on the server (best effort), delete the token file and drop the
    /// account's data from the offline cache. The session carries on without
    /// a token.
    pub async fn logout(&self) -> Result<Logout> {
        let path = self.config.token_path();
        // A token stored by `stickpile login` after this session opened counts too.
        let secret = match self.client.token() {
            Some(t) => Some(t),
            None => token::read_token(&path)?,
        };
        let Some(secret) = secret else {
            return Ok(Logout {
                had_token: false,
                revoked: Ok(false),
            });
        };
        self.client.set_token(Some(secret));
        if let Err(e) = self.usage.flush(&self.client).await {
            tracing::info!("queued usage events are dropped: {e}");
        }
        let revoked = match self.client.current_token_id().await {
            Ok(Some(id)) => self.client.revoke_token(&id).await.map(|_| true),
            Ok(None) => Ok(false),
            Err(e) => Err(e),
        };
        self.client.set_token(None);
        token::delete_token(&path)?;
        self.cache.reset_account_data()?;
        Ok(Logout {
            had_token: true,
            revoked,
        })
    }
}

/// Errors after which the offline cache answers instead: the server is
/// unreachable or failing.
fn falls_back(e: &CoreError) -> bool {
    e.is_offline() || e.status().is_some_and(|s| s >= 500)
}

/// Token name for a client on this machine, e.g. `stickpile-cli@laptop`.
pub fn token_name(client: &str) -> String {
    format!("{client}@{}", hostname())
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// The passwordless login of `stickpile login`: behind `tailscale serve` the
/// server already knows the caller by tailnet identity, and that identity is
/// turned into a token. `Ok(None)`: the server does not identify us without
/// a token. Nothing is stored here (see [`save_login`]).
pub async fn tailnet_login(client: &ApiClient, token_name: &str) -> Result<Option<TokenCreated>> {
    let anon = client.anonymous();
    match anon.me().await {
        Ok(user) => {
            tracing::info!("authenticated by tailnet identity as {}", user.username);
            let req = CreateToken {
                name: token_name.to_string(),
                expires_at: None,
            };
            anon.create_token(&req).await.map(Some)
        }
        Err(CoreError::NotLoggedIn) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Store a new login like `stickpile login`: the token file (0600) and the
/// offline cache bound to this server and account (another account's cached
/// data is dropped). Without `cache` the cache is opened here; failing to
/// open it is only logged.
pub fn save_login(config: &ClientConfig, cache: Option<&Cache>, created: &TokenCreated) -> Result<()> {
    token::write_token(&config.token_path(), &created.token)?;
    let opened;
    let cache = match cache {
        Some(c) => c,
        None => match Cache::open(&config.cache_path(), config.cache_max_mb) {
            Ok(c) => {
                opened = c;
                &opened
            }
            Err(e) => {
                tracing::warn!("offline cache: {e}");
                return Ok(());
            }
        },
    };
    cache.bind_server(&config.server_url)?;
    cache.set_me(&created.user)?;
    Ok(())
}

/// Variant for `target` (sticker rendition when asked and supported). When no
/// variant is compatible, the best available one is used and the reason is
/// returned as a warning.
pub fn pick_variant(
    item: &ItemSummary,
    target: TargetApp,
    sticker: bool,
) -> Result<(VariantInfo, bool, Option<String>)> {
    let shape = ItemShape {
        kind: item.kind,
        format: item.format,
        duration_ms: None,
        variants: &item.variants,
    };
    let reason = match choose_variant(target, &shape, sticker) {
        Ok(c) => return Ok((c.variant, c.sticker, None)),
        Err(e) => e.0,
    };
    if sticker {
        if let Ok(c) = choose_variant(target, &shape, false) {
            return Ok((c.variant, false, Some(reason)));
        }
    }
    let fallback = fallback_order(item.kind)
        .iter()
        .find_map(|want| item.variants.iter().find(|v| v.variant == *want))
        .cloned()
        .ok_or_else(|| CoreError::Other(format!("{}: no downloadable variant", item.name)))?;
    Ok((fallback, false, Some(reason)))
}

fn fallback_order(kind: MediaKind) -> &'static [Variant] {
    use Variant::*;
    match kind {
        MediaKind::Still => &[Original, Png, TgSticker],
        MediaKind::Animation => &[Original, DiscordGif, Mp4, TgVideoSticker],
        MediaKind::Video => &[Original, Mp4, DiscordGif, TgVideoSticker],
    }
}

/// File extension of a variant of `item`.
pub fn variant_extension(item: &ItemSummary, variant: Variant) -> &'static str {
    match variant {
        Variant::Original => item.format.extension(),
        Variant::Thumb | Variant::TgSticker => "webp",
        Variant::Png => "png",
        Variant::TgVideoSticker => "webm",
        Variant::DiscordGif => "gif",
        Variant::Mp4 => "mp4",
    }
}

fn variant_mime(item: &ItemSummary, v: &VariantInfo) -> String {
    if !v.mime.is_empty() {
        return v.mime.clone();
    }
    match v.variant {
        Variant::Original => item.format.mime().to_string(),
        Variant::Thumb | Variant::TgSticker => "image/webp".into(),
        Variant::Png => "image/png".into(),
        Variant::TgVideoSticker => "video/webm".into(),
        Variant::DiscordGif => "image/gif".into(),
        Variant::Mp4 => "video/mp4".into(),
    }
}

/// File name stem for the clipboard file: the item name (shown by Telegram
/// and file managers) with path separators, control characters and leading
/// dots removed, at most 100 bytes.
pub fn clip_file_stem(name: &str, fallback: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    s = s.trim().trim_start_matches('.').trim().to_string();
    if s.len() > 100 {
        let mut cut = 100;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s = s.trim_end().to_string();
    }
    if s.is_empty() {
        safe_component(fallback)
    } else {
        s
    }
}
