//! Telegram import and export through the Bot API (docs/telegram.md).
//!
//! * [`import`] queues a `TelegramImport` job ([`import_job`]):
//!   `stickpile_telegram::import_pack` downloads every sticker (capped at
//!   `limits.max_upload_mb`), a new set (`source = telegram`, named after the
//!   pack title) is created and each sticker goes through the normal ingest
//!   ([`upload::add_to_set`]) with its emoji as user metadata. Animated
//!   `.tgs` stickers and files that fail ingest are listed in the job result.
//!   If the bot itself owns the pack, the set is bound to it and the export
//!   mapping seeded, so a later export keeps the existing stickers.
//! * [`export`] queues a `TelegramExport` job ([`export_job`]): every item
//!   becomes its `tg_sticker` (stills) or `tg_video_sticker` (animations and
//!   videos) variant. Variants the `Variants` job has not produced yet are
//!   rendered in the job. Items that cannot become stickers are skipped and
//!   listed. The pack name and mapping are stored in `sets.telegram_*`.
//!
//! The pack owner is the exporting user's `telegram_user_id` (`PATCH
//! /api/v1/me`), else `telegram.default_owner_user_id`.

use std::collections::HashSet;

use axum::http::StatusCode;
use serde_json::{json, Value};
use stickpile_api::{Job, JobKind, MediaKind, TelegramExport, TelegramImport, Variant, Visibility};
use stickpile_telegram::{
    BotApi, ExportItem, PackMapping, PackState, PackThumbnail, StickerFormat, TelegramError, MAX_PACK_STICKERS,
};

use crate::auth::CurrentUser;
use crate::authz;
use crate::config::read_secret;
use crate::db::{self, VariantRow};
use crate::error::{AppError, AppResult};
use crate::files;
use crate::ids;
use crate::index;
use crate::jobs::{self, JobError, JobResult, JobRow};
use crate::service::sets::{self, NewSet};
use crate::service::upload::{self, clean_list, stem_for, ItemInput, UploadFile};
use crate::service::{cover, hydrate};
use crate::state::AppState;

const DISABLED: &str =
    "the Telegram integration is disabled on this server (telegram.enabled or the bot token is missing)";
const NO_OWNER: &str = "no Telegram user id to own the pack: set yours in your profile (telegram_user_id, \
     PATCH /api/v1/me) or ask the admin to set telegram.default_owner_user_id";

/// The integration is configured: `telegram.enabled` and a readable,
/// non-empty bot token file.
pub fn enabled(state: &AppState) -> bool {
    state.config.telegram.enabled && token(state).is_some()
}

fn token(state: &AppState) -> Option<String> {
    let path = state.config.telegram.bot_token_file.as_ref()?;
    read_secret(path)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn disabled() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "not_found", DISABLED)
}

fn bot(state: &AppState) -> Result<BotApi, JobError> {
    if !state.config.telegram.enabled {
        return Err(JobError::Fatal(DISABLED.into()));
    }
    let token = token(state).ok_or_else(|| JobError::Fatal("cannot read telegram.bot_token_file".into()))?;
    Ok(BotApi::new(&state.config.telegram.api_url, &token))
}

/// `https://t.me/addstickers/<name>`.
pub fn pack_url(name: &str) -> String {
    format!("https://t.me/addstickers/{name}")
}

/// A pack short name as users paste it: bare, or a `t.me/addstickers/` link.
fn clean_pack_name(raw: &str) -> AppResult<String> {
    let s = raw.trim().trim_end_matches('/');
    let s = s.rsplit_once("/addstickers/").map(|(_, n)| n).unwrap_or(s);
    let ok = (1..=64).contains(&s.len())
        && s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(s.to_string())
    } else {
        Err(AppError::bad_request(
            "pack_name must be a sticker pack short name (1-64 of A-Z a-z 0-9 _, starting with a letter) or a t.me/addstickers/ link",
        ))
    }
}

/// Telegram user id that owns packs exported by `user`.
fn owner_id(state: &AppState, user: &db::UserRow) -> Option<i64> {
    user.telegram_user_id
        .or(state.config.telegram.default_owner_user_id)
        .filter(|id| *id > 0)
}

fn tg_error(e: TelegramError) -> JobError {
    match &e {
        TelegramError::Http(_) => JobError::Retry(e.to_string()),
        TelegramError::Api { code, .. } if *code == 429 || *code >= 500 => JobError::Retry(e.to_string()),
        _ if e.is_missing_sticker_set() => JobError::Fatal("sticker pack not found".into()),
        _ => JobError::Fatal(e.to_string()),
    }
}

fn retry(e: impl std::fmt::Display) -> JobError {
    JobError::Retry(e.to_string())
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Queue the import of a pack into a new set of the caller. The slug
/// defaults to the slugified pack name and must be free (409).
pub async fn import(state: &AppState, user: &CurrentUser, req: TelegramImport) -> AppResult<Job> {
    if !enabled(state) {
        return Err(disabled());
    }
    let name = clean_pack_name(&req.pack_name)?;
    let slug = match req.slug.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(s) => s,
        None => stickpile_setfmt::slugify(&name.replace('_', "-")),
    };
    sets::validate_slug(&slug)?;
    let taken: Option<i64> = sqlx::query_scalar("SELECT 1 FROM sets WHERE owner_id = ? AND slug = ?")
        .bind(user.id())
        .bind(&slug)
        .fetch_optional(&state.db)
        .await?;
    if taken.is_some() {
        return Err(AppError::conflict(format!("you already have a set with slug {slug:?}")));
    }
    state.limiters.check_upload(user.id())?;
    jobs::enqueue(
        state,
        JobKind::TelegramImport,
        json!({ "pack_name": name, "slug": slug, "visibility": req.visibility }),
        Some(name.clone()),
        Some(user.id()),
    )
    .await
}

/// `TelegramImport` job.
pub async fn import_job(state: &AppState, job: &JobRow) -> JobResult {
    let p = job.payload();
    let name = p["pack_name"]
        .as_str()
        .ok_or_else(|| JobError::Fatal("payload without pack_name".into()))?
        .to_string();
    let slug = p["slug"]
        .as_str()
        .ok_or_else(|| JobError::Fatal("payload without slug".into()))?
        .to_string();
    let visibility: Visibility = serde_json::from_value(p["visibility"].clone()).unwrap_or_default();
    let Some(owner) = job.owner_id.as_deref() else {
        return Err(JobError::Fatal("job without owner".into()));
    };
    let Some(user) = db::user_by_id(&state.db, owner).await.map_err(retry)? else {
        return Ok(Some(json!({"skipped": "the user no longer exists"})));
    };
    let api = bot(state)?;
    let max = state.config.limits.max_upload_mb.saturating_mul(1024 * 1024);
    // Everything is downloaded before the set exists, so a retry after a
    // network error starts clean.
    let pack = stickpile_telegram::import_pack(&api, &name, max)
        .await
        .map_err(tg_error)?;
    let bot_username = api.get_me().await.ok().and_then(|b| b.username);

    let title = pack.title.trim();
    let set_name = sets::validate_name(title).unwrap_or_else(|_| pack.name.clone());
    let row = sets::insert(
        &state.db,
        &user.id,
        NewSet {
            slug,
            name: set_name,
            description: String::new(),
            visibility,
            aspect_ratio: Some((1, 1)),
            recommended_px: Some(512),
            version: 1,
            extra_toml: None,
        },
    )
    .await
    .map_err(|e| JobError::Fatal(e.message))?;
    sqlx::query("UPDATE sets SET source = 'telegram' WHERE id = ?")
        .bind(&row.id)
        .execute(&state.db)
        .await
        .map_err(retry)?;

    let mut skipped: Vec<Value> = pack
        .skipped
        .iter()
        .map(|(fuid, reason)| json!({"file_unique_id": fuid, "reason": reason}))
        .collect();
    let mut mapping = PackMapping::new();
    let mut imported = 0u32;
    for (i, sticker) in pack.stickers.into_iter().enumerate() {
        let ext = match sticker.format {
            StickerFormat::Video => "webm",
            _ => "webp",
        };
        let item_name = format!("sticker {}", i + 1);
        let input = ItemInput {
            stem: stem_for(&item_name),
            name: item_name,
            emojis: clean_list(&sticker.emojis, "emojis").unwrap_or_default(),
            ..Default::default()
        };
        let file = UploadFile::from_bytes(format!("{:03}-sticker.{ext}", i + 1), sticker.bytes);
        match upload::add_to_set(state, &row, &file, &input).await {
            Ok(added) if added.already_in_set => skipped.push(json!({
                "file_unique_id": sticker.file_unique_id,
                "reason": "the same file as an earlier sticker of the pack",
            })),
            Ok(added) => {
                imported += 1;
                mapping.insert(added.item_id, (added.media.sha256.clone(), sticker.file_unique_id));
            }
            Err(e) if e.status.is_server_error() => return Err(retry(e)),
            Err(e) => skipped.push(json!({"file_unique_id": sticker.file_unique_id, "reason": e.message})),
        }
    }
    index::refresh_set(state, &row.id).await?;
    // A pack the bot created can be exported back: bind it and keep the
    // stickers on the first export. Only for its owner here, or for an admin
    // adopting a pack made outside stickpile; anyone else gets a copy.
    let own_bot_pack = bot_username
        .as_deref()
        .is_some_and(|b| stickpile_telegram::valid_pack_name(&pack.name, b));
    let bound = own_bot_pack
        && match pack_owner(state, &pack.name).await? {
            Some(owner) => owner == user.id,
            None => user.is_admin() && claim_pack(state, &pack.name, &user.id).await? == user.id,
        };
    if bound {
        bind_pack(state, &row.id, &pack.name, &mapping, None).await?;
    }
    Ok(Some(json!({
        "set_id": row.id,
        "slug": row.slug,
        "pack_name": pack.name,
        "title": pack.title,
        "url": pack_url(&pack.name),
        "imported": imported,
        "skipped": skipped,
        "bound": bound,
    })))
}

// ---------------------------------------------------------------------------
// Pack ownership
// ---------------------------------------------------------------------------

/// The bot can edit every pack it ever created, whoever asked for it, so
/// stickpile decides who may: the user who created the pack through this
/// server, or the admin who adopted one made elsewhere (`telegram_packs`).
const PACK_TAKEN: &str = "belongs to another stickpile user; choose another pack name";

/// Owner (stickpile user id) of pack `name`, if it was created or adopted
/// here. Names compare case-insensitively, as on Telegram.
async fn pack_owner(state: &AppState, name: &str) -> Result<Option<String>, JobError> {
    sqlx::query_scalar("SELECT user_id FROM telegram_packs WHERE name = ?")
        .bind(name)
        .fetch_optional(&state.db)
        .await
        .map_err(retry)
}

/// Record `user_id` as the owner of pack `name` unless someone already is,
/// and return the owner afterwards: of two exports racing for a new name,
/// the first claim wins.
async fn claim_pack(state: &AppState, name: &str, user_id: &str) -> Result<String, JobError> {
    sqlx::query("INSERT INTO telegram_packs (name, user_id, created_at) VALUES (?, ?, ?) ON CONFLICT(name) DO NOTHING")
        .bind(name)
        .bind(user_id)
        .bind(ids::now())
        .execute(&state.db)
        .await
        .map_err(retry)?;
    pack_owner(state, name)
        .await?
        .ok_or_else(|| JobError::Retry(format!("the claim on pack {name} vanished")))
}

/// Bind set `set_id` to pack `name`. A pack follows exactly one set: another
/// set bound to it (necessarily the same user's) loses its binding.
/// Bind `set_id` to pack `name` with the sticker `mapping` and `thumbnail`,
/// the key of the thumbnail stickpile set on the pack (None: none).
async fn bind_pack(
    state: &AppState,
    set_id: &str,
    name: &str,
    mapping: &PackMapping,
    thumbnail: Option<&str>,
) -> Result<(), JobError> {
    let mapping = serde_json::to_string(mapping).map_err(retry)?;
    let mut tx = state.db.begin().await.map_err(retry)?;
    sqlx::query(
        "UPDATE sets SET telegram_pack = NULL, telegram_mapping = NULL, telegram_thumbnail = NULL \
         WHERE telegram_pack = ? COLLATE NOCASE AND id <> ?",
    )
    .bind(name)
    .bind(set_id)
    .execute(&mut *tx)
    .await
    .map_err(retry)?;
    sqlx::query("UPDATE sets SET telegram_pack = ?, telegram_mapping = ?, telegram_thumbnail = ? WHERE id = ?")
        .bind(name)
        .bind(mapping)
        .bind(thumbnail)
        .bind(set_id)
        .execute(&mut *tx)
        .await
        .map_err(retry)?;
    tx.commit().await.map_err(retry)
}

/// May `user` export to pack `name`? Claims the name for them when it is
/// new. A pack that exists on Telegram but was not created through this
/// server can only be taken over by an admin.
async fn authorize_export(state: &AppState, api: &BotApi, user: &db::UserRow, name: &str) -> Result<(), JobError> {
    match pack_owner(state, name).await? {
        Some(owner) if owner == user.id => return Ok(()),
        Some(_) => return Err(JobError::Fatal(format!("the pack {name} {PACK_TAKEN}"))),
        None => {}
    }
    match api.get_sticker_set(name).await {
        Err(e) if e.is_missing_sticker_set() => {}
        Err(e) => return Err(tg_error(e)),
        Ok(_) if user.is_admin() => {}
        Ok(_) => {
            return Err(JobError::Fatal(format!(
                "a pack named {name} already exists but was not published through this server; \
                 choose another pack name (an administrator can take it over)"
            )))
        }
    }
    if claim_pack(state, name, &user.id).await? == user.id {
        Ok(())
    } else {
        Err(JobError::Fatal(format!("the pack {name} {PACK_TAKEN}")))
    }
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// Queue the export of one of the caller's sets as a sticker pack the bot
/// owns (created on the first export, updated afterwards).
pub async fn export(state: &AppState, user: &CurrentUser, set_id: &str, req: TelegramExport) -> AppResult<Job> {
    if !enabled(state) {
        return Err(disabled());
    }
    let set = authz::owned_set(state, user.id(), set_id).await?;
    let pack_name = match req.pack_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => Some(clean_pack_name(p)?),
        None => None,
    };
    let title = req.title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    if title
        .as_ref()
        .is_some_and(|t| t.chars().count() > 64 || t.chars().any(char::is_control))
    {
        return Err(AppError::bad_request("title must be at most 64 printable characters"));
    }
    let row = db::user_by_id(&state.db, user.id())
        .await?
        .ok_or_else(|| AppError::unauthorized("account no longer exists"))?;
    if owner_id(state, &row).is_none() {
        return Err(AppError::bad_request(NO_OWNER));
    }
    jobs::enqueue(
        state,
        JobKind::TelegramExport,
        json!({ "set_id": set.id, "pack_name": pack_name, "title": title }),
        Some(set.id.clone()),
        Some(user.id()),
    )
    .await
}

fn find_variant(rows: Option<&Vec<VariantRow>>, want: Variant) -> Option<VariantRow> {
    rows?.iter().find(|v| v.variant() == Some(want)).cloned()
}

/// The pack thumbnail for the set's cover (None: neither a cover image nor a
/// chosen item, Telegram shows the first sticker), made 100×100. A cover
/// image is rendered from its stored original (always a still), a chosen
/// item from its grid thumbnail, which every kind of item has (a still frame
/// for animations and videos). The key says what was sent: `image:<sha256>`
/// or `<item id>:<sha256>`, so a new image or item uploads it again.
async fn cover_thumbnail(
    state: &AppState,
    set: &db::SetRow,
    rows: &[hydrate::ItemFull],
    variants: &mut std::collections::HashMap<String, Vec<VariantRow>>,
) -> Result<Option<PackThumbnail>, JobError> {
    if let Some(media_id) = &set.cover_media_id {
        let missing = || JobError::Fatal("the set's cover image is missing".into());
        let media = db::media_by_id(&state.db, media_id)
            .await
            .map_err(retry)?
            .ok_or_else(missing)?;
        let original = cover::variant_row(&state.db, media_id, Variant::Original)
            .await?
            .ok_or_else(missing)?;
        return Ok(Some(PackThumbnail {
            key: format!("image:{}", media.sha256),
            png: render_thumbnail(state, &original.path).await?,
        }));
    }
    let Some(cover) = sets::chosen_cover(&state.db, &set.id).await? else {
        return Ok(None);
    };
    let Some(r) = rows.iter().find(|r| r.id == cover) else {
        return Ok(None);
    };
    let mut thumb = find_variant(variants.get(&r.media_id), Variant::Thumb);
    if thumb.is_none() {
        jobs::media::generate_variants(state, &r.media_id)
            .await
            .map_err(|e| JobError::Fatal(format!("rendering the cover: {e}")))?;
        let fresh = hydrate::variant_rows(&state.db, std::slice::from_ref(&r.media_id)).await?;
        variants.extend(fresh);
        thumb = find_variant(variants.get(&r.media_id), Variant::Thumb);
    }
    let Some(v) = thumb else {
        return Err(JobError::Fatal(
            "the cover has no thumbnail to make the pack's from".into(),
        ));
    };
    Ok(Some(PackThumbnail {
        key: format!("{}:{}", r.id, r.sha256),
        png: render_thumbnail(state, &v.path).await?,
    }))
}

/// A still media file (`path` under the media dir) as a pack thumbnail PNG.
async fn render_thumbnail(state: &AppState, path: &str) -> Result<Vec<u8>, JobError> {
    let abs = files::resolve(&state.media_dir(), path).ok_or_else(|| JobError::Fatal("bad path".into()))?;
    let bytes = files::read(abs).await?;
    let limits = state.media_limits();
    tokio::task::spawn_blocking(move || {
        let frame = stickpile_media::decode_frame(&bytes, &limits)?;
        stickpile_media::pack_thumbnail(&frame)
    })
    .await
    .map_err(retry)?
    .map_err(|e| JobError::Fatal(format!("rendering the cover: {e}")))
}

/// `TelegramExport` job.
pub async fn export_job(state: &AppState, job: &JobRow) -> JobResult {
    let p = job.payload();
    let set_id = p["set_id"]
        .as_str()
        .ok_or_else(|| JobError::Fatal("payload without set_id".into()))?
        .to_string();
    let lock = jobs::subject_lock(&format!("telegram:{set_id}"));
    let _guard = lock.lock().await;
    let Some(set) = db::set_by_id(&state.db, &set_id).await.map_err(retry)? else {
        return Ok(Some(json!({"skipped": "the set no longer exists"})));
    };
    let Some(user) = db::user_by_id(&state.db, &set.owner_id).await.map_err(retry)? else {
        return Ok(Some(json!({"skipped": "the owner no longer exists"})));
    };
    let owner = owner_id(state, &user).ok_or_else(|| JobError::Fatal(NO_OWNER.into()))?;
    let api = bot(state)?;
    let me = api.get_me().await.map_err(tg_error)?;
    let bot_name = me
        .username
        .ok_or_else(|| JobError::Fatal("the bot has no username (getMe)".into()))?;
    let name = match p["pack_name"].as_str() {
        Some(n) => stickpile_telegram::complete_pack_name(n, &bot_name),
        None => match &set.telegram_pack {
            Some(n) => n.clone(),
            None => stickpile_telegram::default_pack_name(&set.slug, &bot_name)
                .ok_or_else(|| JobError::Fatal(format!("cannot derive a pack name from {:?}", set.slug)))?,
        },
    };
    if !stickpile_telegram::valid_pack_name(&name, &bot_name) {
        return Err(JobError::Fatal(format!(
            "pack name {name:?} is not valid for @{bot_name}: Telegram requires 1-64 characters of A-Z a-z 0-9 _, \
             starting with a letter, without \"__\", ending in _by_{bot_name} (added for you when the name has no \
             _by_…bot suffix), and a bot can only edit its own packs"
        )));
    }
    authorize_export(state, &api, &user, &name).await?;
    let title = p["title"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| set.name.clone());

    let (bound, mapping, thumbnail_key): (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as("SELECT telegram_pack, telegram_mapping, telegram_thumbnail FROM sets WHERE id = ?")
            .bind(&set.id)
            .fetch_one(&state.db)
            .await
            .map_err(retry)?;
    // The stored mapping and thumbnail belong to the bound pack only.
    let same_pack = bound.as_deref() == Some(name.as_str());
    let previous: PackMapping = match (same_pack, mapping) {
        (true, Some(m)) => serde_json::from_str(&m).unwrap_or_default(),
        _ => PackMapping::new(),
    };
    let previous_thumbnail = thumbnail_key.filter(|_| same_pack);

    let rows = hydrate::fetch_by_set(&state.db, &set.id).await?;
    let media_ids: Vec<String> = rows.iter().map(|r| r.media_id.clone()).collect();
    let mut variants = hydrate::variant_rows(&state.db, &media_ids).await?;
    let mut generated: HashSet<String> = HashSet::new();
    let mut items = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    for r in &rows {
        let (want, format) = match r.kind() {
            MediaKind::Still => (Variant::TgSticker, StickerFormat::Static),
            MediaKind::Animation | MediaKind::Video => (Variant::TgVideoSticker, StickerFormat::Video),
        };
        let mut variant = find_variant(variants.get(&r.media_id), want);
        let mut why: Option<String> = None;
        if variant.is_none() && generated.insert(r.media_id.clone()) {
            // The Variants job has not run (or failed): render it now.
            if let Err(e) = jobs::media::generate_variants(state, &r.media_id).await {
                why = Some(e.to_string());
            }
            let fresh = hydrate::variant_rows(&state.db, std::slice::from_ref(&r.media_id)).await?;
            variants.extend(fresh);
            variant = find_variant(variants.get(&r.media_id), want);
        }
        let Some(v) = variant else {
            let reason = why.unwrap_or_else(|| match want {
                Variant::TgSticker => "no Telegram sticker rendition could be made".to_string(),
                _ if !state.ffmpeg_available => "animated stickers need ffmpeg on the server".to_string(),
                _ => "cannot be converted to a Telegram video sticker (at most 3 s, 512 px, 256 KB)".to_string(),
            });
            skipped.push(json!({"item_id": r.id, "name": r.name, "reason": reason}));
            continue;
        };
        let abs = files::resolve(&state.media_dir(), &v.path).ok_or_else(|| JobError::Fatal("bad path".into()))?;
        let mut bytes = files::read(abs).await?;
        if format == StickerFormat::Static {
            // The stored sticker is lossless for chats; a pack takes at most
            // 512 KB, so a larger one is re-encoded lossy for the pack only.
            let fitted = tokio::task::spawn_blocking(move || stickpile_media::tg_pack_sticker(&bytes))
                .await
                .map_err(|e| JobError::Retry(e.to_string()))?;
            match fitted {
                Ok(Some(b)) => bytes = b,
                Ok(None) => {
                    skipped.push(json!({"item_id": r.id, "name": r.name,
                        "reason": "does not fit Telegram's 512 KB sticker limit"}));
                    continue;
                }
                Err(e) => {
                    skipped.push(json!({"item_id": r.id, "name": r.name, "reason": e.to_string()}));
                    continue;
                }
            }
        }
        items.push(ExportItem {
            key: r.id.clone(),
            content_key: r.sha256.clone(),
            bytes,
            format,
            emojis: r.emojis(),
            keywords: r.keywords(),
        });
    }
    if items.is_empty() {
        return Err(JobError::Fatal(
            "nothing to export: no item of the set can become a Telegram sticker".into(),
        ));
    }
    if items.len() > MAX_PACK_STICKERS {
        return Err(JobError::Fatal(format!(
            "the set has {} exportable items; a Telegram pack takes at most {MAX_PACK_STICKERS}: split the set",
            items.len()
        )));
    }
    let thumbnail = cover_thumbnail(state, &set, &rows, &mut variants).await?;
    let previous = PackState {
        mapping: previous,
        thumbnail: previous_thumbnail,
    };
    let report = stickpile_telegram::export_pack(&api, owner, &name, &title, &items, &previous, thumbnail.as_ref())
        .await
        .map_err(tg_error)?;
    bind_pack(state, &set.id, &name, &report.mapping, report.thumbnail.as_deref()).await?;
    Ok(Some(json!({
        "set_id": set.id,
        "pack_name": name,
        "url": pack_url(&name),
        "title": title,
        "created_pack": report.created_pack,
        "added": report.added,
        "removed": report.removed,
        "updated": report.updated,
        "reordered": report.reordered,
        "thumbnail": report.thumbnail.is_some(),
        "thumbnail_changed": report.thumbnail_changed,
        "exported": items.len(),
        "skipped": skipped,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_names_from_links() {
        assert_eq!(clean_pack_name("Animals").unwrap(), "Animals");
        assert_eq!(
            clean_pack_name(" https://t.me/addstickers/Cats_by_bot/ ").unwrap(),
            "Cats_by_bot"
        );
        assert_eq!(clean_pack_name("t.me/addstickers/Dogs").unwrap(), "Dogs");
        for bad in ["", "1abc", "a b", "a/b", "ä", &"a".repeat(65)] {
            assert!(clean_pack_name(bad).is_err(), "{bad}");
        }
    }
}
