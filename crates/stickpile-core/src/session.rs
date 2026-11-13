//! High-level facade for UI shells. Owned by the core agent; consumed by the
//! desktop client (and later the UniFFI bindings).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use stickpile_api::profiles::{choose_variant, ItemShape};
use stickpile_api::{ItemSummary, MediaKind, SearchHit, TargetApp, Variant, VariantInfo};

use crate::cache::{self, safe_component, Cache, OfflineIndex, SyncReport};
use crate::client::{summary_from_detail, ApiClient, SearchParams};
use crate::clipboard::adjust::adjust_copy_file;
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
            Err(e) if e.is_offline() || e.status().is_some_and(|s| s >= 500) => {
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
        // `[copy]` padding / flattening; the offers below are built from the result.
        let (file, mime) = adjust_copy_file(
            &self.cache,
            &rel,
            file,
            variant_mime(item, &variant),
            item.kind,
            is_sticker,
            self.config.copy,
        )
        .await?;
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
