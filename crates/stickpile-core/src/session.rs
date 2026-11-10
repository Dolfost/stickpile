//! High-level facade for UI shells. Owned by the core agent; consumed by the
//! desktop client (and later the UniFFI bindings).

use std::path::PathBuf;

use stickpile_api::{ItemSummary, SearchHit, TargetApp, Variant};

use crate::clipboard::ClipOffer;
use crate::{ClientConfig, Result};

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
    _private: (),
}

impl Session {
    /// Load the token (if any) and open the cache. Never fails because the
    /// server is down.
    pub async fn open(config: ClientConfig) -> Result<Session> {
        let _ = config;
        todo!("core agent")
    }
    pub fn config(&self) -> &ClientConfig {
        todo!("core agent")
    }
    pub fn is_logged_in(&self) -> bool {
        todo!("core agent")
    }
    /// Pull item metadata changes into the cache and flush queued usage events.
    pub async fn sync(&self) -> Result<()> {
        todo!("core agent")
    }
    /// Server search with `target` compatibility hints; on [`crate::CoreError::Offline`]
    /// falls back to the offline cache.
    pub async fn search(&self, query: &str, target: Option<TargetApp>, limit: usize) -> Result<SearchOutcome> {
        let _ = (query, target, limit);
        todo!("core agent")
    }
    /// Most recently copied items (from the local usage log), newest first.
    pub fn recent(&self, limit: usize) -> Vec<ItemSummary> {
        let _ = limit;
        todo!("core agent")
    }
    /// Path of the cached thumbnail, downloading it if needed.
    pub async fn thumbnail(&self, item: &ItemSummary) -> Result<PathBuf> {
        let _ = item;
        todo!("core agent")
    }
    /// Choose the variant (profiles::choose_variant), materialise it in the
    /// cache and build the clipboard offers.
    pub async fn prepare_copy(&self, item: &ItemSummary, target: TargetApp, sticker: bool) -> Result<PreparedCopy> {
        let _ = (item, target, sticker);
        todo!("core agent")
    }
    /// Queue a usage event and try to flush in the background. Works offline.
    pub fn record_copy(&self, item: &ItemSummary, prepared: &PreparedCopy) -> Result<()> {
        let _ = (item, prepared);
        todo!("core agent")
    }
}
