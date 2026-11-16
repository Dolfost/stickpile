//! Signal sticker packs (docs/signal.md), on top of `stickpile-signal`.
//!
//! * [`import`] queues a `SignalImport` job ([`import_job`]):
//!   `stickpile_signal::import_pack` reads a pack from its link (CDN only,
//!   no account) and a new set named after the pack's title gets its
//!   stickers in order, each with its emoji, and its cover. The link holds
//!   the pack's key: it is in the job's payload only while the job may run
//!   and is removed from it when the job finishes
//!   ([`crate::jobs`] `scrub`); it is never logged or put in a result.
//! * Publishing is the [`SignalPlatform`] of the registry
//!   ([`crate::service::publish`]), configured when `signal.enabled` and
//!   `signal.credentials_file` reads as credentials. The `SignalExport` job
//!   ([`export_job`]) renders every item as a 512x512 sticker
//!   ([`crate::service::renditions`]) and uploads a **new** pack each time
//!   (Signal packs cannot change); the newest link is the set's
//!   publication link.
//! * [`ready_zip`]: the Signal-ready zip (`stickpile_signal::signal_ready_zip`)
//!   for anyone who can read the set, with or without Signal configured:
//!   the same stickers for Signal Desktop's sticker creator or signal-cli.
//!   The last one is kept in `<data_dir>/exports/<set id>/signal/` and
//!   reused while the set is unchanged.

use std::path::PathBuf;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use stickpile_api::{Job, JobKind, SignalImport, Visibility};
use stickpile_signal::{
    Credentials, ImageFormat, PackFile, PackLink, SignalClient, SignalConfig, SignalError, UploadPack, UploadSticker,
    MAX_STICKERS,
};

use crate::auth::CurrentUser;
use crate::authz;
use crate::db::{self, SetRow};
use crate::error::{AppError, AppResult};
use crate::files;
use crate::i18n::{t, tf};
use crate::ids::is_id;
use crate::index;
use crate::jobs::{self, JobError, JobResult, JobRow};
use crate::service::cover;
use crate::service::hydrate;
use crate::service::publish::{self, BoxFuture, Platform, PlatformOption};
use crate::service::renditions::{self, Kind, Rendered};
use crate::service::sets::{self, NewSet};
use crate::service::upload::{self, clean_list, stem_for, ItemInput, UploadFile};
use crate::service::whatsapp;
use crate::state::AppState;

/// The registry's id, and the `set_publications.platform` of Signal rows.
pub const PLATFORM: &str = "signal";

const DISABLED: &str = "the Signal integration is disabled on this server (signal.enabled)";
/// Changes when the zip would come out differently for the same set.
const ZIP_FORMAT: &str = "signal-zip-1";
const ZIP_MANIFEST: &str = "manifest.json";

/// Pack import (and the integration as a whole) is on: `signal.enabled`.
pub fn enabled(state: &AppState) -> bool {
    state.config.signal.enabled
}

/// Publishing is possible: enabled, and the credentials file reads as
/// credentials.
pub fn configured(state: &AppState) -> bool {
    enabled(state) && credentials(state).is_ok()
}

/// The account that uploads packs. Errors name the file, never its content.
fn credentials(state: &AppState) -> Result<Credentials, SignalError> {
    let path = state
        .config
        .signal
        .credentials_file
        .as_ref()
        .ok_or_else(|| SignalError::Credentials("signal.credentials_file is not set".into()))?;
    Credentials::from_file(path)
}

fn disabled() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "not_found", DISABLED)
}

fn client(state: &AppState) -> Result<SignalClient, JobError> {
    let c = &state.config.signal;
    SignalClient::new(&SignalConfig {
        service_url: c.service_url.clone(),
        cdn_url: c.cdn_url.clone(),
        trust_public_roots: c.trust_public_roots,
    })
    .map_err(|e| JobError::Fatal(e.to_string()))
}

fn signal_error(e: SignalError) -> JobError {
    match &e {
        SignalError::Http(_) => JobError::Retry(e.to_string()),
        SignalError::Status { status, .. } if *status >= 500 => JobError::Retry(e.to_string()),
        _ => JobError::Fatal(e.to_string()),
    }
}

fn retry(e: impl std::fmt::Display) -> JobError {
    JobError::Retry(e.to_string())
}

/// The payload of a finished `SignalImport` job without the link: it holds
/// the pack's key.
pub fn scrubbed_payload(payload: &Value) -> Option<Value> {
    let mut p = payload.clone();
    let obj = p.as_object_mut()?;
    obj.remove("link")?;
    Some(p)
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Queue the import of a pack (its link) into a new set of the caller. The
/// slug, when not given, comes from the pack's title.
pub async fn import(state: &AppState, user: &CurrentUser, req: SignalImport) -> AppResult<Job> {
    if !enabled(state) {
        return Err(disabled());
    }
    let link = PackLink::parse(&req.link).map_err(|_| {
        AppError::bad_request(
            "link must be a Signal sticker pack link: https://signal.art/addstickers/#pack_id=...&pack_key=... or sgnl://addstickers/?pack_id=...&pack_key=...",
        )
    })?;
    let slug = sets::requested_slug(&state.db, user.id(), req.slug.as_deref()).await?;
    state.limiters.check_upload(user.id())?;
    jobs::enqueue(
        state,
        JobKind::SignalImport,
        json!({ "link": link.url(), "slug": slug, "visibility": req.visibility }),
        None,
        Some(user.id()),
    )
    .await
}

fn extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Jpeg => "jpg",
        _ => "png",
    }
}

/// `SignalImport` job.
pub async fn import_job(state: &AppState, job: &JobRow) -> JobResult {
    let p = job.payload();
    let link = PackLink::parse(p["link"].as_str().unwrap_or_default())
        .map_err(|_| JobError::Fatal("payload without a valid link".into()))?;
    let visibility: Visibility = serde_json::from_value(p["visibility"].clone()).unwrap_or_default();
    let Some(owner) = job.owner_id.as_deref() else {
        return Err(JobError::Fatal("job without owner".into()));
    };
    let Some(user) = db::user_by_id(&state.db, owner).await.map_err(retry)? else {
        return Ok(Some(json!({"skipped": "the user no longer exists"})));
    };
    if !enabled(state) {
        return Err(JobError::Fatal(DISABLED.into()));
    }
    let max = state.config.limits.max_upload_mb.saturating_mul(1024 * 1024);
    // Everything is downloaded before the set exists, so a retry after a
    // network error starts clean.
    let pack = stickpile_signal::import_pack(&client(state)?, &link, max)
        .await
        .map_err(signal_error)?;
    let title = pack.title.trim();
    let set_name = sets::validate_name(title).unwrap_or_else(|_| "Signal".to_string());
    let slug = match p["slug"].as_str() {
        Some(s) => s.to_string(),
        None => {
            let base = stickpile_setfmt::slugify(title);
            sets::free_slug(&state.db, &user.id, if base.is_empty() { "signal" } else { &base }).await?
        }
    };
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

    let mut skipped: Vec<Value> = pack
        .skipped
        .iter()
        .map(|(id, reason)| json!({"sticker_id": id, "reason": reason}))
        .collect();
    let mut cover_item: Option<String> = None;
    let mut count = 0u32;
    for (i, s) in pack.stickers.into_iter().enumerate() {
        let name = format!("sticker {}", i + 1);
        let emojis: Vec<String> = s.emoji.iter().cloned().collect();
        let input = ItemInput {
            stem: stem_for(&name),
            name,
            emojis: clean_list(&emojis, "emojis").unwrap_or_default(),
            ..Default::default()
        };
        let file = UploadFile::from_bytes(format!("{:03}-sticker.{}", i + 1, extension(s.format)), s.bytes);
        match upload::add_to_set(state, &row, &file, &input, &row.owner_id).await {
            Ok(added) if added.already_in_set => skipped.push(json!({
                "sticker_id": s.id,
                "reason": "the same file as an earlier sticker of the pack",
            })),
            Ok(added) => {
                count += 1;
                if pack.cover_id == Some(s.id) && pack.cover.is_none() {
                    cover_item = Some(added.item_id);
                }
            }
            Err(e) if e.status.is_server_error() => return Err(retry(e)),
            Err(e) => skipped.push(json!({"sticker_id": s.id, "reason": e.message})),
        }
    }
    // The cover: a sticker of the pack, or an image of its own.
    let mut cover = false;
    if let Some(item) = &cover_item {
        sqlx::query("UPDATE sets SET cover_item_id = ? WHERE id = ?")
            .bind(item)
            .bind(&row.id)
            .execute(&state.db)
            .await
            .map_err(retry)?;
        cover = true;
    } else if let Some(c) = pack.cover {
        let file = UploadFile::from_bytes(format!("cover.{}", extension(c.format)), c.bytes);
        // An animated or unusual cover is not worth failing the import.
        cover = cover::store(state, &row, &file, false).await.is_ok();
    }
    index::refresh_set(state, &row.id).await?;
    Ok(Some(json!({
        "set_id": row.id,
        "slug": row.slug,
        "title": pack.title,
        "author": pack.author,
        "imported": count,
        "skipped": skipped,
        "cover": cover,
    })))
}

// ---------------------------------------------------------------------------
// The pack of a set
// ---------------------------------------------------------------------------

/// A set as a Signal pack, with the items left out and the notes.
struct Built {
    pack: UploadPack,
    skipped: Vec<Value>,
    notes: Vec<Value>,
}

/// Render the set's items (and its cover) as Signal stickers.
async fn build(state: &AppState, set: &SetRow) -> Result<Built, JobError> {
    let rows = hydrate::fetch_by_set(&state.db, &set.id).await?;
    let media_ids: Vec<String> = rows.iter().map(|r| r.media_id.clone()).collect();
    let variants = hydrate::variant_rows(&state.db, &media_ids).await?;
    let mut stickers = Vec::new();
    let mut skipped = Vec::new();
    let mut notes = Vec::new();
    for r in &rows {
        if stickers.len() == MAX_STICKERS {
            skipped
                .push(json!({"item_id": r.id, "name": r.name, "reason": "a Signal pack holds at most 200 stickers"}));
            continue;
        }
        let original = renditions::original(state, &variants, r);
        match renditions::render(state, r, original, Kind::Signal).await? {
            Rendered::Made(m) => {
                if let Some(note) = m.note {
                    notes.push(json!({"item_id": r.id, "name": r.name, "note": note}));
                }
                stickers.push(UploadSticker {
                    bytes: m.bytes,
                    emoji: stickpile_signal::sticker_emoji(&r.emojis()),
                });
            }
            Rendered::Skip(reason) => skipped.push(json!({"item_id": r.id, "name": r.name, "reason": reason})),
        }
    }
    let cover = match whatsapp::cover_frame(state, set, &rows).await? {
        Some(frame) => renditions::render_still(frame, Kind::Signal).await?,
        None => None,
    };
    let author = whatsapp::publisher(state, set)
        .await?
        .unwrap_or_else(|| "stickpile".into());
    Ok(Built {
        pack: UploadPack {
            title: set.name.clone(),
            author,
            stickers,
            cover,
        },
        skipped,
        notes,
    })
}

// ---------------------------------------------------------------------------
// Publishing
// ---------------------------------------------------------------------------

/// Check a publish of `set_id` by `user`; Signal takes no options.
pub async fn prepare_export(state: &AppState, user: &CurrentUser, set_id: &str, options: &Value) -> AppResult<Value> {
    if !configured(state) {
        return Err(disabled());
    }
    let set = authz::owned_set(state, user.id(), set_id).await?;
    if !(options.is_null() || options.as_object().is_some_and(|o| o.is_empty())) {
        return Err(AppError::bad_request("options.signal: Signal takes no options"));
    }
    Ok(json!({ "set_id": set.id }))
}

/// `SignalExport` job: upload the set as a new pack.
pub async fn export_job(state: &AppState, job: &JobRow) -> JobResult {
    let p = job.payload();
    let set_id = p["set_id"]
        .as_str()
        .ok_or_else(|| JobError::Fatal("payload without set_id".into()))?
        .to_string();
    let lock = jobs::subject_lock(&format!("signal:{set_id}"));
    let _guard = lock.lock().await;
    let Some(set) = db::set_by_id(&state.db, &set_id).await.map_err(retry)? else {
        return Ok(Some(json!({"skipped": "the set no longer exists"})));
    };
    if db::user_by_id(&state.db, &set.owner_id).await.map_err(retry)?.is_none() {
        return Ok(Some(json!({"skipped": "the owner no longer exists"})));
    }
    if !enabled(state) {
        return Err(JobError::Fatal(DISABLED.into()));
    }
    let creds = credentials(state).map_err(|e| JobError::Fatal(e.to_string()))?;
    let client = client(state)?;
    let built = build(state, &set).await?;
    if built.pack.stickers.is_empty() {
        return Err(JobError::Fatal(
            "nothing to publish: no item of the set can become a Signal sticker".into(),
        ));
    }
    let up = stickpile_signal::upload_pack(&client, &creds, &built.pack)
        .await
        .map_err(signal_error)?;
    let url = up.link.url();
    let stored = json!({"pack_id": up.link.pack_id, "stickers": up.stickers});
    publish::bind(&state.db, &set.id, PLATFORM, Some(&set.name), Some(&url), Some(&stored)).await?;
    Ok(Some(json!({
        "set_id": set.id,
        "title": set.name,
        "url": url,
        "exported": up.stickers,
        "skipped": built.skipped,
        "notes": built.notes,
    })))
}

/// Signal in the publish registry: a new pack per publish.
pub struct SignalPlatform;

impl Platform for SignalPlatform {
    fn id(&self) -> &'static str {
        PLATFORM
    }

    fn name(&self) -> &'static str {
        "Signal"
    }

    fn job_kind(&self) -> JobKind {
        JobKind::SignalExport
    }

    fn configured(&self, state: &AppState) -> bool {
        configured(state)
    }

    fn options(&self) -> Vec<PlatformOption> {
        Vec::new()
    }

    fn prepare<'a>(
        &'a self,
        state: &'a AppState,
        user: &'a CurrentUser,
        set_id: &'a str,
        options: &'a Value,
    ) -> BoxFuture<'a, AppResult<Value>> {
        Box::pin(prepare_export(state, user, set_id, options))
    }

    fn summary(&self, r: &Value) -> String {
        let exported = r.get("exported").and_then(Value::as_u64).unwrap_or(0);
        let skipped = r.get("skipped").and_then(Value::as_array).map_or(0, Vec::len);
        tf(
            "{exported} stickers in a new pack, {skipped} left out",
            &[("exported", &exported), ("skipped", &skipped)],
        )
    }

    fn notice(&self, _state: &AppState, _user: &CurrentUser) -> Option<&'static str> {
        Some(t(
            "Signal packs cannot be changed: every publish uploads a new pack with a new link. People who added an older pack keep its stickers.",
        ))
    }
}

// ---------------------------------------------------------------------------
// The Signal-ready zip
// ---------------------------------------------------------------------------

/// `manifest.json` next to the kept zip.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ZipStored {
    fingerprint: String,
    file: String,
}

fn zip_dir(state: &AppState, set_id: &str) -> Option<PathBuf> {
    is_id(set_id).then(|| state.config.exports_dir().join(set_id).join("signal"))
}

/// What the zip is made from: the items (media, emoji, name, order), the
/// set's name and cover, the owner's name, and whether ffmpeg is there.
async fn fingerprint(state: &AppState, set: &SetRow) -> AppResult<String> {
    let rows = hydrate::fetch_by_set(&state.db, &set.id).await?;
    let chosen = sets::chosen_cover(&state.db, &set.id).await?;
    let publisher = whatsapp::publisher(state, set).await?.unwrap_or_default();
    let cover_sha = match &set.cover_media_id {
        Some(m) => db::media_by_id(&state.db, m)
            .await?
            .map(|m| m.sha256)
            .unwrap_or_default(),
        None => String::new(),
    };
    let mut h = Sha256::new();
    for part in [
        ZIP_FORMAT,
        if state.ffmpeg_available { "ffmpeg" } else { "no-ffmpeg" },
        &set.slug,
        &set.name,
        &publisher,
        &cover_sha,
        chosen.as_deref().unwrap_or(""),
    ] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    for r in &rows {
        for part in [&r.id, &r.sha256, &r.emojis, &r.name] {
            h.update(part.as_bytes());
            h.update([0x1f]);
        }
        h.update([0]);
    }
    Ok(hex::encode(h.finalize()))
}

fn job_to_app(e: JobError) -> AppError {
    AppError::internal(e.to_string())
}

/// The Signal-ready zip of a set the caller can read: its download name and
/// bytes. Made on the first request after a change (that counts as an
/// upload for rate limiting) and kept until the set changes.
pub async fn ready_zip(state: &AppState, user: &CurrentUser, set_id: &str) -> AppResult<(String, Vec<u8>)> {
    let set = authz::readable_set(state, user.id(), set_id).await?;
    let dir = zip_dir(state, &set.id).ok_or_else(AppError::not_found)?;
    let lock = jobs::subject_lock(&format!("signal-zip:{}", set.id));
    let _guard = lock.lock().await;
    let fp = fingerprint(state, &set).await?;
    let manifest = tokio::fs::read(dir.join(ZIP_MANIFEST))
        .await
        .ok()
        .and_then(|b| serde_json::from_slice::<ZipStored>(&b).ok());
    if let Some(m) = manifest.filter(|m| m.fingerprint == fp) {
        if let Ok(bytes) = tokio::fs::read(dir.join(&m.file)).await {
            return Ok((m.file, bytes));
        }
    }
    state.limiters.check_upload(user.id())?;
    let built = build(state, &set).await.map_err(job_to_app)?;
    if built.pack.stickers.is_empty() {
        return Err(AppError::bad_request("no item of this set can become a Signal sticker"));
    }
    let mut files = stickpile_signal::signal_ready_files(&built.pack).map_err(|e| AppError::internal(e.to_string()))?;
    if !built.skipped.is_empty() {
        let mut text = String::from("These items of the set are not in the pack:\n\n");
        for s in &built.skipped {
            text.push_str(&format!(
                "{}: {}\n",
                s["name"].as_str().unwrap_or(""),
                s["reason"].as_str().unwrap_or("")
            ));
        }
        files.push(PackFile {
            path: "left-out.txt".into(),
            bytes: text.into_bytes(),
        });
    }
    let bytes = tokio::task::spawn_blocking(move || stickpile_signal::zip_stored(&files))
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
        .map_err(|e| AppError::internal(e.to_string()))?;
    let file = format!("{}-signal.zip", set.slug);
    files::write_atomic(dir.join(&file), bytes.clone()).await?;
    let stored = serde_json::to_vec_pretty(&ZipStored {
        fingerprint: fp,
        file: file.clone(),
    })
    .map_err(|e| AppError::internal(e.to_string()))?;
    files::write_atomic(dir.join(ZIP_MANIFEST), stored).await?;
    if let Some(old) = manifest_file_other_than(&dir, &file).await {
        let _ = tokio::fs::remove_file(dir.join(old)).await;
    }
    Ok((file, bytes))
}

/// A zip kept from before under another name (the slug changed).
async fn manifest_file_other_than(dir: &std::path::Path, file: &str) -> Option<String> {
    let mut entries = tokio::fs::read_dir(dir).await.ok()?;
    while let Ok(Some(e)) = entries.next_entry().await {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with("-signal.zip") && name != file {
            return Some(name);
        }
    }
    None
}

/// The download link on the export page.
pub fn zip_url(set_id: &str) -> String {
    stickpile_api::routes::path(stickpile_api::routes::SET_SIGNAL_ZIP, &[set_id])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_imports_forget_the_link() {
        let p = json!({"link": "https://signal.art/addstickers/#pack_id=x&pack_key=y", "slug": null});
        let s = scrubbed_payload(&p).unwrap();
        assert!(!s.to_string().contains("pack_key"));
        assert_eq!(s["slug"], Value::Null);
        assert!(scrubbed_payload(&json!({"set_id": "x"})).is_none());
    }

    #[test]
    fn summary() {
        let r = json!({"exported": 3, "skipped": [{}]});
        assert_eq!(SignalPlatform.summary(&r), "3 stickers in a new pack, 1 left out");
    }
}
