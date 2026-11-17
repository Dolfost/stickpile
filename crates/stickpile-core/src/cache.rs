//! Offline cache in `<cache_dir>` (SQLite `cache.sqlite3`, WAL, shared safely
//! by the CLI and the resident desktop client):
//!
//! * `items`: [`ItemSummary`] JSON keyed by item id, filled by [`sync`]
//!   (incremental via `GET /items?updated_since=`), searchable offline through
//!   [`OfflineIndex`] (same [`Query`] syntax and ranking code as the server,
//!   metadata only).
//! * `meta`: sync cursor (`server_time`), the bound server URL and the cached
//!   `/me` user, an item generation counter (cross-process change signal),
//!   and the format of the cached items ([`ITEM_FORMAT`]).
//! * `files`: thumbnails (`files/thumb/<media>.webp`) and materialised
//!   variants (`clip/<media>.<variant>/<item name>.<ext>`), with size and last
//!   access, evicted least-recently-used beyond `cache_max_mb`.
//! * `history`: the local copy history (for "recently used").
//! * `usage_queue`: pending usage events (see [`crate::usage`]).
//! * Keeping offline ([`crate::offline`], docs/offline.md): `files.kept`
//!   marks the files kept on the device (1), which the size cap never
//!   evicts and "Clear cache" never removes, and those a reset is deleting
//!   (2, [`Cache::offline_reset`]); `offline_keeps` holds what the user
//!   chose to keep, `offline_failures` the downloads that failed and why,
//!   `offline_claims` which process is downloading a file, `offline_peers`
//!   the offline options of the processes on the cache. All of it is
//!   additive ([`OFFLINE_FORMAT`]): a client up to 0.8.0 opens and uses the
//!   cache as before.

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
use crate::offline::{OfflineFailure, OfflineUsage, Scope};
use crate::{CoreError, Result};

pub const DB_FILE: &str = "cache.sqlite3";
const SCHEMA_VERSION: i64 = 1;
/// What the cached [`ItemSummary`] JSON holds, kept in `meta` as
/// `item_format` (absent: 1). [`sync`] lists everything again, once, when
/// the cache's items are older, and then records this. The table schema
/// does not change (only the index `items_without_text` is new), so an
/// older client can still open the cache; what it writes without `text`
/// [`sync`] lists again ([`Cache::has_items_without_text`]).
///
/// - 1: up to stickpile 0.7.0.
/// - 2: `text`, the words in the picture.
pub const ITEM_FORMAT: i64 = 2;
/// What the cache holds for keeping offline, in `meta` as `offline_format`
/// (absent: nothing written yet). The schema version stays 1: the tables
/// are new and `files.kept` is a column added with a default, so a client
/// up to 0.8.0 opens the cache, ignores them and adds files
/// unflagged (`kept` 0). What such a client does to kept files (evict them
/// to make room, clear them, register them again) heals on the next
/// derivation of the offline plan, which flags the wanted files again and
/// downloads the missing ones ([`Cache::offline_mark`]).
///
/// - 1: `files.kept`, `offline_keeps`, `offline_failures`, `offline_claims`,
///   `offline_peers`.
pub const OFFLINE_FORMAT: i64 = 1;
/// `files.last_access` of a kept file. A client up to 0.8.0 evicts by
/// `last_access` alone, so this makes it take every other file first; a kept
/// file goes back to the current time when it is released.
const KEPT_ACCESS: i64 = i64::MAX;
/// `files.kept` of a file a reset is deleting: kept before
/// [`Cache::offline_reset`] (or before a reset by a process that died
/// before it deleted its files). Neither kept nor evicted; the next open,
/// refresh or reset deletes it ([`Cache::offline_finish_reset`]).
const CONDEMNED: i64 = 2;
/// A process's offline options in `offline_peers` count this long after it
/// last brought the kept files in line with them ([`Cache::offline_peer_note`]).
pub const PEER_TTL_SECS: i64 = 24 * 3600;
/// A download claim older than this belongs to a process that died (the
/// downloader renews its claims every 30 s).
pub const CLAIM_STALE_SECS: i64 = 120;
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
-- Items written without `text` (by a client up to 0.7.0 sharing the cache),
-- so that `sync` finds them in one indexed query.
CREATE INDEX IF NOT EXISTS items_without_text ON items(id) WHERE json_extract(json, '$.text') IS NULL;
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
-- Keeping offline (OFFLINE_FORMAT 1). `files.kept` is added in `Cache::open`.
CREATE TABLE IF NOT EXISTS offline_keeps (
    scope TEXT NOT NULL,
    id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (scope, id)
);
CREATE TABLE IF NOT EXISTS offline_failures (
    path TEXT PRIMARY KEY,
    item_id TEXT NOT NULL,
    error TEXT NOT NULL,
    attempts INTEGER NOT NULL,
    at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS offline_claims (
    path TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS offline_peers (
    id TEXT PRIMARY KEY,
    at INTEGER NOT NULL,
    json TEXT NOT NULL
);
";

/// Add `files.kept` when the cache lacks it (written by a client up to
/// 0.8.0, or new). In an immediate transaction, so two processes opening the
/// cache at once do not both add it.
fn ensure_kept_column(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let has: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('files') WHERE name = 'kept')",
        [],
        |r| r.get(0),
    )?;
    if !has {
        tx.execute_batch("ALTER TABLE files ADD COLUMN kept INTEGER NOT NULL DEFAULT 0;")?;
    }
    tx.execute_batch("CREATE INDEX IF NOT EXISTS files_kept ON files(kept) WHERE kept = 1;")?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('offline_format', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value
         WHERE CAST(meta.value AS INTEGER) < CAST(excluded.value AS INTEGER)",
        [OFFLINE_FORMAT.to_string()],
    )?;
    // The epoch the kept files belong to (see `condemn_stale`); a cache
    // without it starts at the current one.
    tx.execute(
        "INSERT OR IGNORE INTO meta(key, value)
         SELECT 'offline_kept_epoch', COALESCE((SELECT value FROM meta WHERE key = 'offline_epoch'), '0')",
        [],
    )?;
    tx.commit()
}

/// `meta` value `key` as a number (absent or unreadable: 0).
fn meta_u64(conn: &Connection, key: &str) -> rusqlite::Result<u64> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

/// The kept files belong to the epoch in `offline_kept_epoch`. When the
/// epoch moved past it and the kept files were not condemned (a reset by a
/// process that died in it), condemn them now, before anything is flagged
/// under the new epoch: they were kept before the reset and are to be
/// deleted ([`Cache::offline_finish_reset`]). In the caller's immediate
/// transaction. Returns the epoch.
fn condemn_stale(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<u64> {
    let epoch = meta_u64(tx, "offline_epoch")?;
    if meta_u64(tx, "offline_kept_epoch")? != epoch {
        tx.execute("UPDATE files SET kept = ?1 WHERE kept = 1", [CONDEMNED])?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('offline_kept_epoch', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [epoch.to_string()],
        )?;
    }
    Ok(epoch)
}

/// The file a downloader's part file `<file>.part.<owner>` belongs to: the
/// owner is one token (letters, digits, `-`, `_`), so that a file whose own
/// name holds `.part.` is not taken for one. `None` for other names.
pub(crate) fn part_file_target(name: &str) -> Option<&str> {
    let (stem, owner) = name.rsplit_once(".part.")?;
    let token = !owner.is_empty() && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    (token && !stem.is_empty()).then_some(stem)
}

/// The cached account as far as this version reads it
/// ([`Cache::cached_account`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CachedAccount {
    /// No account is cached: signed out, or not signed in yet.
    None,
    /// The account's id and username.
    Known { id: String, username: String },
    /// An account is cached, but this version cannot read its id.
    Unreadable,
}

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

/// Result of [`Cache::evict`] and [`Cache::clear_files`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvictReport {
    pub removed_files: usize,
    pub freed_bytes: u64,
    /// Bytes of the files that are not kept, afterwards.
    pub total_bytes: u64,
    /// Kept files, which stay (docs/offline.md): how many, afterwards.
    pub kept_files: usize,
    /// Their bytes.
    pub kept_bytes: u64,
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
        let mut conn = Connection::open(dir.join(DB_FILE))?;
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
        ensure_kept_column(&mut conn)?;
        conn.execute_batch(&format!("PRAGMA user_version={SCHEMA_VERSION}"))?;
        let cache = Cache {
            conn: Mutex::new(conn),
            dir: dir.to_path_buf(),
            max_bytes: max_mb.saturating_mul(1024 * 1024),
            clock: AtomicI64::new(0),
        };
        // What a reset that did not get to delete its files left.
        cache.offline_finish_reset()?;
        Ok(cache)
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

    /// The cached account's id and username, read without the whole
    /// [`User`]: a newer client sharing the cache may have stored values
    /// this version cannot read (a theme or a role added later), which make
    /// [`Cache::me`] `None` although an account is signed in.
    pub fn cached_account(&self) -> Result<CachedAccount> {
        let Some(json) = self.meta_get("me")? else {
            return Ok(CachedAccount::None);
        };
        let value: Option<serde_json::Value> = serde_json::from_str(&json).ok();
        let field = |name: &str| {
            value
                .as_ref()
                .and_then(|v| v.get(name))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        Ok(match (field("id"), field("username")) {
            (Some(id), Some(username)) => CachedAccount::Known { id, username },
            _ => CachedAccount::Unreadable,
        })
    }

    /// Store `/me`. If a different user was cached before, account data is
    /// reset first. Returns true if a reset happened. Before that, offline
    /// data a client that does not know it (up to 0.8.0) left behind when
    /// it signed out is removed ([`Cache::offline_check_owner`]), so that a
    /// login does not make it pass for the new account's.
    pub fn set_me(&self, user: &User) -> Result<bool> {
        self.offline_check_owner()?;
        let reset = matches!(self.cached_account()?, CachedAccount::Known { id, .. } if id != user.id);
        if reset {
            tracing::info!(user = %user.username, "account changed; resetting offline cache");
            self.reset_account_data()?;
        }
        let json = serde_json::to_string(user).map_err(|e| CoreError::Cache(e.to_string()))?;
        self.meta_set("me", &json)?;
        Ok(reset)
    }

    /// Drop items, sync cursor, cached user, copy history and queued usage,
    /// and everything kept offline: the kept files are deleted and what to
    /// keep is forgotten ([`Cache::offline_reset`]). The other cached files
    /// stay (they are content-addressed and evicted by LRU).
    pub fn reset_account_data(&self) -> Result<()> {
        {
            let mut db = self.db();
            let tx = db.transaction()?;
            tx.execute("DELETE FROM items", [])?;
            tx.execute("DELETE FROM history", [])?;
            tx.execute("DELETE FROM usage_queue", [])?;
            tx.execute("DELETE FROM meta WHERE key IN ('me', 'updated_since')", [])?;
            bump_generation(&tx)?;
            tx.commit()?;
        }
        self.offline_reset()?;
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

    /// The [`ITEM_FORMAT`] the cached items were listed in.
    pub fn item_format(&self) -> Result<i64> {
        Ok(self.meta_get("item_format")?.and_then(|v| v.parse().ok()).unwrap_or(1))
    }

    pub fn set_item_format(&self, format: i64) -> Result<()> {
        self.meta_set("item_format", &format.to_string())
    }

    /// Whether a cached item lacks `text`: a client up to 0.7.0 sharing
    /// the cache wrote it (its sync, or an item it got back), and may have
    /// moved the sync cursor past the item's change. One indexed query
    /// (`items_without_text`).
    pub fn has_items_without_text(&self) -> Result<bool> {
        Ok(self.db().query_row(
            "SELECT EXISTS (SELECT 1 FROM items WHERE json_extract(json, '$.text') IS NULL)",
            [],
            |r| r.get(0),
        )?)
    }

    /// Changes whenever cached items change (also across processes).
    pub fn generation(&self) -> Result<u64> {
        Ok(self.meta_get("items_gen")?.and_then(|v| v.parse().ok()).unwrap_or(0))
    }

    // -- items ----------------------------------------------------------------

    /// Upsert items and delete removed ids in one transaction. Returns
    /// whether the cached items changed (an item new or different, or one
    /// removed): only then does [`Cache::generation`] move, so an item the
    /// server sends again as it was changes nothing.
    pub fn apply_changes(&self, items: &[ItemSummary], removed: &[String]) -> Result<bool> {
        if items.is_empty() && removed.is_empty() {
            return Ok(false);
        }
        let mut db = self.db();
        let tx = db.transaction()?;
        let mut changed = 0;
        {
            let mut up = tx.prepare(
                "INSERT INTO items(id, set_id, updated_at, json) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET set_id = excluded.set_id, updated_at = excluded.updated_at, json = excluded.json
                 WHERE items.json IS NOT excluded.json",
            )?;
            for it in items {
                let json = serde_json::to_string(it).map_err(|e| CoreError::Cache(e.to_string()))?;
                changed += up.execute(params![it.id, it.set_id, it.updated_at, json])?;
            }
            let mut del = tx.prepare("DELETE FROM items WHERE id = ?1")?;
            let mut del_hist = tx.prepare("DELETE FROM history WHERE item_id = ?1")?;
            for id in removed {
                changed += del.execute([id])?;
                del_hist.execute([id])?;
            }
        }
        if changed > 0 {
            bump_generation(&tx)?;
        }
        tx.commit()?;
        Ok(changed > 0)
    }

    pub fn upsert_items(&self, items: &[ItemSummary]) -> Result<()> {
        self.apply_changes(items, &[]).map(drop)
    }

    pub fn remove_items(&self, ids: &[String]) -> Result<()> {
        self.apply_changes(&[], ids).map(drop)
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

    /// The `media_id` of every cached item, also of those this version
    /// cannot read as an [`ItemSummary`] (a newer client stored a rendition
    /// or kind this version does not know): what decides whether a kept
    /// file's item is gone.
    pub fn item_media(&self) -> Result<HashSet<String>> {
        let db = self.db();
        let mut st = db.prepare("SELECT DISTINCT json_extract(json, '$.media_id') FROM items")?;
        let rows = st.query_map([], |r| r.get::<_, Option<String>>(0))?;
        let mut out = HashSet::new();
        for row in rows {
            if let Some(m) = row? {
                out.insert(m);
            }
        }
        Ok(out)
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
        match row {
            Some(_) if on_disk(&abs, expect_size) => {
                self.touch_file(rel)?;
                Ok(Some(abs))
            }
            Some(_) => {
                self.db().execute("DELETE FROM files WHERE path = ?1", [rel])?;
                Ok(None)
            }
            None => Ok(None),
        }
    }

    /// Mark `rel` as used now (least recently used goes first). A kept file
    /// keeps its place at the end (`last_access` at its largest).
    pub fn touch_file(&self, rel: &str) -> Result<()> {
        let t = self.tick();
        self.db().execute(
            "UPDATE files SET last_access = ?2 WHERE path = ?1 AND kept = 0",
            params![rel, t],
        )?;
        Ok(())
    }

    /// Register a file written under the cache dir and evict others beyond the cap
    /// (the file just registered is never evicted, nor is a kept file). A
    /// file registered again keeps its `kept` flag.
    pub fn register_file(&self, rel: &str) -> Result<EvictReport> {
        let size = std::fs::metadata(self.abs(rel))
            .map_err(|e| CoreError::Cache(format!("{}: {e}", self.abs(rel).display())))?
            .len();
        let t = self.tick();
        self.db().execute(
            "INSERT INTO files(path, size, last_access) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET size = excluded.size,
                 last_access = CASE WHEN files.kept = 1 THEN files.last_access ELSE excluded.last_access END",
            params![rel, size as i64, t],
        )?;
        self.evict(Some(rel))
    }

    /// Bytes of every registered file, kept ones included.
    pub fn files_total_bytes(&self) -> Result<u64> {
        Ok(self
            .db()
            .query_row("SELECT COALESCE(SUM(size), 0) FROM files", [], |r| r.get::<_, i64>(0))? as u64)
    }

    /// Bytes of the files that are not kept: what `cache_max_mb` caps.
    pub fn unkept_bytes(&self) -> Result<u64> {
        Ok(self
            .db()
            .query_row("SELECT COALESCE(SUM(size), 0) FROM files WHERE kept = 0", [], |r| {
                r.get::<_, i64>(0)
            })? as u64)
    }

    /// Delete least-recently-used files until the files that are not kept
    /// are within the cap. Kept files are never deleted to make room, and
    /// do not count against the cap (`offline.max_mb` bounds them).
    pub fn evict(&self, keep: Option<&str>) -> Result<EvictReport> {
        self.evict_to(self.max_bytes, keep)
    }

    /// "Clear cache": delete every cached file that is not kept
    /// (thumbnails and copied renditions). The item metadata, and so the
    /// offline search, stays, and so do the kept files: the report says how
    /// many ([`EvictReport::kept_files`]). [`Cache::offline_reset`] removes
    /// those.
    pub fn clear_files(&self) -> Result<EvictReport> {
        let report = self.evict_to(0, None)?;
        self.sweep_part_files(None, CLAIM_STALE_SECS);
        Ok(report)
    }

    /// Delete least-recently-used files that are not kept, except `keep`,
    /// until they are within `limit` bytes.
    fn evict_to(&self, limit: u64, keep: Option<&str>) -> Result<EvictReport> {
        let mut report = EvictReport {
            total_bytes: self.unkept_bytes()?,
            ..Default::default()
        };
        if report.total_bytes > limit {
            let candidates: Vec<(String, u64)> = {
                let db = self.db();
                let mut st = db.prepare("SELECT path, size FROM files WHERE kept = 0 ORDER BY last_access ASC")?;
                let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (rel, size) in candidates {
                if report.total_bytes <= limit {
                    break;
                }
                if Some(rel.as_str()) == keep {
                    continue;
                }
                // Only while it is still not kept (another process may have
                // flagged it since the list was read).
                if !self.remove_file_where(&rel, "kept = 0")? {
                    continue;
                }
                report.removed_files += 1;
                report.freed_bytes += size;
                report.total_bytes = report.total_bytes.saturating_sub(size);
            }
        }
        let (files, bytes) = self.db().query_row(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM files WHERE kept = 1",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )?;
        report.kept_files = files as usize;
        report.kept_bytes = bytes as u64;
        Ok(report)
    }

    /// Delete the cached file `rel` if its row matches `cond` (SQL on
    /// `files`), pruning its `clip/` directory when it is left empty, and
    /// then its row. The check, the file's deletion and the row's go
    /// together in one immediate transaction, so a file another process
    /// flags as kept meanwhile is not deleted; when the file cannot be
    /// deleted its row stays, and a later clear, eviction or reset tries
    /// again. True only when the file is gone.
    fn remove_file_where(&self, rel: &str, cond: &str) -> Result<bool> {
        let mut db = self.db();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let matches: bool = tx.query_row(
            &format!("SELECT EXISTS (SELECT 1 FROM files WHERE path = ?1 AND {cond})"),
            [rel],
            |r| r.get(0),
        )?;
        if !matches || !self.remove_cache_file(rel) {
            return Ok(false);
        }
        tx.execute("DELETE FROM files WHERE path = ?1", [rel])?;
        tx.commit()?;
        Ok(true)
    }

    /// Delete a file under the cache dir (a missing one counts as deleted)
    /// and its `clip/` directory when it is left empty.
    fn remove_cache_file(&self, rel: &str) -> bool {
        let abs = self.abs(rel);
        match std::fs::remove_file(&abs) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!("deleting {}: {e}", abs.display());
                return false;
            }
        }
        if let Some(parent) = abs.parent() {
            if parent != self.dir && parent.starts_with(self.dir.join("clip")) {
                let _ = std::fs::remove_dir(parent); // only succeeds when empty
            }
        }
        true
    }

    // -- keeping offline (crate::offline) ----------------------------------------

    /// Keep `scope` on this device. False when it was kept already.
    pub fn offline_keep(&self, scope: &Scope) -> Result<bool> {
        let (kind, id) = scope.key();
        let n = self.db().execute(
            "INSERT OR IGNORE INTO offline_keeps(scope, id, created_at) VALUES (?1, ?2, ?3)",
            params![kind, id, crate::now_unix()],
        )?;
        Ok(n > 0)
    }

    /// Stop keeping `scope`. False when it was not kept. Its files stay
    /// until the next [`Cache::offline_mark`] releases them to the LRU.
    pub fn offline_release(&self, scope: &Scope) -> Result<bool> {
        let (kind, id) = scope.key();
        let n = self.db().execute(
            "DELETE FROM offline_keeps WHERE scope = ?1 AND id = ?2",
            params![kind, id],
        )?;
        Ok(n > 0)
    }

    /// What the user keeps, oldest choice first, with when it was chosen
    /// (unix seconds). Rows this version cannot read are left out.
    pub fn offline_keeps(&self) -> Result<Vec<(Scope, i64)>> {
        let db = self.db();
        let mut st = db.prepare("SELECT scope, id, created_at FROM offline_keeps ORDER BY created_at, scope, id")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (kind, id, at) = row?;
            if let Some(scope) = Scope::from_key(&kind, &id) {
                out.push((scope, at));
            }
        }
        Ok(out)
    }

    /// Counts against [`Cache::offline_reset`]: a download started before a
    /// reset must not register its file after it.
    pub fn offline_epoch(&self) -> Result<u64> {
        Ok(self
            .meta_get("offline_epoch")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    /// Every registered file: path -> (size, kept). A file a reset is
    /// deleting does not count as kept.
    pub fn file_index(&self) -> Result<HashMap<String, (u64, bool)>> {
        let db = self.db();
        let mut st = db.prepare("SELECT path, size, kept FROM files")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)? == 1),
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }

    /// Whether the cached file `rel` is registered and on disk (with
    /// `expect_size`, of that size; 0 or `None`: any). Unlike
    /// [`Cache::lookup_file`] it neither touches nor drops anything.
    pub fn file_present(&self, rel: &str, expect_size: Option<u64>) -> Result<bool> {
        let row: Option<i64> = self
            .db()
            .query_row("SELECT size FROM files WHERE path = ?1", [rel], |r| r.get(0))
            .optional()?;
        Ok(row.is_some() && on_disk(&self.abs(rel), expect_size))
    }

    /// Bring the `kept` flags in line with the offline plan made under
    /// `epoch` ([`Cache::offline_epoch`] when its snapshot was read): the
    /// registered files in `flag` are flagged (and moved behind every other
    /// file in LRU order: `last_access` at its largest), the other kept
    /// files in `release` go back to ordinary least-recently-used life (as
    /// used now), and the kept files in `delete` (of items a sync removed)
    /// are deleted. Failures of files not in `flag` are forgotten. `None`, and
    /// nothing changed, when the epoch moved meanwhile (a reset: the plan
    /// is out of date). The flags in one immediate transaction.
    pub fn offline_mark(
        &self,
        epoch: u64,
        flag: &HashSet<String>,
        release: &HashSet<String>,
        delete: &HashSet<String>,
    ) -> Result<Option<MarkReport>> {
        let mut report = MarkReport::default();
        let now = self.tick();
        {
            let mut db = self.db();
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if condemn_stale(&tx)? != epoch {
                return Ok(None);
            }
            tx.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS offline_flag (path TEXT PRIMARY KEY);
                 CREATE TEMP TABLE IF NOT EXISTS offline_release (path TEXT PRIMARY KEY);
                 DELETE FROM temp.offline_flag; DELETE FROM temp.offline_release;",
            )?;
            {
                let mut ins = tx.prepare("INSERT OR IGNORE INTO temp.offline_flag(path) VALUES (?1)")?;
                for rel in flag {
                    ins.execute([rel])?;
                }
                let mut ins = tx.prepare("INSERT OR IGNORE INTO temp.offline_release(path) VALUES (?1)")?;
                for rel in release.iter().filter(|r| !flag.contains(*r) && !delete.contains(*r)) {
                    ins.execute([rel])?;
                }
            }
            report.released = tx.execute(
                "UPDATE files SET kept = 0, last_access = ?1
                 WHERE kept = 1 AND path IN (SELECT path FROM temp.offline_release)",
                [now],
            )?;
            report.flagged = tx.execute(
                "UPDATE files SET kept = 1, last_access = ?1
                 WHERE path IN (SELECT path FROM temp.offline_flag)
                   AND (kept = 0 OR (kept = 1 AND last_access != ?1))",
                [KEPT_ACCESS],
            )?;
            tx.execute(
                "DELETE FROM offline_failures WHERE path NOT IN (SELECT path FROM temp.offline_flag)",
                [],
            )?;
            tx.execute_batch("DELETE FROM temp.offline_flag; DELETE FROM temp.offline_release;")?;
            tx.commit()?;
        }
        for rel in delete {
            if self.remove_file_where(rel, "kept = 1")? {
                report.deleted += 1;
            }
        }
        Ok(Some(report))
    }

    /// Register a file that is in place (a copy fetched it, or another
    /// process) as kept by the offline downloader. False, and nothing
    /// changed, when [`Cache::offline_reset`] ran since the download started
    /// (`epoch` is [`Cache::offline_epoch`] from then): the file is left
    /// alone, it may be kept under the new epoch.
    pub fn register_kept_file(&self, rel: &str, epoch: u64) -> Result<bool> {
        let size = std::fs::metadata(self.abs(rel))
            .map_err(|e| CoreError::Cache(format!("{}: {e}", self.abs(rel).display())))?
            .len();
        let mut db = self.db();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if condemn_stale(&tx)? != epoch {
            return Ok(false);
        }
        flag_kept(&tx, rel, size)?;
        tx.commit()?;
        Ok(true)
    }

    /// Put a file the offline downloader fetched into place and register it
    /// as kept: `part` is renamed to `rel` in the same immediate transaction
    /// that checks `epoch` ([`Cache::offline_epoch`] when the download
    /// started), so a reset either comes first (the epoch moved: only `part`
    /// is deleted, and false) or after (it finds the file kept and deletes
    /// it). Nothing is overwritten by a download a reset made stale.
    pub fn install_kept_file(&self, part: &Path, rel: &str, epoch: u64) -> Result<bool> {
        let dest = self.abs(rel);
        let mut db = self.db();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if condemn_stale(&tx)? != epoch {
            drop(tx);
            let _ = std::fs::remove_file(part);
            return Ok(false);
        }
        std::fs::rename(part, &dest)?;
        let size = std::fs::metadata(&dest)?.len();
        flag_kept(&tx, rel, size)?;
        tx.commit()?;
        Ok(true)
    }

    /// Note that downloading `rel` (a file of `item_id`) failed `attempts`
    /// more times with `error`; returns the attempts so far.
    pub fn offline_fail(&self, rel: &str, item_id: &str, error: &str, attempts: u32) -> Result<u32> {
        let db = self.db();
        db.execute(
            "INSERT INTO offline_failures(path, item_id, error, attempts, at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET item_id = excluded.item_id, error = excluded.error,
                 attempts = offline_failures.attempts + excluded.attempts, at = excluded.at",
            params![rel, item_id, error, attempts, crate::now_unix()],
        )?;
        Ok(
            db.query_row("SELECT attempts FROM offline_failures WHERE path = ?1", [rel], |r| {
                r.get::<_, u32>(0)
            })?,
        )
    }

    /// Downloads that failed and why, most recent first.
    pub fn offline_failures(&self) -> Result<Vec<OfflineFailure>> {
        let db = self.db();
        let mut st =
            db.prepare("SELECT path, item_id, error, attempts, at FROM offline_failures ORDER BY at DESC, path")?;
        let rows = st.query_map([], |r| {
            Ok(OfflineFailure {
                path: r.get(0)?,
                item_id: r.get(1)?,
                error: r.get(2)?,
                attempts: r.get(3)?,
                at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Claim `rel` for downloading by `owner`: true when it was free, held
    /// by `owner` already, or its claim is older than `stale_secs` (the
    /// holder died). Another process skips a fresh claim.
    pub fn offline_claim(&self, rel: &str, owner: &str, stale_secs: i64) -> Result<bool> {
        let now = crate::now_unix();
        let n = self.db().execute(
            "INSERT INTO offline_claims(path, owner, at) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET owner = excluded.owner, at = excluded.at
             WHERE offline_claims.owner = excluded.owner OR offline_claims.at <= ?4",
            params![rel, owner, now, now - stale_secs],
        )?;
        Ok(n > 0)
    }

    /// Whether another owner holds a fresh claim on `rel`.
    pub fn offline_claimed_by_other(&self, rel: &str, owner: &str, stale_secs: i64) -> Result<bool> {
        Ok(self.db().query_row(
            "SELECT EXISTS (SELECT 1 FROM offline_claims WHERE path = ?1 AND owner != ?2 AND at > ?3)",
            params![rel, owner, crate::now_unix() - stale_secs],
            |r| r.get(0),
        )?)
    }

    /// Give up `owner`'s claim on `rel` (nothing if it is not theirs).
    pub fn offline_unclaim(&self, rel: &str, owner: &str) -> Result<()> {
        self.db().execute(
            "DELETE FROM offline_claims WHERE path = ?1 AND owner = ?2",
            params![rel, owner],
        )?;
        Ok(())
    }

    /// Renew `owner`'s claim on `rel` while the download goes on.
    pub fn offline_renew_claim(&self, rel: &str, owner: &str) -> Result<()> {
        self.db().execute(
            "UPDATE offline_claims SET at = ?3 WHERE path = ?1 AND owner = ?2",
            params![rel, owner, crate::now_unix()],
        )?;
        Ok(())
    }

    /// Delete the part files (`<file>.part.<owner>`, [`part_file_target`])
    /// in `files/thumb`, `files/cover` and `clip/*` that no live download
    /// writes: their file has no claim fresher than `stale_secs` by another
    /// owner than `owner` (with `None`: by anyone). Returns how many went.
    /// A killed downloader leaves them; the downloader sweeps when a run
    /// starts, "Clear cache" and "Remove kept files" too.
    pub fn sweep_part_files(&self, owner: Option<&str>, stale_secs: i64) -> usize {
        let mut dirs: Vec<String> = vec!["files/thumb".into(), "files/cover".into()];
        if let Ok(entries) = std::fs::read_dir(self.abs("clip")) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    dirs.push(format!("clip/{}", entry.file_name().to_string_lossy()));
                }
            }
        }
        let mut removed = 0;
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(self.abs(&dir)) else {
                continue;
            };
            let before = removed;
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let Some(target) = part_file_target(&name) else {
                    continue;
                };
                if owner.is_some_and(|o| name.ends_with(&format!(".part.{o}"))) {
                    continue;
                }
                let rel = format!("{dir}/{target}");
                let live = self
                    .offline_claimed_by_other(&rel, owner.unwrap_or(""), stale_secs)
                    .unwrap_or(true);
                if !live && std::fs::remove_file(entry.path()).is_ok() {
                    removed += 1;
                }
            }
            // A rendition's directory this left empty (only then: another
            // process may have made one for a download just now).
            if removed > before && dir.starts_with("clip/") {
                let _ = std::fs::remove_dir(self.abs(&dir)); // only when empty
            }
        }
        removed
    }

    /// Note that this process (`id`) brings the kept files in line with
    /// offline options `json` now, forget the processes that have not for
    /// [`PEER_TTL_SECS`], and return the options of the others, newest
    /// first. One immediate transaction.
    pub fn offline_peer_note(&self, id: &str, json: &str) -> Result<Vec<String>> {
        let now = crate::now_unix();
        let mut db = self.db();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM offline_peers WHERE at < ?1", [now - PEER_TTL_SECS])?;
        tx.execute(
            "INSERT INTO offline_peers(id, at, json) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET at = excluded.at, json = excluded.json",
            params![id, now, json],
        )?;
        let others = {
            let mut st = tx.prepare("SELECT json FROM offline_peers WHERE id != ?1 ORDER BY at DESC")?;
            let rows = st.query_map([id], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        tx.commit()?;
        Ok(others)
    }

    /// Forget this process's offline options (it ends).
    pub fn offline_peer_forget(&self, id: &str) -> Result<()> {
        self.db().execute("DELETE FROM offline_peers WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Files and bytes kept on the device and the others.
    pub fn offline_usage(&self) -> Result<OfflineUsage> {
        let db = self.db();
        let mut st = db.prepare("SELECT kept, COUNT(*), COALESCE(SUM(size), 0) FROM files GROUP BY kept")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)? as u64,
                r.get::<_, i64>(2)? as u64,
            ))
        })?;
        let mut usage = OfflineUsage::default();
        for row in rows {
            let (kept, files, bytes) = row?;
            if kept != 0 {
                usage.kept_files += files;
                usage.kept_bytes += bytes;
            } else {
                usage.other_files += files;
                usage.other_bytes += bytes;
            }
        }
        Ok(usage)
    }

    /// Forget everything kept offline and delete the kept files: what to
    /// keep, the failures, the claims (a download still running throws its
    /// file away, [`Cache::install_kept_file`]). The confirmed "Remove kept
    /// files", and part of [`Cache::reset_account_data`]. The automatic
    /// rules are options, not choices, so they keep the files they hold
    /// coming back until they are turned off.
    ///
    /// The kept files are condemned (`kept` 2) in the transaction that
    /// forgets the keeps and moves the epoch, and deleted afterwards
    /// ([`Cache::offline_finish_reset`]); a process that dies in between
    /// leaves them condemned, and the next open, refresh or reset deletes
    /// them.
    pub fn offline_reset(&self) -> Result<EvictReport> {
        {
            let mut db = self.db();
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            tx.execute("DELETE FROM offline_keeps", [])?;
            tx.execute("DELETE FROM offline_failures", [])?;
            tx.execute("DELETE FROM offline_claims", [])?;
            tx.execute(
                "DELETE FROM meta WHERE key IN ('offline_owner', 'offline_liked', 'offline_covers')",
                [],
            )?;
            tx.execute(
                "INSERT INTO meta(key, value) VALUES ('offline_epoch', '1')
                 ON CONFLICT(key) DO UPDATE SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)",
                [],
            )?;
            condemn_stale(&tx)?;
            tx.commit()?;
        }
        let mut report = self.offline_finish_reset()?;
        // Downloads that were running are stale now (the epoch moved), and
        // their claims are gone.
        self.sweep_part_files(None, CLAIM_STALE_SECS);
        report.total_bytes = self.unkept_bytes()?;
        Ok(report)
    }

    /// Finish what a reset began: condemn the kept files when the epoch
    /// moved past theirs (a reset by a process that died before it
    /// condemned them, [`condemn_stale`]), and delete the condemned files
    /// (one whose deletion fails stays condemned, for the next try).
    /// Called when the cache opens, by every refresh and by every reset.
    pub fn offline_finish_reset(&self) -> Result<EvictReport> {
        let pending = {
            let db = self.db();
            meta_u64(&db, "offline_kept_epoch")? != meta_u64(&db, "offline_epoch")?
                || db.query_row(
                    "SELECT EXISTS (SELECT 1 FROM files WHERE kept = ?1)",
                    [CONDEMNED],
                    |r| r.get::<_, bool>(0),
                )?
        };
        let mut report = EvictReport::default();
        if !pending {
            return Ok(report);
        }
        {
            let mut db = self.db();
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            condemn_stale(&tx)?;
            tx.commit()?;
        }
        let condemned: Vec<(String, u64)> = {
            let db = self.db();
            let mut st = db.prepare("SELECT path, size FROM files WHERE kept = ?1")?;
            let rows = st.query_map([CONDEMNED], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (rel, size) in condemned {
            if self.remove_file_where(&rel, &format!("kept = {CONDEMNED}"))? {
                report.removed_files += 1;
                report.freed_bytes += size;
            }
        }
        if report.removed_files > 0 {
            tracing::info!(
                files = report.removed_files,
                bytes = report.freed_bytes,
                "deleted the files kept offline before a reset"
            );
        }
        Ok(report)
    }

    /// The kept data belongs to the account that kept it (`offline_owner`).
    /// It is reset (kept files deleted, keeps forgotten, true) when it
    /// belongs to another account than the cached one, or to none:
    ///
    /// - no account is cached any more (a client up to 0.8.0 signed out:
    ///   its sign-out drops the items and the account, not the offline
    ///   data);
    /// - another account is cached (such a client signed in as another);
    /// - the same account is cached, but the items and the sync cursor are
    ///   gone (such a client signed out and in again, and has not synced
    ///   since: its login stores the account at once).
    ///
    /// A cached account this version cannot read is not a sign-out:
    /// nothing is reset. The owner is set once a sync completed (the
    /// cursor is there), so the third case cannot be a new login's first
    /// sync.
    pub fn offline_check_owner(&self) -> Result<bool> {
        let owner = self.meta_get("offline_owner")?;
        let account = self.cached_account()?;
        let synced = self.sync_cursor()?.is_some();
        let reset = match (owner.as_deref(), &account) {
            (_, CachedAccount::Unreadable) | (None, _) => false,
            (Some(_), CachedAccount::None) => true,
            (Some(o), CachedAccount::Known { id, .. }) if o != id => true,
            (Some(_), CachedAccount::Known { .. }) => !synced && self.item_count()? == 0,
        };
        if reset {
            tracing::info!("offline data of another account (or a signed-out one) found; removing it");
            self.offline_reset()?;
        }
        if let (true, CachedAccount::Known { id, .. }) = (synced, &account) {
            if reset || owner.is_none() {
                self.meta_set("offline_owner", id)?;
            }
        }
        Ok(reset)
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

    /// The target apps in the copy history, most recently used first.
    pub fn history_targets(&self) -> Result<Vec<TargetApp>> {
        let db = self.db();
        let mut st =
            db.prepare("SELECT target FROM history WHERE target IS NOT NULL GROUP BY target ORDER BY MAX(seq) DESC")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            if let Some(t) = TargetApp::parse(&row?) {
                out.push(t);
            }
        }
        Ok(out)
    }

    /// How often each item was copied on this device.
    pub fn copy_counts(&self) -> Result<HashMap<String, u64>> {
        let db = self.db();
        let mut st = db.prepare("SELECT item_id, COUNT(*) FROM history GROUP BY item_id")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
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

/// Whether `abs` is a file (with `expect_size`, of that size; 0: any).
fn on_disk(abs: &Path, expect_size: Option<u64>) -> bool {
    std::fs::metadata(abs)
        .ok()
        .filter(|m| m.is_file())
        .is_some_and(|m| expect_size.is_none_or(|s| s == 0 || s == m.len()))
}

/// What [`Cache::offline_mark`] changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarkReport {
    /// Files flagged as kept (newly, or again after an older client).
    pub flagged: usize,
    /// Kept files released to the LRU.
    pub released: usize,
    /// Kept files of removed items, deleted.
    pub deleted: usize,
}

/// Register `rel` (`size` bytes) as kept, in a transaction that checked the
/// epoch; its download failures are forgotten.
fn flag_kept(tx: &rusqlite::Transaction<'_>, rel: &str, size: u64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO files(path, size, last_access, kept) VALUES (?1, ?2, ?3, 1)
         ON CONFLICT(path) DO UPDATE SET size = excluded.size, last_access = excluded.last_access, kept = 1",
        params![rel, size as i64, KEPT_ACCESS],
    )?;
    tx.execute("DELETE FROM offline_failures WHERE path = ?1", [rel])?;
    Ok(())
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
/// listed are deleted. Items cached in an older [`ITEM_FORMAT`] (fields
/// missing) are listed again in full the same way, once; so are the items
/// when one of them lacks `text` (an older client sharing the cache
/// stored it, and its cursor would skip the item's change). The cursor is
/// advanced only after the last page.
pub async fn sync(cache: &Cache, client: &ApiClient, page_limit: u32) -> Result<SyncReport> {
    let since = if cache.item_format()? < ITEM_FORMAT || cache.has_items_without_text()? {
        None
    } else {
        cache.sync_cursor()?
    };
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
    if full {
        cache.set_item_format(ITEM_FORMAT)?;
    }
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
        text: item.text.clone(),
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

    /// Every cached item, in no particular order.
    pub fn items(&self) -> impl Iterator<Item = &ItemSummary> {
        self.items.values()
    }

    /// Search with the server's query syntax (no image-content matching; the
    /// liked set is unknown offline, so `is:liked` matches nothing).
    pub fn search(&self, query: &str, target: Option<TargetApp>, limit: usize) -> Vec<SearchHit> {
        let q = Query::parse(query);
        let viewer = Viewer {
            user_id: self.me.clone().unwrap_or_default(),
            liked: HashSet::new(),
            member_sets: HashSet::new(),
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
