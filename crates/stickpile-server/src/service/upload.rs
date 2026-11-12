//! Upload pipeline: stream multipart parts to temp files under per-file and
//! per-request limits, dedup by sha256, ingest new media with
//! `stickpile_media::ingest` (sniff, validate, sanitise, hash, score, thumb)
//! on the blocking pool, store original + thumb atomically, create the item at
//! the end of the set, report near-duplicates among the caller's own items,
//! and enqueue the `Variants` / `Enrich` jobs.

use std::io::Write;

use axum::extract::multipart::{Field, Multipart, MultipartError};
use axum::http::StatusCode;
use serde_json::json;
use sha2::{Digest, Sha256};
use stickpile_api::{FieldSource, JobKind, NearDuplicate, UploadMeta, UploadResult, Variant};
use stickpile_media::hash::hamming;

use crate::auth::CurrentUser;
use crate::authz;
use crate::db::{self, enum_str, i64_to_u64, source_str, to_json_list, u64_to_i64, MediaRow, SetRow, MEDIA_COLUMNS};
use crate::error::{AppError, AppResult};
use crate::files;
use crate::ids::{new_id, now, sha256_hex};
use crate::index;
use crate::jobs;
use crate::service::ai::{self as ai_fill, AiFill};
use crate::service::{hydrate, sets};
use crate::state::AppState;

pub const MAX_META_BYTES: usize = 1024 * 1024;
const MAX_LIST: usize = 50;
const MAX_ENTRY_CHARS: usize = 100;

/// One uploaded file, already streamed to disk (or held in memory).
pub struct UploadFile {
    /// Original multipart filename (may include directories for imports).
    pub filename: String,
    pub sha256: String,
    pub size: u64,
    pub data: UploadData,
}

pub enum UploadData {
    Memory(Vec<u8>),
    Temp(tempfile::NamedTempFile),
}

impl UploadFile {
    pub fn from_bytes(filename: impl Into<String>, bytes: Vec<u8>) -> UploadFile {
        UploadFile {
            filename: filename.into(),
            sha256: sha256_hex(&bytes),
            size: bytes.len() as u64,
            data: UploadData::Memory(bytes),
        }
    }
    pub async fn bytes(&self) -> AppResult<Vec<u8>> {
        match &self.data {
            UploadData::Memory(b) => Ok(b.clone()),
            UploadData::Temp(t) => Ok(tokio::fs::read(t.path()).await?),
        }
    }
}

pub fn multipart_error(e: MultipartError) -> AppError {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        AppError::too_large("request body too large")
    } else {
        AppError::bad_request(format!("invalid multipart body: {}", e.body_text()))
    }
}

/// Stream one multipart field to a temp file, enforcing `max_bytes`.
pub async fn field_to_file(state: &AppState, field: &mut Field<'_>, max_bytes: u64) -> AppResult<UploadFile> {
    let filename = field.file_name().unwrap_or("upload").to_string();
    let tmp_dir = state.config.tmp_dir();
    let mut tmp =
        tokio::task::spawn_blocking(move || tempfile::Builder::new().prefix("upload-").tempfile_in(tmp_dir)).await??;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
        size += chunk.len() as u64;
        if size > max_bytes {
            return Err(AppError::too_large(format!(
                "{filename:?} exceeds the per-file limit of {} MiB",
                max_bytes / (1024 * 1024)
            )));
        }
        hasher.update(&chunk);
        tmp.write_all(&chunk)?;
    }
    tmp.flush()?;
    Ok(UploadFile {
        filename,
        sha256: hex::encode(hasher.finalize()),
        size,
        data: UploadData::Temp(tmp),
    })
}

/// Read a small text field.
pub async fn field_text(field: &mut Field<'_>, max: usize) -> AppResult<String> {
    let mut buf = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
        if buf.len() + chunk.len() > max {
            return Err(AppError::too_large("form field too large"));
        }
        buf.extend_from_slice(&chunk);
    }
    String::from_utf8(buf).map_err(|_| AppError::bad_request("form field is not UTF-8"))
}

/// Parsed upload request: files in order plus the optional `meta` list.
pub struct UploadParts {
    pub files: Vec<UploadFile>,
    pub meta: Vec<UploadMeta>,
}

/// Read an upload multipart body: every part with a filename is a file
/// (per-file limit `limits.max_upload_mb`); a `meta` part holds JSON
/// `Vec<UploadMeta>` matched by index. The whole body is capped at
/// `limits.max_request_mb` by the route's body limit.
pub async fn read_multipart(state: &AppState, mut mp: Multipart) -> AppResult<UploadParts> {
    let max_file = state.config.limits.max_upload_mb * 1024 * 1024;
    let mut files = Vec::new();
    let mut meta = Vec::new();
    while let Some(mut field) = mp.next_field().await.map_err(multipart_error)? {
        if field.name() == Some("meta") {
            meta = parse_meta(&field_text(&mut field, MAX_META_BYTES).await?)?;
        } else if field.file_name().is_some() {
            files.push(field_to_file(state, &mut field, max_file).await?);
        }
    }
    if files.is_empty() {
        return Err(AppError::bad_request("no file parts in the upload"));
    }
    Ok(UploadParts { files, meta })
}

/// The `meta` part: a JSON array of [`UploadMeta`], matched to the files by
/// index. Empty text is no metadata.
pub fn parse_meta(text: &str) -> AppResult<Vec<UploadMeta>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text).map_err(|e| AppError::bad_request(format!("meta: {e}")))
}

// ---------------------------------------------------------------------------
// Item metadata input
// ---------------------------------------------------------------------------

/// Metadata for a new item (from an upload or a set.toml entry).
#[derive(Debug, Clone, Default)]
pub struct ItemInput {
    pub name: String,
    pub name_source: FieldSource,
    pub keywords: Vec<String>,
    pub keywords_source: FieldSource,
    pub emojis: Vec<String>,
    pub emojis_source: FieldSource,
    pub vibe: Option<String>,
    pub vibe_source: FieldSource,
    pub tags: Vec<String>,
    pub source_url: Option<String>,
    /// Export filename stem (no numeric prefix, no extension).
    pub stem: String,
    pub extra_toml: Option<String>,
}

pub fn clean_list(v: &[String], what: &str) -> AppResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for s in v {
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        if s.chars().count() > MAX_ENTRY_CHARS || s.chars().any(char::is_control) {
            return Err(AppError::bad_request(format!(
                "{what} entries must be at most {MAX_ENTRY_CHARS} printable characters"
            )));
        }
        if !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    }
    if out.len() > MAX_LIST {
        return Err(AppError::bad_request(format!("at most {MAX_LIST} {what}")));
    }
    Ok(out)
}

pub fn clean_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.chars().count() > 200 || name.chars().any(char::is_control) {
        return Err(AppError::bad_request(
            "item name must be at most 200 printable characters",
        ));
    }
    Ok(name.to_string())
}

pub fn clean_vibe(v: Option<&str>) -> AppResult<Option<String>> {
    match v.map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) if v.chars().count() <= 50 && !v.chars().any(char::is_control) => Ok(Some(v.to_lowercase())),
        Some(_) => Err(AppError::bad_request("vibe must be at most 50 printable characters")),
    }
}

/// Export stem for an item name: slugified, `item` when nothing is left.
pub fn stem_for(name: &str) -> String {
    if name.chars().any(|c| c.is_alphanumeric()) {
        let s = stickpile_setfmt::slugify(name);
        if !s.is_empty() {
            return s;
        }
    }
    "item".into()
}

/// Strip any directories from a multipart filename.
pub fn base_name(filename: &str) -> &str {
    filename.rsplit(['/', '\\']).next().unwrap_or(filename)
}

impl ItemInput {
    pub fn from_upload(filename: &str, meta: Option<&UploadMeta>) -> AppResult<ItemInput> {
        let name = match meta.and_then(|m| m.name.as_deref()).map(clean_name).transpose()? {
            Some(n) if !n.is_empty() => n,
            _ => stickpile_setfmt::display_stem(base_name(filename)),
        };
        let name = if name.trim().is_empty() {
            "item".to_string()
        } else {
            name
        };
        let source_url = meta.and_then(|m| m.source_url.clone()).filter(|u| u.len() <= 2048);
        Ok(ItemInput {
            stem: stem_for(&name),
            name,
            keywords: clean_list(meta.and_then(|m| m.keywords.as_deref()).unwrap_or(&[]), "keywords")?,
            emojis: clean_list(meta.and_then(|m| m.emojis.as_deref()).unwrap_or(&[]), "emojis")?,
            tags: clean_list(meta.and_then(|m| m.tags.as_deref()).unwrap_or(&[]), "tags")?,
            vibe: clean_vibe(meta.and_then(|m| m.vibe.as_deref()))?,
            source_url,
            ..Default::default()
        })
    }
}

// ---------------------------------------------------------------------------
// Media
// ---------------------------------------------------------------------------

pub async fn media_by_sha(db: &sqlx::SqlitePool, sha: &str) -> AppResult<Option<MediaRow>> {
    Ok(sqlx::query_as::<_, MediaRow>(&format!(
        "SELECT {MEDIA_COLUMNS} FROM media WHERE sha256 = ? OR original_sha256 = ? ORDER BY sha256 = ? DESC LIMIT 1"
    ))
    .bind(sha)
    .bind(sha)
    .bind(sha)
    .fetch_optional(db)
    .await?)
}

/// Existing media for `file`'s hash, or ingest and store new media. The
/// caller must hold `state.media_lock` (shared) until the media is referenced.
pub async fn media_for_file(state: &AppState, file: &UploadFile, recommended_px: u32) -> AppResult<(MediaRow, bool)> {
    if let Some(m) = media_by_sha(&state.db, &file.sha256).await? {
        return Ok((m, false));
    }
    let max = state.config.limits.max_upload_mb * 1024 * 1024;
    if file.size > max {
        return Err(AppError::too_large(format!(
            "file exceeds the per-file limit of {} MiB",
            max / (1024 * 1024)
        )));
    }
    let bytes = file.bytes().await?;
    let opts = stickpile_media::IngestOptions {
        limits: state.media_limits(),
        thumb_px: state.config.media.thumb_px,
        recommended_px,
    };
    let tools = state.tools.clone();
    let ing = tokio::task::spawn_blocking(move || stickpile_media::ingest(&bytes, &opts, &tools)).await??;

    // Someone may have stored the same bytes meanwhile.
    if let Some(m) = media_by_sha(&state.db, &file.sha256).await? {
        return Ok((m, false));
    }
    let sha = file.sha256.clone();
    let format = ing.probe.format;
    let orig_rel = files::variant_rel_path(&sha, Variant::Original, format);
    let thumb_rel = files::variant_rel_path(&sha, Variant::Thumb, format);
    let media_dir = state.media_dir();
    let orig_sha = sha256_hex(&ing.original);
    let thumb_sha = sha256_hex(&ing.thumb.bytes);
    let orig_len = ing.original.len() as i64;
    let thumb_len = ing.thumb.bytes.len() as i64;
    files::write_atomic(
        files::resolve(&media_dir, &orig_rel).expect("safe path"),
        ing.original.clone(),
    )
    .await?;
    files::write_atomic(
        files::resolve(&media_dir, &thumb_rel).expect("safe path"),
        ing.thumb.bytes.clone(),
    )
    .await?;

    let id = new_id();
    let p = &ing.probe;
    let mut tx = sets::begin(&state.db).await?;
    let inserted = sqlx::query(
        "INSERT INTO media (id, sha256, original_sha256, kind, format, width, height, duration_ms, frame_count, fps, \
         has_alpha, size_bytes, phash, dhash, quality, quality_report, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(sha256) DO NOTHING",
    )
    .bind(&id)
    .bind(&sha)
    .bind(&orig_sha)
    .bind(enum_str(&p.kind))
    .bind(enum_str(&p.format))
    .bind(p.width as i64)
    .bind(p.height as i64)
    .bind(p.duration_ms.map(|v| v as i64))
    .bind(p.frame_count.map(|v| v as i64))
    .bind(p.fps.map(|v| v as f64))
    .bind(p.has_alpha)
    .bind(orig_len)
    .bind(u64_to_i64(ing.phash))
    .bind(u64_to_i64(ing.dhash))
    .bind(ing.quality.score as i64)
    .bind(serde_json::to_string(&ing.quality).ok())
    .bind(now())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        drop(tx);
        let m = media_by_sha(&state.db, &sha)
            .await?
            .ok_or_else(|| AppError::internal("media vanished"))?;
        return Ok((m, false));
    }
    for (variant, mime, w, h, size, vsha, rel) in [
        (
            Variant::Original,
            format.mime(),
            p.width,
            p.height,
            orig_len,
            &orig_sha,
            &orig_rel,
        ),
        (
            Variant::Thumb,
            ing.thumb.mime,
            ing.thumb.width,
            ing.thumb.height,
            thumb_len,
            &thumb_sha,
            &thumb_rel,
        ),
    ] {
        sqlx::query(
            "INSERT OR REPLACE INTO media_variants (media_id, variant, mime, width, height, size_bytes, sha256, path) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(variant.as_str())
        .bind(mime)
        .bind(w as i64)
        .bind(h as i64)
        .bind(size)
        .bind(vsha)
        .bind(rel)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    let m = db::media_by_id(&state.db, &id)
        .await?
        .ok_or_else(|| AppError::internal("media vanished"))?;
    Ok((m, true))
}

/// Outcome of adding one file to a set.
pub struct Added {
    pub item_id: String,
    pub media: MediaRow,
    pub already_in_set: bool,
}

/// Add `file` to the end of `set` (dedup by content: the same bytes already
/// in the set -> `already_in_set`, no new item). Does not refresh the index.
pub async fn add_to_set(state: &AppState, set: &SetRow, file: &UploadFile, input: &ItemInput) -> AppResult<Added> {
    let recommended = set
        .recommended_px
        .map(|v| v as u32)
        .unwrap_or(state.config.media.default_recommended_px);
    let guard = state.media_lock.read().await;
    let (media, new_media) = media_for_file(state, file, recommended).await?;
    let mut tx = sets::begin(&state.db).await?;
    let item_id = new_id();
    let t = now();
    let inserted = sqlx::query(
        "INSERT INTO items (id, set_id, media_id, position, filename, name, name_source, keywords, keywords_source, \
         emojis, emojis_source, vibe, vibe_source, tags, source_url, extra_toml, created_at, updated_at) \
         VALUES (?, ?, ?, (SELECT COALESCE(MAX(position) + 1, 0) FROM items WHERE set_id = ?), ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(set_id, media_id) DO NOTHING",
    )
    .bind(&item_id)
    .bind(&set.id)
    .bind(&media.id)
    .bind(&set.id)
    .bind(&input.stem)
    .bind(&input.name)
    .bind(source_str(input.name_source))
    .bind(to_json_list(&input.keywords))
    .bind(source_str(input.keywords_source))
    .bind(to_json_list(&input.emojis))
    .bind(source_str(input.emojis_source))
    .bind(&input.vibe)
    .bind(source_str(input.vibe_source))
    .bind(to_json_list(&input.tags))
    .bind(&input.source_url)
    .bind(&input.extra_toml)
    .bind(t)
    .bind(t)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        drop(tx);
        drop(guard);
        let existing: String = sqlx::query_scalar("SELECT id FROM items WHERE set_id = ? AND media_id = ?")
            .bind(&set.id)
            .bind(&media.id)
            .fetch_one(&state.db)
            .await?;
        return Ok(Added {
            item_id: existing,
            media,
            already_in_set: true,
        });
    }
    // Media enriched before (same bytes uploaded elsewhere): reuse its suggestions.
    if let Some(fill) = AiFill::from_media(&media) {
        ai_fill::apply(&mut tx, &media.id, &fill, Some(&item_id)).await?;
    }
    sets::bump_version(&mut tx, &set.id).await?;
    tx.commit().await?;
    drop(guard);

    // New media always gets a Variants job; reused media only if an earlier
    // one failed and none is pending.
    if new_media || (!media.variants_done && !jobs::pending_exists(&state.db, JobKind::Variants, &media.id).await?) {
        {
            jobs::enqueue(
                state,
                JobKind::Variants,
                json!({"media_id": media.id}),
                Some(media.id.clone()),
                Some(&set.owner_id),
            )
            .await?;
        }
    }
    if let Some(model) = state.ai_model_id() {
        if media.embedding_model.as_deref() != Some(model)
            && !jobs::pending_exists(&state.db, JobKind::Enrich, &media.id).await?
        {
            jobs::enqueue(
                state,
                JobKind::Enrich,
                json!({"media_id": media.id}),
                Some(media.id.clone()),
                Some(&set.owner_id),
            )
            .await?;
        }
    }
    Ok(Added {
        item_id,
        media,
        already_in_set: false,
    })
}

// ---------------------------------------------------------------------------
// Near-duplicates (only ever within the caller's own library)
// ---------------------------------------------------------------------------

/// (item id, media id, phash) of every item the user owns.
pub async fn own_hashes(db: &sqlx::SqlitePool, owner_id: &str) -> AppResult<Vec<(String, String, u64)>> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT i.id, i.media_id, m.phash FROM items i JOIN sets s ON s.id = i.set_id JOIN media m ON m.id = i.media_id \
         WHERE s.owner_id = ?",
    )
    .bind(owner_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|(i, m, p)| (i, m, i64_to_u64(p))).collect())
}

/// Near-duplicates of (item, media, phash) among `pool` (the owner's items).
pub async fn near_duplicates(
    state: &AppState,
    pool: &[(String, String, u64)],
    item_id: &str,
    media_id: &str,
    phash: u64,
) -> AppResult<Vec<NearDuplicate>> {
    let max = state.config.media.near_duplicate_distance;
    let mut found: Vec<(String, u32, bool)> = pool
        .iter()
        .filter(|(i, _, _)| i != item_id)
        .filter_map(|(i, m, p)| {
            let d = hamming(phash, *p);
            (d <= max || m == media_id).then(|| (i.clone(), if m == media_id { 0 } else { d }, m == media_id))
        })
        .collect();
    found.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    found.truncate(20);
    let ids: Vec<String> = found.iter().map(|f| f.0.clone()).collect();
    let summaries = hydrate::summaries_by_id(&state.db, &ids).await?;
    Ok(found
        .into_iter()
        .filter_map(|(id, distance, same_content)| {
            summaries.get(&id).map(|s| NearDuplicate {
                item_id: item_id.to_string(),
                existing: s.clone(),
                distance,
                same_content,
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Upload endpoint logic
// ---------------------------------------------------------------------------

/// Authorize (edit rights on the set), rate-limit, read the multipart body
/// and add the files: the whole upload endpoint, shared with the web UI.
pub async fn upload_multipart(
    state: &AppState,
    viewer: &CurrentUser,
    set_id: &str,
    mp: Multipart,
) -> AppResult<Vec<UploadResult>> {
    let set = authz::editable_set(state, viewer.id(), set_id).await?;
    state.limiters.check_upload(viewer.id())?;
    let parts = read_multipart(state, mp).await?;
    upload_parts(state, viewer, &set, parts).await
}

/// [`upload_multipart`] for already-parsed parts.
pub async fn upload(
    state: &AppState,
    viewer: &CurrentUser,
    set_id: &str,
    parts: UploadParts,
) -> AppResult<Vec<UploadResult>> {
    let set = authz::editable_set(state, viewer.id(), set_id).await?;
    state.limiters.check_upload(viewer.id())?;
    upload_parts(state, viewer, &set, parts).await
}

/// Add uploaded files to `set` (authorization and rate limiting already
/// done). Per-file problems are reported in `UploadResult::error`; a
/// single-file upload that fails returns the error itself (413 / 415 / ...).
async fn upload_parts(
    state: &AppState,
    viewer: &CurrentUser,
    set: &SetRow,
    parts: UploadParts,
) -> AppResult<Vec<UploadResult>> {
    let set = set.clone();
    let single = parts.files.len() == 1;
    let mut pool = own_hashes(&state.db, viewer.id()).await?;
    let mut results = Vec::with_capacity(parts.files.len());
    let mut touched = Vec::new();
    for (i, file) in parts.files.iter().enumerate() {
        let shown = base_name(&file.filename).to_string();
        let outcome = async {
            let input = ItemInput::from_upload(&file.filename, parts.meta.get(i))?;
            add_to_set(state, &set, file, &input).await
        }
        .await;
        match outcome {
            Ok(added) => {
                let near = near_duplicates(
                    state,
                    &pool,
                    &added.item_id,
                    &added.media.id,
                    i64_to_u64(added.media.phash),
                )
                .await?;
                if !added.already_in_set {
                    pool.push((
                        added.item_id.clone(),
                        added.media.id.clone(),
                        i64_to_u64(added.media.phash),
                    ));
                    touched.push(added.item_id.clone());
                }
                let row = hydrate::fetch_one(&state.db, &added.item_id)
                    .await?
                    .ok_or_else(AppError::not_found)?;
                let item = hydrate::items(&state.db, viewer.id(), std::slice::from_ref(&row))
                    .await?
                    .pop();
                results.push(UploadResult {
                    file: shown.clone(),
                    item,
                    error: None,
                    already_in_set: added.already_in_set,
                    near_duplicates: near,
                });
            }
            Err(e) if single => {
                index::refresh_items(state, &touched).await?;
                return Err(e);
            }
            Err(e) => {
                if e.status.is_server_error() {
                    tracing::error!("upload of {shown:?} failed: {e}");
                }
                results.push(UploadResult {
                    file: shown.clone(),
                    item: None,
                    error: Some(e.message.clone()),
                    already_in_set: false,
                    near_duplicates: Vec::new(),
                });
            }
        }
    }
    index::refresh_items(state, &touched).await?;
    Ok(results)
}
