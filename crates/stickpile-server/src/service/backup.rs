//! `stickpile-server backup <dir>` (docs/backup.md): a full, consistent
//! backup that needs no running server and is safe while one runs.
//!
//! 1. `<dir>/stickpile.db`: `VACUUM INTO` a snapshot of the live database.
//!    SQLite runs it in one read transaction, so with WAL it neither blocks
//!    nor sees half of a concurrent write.
//! 2. `<dir>/sets/<username>/<slug>/`: every set of every user in the set
//!    directory format, exported **from the snapshot** (so directories and
//!    database agree), with the stored originals from `<data_dir>/media`.
//! 3. `<dir>/proposals/<media id>.<ext>`: the stored originals of the
//!    stickers and cover images of open proposals (they are in no set).
//! 4. `<dir>/backup.json`: time, versions and counts.
//!
//! [`restore`] (`stickpile-server restore <dir>`) turns such a backup into
//! the data directory of a fresh server: it copies the snapshot (newer
//! migrations run on it), writes every stored original back from the set
//! directories (verified by its sha256), regenerates thumbnails and queues
//! the other variants (cover images have none). Accounts, tokens, likes,
//! usage and AI data come back with the database.
//!
//! The target must be empty, missing, or an earlier backup (it holds
//! nothing but `stickpile.db`, `sets/` and `backup.json`), which is then
//! replaced.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

use stickpile_api::{JobKind, MediaFormat, Variant};

use crate::config::Config;
use crate::db::{SetRow, SET_COLUMNS};
use crate::files;
use crate::ids::sha256_hex;
use crate::service::{hydrate, transfer};

/// Version of the backup layout in `backup.json`.
pub const FORMAT: u32 = 1;
pub const MANIFEST: &str = "backup.json";
pub const DB_FILE: &str = "stickpile.db";
pub const SETS_DIR: &str = "sets";
pub const PROPOSALS_DIR: &str = "proposals";

/// Contents of `backup.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackupInfo {
    pub format: u32,
    /// Unix seconds.
    pub created_at: i64,
    pub stickpile_version: String,
    /// Highest applied schema migration.
    pub schema_version: i64,
    pub users: u64,
    pub sets: u64,
    pub items: u64,
    pub media: u64,
    /// Sum of the exported media files.
    pub bytes: u64,
    /// Sets that could not be exported (`username/slug: error`). The database
    /// snapshot is complete regardless.
    pub errors: Vec<String>,
}

async fn open(path: &Path, create: bool, journal: SqliteJournalMode) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(create)
        .journal_mode(journal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(30));
    Ok(SqlitePoolOptions::new().max_connections(2).connect_with(opts).await?)
}

/// Make sure `dir` can take a backup; clear an earlier one.
fn prepare(dir: &Path) -> anyhow::Result<()> {
    match std::fs::read_dir(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => anyhow::bail!("reading {}: {e}", dir.display()),
        Ok(entries) => {
            let names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            if !names.is_empty() {
                // Ours: an earlier (possibly interrupted) backup.
                let ours = names
                    .iter()
                    .all(|n| [DB_FILE, MANIFEST, SETS_DIR, PROPOSALS_DIR].contains(&n.as_str()));
                if !ours {
                    anyhow::bail!(
                        "{} is not empty and is not an earlier backup; refusing to write into it",
                        dir.display()
                    );
                }
                for name in [DB_FILE, MANIFEST] {
                    let p = dir.join(name);
                    if p.exists() {
                        std::fs::remove_file(&p).map_err(|e| anyhow::anyhow!("removing {}: {e}", p.display()))?;
                    }
                }
                for sub in [SETS_DIR, PROPOSALS_DIR] {
                    let p = dir.join(sub);
                    if p.exists() {
                        std::fs::remove_dir_all(&p).map_err(|e| anyhow::anyhow!("removing {}: {e}", p.display()))?;
                    }
                }
            }
        }
    }
    // The snapshot holds password and token hashes, TOTP and webhook
    // secrets: keep the whole backup private to the service user.
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| anyhow::anyhow!("creating {}: {e}", dir.display()))?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| anyhow::anyhow!("chmod {}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(dir.join(SETS_DIR)).map_err(|e| anyhow::anyhow!("creating {}: {e}", dir.display()))?;
    Ok(())
}

/// Write a backup of the server configured by `config` into `dir`.
pub async fn run(config: &Config, dir: &Path) -> anyhow::Result<BackupInfo> {
    let db_path = config.db_path();
    if !db_path.is_file() {
        anyhow::bail!("no database at {} (check server.data_dir)", db_path.display());
    }
    let dir = dir.to_path_buf();
    {
        let d = dir.clone();
        tokio::task::spawn_blocking(move || prepare(&d)).await??;
    }
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);

    // 1. Snapshot. No migrations here: the server owns the schema.
    let live = open(&db_path, false, SqliteJournalMode::Wal).await?;
    let snapshot_path = dir.join(DB_FILE);
    sqlx::query("VACUUM INTO ?")
        .bind(snapshot_path.to_string_lossy().into_owned())
        .execute(&live)
        .await
        .map_err(|e| anyhow::anyhow!("VACUUM INTO {}: {e}", snapshot_path.display()))?;
    live.close().await;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&snapshot_path, std::fs::Permissions::from_mode(0o600))?;
    }
    // A single self-contained file (no -wal / -shm next to it).
    let snap = open(&snapshot_path, false, SqliteJournalMode::Delete).await?;

    // 2. Set directories from the snapshot.
    let media_dir = config.media_dir();
    let sets = sets_with_owner(&snap).await?;
    let mut info = BackupInfo {
        format: FORMAT,
        created_at: crate::ids::now(),
        stickpile_version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version: sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1")
            .fetch_one(&snap)
            .await
            .unwrap_or(0),
        users: count(&snap, "users").await?,
        sets: 0,
        items: 0,
        media: count(&snap, "media").await?,
        bytes: 0,
        errors: Vec::new(),
    };
    for (username, set) in &sets {
        let out: PathBuf = dir.join(SETS_DIR).join(username).join(&set.slug);
        let bundle = match transfer::export_set(&snap, &media_dir, set).await {
            Ok(b) => b,
            Err(e) => {
                info.errors.push(format!("{username}/{}: {}", set.slug, e.message));
                continue;
            }
        };
        let items = bundle.files.len() as u64;
        let bytes: u64 = bundle.files.iter().map(|f| f.bytes.len() as u64).sum::<u64>()
            + bundle.cover_image.as_ref().map_or(0, |c| c.bytes.len() as u64);
        let written = tokio::task::spawn_blocking(move || stickpile_setfmt::write_dir(&out, &bundle)).await?;
        match written {
            Ok(()) => {
                info.sets += 1;
                info.items += items;
                info.bytes += bytes;
            }
            Err(e) => info.errors.push(format!("{username}/{}: {e}", set.slug)),
        }
    }
    // 3. Media of open proposals, which no set directory holds.
    for (media_id, path) in open_proposal_media(&snap).await? {
        let Some(src) = files::resolve(&media_dir, &path) else {
            continue;
        };
        let ext = path.rsplit('.').next().unwrap_or("bin");
        let out = dir.join(PROPOSALS_DIR).join(format!("{media_id}.{ext}"));
        let copied = async {
            tokio::fs::create_dir_all(dir.join(PROPOSALS_DIR)).await?;
            tokio::fs::copy(&src, &out).await
        }
        .await;
        match copied {
            Ok(n) => info.bytes += n,
            Err(e) => info.errors.push(format!("proposal media {media_id}: {e}")),
        }
    }
    snap.close().await;

    // 4. backup.json last: its presence marks a finished backup.
    let json = serde_json::to_vec_pretty(&info)?;
    tokio::fs::write(dir.join(MANIFEST), json).await?;
    Ok(info)
}

/// (media id, stored original's path) of every media an open proposal needs.
async fn open_proposal_media(db: &SqlitePool) -> anyhow::Result<Vec<(String, String)>> {
    Ok(sqlx::query_as(
        "SELECT v.media_id, v.path FROM media_variants v WHERE v.variant = 'original' AND v.media_id IN ( \
           SELECT pi.media_id FROM proposal_items pi JOIN proposals p ON p.id = pi.proposal_id WHERE p.status = 'open' \
           UNION SELECT cover_media_id FROM proposals WHERE status = 'open') \
         ORDER BY v.media_id",
    )
    .fetch_all(db)
    .await?)
}

/// Every set with its owner's username, by username and slug.
async fn sets_with_owner(db: &SqlitePool) -> anyhow::Result<Vec<(String, SetRow)>> {
    let cols = SET_COLUMNS
        .split(", ")
        .map(|c| format!("s.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    let rows: Vec<SetWithOwner> = sqlx::query_as(&format!(
        "SELECT u.username AS owner_username, {cols} FROM sets s JOIN users u ON u.id = s.owner_id \
         ORDER BY u.username, s.slug"
    ))
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.owner_username, r.set)).collect())
}

/// Outcome of [`restore`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RestoreInfo {
    pub backup: Option<BackupInfo>,
    pub sets: u64,
    /// Media files written back (originals + thumbnails).
    pub media: u64,
    /// `Variants` jobs queued for the restored media.
    pub variant_jobs: u64,
    /// Media whose original was not found (or did not match) in the set
    /// directories; their items stay without files.
    pub missing: Vec<String>,
}

/// Restore a backup written by [`run`] into the (fresh) data directory of
/// `config`. Refuses to overwrite an existing database.
pub async fn restore(config: &Config, dir: &Path) -> anyhow::Result<RestoreInfo> {
    let manifest = dir.join(MANIFEST);
    let backup: BackupInfo = serde_json::from_slice(
        &std::fs::read(&manifest).map_err(|e| anyhow::anyhow!("reading {}: {e}", manifest.display()))?,
    )
    .map_err(|e| anyhow::anyhow!("{}: {e}", manifest.display()))?;
    if backup.format > FORMAT {
        anyhow::bail!(
            "backup format {} is newer than this server understands ({FORMAT})",
            backup.format
        );
    }
    let db_path = config.db_path();
    if db_path.exists() {
        anyhow::bail!(
            "{} already exists: restore into a fresh server.data_dir (or move the old one away)",
            db_path.display()
        );
    }
    for d in [
        config.server.data_dir.clone(),
        config.media_dir(),
        config.tmp_dir(),
        config.gitpacks_dir(),
    ] {
        std::fs::create_dir_all(&d).map_err(|e| anyhow::anyhow!("creating {}: {e}", d.display()))?;
    }
    tokio::fs::copy(dir.join(DB_FILE), &db_path).await?;
    let pool = crate::db::connect(&db_path).await?;
    let mut info = RestoreInfo {
        backup: Some(backup),
        ..Default::default()
    };
    let l = &config.limits;
    let opts = stickpile_media::IngestOptions {
        limits: stickpile_media::Limits {
            max_bytes: l.max_upload_mb * 1024 * 1024,
            max_pixels: l.max_pixels,
            max_frames: l.max_animation_frames,
            max_video_ms: u64::from(l.max_video_seconds) * 1000,
        },
        thumb_px: config.media.thumb_px,
        recommended_px: config.media.default_recommended_px,
    };
    let tools = stickpile_media::Tools {
        ffmpeg: config.media.ffmpeg.clone(),
        ffprobe: config.media.ffprobe.clone(),
        timeout: Duration::from_secs(config.media.ffmpeg_timeout_secs),
        tmp_dir: config.tmp_dir(),
    };
    let media_dir = config.media_dir();
    let mut restored: HashSet<String> = HashSet::new();
    let mut wanted: HashMap<String, String> = HashMap::new();
    for (username, set) in sets_with_owner(&pool).await? {
        let rows = hydrate::fetch_by_set(&pool, &set.id).await?;
        for r in &rows {
            wanted
                .entry(r.media_id.clone())
                .or_insert_with(|| format!("{username}/{}/{}", set.slug, r.name));
        }
        if let Some(media_id) = &set.cover_media_id {
            wanted
                .entry(media_id.clone())
                .or_insert_with(|| format!("{username}/{}/cover image", set.slug));
        }
        let set_dir = dir.join(SETS_DIR).join(&username).join(&set.slug);
        let bundle = match tokio::task::spawn_blocking(move || stickpile_setfmt::read_dir(&set_dir)).await? {
            Ok(b) => b,
            Err(_) => continue,
        };
        info.sets += 1;
        let by_sha: HashMap<String, &[u8]> = bundle
            .files
            .iter()
            .map(|f| (sha256_hex(&f.bytes), f.bytes.as_slice()))
            .collect();
        for r in &rows {
            if restored.contains(&r.media_id) {
                continue;
            }
            // The export wrote the stored original: its hash must match.
            let Some(bytes) = by_sha.get(&r.original_sha256) else {
                continue;
            };
            let media = Restored {
                id: &r.media_id,
                sha256: &r.sha256,
                format: r.format(),
            };
            info.media += media.write(&pool, &media_dir, &opts, &tools, bytes.to_vec()).await?;
            sqlx::query("DELETE FROM media_variants WHERE media_id = ? AND variant NOT IN ('original', 'thumb')")
                .bind(&r.media_id)
                .execute(&pool)
                .await?;
            sqlx::query("UPDATE media SET variants_done = 0 WHERE id = ?")
                .bind(&r.media_id)
                .execute(&pool)
                .await?;
            crate::jobs::insert(
                &pool,
                JobKind::Variants,
                serde_json::json!({ "media_id": r.media_id }),
                Some(r.media_id.clone()),
                Some(&set.owner_id),
            )
            .await?;
            info.variant_jobs += 1;
            restored.insert(r.media_id.clone());
        }
        // The cover image: the original and its thumbnail are all it has.
        if let Some(media_id) = set.cover_media_id.as_deref().filter(|m| !restored.contains(*m)) {
            let media = crate::db::media_by_id(&pool, media_id).await?;
            let file = bundle.cover_image.as_ref();
            if let (Some(m), Some(c)) = (media, file) {
                if sha256_hex(&c.bytes) == m.original_sha256 {
                    let target = Restored {
                        id: media_id,
                        sha256: &m.sha256,
                        format: m.format(),
                    };
                    info.media += target.write(&pool, &media_dir, &opts, &tools, c.bytes.clone()).await?;
                    restored.insert(media_id.to_string());
                }
            }
        }
    }
    // Media of open proposals.
    for (media_id, path) in open_proposal_media(&pool).await? {
        wanted
            .entry(media_id.clone())
            .or_insert_with(|| format!("a proposal's file {media_id}"));
        if restored.contains(&media_id) {
            continue;
        }
        let ext = path.rsplit('.').next().unwrap_or("bin");
        let Ok(bytes) = tokio::fs::read(dir.join(PROPOSALS_DIR).join(format!("{media_id}.{ext}"))).await else {
            continue;
        };
        let Some(m) = crate::db::media_by_id(&pool, &media_id).await? else {
            continue;
        };
        if sha256_hex(&bytes) != m.original_sha256 {
            continue;
        }
        let target = Restored {
            id: &media_id,
            sha256: &m.sha256,
            format: m.format(),
        };
        info.media += target.write(&pool, &media_dir, &opts, &tools, bytes).await?;
        // Its other variants are made when the proposal is accepted.
        sqlx::query("DELETE FROM media_variants WHERE media_id = ? AND variant NOT IN ('original', 'thumb')")
            .bind(&media_id)
            .execute(&pool)
            .await?;
        sqlx::query("UPDATE media SET variants_done = 0 WHERE id = ?")
            .bind(&media_id)
            .execute(&pool)
            .await?;
        restored.insert(media_id);
    }
    let mut missing: Vec<String> = wanted
        .into_iter()
        .filter(|(m, _)| !restored.contains(m))
        .map(|(_, what)| what)
        .collect();
    missing.sort();
    info.missing = missing;
    pool.close().await;
    Ok(info)
}

/// Media whose files [`restore`] writes back.
struct Restored<'a> {
    id: &'a str,
    sha256: &'a str,
    format: MediaFormat,
}

impl Restored<'_> {
    /// Write the stored original `bytes` back and render its thumbnail
    /// again; the number of files written.
    async fn write(
        &self,
        pool: &SqlitePool,
        media_dir: &Path,
        opts: &stickpile_media::IngestOptions,
        tools: &stickpile_media::Tools,
        bytes: Vec<u8>,
    ) -> anyhow::Result<u64> {
        let orig = files::resolve(
            media_dir,
            &files::variant_rel_path(self.sha256, Variant::Original, self.format),
        )
        .ok_or_else(|| anyhow::anyhow!("bad media path"))?;
        files::write_atomic(orig, bytes.clone()).await?;
        let (o, t) = (opts.clone(), tools.clone());
        let ing = tokio::task::spawn_blocking(move || stickpile_media::ingest(&bytes, &o, &t)).await?;
        match ing {
            Ok(ing) => {
                let rel = files::variant_rel_path(self.sha256, Variant::Thumb, self.format);
                let abs = files::resolve(media_dir, &rel).ok_or_else(|| anyhow::anyhow!("bad media path"))?;
                let thumb = ing.thumb;
                sqlx::query(
                    "INSERT OR REPLACE INTO media_variants (media_id, variant, mime, width, height, size_bytes, \
                     sha256, path) VALUES (?, 'thumb', ?, ?, ?, ?, ?, ?)",
                )
                .bind(self.id)
                .bind(thumb.mime)
                .bind(i64::from(thumb.width))
                .bind(i64::from(thumb.height))
                .bind(thumb.bytes.len() as i64)
                .bind(sha256_hex(&thumb.bytes))
                .bind(&rel)
                .execute(pool)
                .await?;
                files::write_atomic(abs, thumb.bytes).await?;
                Ok(2)
            }
            Err(e) => {
                tracing::warn!("restoring the thumbnail of media {}: {e}", self.id);
                Ok(1)
            }
        }
    }
}

#[derive(sqlx::FromRow)]
struct SetWithOwner {
    owner_username: String,
    #[sqlx(flatten)]
    set: SetRow,
}

async fn count(db: &SqlitePool, table: &str) -> anyhow::Result<u64> {
    let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(db)
        .await?;
    Ok(n.max(0) as u64)
}
