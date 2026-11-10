//! Set directory import and export (stickpile-setfmt).
//!
//! * Import into a new set: slug from `set.toml`'s `slug` or the slugified
//!   name; an existing slug of the caller's -> 409 (unless `into`).
//! * Import `into` an existing own set makes it equal to the directory:
//!   files are matched by sha256 (of the uploaded bytes or of the stored
//!   sanitised original, so re-importing an export matches), metadata is
//!   updated (AI source markers and unknown keys preserved), new files are
//!   added, missing ones removed, and the order follows the directory.
//! * Export writes `set.toml` + originals named with `export_filename` as a
//!   tar; importing that tar into a fresh set and exporting again yields the
//!   same manifest data and byte-identical files.

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::extract::Multipart;
use stickpile_api::{ImportError, ImportReport, Variant};
use stickpile_setfmt::{ItemMeta, Manifest, SetBundle, SetFile};

use crate::auth::CurrentUser;
use crate::authz;
use crate::db::{self, i64_to_u64, source, source_str, to_json_list};
use crate::error::{AppError, AppResult};
use crate::files;
use crate::ids::{now, sha256_hex};
use crate::index;
use crate::service::hydrate::{self, ItemFull};
use crate::service::sets::{self, Audience, NewSet};
use crate::service::upload::{
    self, add_to_set, base_name, clean_list, clean_name, clean_vibe, field_text, multipart_error, own_hashes, stem_for,
    ItemInput, UploadFile,
};
use crate::state::AppState;

fn table_to_string(t: &toml::Table) -> Option<String> {
    if t.is_empty() {
        None
    } else {
        toml::to_string(t).ok()
    }
}

fn string_to_table(s: Option<&str>) -> toml::Table {
    s.and_then(|s| toml::from_str::<toml::Table>(s).ok())
        .unwrap_or_default()
}

/// Read an import multipart body: a `set.toml` part (by field name or
/// filename) plus file parts whose filename is the path relative to the set
/// directory, or a single `archive` part holding a tar.
pub async fn read_multipart(state: &AppState, mut mp: Multipart) -> AppResult<SetBundle> {
    let max_file = state.config.limits.max_upload_mb * 1024 * 1024;
    let max_total = state.config.limits.max_request_mb * 1024 * 1024;
    let mut manifest: Option<String> = None;
    let mut archive: Option<Vec<u8>> = None;
    let mut loose: Vec<(String, Vec<u8>)> = Vec::new();
    while let Some(mut field) = mp.next_field().await.map_err(multipart_error)? {
        let name = field.name().unwrap_or("").to_string();
        let filename = field.file_name().map(str::to_string);
        if name == "archive" {
            let mut buf = Vec::new();
            while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
                if (buf.len() + chunk.len()) as u64 > max_total {
                    return Err(AppError::too_large("archive too large"));
                }
                buf.extend_from_slice(&chunk);
            }
            archive = Some(buf);
        } else if name == stickpile_setfmt::MANIFEST
            || filename.as_deref().map(base_name) == Some(stickpile_setfmt::MANIFEST)
        {
            manifest = Some(field_text(&mut field, 1024 * 1024).await?);
        } else if let Some(filename) = filename {
            let f = upload::field_to_file(state, &mut field, max_file).await?;
            loose.push((filename, f.bytes().await?));
        }
    }
    let bundle = match archive {
        Some(tar) => tokio::task::spawn_blocking(move || stickpile_setfmt::read_tar(&tar, max_total)).await??,
        None if loose.is_empty() && manifest.is_none() => {
            return Err(AppError::bad_request(
                "expected a set.toml part and file parts, or an archive part",
            ));
        }
        None => stickpile_setfmt::from_files(manifest.as_deref(), loose)?,
    };
    Ok(bundle)
}

fn input_from_meta(filename: &str, meta: &ItemMeta) -> AppResult<ItemInput> {
    let stem_name = stickpile_setfmt::display_stem(filename);
    let name = match clean_name(&meta.name)? {
        n if n.is_empty() => stem_name.clone(),
        n => n,
    };
    Ok(ItemInput {
        name: if name.is_empty() { "item".into() } else { name },
        name_source: meta.name_source,
        keywords: clean_list(&meta.keywords, "keywords")?,
        keywords_source: meta.keywords_source,
        emojis: clean_list(&meta.emojis, "emojis")?,
        emojis_source: meta.emojis_source,
        vibe: clean_vibe(meta.vibe.as_deref())?,
        vibe_source: meta.vibe_source,
        tags: clean_list(&meta.tags, "tags")?,
        source_url: None,
        stem: stem_for(&stem_name),
        extra_toml: table_to_string(&meta.extra),
    })
}

/// Update an existing item's metadata to `input`; true if anything changed.
async fn apply_meta(conn: &mut sqlx::SqliteConnection, cur: &ItemFull, input: &ItemInput) -> AppResult<bool> {
    let same = cur.name == input.name
        && source(&cur.name_source) == input.name_source
        && cur.keywords() == input.keywords
        && source(&cur.keywords_source) == input.keywords_source
        && cur.emojis() == input.emojis
        && source(&cur.emojis_source) == input.emojis_source
        && cur.vibe == input.vibe
        && source(&cur.vibe_source) == input.vibe_source
        && cur.tags() == input.tags
        && cur.filename == input.stem
        && cur.extra_toml == input.extra_toml;
    if same {
        return Ok(false);
    }
    sqlx::query(
        "UPDATE items SET name = ?, name_source = ?, keywords = ?, keywords_source = ?, emojis = ?, emojis_source = ?, \
         vibe = ?, vibe_source = ?, tags = ?, filename = ?, extra_toml = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&input.name)
    .bind(source_str(input.name_source))
    .bind(to_json_list(&input.keywords))
    .bind(source_str(input.keywords_source))
    .bind(to_json_list(&input.emojis))
    .bind(source_str(input.emojis_source))
    .bind(&input.vibe)
    .bind(source_str(input.vibe_source))
    .bind(to_json_list(&input.tags))
    .bind(&input.stem)
    .bind(&input.extra_toml)
    .bind(now())
    .bind(&cur.id)
    .execute(conn)
    .await?;
    Ok(true)
}

/// The whole import endpoint (shared with the web UI): authorize `into`,
/// rate-limit, read the multipart body, import.
pub async fn import_multipart(
    state: &AppState,
    viewer: &CurrentUser,
    into: Option<&str>,
    mp: Multipart,
) -> AppResult<ImportReport> {
    if let Some(id) = into {
        authz::editable_set(state, viewer.id(), id).await?;
    }
    state.limiters.check_upload(viewer.id())?;
    let bundle = read_multipart(state, mp).await?;
    import_bundle(state, viewer, bundle, into).await
}

/// Import a bundle (rate-limited like an upload).
pub async fn import(
    state: &AppState,
    viewer: &CurrentUser,
    bundle: SetBundle,
    into: Option<&str>,
) -> AppResult<ImportReport> {
    state.limiters.check_upload(viewer.id())?;
    import_bundle(state, viewer, bundle, into).await
}

/// Import a bundle as a new set, or into an existing own set (`into`).
/// No rate limiting (callers do it); also used by git-pack pulls.
pub async fn import_bundle(
    state: &AppState,
    viewer: &CurrentUser,
    bundle: SetBundle,
    into: Option<&str>,
) -> AppResult<ImportReport> {
    let m = &bundle.manifest;
    let aspect = m.aspect();
    let extra = table_to_string(&m.extra);
    let mut report = ImportReport::default();
    for w in &bundle.warnings {
        report.errors.push(ImportError {
            file: String::new(),
            message: w.clone(),
        });
    }
    let (set, is_new) = match into {
        Some(id) => (authz::editable_set(state, viewer.id(), id).await?, false),
        None => {
            let slug = match m.slug.as_deref() {
                Some(s) => {
                    sets::validate_slug(s)?;
                    s.to_string()
                }
                None => stickpile_setfmt::slugify(&m.name),
            };
            let row = sets::insert(
                &state.db,
                viewer.id(),
                NewSet {
                    slug,
                    name: if m.name.trim().is_empty() {
                        "Imported set".into()
                    } else {
                        m.name.clone()
                    },
                    description: m.description.clone(),
                    visibility: m.visibility,
                    aspect_ratio: aspect,
                    recommended_px: m.recommended_px,
                    version: m.version,
                    extra_toml: extra.clone(),
                },
            )
            .await?;
            (row, true)
        }
    };

    let existing = if is_new {
        Vec::new()
    } else {
        hydrate::fetch_by_set(&state.db, &set.id).await?
    };
    let mut by_sha: HashMap<String, ItemFull> = HashMap::new();
    for it in &existing {
        by_sha.insert(it.original_sha256.clone(), it.clone());
        by_sha.insert(it.sha256.clone(), it.clone());
    }
    let mut pool = own_hashes(&state.db, viewer.id()).await?;
    let mut order: Vec<String> = Vec::new();
    let mut kept: HashSet<String> = HashSet::new();
    let mut changed = false;

    for f in bundle.files {
        let input = match input_from_meta(&f.filename, &f.meta) {
            Ok(i) => i,
            Err(e) => {
                report.errors.push(ImportError {
                    file: f.filename.clone(),
                    message: e.message,
                });
                continue;
            }
        };
        let sha = sha256_hex(&f.bytes);
        if let Some(cur) = by_sha.get(&sha) {
            if !kept.insert(cur.id.clone()) {
                report.errors.push(ImportError {
                    file: f.filename,
                    message: "duplicate content, skipped".into(),
                });
                continue;
            }
            let mut conn = state.db.acquire().await?;
            if apply_meta(&mut conn, cur, &input).await? {
                report.updated += 1;
                changed = true;
            } else {
                report.unchanged += 1;
            }
            order.push(cur.id.clone());
            continue;
        }
        let file = UploadFile::from_bytes(f.filename.clone(), f.bytes);
        match add_to_set(state, &set, &file, &input).await {
            Ok(added) if added.already_in_set => {
                if !kept.insert(added.item_id.clone()) {
                    report.errors.push(ImportError {
                        file: f.filename,
                        message: "duplicate content, skipped".into(),
                    });
                    continue;
                }
                let cur = hydrate::fetch_one(&state.db, &added.item_id)
                    .await?
                    .ok_or_else(AppError::not_found)?;
                let mut conn = state.db.acquire().await?;
                if apply_meta(&mut conn, &cur, &input).await? {
                    report.updated += 1;
                    changed = true;
                } else {
                    report.unchanged += 1;
                }
                order.push(added.item_id);
            }
            Ok(added) => {
                let phash = i64_to_u64(added.media.phash);
                let near = upload::near_duplicates(state, &pool, &added.item_id, &added.media.id, phash).await?;
                report.near_duplicates.extend(near);
                pool.push((added.item_id.clone(), added.media.id.clone(), phash));
                kept.insert(added.item_id.clone());
                order.push(added.item_id);
                report.created += 1;
                changed = true;
            }
            Err(e) => report.errors.push(ImportError {
                file: f.filename,
                message: e.message,
            }),
        }
    }

    let mut tx = sets::begin(&state.db).await?;
    // Remove items that are not in the directory.
    if !is_new {
        let stale: Vec<String> = existing
            .iter()
            .filter(|e| !kept.contains(&e.id))
            .map(|e| e.id.clone())
            .collect();
        if !stale.is_empty() {
            sets::tombstone(&mut tx, &stale, &set.owner_id, Audience::for_removal(set.visibility())).await?;
            for id in &stale {
                sqlx::query("DELETE FROM items WHERE id = ?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
            report.removed = stale.len() as u64;
            changed = true;
        }
    }
    // Order follows the directory; anything else (should be nothing) after.
    let current: Vec<String> = sqlx::query_scalar("SELECT id FROM items WHERE set_id = ? ORDER BY position, id")
        .bind(&set.id)
        .fetch_all(&mut *tx)
        .await?;
    let mut final_order = order.clone();
    final_order.extend(current.iter().filter(|id| !order.contains(id)).cloned());
    if final_order != current {
        changed = true;
    }
    for (pos, id) in final_order.iter().enumerate() {
        sqlx::query("UPDATE items SET position = ? WHERE id = ? AND set_id = ?")
            .bind(pos as i64)
            .bind(id)
            .bind(&set.id)
            .execute(&mut *tx)
            .await?;
    }
    if is_new {
        sqlx::query("UPDATE sets SET version = ?, updated_at = ? WHERE id = ?")
            .bind(m.version.clamp(1, i64::MAX as u64) as i64)
            .bind(now())
            .bind(&set.id)
            .execute(&mut *tx)
            .await?;
    } else {
        let meta_changed = set.name != m.name
            || set.description != m.description
            || set.visibility() != m.visibility
            || set.aspect() != aspect
            || set.recommended_px.map(|v| v as u32) != m.recommended_px
            || set.extra_toml != extra;
        if meta_changed {
            sets::on_visibility_change(&mut tx, &set, m.visibility).await?;
            let name = sets::validate_name(&m.name)?;
            let description = sets::validate_description(&m.description)?;
            sqlx::query(
                "UPDATE sets SET name = ?, description = ?, visibility = ?, aspect_w = ?, aspect_h = ?, \
                 recommended_px = ?, extra_toml = ? WHERE id = ?",
            )
            .bind(&name)
            .bind(&description)
            .bind(db::enum_str(&m.visibility))
            .bind(aspect.map(|a| a.0 as i64))
            .bind(aspect.map(|a| a.1 as i64))
            .bind(m.recommended_px.map(|v| v as i64))
            .bind(&extra)
            .bind(&set.id)
            .execute(&mut *tx)
            .await?;
            if set.name != name || set.visibility() != m.visibility {
                sets::touch_items(&mut tx, &set.id).await?;
            }
            changed = true;
        }
        let version = if changed {
            (set.version + 1).max(m.version.min(i64::MAX as u64) as i64)
        } else {
            set.version
        };
        sqlx::query("UPDATE sets SET version = ?, updated_at = ? WHERE id = ?")
            .bind(version)
            .bind(if changed { now() } else { set.updated_at })
            .bind(&set.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    index::refresh_set(state, &set.id).await?;
    let row = db::set_by_id(&state.db, &set.id)
        .await?
        .ok_or_else(AppError::not_found)?;
    report.set = Some(sets::load_api(&state.db, &row, viewer.id()).await?);
    Ok(report)
}

/// Build the export bundle of a set the caller can read.
pub async fn export_bundle(state: &AppState, viewer: &CurrentUser, set_id: &str) -> AppResult<(String, SetBundle)> {
    let set = authz::readable_set(state, viewer.id(), set_id).await?;
    let rows = hydrate::fetch_by_set(&state.db, &set.id).await?;
    let media_ids: Vec<String> = rows.iter().map(|r| r.media_id.clone()).collect();
    let variants = hydrate::variant_rows(&state.db, &media_ids).await?;
    let total = rows.len();
    let mut out_files = Vec::with_capacity(total);
    for (i, r) in rows.iter().enumerate() {
        let original = variants
            .get(&r.media_id)
            .and_then(|vs| vs.iter().find(|v| v.variant() == Some(Variant::Original)))
            .ok_or_else(|| AppError::internal(format!("original of media {} missing", r.media_id)))?;
        let abs = files::resolve(&state.media_dir(), &original.path).ok_or_else(|| AppError::internal("bad path"))?;
        let bytes = files::read(abs).await?;
        let filename = stickpile_setfmt::export_filename(i + 1, total, &r.filename, r.format().extension());
        let meta = ItemMeta {
            name: r.name.clone(),
            name_source: source(&r.name_source),
            keywords: r.keywords(),
            keywords_source: source(&r.keywords_source),
            emojis: r.emojis(),
            emojis_source: source(&r.emojis_source),
            vibe: r.vibe.clone(),
            vibe_source: source(&r.vibe_source),
            tags: r.tags(),
            extra: string_to_table(r.extra_toml.as_deref()),
        };
        out_files.push(SetFile { filename, meta, bytes });
    }
    let manifest = Manifest {
        version: set.version.max(1) as u64,
        name: set.name.clone(),
        slug: Some(set.slug.clone()),
        description: set.description.clone(),
        visibility: set.visibility(),
        aspect_ratio: set.aspect().map(|(w, h)| format!("{w}:{h}")),
        recommended_px: set.recommended_px.map(|v| v as u32),
        items: out_files
            .iter()
            .map(|f| (f.filename.clone(), f.meta.clone()))
            .collect::<BTreeMap<_, _>>(),
        extra: string_to_table(set.extra_toml.as_deref()),
    };
    Ok((
        set.slug.clone(),
        SetBundle {
            manifest,
            files: out_files,
            warnings: Vec::new(),
        },
    ))
}

/// Export a set as a tar (`<slug>/set.toml` + files).
pub async fn export_tar(state: &AppState, viewer: &CurrentUser, set_id: &str) -> AppResult<(String, Vec<u8>)> {
    let (slug, bundle) = export_bundle(state, viewer, set_id).await?;
    let top = slug.clone();
    let tar = tokio::task::spawn_blocking(move || stickpile_setfmt::write_tar(&bundle, &top)).await??;
    Ok((slug, tar))
}
