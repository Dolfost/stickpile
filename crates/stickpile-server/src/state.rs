//! Shared application state handed to every handler, job and page.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use sqlx::SqlitePool;
use stickpile_ai::{Enricher, Vocabulary, ZeroShot};
use stickpile_search::Index;
use tokio::sync::Notify;

use crate::config::Config;
use crate::security::RateLimiters;

/// Loaded AI backend: the enricher plus the zero-shot vocabulary embeddings.
pub struct Ai {
    pub enricher: Arc<dyn Enricher>,
    pub zeroshot: ZeroShot,
}

impl Ai {
    /// Id of the image embedding space (stored next to every embedding).
    /// Switching only the text encoder keeps it, so images need no re-embedding.
    pub fn image_model_id(&self) -> &str {
        self.enricher.image_model_id()
    }
}

/// Lifecycle of the AI backend. Loading CLIP and embedding the vocabulary
/// takes seconds, so it happens in the background after startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AiStatus {
    /// `ai.enabled = false`.
    Off = 0,
    Loading = 1,
    Ready = 2,
    /// The model could not be loaded; search runs without semantics.
    Failed = 3,
}

/// Cheap to clone (one `Arc`).
#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

pub struct Inner {
    pub config: Config,
    pub db: SqlitePool,
    /// In-memory hybrid search index, fed from the database at startup and on
    /// every write ([`crate::index`]).
    pub index: RwLock<Index>,
    /// Set once the AI backend is ready (see [`AppState::ai`]).
    pub ai: OnceLock<Ai>,
    pub ai_status: AtomicU8,
    /// Vibe synonyms for query expansion; available even without a model.
    pub vocab: Vocabulary,
    pub limiters: RateLimiters,
    /// Wakes job workers when a job is enqueued.
    pub jobs_notify: Notify,
    pub tools: stickpile_media::Tools,
    pub ffmpeg_available: bool,
    /// Shared secret of a loopback TCP reverse proxy (`trusted_proxy.secret_file`).
    pub proxy_secret: Option<String>,
    /// Upload/import hold it shared while they look up, write and reference
    /// media; orphan-media GC holds it exclusively.
    pub media_lock: tokio::sync::RwLock<()>,
    /// Query text -> embedding.
    pub text_embeddings: Mutex<LruCache<Arc<[f32]>>>,
}

impl std::ops::Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl AppState {
    pub fn config(&self) -> &Config {
        &self.0.config
    }
    pub fn media_dir(&self) -> PathBuf {
        self.config.media_dir()
    }
    /// The AI backend, once loaded.
    pub fn ai(&self) -> Option<&Ai> {
        self.0.ai.get()
    }
    pub fn ai_status(&self) -> AiStatus {
        match self.0.ai_status.load(Ordering::Acquire) {
            1 => AiStatus::Loading,
            2 => AiStatus::Ready,
            3 => AiStatus::Failed,
            _ => AiStatus::Off,
        }
    }
    pub fn set_ai_status(&self, s: AiStatus) {
        self.0.ai_status.store(s as u8, Ordering::Release);
    }
    /// Image embedding space in use, if AI is loaded.
    pub fn ai_model_id(&self) -> Option<&str> {
        self.ai().map(|a| a.image_model_id())
    }
    pub fn wake_workers(&self) {
        self.jobs_notify.notify_waiters();
        self.jobs_notify.notify_one();
    }
    pub fn media_limits(&self) -> stickpile_media::Limits {
        let l = &self.config.limits;
        stickpile_media::Limits {
            max_bytes: l.max_upload_mb * 1024 * 1024,
            max_pixels: l.max_pixels,
            max_frames: l.max_animation_frames,
            max_video_ms: l.max_video_seconds as u64 * 1000,
        }
    }
}

/// Minimal LRU map keyed by string.
pub struct LruCache<V> {
    cap: usize,
    map: HashMap<String, V>,
    order: VecDeque<String>,
}

impl<V: Clone> LruCache<V> {
    pub fn new(cap: usize) -> Self {
        LruCache {
            cap: cap.max(1),
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }
    pub fn get(&mut self, k: &str) -> Option<V> {
        let v = self.map.get(k)?.clone();
        if let Some(pos) = self.order.iter().position(|x| x == k) {
            let key = self.order.remove(pos).expect("present");
            self.order.push_back(key);
        }
        Some(v)
    }
    pub fn put(&mut self, k: String, v: V) {
        if self.map.insert(k.clone(), v).is_none() {
            self.order.push_back(k);
            while self.order.len() > self.cap {
                if let Some(old) = self.order.pop_front() {
                    self.map.remove(&old);
                }
            }
        }
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::LruCache;

    #[test]
    fn lru_evicts_oldest() {
        let mut c = LruCache::new(2);
        c.put("a".into(), 1);
        c.put("b".into(), 2);
        assert_eq!(c.get("a"), Some(1));
        c.put("c".into(), 3);
        assert_eq!(c.get("b"), None);
        assert_eq!(c.get("a"), Some(1));
        assert_eq!(c.len(), 2);
    }
}
