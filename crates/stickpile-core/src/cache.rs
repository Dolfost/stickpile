//! Offline cache in `<cache_dir>` (SQLite `cache.sqlite3`, WAL, shared safely
//! by the CLI and the resident desktop client):
//!
//! * `items`: [`ItemSummary`] JSON keyed by item id, filled by [`sync`]
//!   (incremental via `GET /items?updated_since=`), searchable offline through
//!   [`OfflineIndex`] (same [`Query`] syntax and ranking code as the server,
//!   metadata only).
//! * `meta`: sync cursor (`server_time`), the bound server URL and the cached
//!   `/me` user, and an item generation counter (cross-process change signal).
//! * `files`: thumbnails (`files/thumb/<media>.webp`) and materialised
//!   variants (`clip/<media>.<variant>/<item name>.<ext>`), with size and last
//!   access, evicted least-recently-used beyond `cache_max_mb`.
//! * `history`: the local copy history (for "recently used").
//! * `usage_queue`: pending usage events (see [`crate::usage`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use stickpile_api::profiles::{choose_variant, ItemShape};
use stickpile_api::{ItemSummary, SearchHit, TargetApp, UsageEvent, User, Visibility};
use stickpile_search::{Doc, Index, Query, SearchOptions, Viewer};

use crate::client::ApiClient;
use crate::{CoreError, Result};

pub const DB_FILE: &str = "cache.sqlite3";
const SCHEMA_VERSION: i64 = 1;
/// Copy history rows kept.
const HISTORY_MAX: i64 = 2000;
/// Default `limit` for `GET /items` pages.
pub const SYNC_PAGE_LIMIT: u32 = 500;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS items (
    id TEXT PRIMARY KEY,
    set_id TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS items_set ON items(set_id);
CREATE TABLE IF NOT EXISTS files (
    path TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    last_access INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS files_lru ON files(last_access);
CREATE TABLE IF NOT EXISTS history (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id TEXT NOT NULL,
    copied_at INTEGER NOT NULL,
    target TEXT,
    variant TEXT,
    item_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS history_item ON history(item_id);
CREATE TABLE IF NOT EXISTS usage_queue (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    json TEXT NOT NULL,
    queued_at INTEGER NOT NULL
);
";

/// Result of [`sync`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// True when there was no cursor yet (full listing, stale items swept).
    pub full: bool,
    pub pages: usize,
    pub upserted: usize,
    pub removed: usize,
    /// Cursor stored for the next sync.
    pub server_time: i64,
    /// Items in the cache afterwards.
    pub total: usize,
}

/// Result of [`Cache::evict`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvictReport {
    pub removed_files: usize,
    pub freed_bytes: u64,
    pub total_bytes: u64,
}

/// The offline cache. `Send + Sync`; the SQLite connection sits behind a mutex
/// that is never held across an `.await`.
pub struct Cache {
    conn: Mutex<Connection>,
    dir: PathBuf,
    max_bytes: u64,
    clock: AtomicI64,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("dir", &self.dir)
            .field("max_bytes", &self.max_bytes)
            .finish()
    }
}

impl Cache {
    /// Open (creating) the cache in `dir` with an LRU cap of `max_mb` MiB for files.
    pub fn open(dir: &Path, max_mb: u64) -> Result<Cache> {
        std::fs::create_dir_all(dir).map_err(|e| CoreError::Cache(format!("creating {}: {e}", dir.display())))?;
        let conn = Connection::open(dir.join(DB_FILE))?;
        conn.busy_timeout(Duration::from_secs(10))?;
        let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(CoreError::Cache(format!(
                "{} was written by a newer stickpile (schema {version}); delete it to rebuild",
                dir.join(DB_FILE).display()
            )));
        }
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(&format!("PRAGMA user_version={SCHEMA_VERSION}"))?;
        Ok(Cache {
            conn: Mutex::new(conn),
            dir: dir.to_path_buf(),
            max_bytes: max_mb.saturating_mul(1024 * 1024),
            clock: AtomicI64::new(0),
        })
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    // -- meta -----------------------------------------------------------------

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .db()
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.db().execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    pub fn meta_delete(&self, key: &str) -> Result<()> {
        self.db().execute("DELETE FROM meta WHERE key = ?1", [key])?;
        Ok(())
    }

    /// Bind the cache to a server URL. If it was bound to a different server,
    /// all account data (items, cursor, user, history, queued usage) is
    /// dropped. Returns true if a reset happened.
    pub fn bind_server(&self, server_url: &str) -> Result<bool> {
        let url = server_url.trim_end_matches('/');
        let previous = self.meta_get("server_url")?;
        let reset = matches!(&previous, Some(p) if p != url);
        if reset {
            tracing::info!(
                from = previous.as_deref().unwrap_or(""),
                to = url,
                "server changed; resetting offline cache"
            );
            self.reset_account_data()?;
        }
        self.meta_set("server_url", url)?;
        Ok(reset)
    }

    /// Cached `/me`.
    pub fn me(&self) -> Result<Option<User>> {
        Ok(self.meta_get("me")?.and_then(|j| serde_json::from_str(&j).ok()))
    }

    /// Store `/me`. If a different user was cached before, account data is
    /// reset first. Returns true if a reset happened.
    pub fn set_me(&self, user: &User) -> Result<bool> {
        let reset = matches!(self.me()?, Some(prev) if prev.id != user.id);
        if reset {
            tracing::info!(user = %user.username, "account changed; resetting offline cache");
            self.reset_account_data()?;
        }
        let json = serde_json::to_string(user).map_err(|e| CoreError::Cache(e.to_string()))?;
        self.meta_set("me", &json)?;
        Ok(reset)
    }

    /// Drop items, sync cursor, cached user, copy history and queued usage.
    /// Cached files stay (they are content-addressed and evicted by LRU).
    pub fn reset_account_data(&self) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        tx.execute("DELETE FROM items", [])?;
        tx.execute("DELETE FROM history", [])?;
        tx.execute("DELETE FROM usage_queue", [])?;
        tx.execute("DELETE FROM meta WHERE key IN ('me', 'updated_since')", [])?;
        bump_generation(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// `server_time` of the last completed sync.
    pub fn sync_cursor(&self) -> Result<Option<i64>> {
        Ok(self.meta_get("updated_since")?.and_then(|v| v.parse().ok()))
    }

    pub fn set_sync_cursor(&self, server_time: Option<i64>) -> Result<()> {
        match server_time {
            Some(t) => self.meta_set("updated_since", &t.to_string()),
            None => self.meta_delete("updated_since"),
        }
    }

    /// Changes whenever cached items change (also across processes).
    pub fn generation(&self) -> Result<u64> {
        Ok(self.meta_get("items_gen")?.and_then(|v| v.parse().ok()).unwrap_or(0))
    }

    // -- items ----------------------------------------------------------------

    /// Upsert items and delete removed ids in one transaction.
    pub fn apply_changes(&self, items: &[ItemSummary], removed: &[String]) -> Result<()> {
        if items.is_empty() && removed.is_empty() {
            return Ok(());
        }
        let mut db = self.db();
        let tx = db.transaction()?;
        {
            let mut up = tx.prepare(
                "INSERT INTO items(id, set_id, updated_at, json) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET set_id = excluded.set_id, updated_at = excluded.updated_at, json = excluded.json",
            )?;
            for it in items {
                let json = serde_json::to_string(it).map_err(|e| CoreError::Cache(e.to_string()))?;
                up.execute(params![it.id, it.set_id, it.updated_at, json])?;
            }
            let mut del = tx.prepare("DELETE FROM items WHERE id = ?1")?;
            let mut del_hist = tx.prepare("DELETE FROM history WHERE item_id = ?1")?;
            for id in removed {
                del.execute([id])?;
                del_hist.execute([id])?;
            }
        }
        bump_generation(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_items(&self, items: &[ItemSummary]) -> Result<()> {
        self.apply_changes(items, &[])
    }

    pub fn remove_items(&self, ids: &[String]) -> Result<()> {
        self.apply_changes(&[], ids)
    }

    /// Delete every cached item whose id is not in `keep`. Returns the count.
    pub fn retain_items(&self, keep: &HashSet<String>) -> Result<usize> {
        let stale: Vec<String> = self.item_ids()?.into_iter().filter(|id| !keep.contains(id)).collect();
        self.remove_items(&stale)?;
        Ok(stale.len())
    }

    pub fn item_ids(&self) -> Result<Vec<String>> {
        let db = self.db();
        let mut st = db.prepare("SELECT id FROM items")?;
        let ids = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    pub fn item(&self, id: &str) -> Result<Option<ItemSummary>> {
        let json: Option<String> = self
            .db()
            .query_row("SELECT json FROM items WHERE id = ?1", [id], |r| r.get(0))
            .optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
    }

    pub fn items(&self) -> Result<Vec<ItemSummary>> {
        let db = self.db();
        let mut st = db.prepare("SELECT json FROM items ORDER BY updated_at DESC, id")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            match serde_json::from_str(&row?) {
                Ok(item) => out.push(item),
                Err(e) => tracing::warn!("skipping unreadable cached item: {e}"),
            }
        }
        Ok(out)
    }

    pub fn item_count(&self) -> Result<usize> {
        Ok(self
            .db()
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get::<_, i64>(0))? as usize)
    }

    /// Build the in-memory offline search index from the cached items.
    pub fn offline_index(&self) -> Result<OfflineIndex> {
        let generation = self.generation()?;
        let me = self.me()?.map(|u| u.username);
        Ok(OfflineIndex::build(self.items()?, me.as_deref(), generation))
    }

    // -- files (LRU) ------------------------------------------------------------

    /// Absolute path of a cache-relative file path.
    pub fn abs(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    fn tick(&self) -> i64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let mut prev = self.clock.load(Ordering::Relaxed);
        loop {
            let next = now.max(prev + 1);
            match self
                .clock
                .compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return next,
                Err(p) => prev = p,
            }
        }
    }

    /// The cached file at `rel` if it is registered and present on disk (and,
    /// with `expect_size`, has that size). Touches it. Stale rows are dropped.
    pub fn lookup_file(&self, rel: &str, expect_size: Option<u64>) -> Result<Option<PathBuf>> {
        let abs = self.abs(rel);
        let row: Option<i64> = self
            .db()
            .query_row("SELECT size FROM files WHERE path = ?1", [rel], |r| r.get(0))
            .optional()?;
        let on_disk = std::fs::metadata(&abs).ok().filter(|m| m.is_file()).map(|m| m.len());
        match (row, on_disk) {
            (Some(_), Some(len)) if expect_size.is_none_or(|s| s == 0 || s == len) => {
                self.touch_file(rel)?;
                Ok(Some(abs))
            }
            (Some(_), _) => {
                self.db().execute("DELETE FROM files WHERE path = ?1", [rel])?;
                Ok(None)
            }
            (None, _) => Ok(None),
        }
    }

    pub fn touch_file(&self, rel: &str) -> Result<()> {
        let t = self.tick();
        self.db()
            .execute("UPDATE files SET last_access = ?2 WHERE path = ?1", params![rel, t])?;
        Ok(())
    }

    /// Register a file written under the cache dir and evict others beyond the cap
    /// (the file just registered is never evicted).
    pub fn register_file(&self, rel: &str) -> Result<EvictReport> {
        let size = std::fs::metadata(self.abs(rel))
            .map_err(|e| CoreError::Cache(format!("{}: {e}", self.abs(rel).display())))?
            .len();
        let t = self.tick();
        self.db().execute(
            "INSERT INTO files(path, size, last_access) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET size = excluded.size, last_access = excluded.last_access",
            params![rel, size as i64, t],
        )?;
        self.evict(Some(rel))
    }

    pub fn files_total_bytes(&self) -> Result<u64> {
        Ok(self
            .db()
            .query_row("SELECT COALESCE(SUM(size), 0) FROM files", [], |r| r.get::<_, i64>(0))? as u64)
    }

    /// Delete least-recently-used files until the total is within the cap.
    pub fn evict(&self, keep: Option<&str>) -> Result<EvictReport> {
        let mut report = EvictReport {
            total_bytes: self.files_total_bytes()?,
            ..Default::default()
        };
        if report.total_bytes <= self.max_bytes {
            return Ok(report);
        }
        let candidates: Vec<(String, u64)> = {
            let db = self.db();
            let mut st = db.prepare("SELECT path, size FROM files ORDER BY last_access ASC")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (rel, size) in candidates {
            if report.total_bytes <= self.max_bytes {
                break;
            }
            if Some(rel.as_str()) == keep {
                continue;
            }
            let abs = self.abs(&rel);
            match std::fs::remove_file(&abs) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!("evicting {}: {e}", abs.display());
                    continue;
                }
            }
            if let Some(parent) = abs.parent() {
                if parent != self.dir && parent.starts_with(self.dir.join("clip")) {
                    let _ = std::fs::remove_dir(parent); // only succeeds when empty
                }
            }
            self.db().execute("DELETE FROM files WHERE path = ?1", [&rel])?;
            report.removed_files += 1;
            report.freed_bytes += size;
            report.total_bytes = report.total_bytes.saturating_sub(size);
        }
        Ok(report)
    }

    // -- copy history -------------------------------------------------------------

    pub fn add_history(&self, item: &ItemSummary, event: &UsageEvent) -> Result<()> {
        let json = serde_json::to_string(item).map_err(|e| CoreError::Cache(e.to_string()))?;
        let target = event.target.map(|t| t.as_str().to_string());
        let variant = event.variant.map(|v| v.as_str().to_string());
        let db = self.db();
        db.execute(
            "INSERT INTO history(item_id, copied_at, target, variant, item_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![item.id, event.copied_at, target, variant, json],
        )?;
        db.execute(
            "DELETE FROM history WHERE seq <= (SELECT MAX(seq) FROM history) - ?1",
            [HISTORY_MAX],
        )?;
        Ok(())
    }

    /// Most recently copied distinct items, newest first. The current cached
    /// summary is preferred over the one stored at copy time.
    pub fn recent(&self, limit: usize) -> Result<Vec<ItemSummary>> {
        let rows: Vec<(String, String, Option<String>)> = {
            let db = self.db();
            let mut st = db.prepare(
                "SELECT h.item_id, h.item_json, i.json FROM history h LEFT JOIN items i ON i.id = h.item_id
                 ORDER BY h.seq DESC",
            )?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (id, at_copy, current) in rows {
            if out.len() >= limit {
                break;
            }
            if !seen.insert(id) {
                continue;
            }
            if let Some(item) = current.or(Some(at_copy)).and_then(|j| serde_json::from_str(&j).ok()) {
                out.push(item);
            }
        }
        Ok(out)
    }

    /// Distinct items by how often they were copied on this device, most
    /// first (ties: most recently copied first). Summaries as in [`Cache::recent`].
    pub fn most_copied(&self, limit: usize) -> Result<Vec<ItemSummary>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT i.json, (SELECT item_json FROM history WHERE seq = g.last)
             FROM (SELECT item_id, COUNT(*) AS n, MAX(seq) AS last FROM history GROUP BY item_id) g
             LEFT JOIN items i ON i.id = g.item_id
             ORDER BY g.n DESC, g.last DESC LIMIT ?1",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = st.query_map([limit], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (current, at_copy) = row?;
            if let Some(item) = current.or(Some(at_copy)).and_then(|j| serde_json::from_str(&j).ok()) {
                out.push(item);
            }
        }
        Ok(out)
    }

    // -- usage queue ----------------------------------------------------------------

    /// Queue an event; a duplicate id is ignored.
    pub fn queue_push(&self, event: &UsageEvent) -> Result<()> {
        let json = serde_json::to_string(event).map_err(|e| CoreError::Cache(e.to_string()))?;
        self.db().execute(
            "INSERT OR IGNORE INTO usage_queue(id, json, queued_at) VALUES (?1, ?2, ?3)",
            params![event.id, json, crate::now_unix()],
        )?;
        Ok(())
    }

    /// Oldest `limit` queued events.
    pub fn queue_peek(&self, limit: usize) -> Result<Vec<UsageEvent>> {
        let db = self.db();
        let rows: Vec<(i64, String)> = {
            let mut st = db.prepare("SELECT seq, json FROM usage_queue ORDER BY seq LIMIT ?1")?;
            let rows = st.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut out = Vec::new();
        for (seq, json) in rows {
            match serde_json::from_str(&json) {
                Ok(ev) => out.push(ev),
                Err(e) => {
                    tracing::warn!("dropping unreadable queued usage event: {e}");
                    db.execute("DELETE FROM usage_queue WHERE seq = ?1", [seq])?;
                }
            }
        }
        Ok(out)
    }

    pub fn queue_remove(&self, ids: &[String]) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        {
            let mut st = tx.prepare("DELETE FROM usage_queue WHERE id = ?1")?;
            for id in ids {
                st.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn queue_len(&self) -> Result<usize> {
        Ok(self
            .db()
            .query_row("SELECT COUNT(*) FROM usage_queue", [], |r| r.get::<_, i64>(0))? as usize)
    }
}

fn bump_generation(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('items_gen', '1')
         ON CONFLICT(key) DO UPDATE SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)",
        [],
    )?;
    Ok(())
}

/// Pull item changes into the cache: `GET /items?updated_since=<cursor>`,
/// following `next_cursor` pages, applying `removed`. Without a cursor (first
/// sync, or after a reset) it is a full listing and cached items that were not
/// listed are deleted. The cursor is advanced only after the last page.
pub async fn sync(cache: &Cache, client: &ApiClient, page_limit: u32) -> Result<SyncReport> {
    let since = cache.sync_cursor()?;
    let full = since.is_none();
    let mut report = SyncReport {
        full,
        ..Default::default()
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut cursor: Option<String> = None;
    let mut server_time: Option<i64> = None;
    loop {
        let page = client.items(since, cursor.as_deref(), Some(page_limit)).await?;
        report.pages += 1;
        // The first page's clock is the safe lower bound for the next sync.
        server_time.get_or_insert(page.server_time);
        if full {
            seen.extend(page.items.iter().map(|i| i.id.clone()));
        }
        report.upserted += page.items.len();
        report.removed += page.removed.len();
        cache.apply_changes(&page.items, &page.removed)?;
        match page.next_cursor {
            Some(next) if Some(&next) != cursor.as_ref() && !(page.items.is_empty() && page.removed.is_empty()) => {
                cursor = Some(next)
            }
            _ => break,
        }
    }
    if full {
        report.removed += cache.retain_items(&seen)?;
    }
    let t = server_time.unwrap_or_else(crate::now_unix);
    cache.set_sync_cursor(Some(t))?;
    report.server_time = t;
    report.total = cache.item_count()?;
    Ok(report)
}

/// In-memory index over the cached items for offline search.
pub struct OfflineIndex {
    generation: u64,
    index: Index,
    items: HashMap<String, ItemSummary>,
    me: Option<String>,
}

impl std::fmt::Debug for OfflineIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OfflineIndex")
            .field("generation", &self.generation)
            .field("items", &self.items.len())
            .finish()
    }
}

/// Search document for a cached item. `ItemSummary` carries no owner id or set
/// visibility: the owner username stands in for the id, own items count as
/// private and everything else in the cache is public (`GET /items` only lists
/// own and public items).
pub fn doc_for(item: &ItemSummary, me: Option<&str>) -> Doc {
    let mine = me.is_some_and(|m| m == item.owner_username);
    Doc {
        item_id: item.id.clone(),
        set_id: item.set_id.clone(),
        set_slug: item.set_slug.clone(),
        set_name: item.set_name.clone(),
        owner_id: item.owner_username.clone(),
        owner_username: item.owner_username.clone(),
        visibility: if mine { Visibility::Private } else { Visibility::Public },
        name: item.name.clone(),
        keywords: item.keywords.clone(),
        emojis: item.emojis.clone(),
        vibe: item.vibe.clone(),
        tags: item.tags.clone(),
        kind: item.kind,
        format: item.format,
        quality: item.quality,
        likes: item.likes,
        copies: item.copies,
        media_id: item.media_id.clone(),
        embedding: None,
        updated_at: item.updated_at,
    }
}

/// Why `item` cannot be sent to `target`, if it cannot.
pub fn incompatibility(item: &ItemSummary, target: TargetApp) -> Option<String> {
    let shape = ItemShape {
        kind: item.kind,
        format: item.format,
        duration_ms: None,
        variants: &item.variants,
    };
    choose_variant(target, &shape, false).err().map(|e| e.0)
}

impl OfflineIndex {
    pub fn build(items: Vec<ItemSummary>, me: Option<&str>, generation: u64) -> OfflineIndex {
        let mut index = Index::new();
        for item in &items {
            index.upsert(doc_for(item, me));
        }
        let items = items.into_iter().map(|i| (i.id.clone(), i)).collect();
        OfflineIndex {
            generation,
            index,
            items,
            me: me.map(str::to_string),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&ItemSummary> {
        self.items.get(id)
    }

    /// Search with the server's query syntax (no image-content matching; the
    /// liked set is unknown offline, so `is:liked` matches nothing).
    pub fn search(&self, query: &str, target: Option<TargetApp>, limit: usize) -> Vec<SearchHit> {
        let q = Query::parse(query);
        let viewer = Viewer {
            user_id: self.me.clone().unwrap_or_default(),
            liked: HashSet::new(),
        };
        let opts = SearchOptions {
            limit,
            offset: 0,
            ..Default::default()
        };
        let outcome = self.index.search(&q, &viewer, &opts);
        outcome
            .hits
            .into_iter()
            .filter_map(|h| {
                let item = self.items.get(&h.item_id)?.clone();
                let incompatible = target.and_then(|t| incompatibility(&item, t));
                Some(SearchHit {
                    item,
                    score: h.score,
                    matched: h.matched.iter().map(|m| m.to_string()).collect(),
                    incompatible,
                })
            })
            .collect()
    }
}

/// A file-name-safe single path component for server-provided ids.
pub(crate) fn safe_component(s: &str) -> String {
    if !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        s.to_string()
    } else {
        use sha2::Digest;
        hex::encode(&sha2::Sha256::digest(s.as_bytes())[..16])
    }
}

/// Shared handle type used by the session.
pub type SharedCache = Arc<Cache>;
