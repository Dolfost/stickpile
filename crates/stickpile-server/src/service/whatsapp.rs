//! WhatsApp sticker packs as a download (docs/whatsapp.md).
//!
//! WhatsApp has no API to publish sticker packs: a pack reaches it through
//! an app on the phone. So the server builds the packs as files, in the
//! `.wastickers` layout sticker apps import, and anyone who can read the
//! set downloads them (the Android app can fetch them for its "Add to
//! WhatsApp").
//!
//! * [`build`] queues a `WhatsappExport` job ([`build_job`]). Every item
//!   becomes a 512x512 WebP sticker (`stickpile_media::whatsapp`): stills
//!   static, animations and videos animated (videos need ffmpeg). Packs hold
//!   3 to 30 stickers of one kind, so a larger set is split and one or two
//!   animated items among stills are sent as stills. Items that cannot
//!   become stickers are listed with the reason.
//! * The result is kept in `<data_dir>/exports/<set id>/whatsapp/`: one zip
//!   (every `.wastickers` plus a `contents.json` in the layout of WhatsApp's
//!   sample app) and `manifest.json` (the [`WhatsappBuild`] and a
//!   fingerprint of what it was made from). A build of an unchanged set is
//!   reused.
//! * [`status`], [`zip_file`] and [`pack_file`] serve it.

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use stickpile_api::routes as r;
use stickpile_api::{
    ExportSkip, Job, JobKind, MediaKind, Variant, WhatsappBuild, WhatsappPack, WhatsappStatus, WhatsappSticker,
};
use stickpile_media::whatsapp as wa;

use crate::archive::{self, Zip};
use crate::auth::CurrentUser;
use crate::authz;
use crate::db::{self, enum_str, SetRow};
use crate::error::{AppError, AppResult};
use crate::files;
use crate::ids::{is_id, now};
use crate::jobs::{self, JobError, JobResult, JobRow, JOB_COLUMNS};
use crate::service::hydrate::{self, ItemFull};
use crate::service::{cover, sets};
use crate::state::AppState;

/// Changes when the packs would come out differently for the same set.
const FORMAT_VERSION: &str = "whatsapp-1";
const MANIFEST: &str = "manifest.json";
/// The tray icon's name inside a `.wastickers` (as sticker-convert writes it).
const TRAY_FILE: &str = "cover.png";
/// Largest pack read back out of the zip: 30 animated stickers of 500 KB
/// plus a tray icon and two text files.
const MAX_PACK_BYTES: u64 = 32 * 1024 * 1024;

fn retry(e: impl std::fmt::Display) -> JobError {
    JobError::Retry(e.to_string())
}

fn not_built() -> AppError {
    AppError::new(
        axum::http::StatusCode::NOT_FOUND,
        "not_found",
        "this set has no WhatsApp packs yet; build them first (POST /api/v1/sets/{id}/whatsapp)",
    )
}

/// `<data_dir>/exports/<set id>/whatsapp`.
fn dir(state: &AppState, set_id: &str) -> Option<PathBuf> {
    is_id(set_id).then(|| state.config.exports_dir().join(set_id).join("whatsapp"))
}

/// `manifest.json`: the build and what it was made from.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stored {
    fingerprint: String,
    build: WhatsappBuild,
}

async fn load_stored(state: &AppState, set_id: &str) -> Option<Stored> {
    let path = dir(state, set_id)?.join(MANIFEST);
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// What the packs are made from: the items (media, emoji, name, order),
/// the set's name, slug, cover and owner name, and whether ffmpeg is there.
async fn fingerprint(state: &AppState, set: &SetRow, publisher: &str, rows: &[ItemFull]) -> AppResult<String> {
    let chosen = sets::chosen_cover(&state.db, &set.id).await?;
    let mut h = Sha256::new();
    for part in [
        FORMAT_VERSION,
        if state.ffmpeg_available { "ffmpeg" } else { "no-ffmpeg" },
        &set.slug,
        &set.name,
        publisher,
        set.cover_media_id.as_deref().unwrap_or(""),
        chosen.as_deref().unwrap_or(""),
    ] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    for r in rows {
        for part in [&r.id, &r.sha256, &r.name, &r.emojis] {
            h.update(part.as_bytes());
            h.update([0x1f]);
        }
        h.update([0]);
    }
    Ok(hex::encode(h.finalize()))
}

pub(crate) async fn publisher(state: &AppState, set: &SetRow) -> AppResult<Option<String>> {
    Ok(db::user_by_id(&state.db, &set.owner_id).await?.map(|u| {
        let name = u.display_name.trim();
        if name.is_empty() {
            u.username.clone()
        } else {
            name.to_string()
        }
    }))
}

/// The caller's build of `set_id` that is queued or running.
async fn pending_job(state: &AppState, set_id: &str, user_id: &str) -> AppResult<Option<Job>> {
    let row: Option<JobRow> = sqlx::query_as(&format!(
        "SELECT {JOB_COLUMNS} FROM jobs WHERE kind = ? AND subject = ? AND owner_id = ? \
         AND status IN ('queued', 'running') ORDER BY created_at DESC, id DESC LIMIT 1"
    ))
    .bind(enum_str(&JobKind::WhatsappExport))
    .bind(set_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|r| r.to_api()))
}

/// The last build of a set the caller can read, whether it is still up to
/// date, and the caller's build in progress.
pub async fn status(state: &AppState, user: &CurrentUser, set_id: &str) -> AppResult<WhatsappStatus> {
    let set = authz::readable_set(state, user.id(), set_id).await?;
    let stored = load_stored(state, &set.id).await;
    let current = match &stored {
        Some(s) => {
            let rows = hydrate::fetch_by_set(&state.db, &set.id).await?;
            let publisher = publisher(state, &set).await?.unwrap_or_default();
            fingerprint(state, &set, &publisher, &rows).await? == s.fingerprint
        }
        None => false,
    };
    Ok(WhatsappStatus {
        build: stored.map(|s| s.build),
        current,
        job: pending_job(state, &set.id, user.id()).await?,
    })
}

/// Queue a build of the WhatsApp packs of a set the caller can read; the
/// caller's build already in the queue is returned instead. Counts as an
/// upload for rate limiting.
pub async fn build(state: &AppState, user: &CurrentUser, set_id: &str) -> AppResult<Job> {
    let set = authz::readable_set(state, user.id(), set_id).await?;
    if let Some(job) = pending_job(state, &set.id, user.id()).await? {
        return Ok(job);
    }
    state.limiters.check_upload(user.id())?;
    jobs::enqueue(
        state,
        JobKind::WhatsappExport,
        json!({ "set_id": set.id }),
        Some(set.id.clone()),
        Some(user.id()),
    )
    .await
}

/// The zip of the last build: its download name and path.
pub async fn zip_file(state: &AppState, user: &CurrentUser, set_id: &str) -> AppResult<(String, PathBuf)> {
    let set = authz::readable_set(state, user.id(), set_id).await?;
    let stored = load_stored(state, &set.id).await.ok_or_else(not_built)?;
    let path = dir(state, &set.id)
        .ok_or_else(AppError::not_found)?
        .join(&stored.build.file);
    if !path.is_file() {
        return Err(not_built());
    }
    Ok((stored.build.file, path))
}

/// Pack `number` of the last build as a `.wastickers`: its name and bytes.
pub async fn pack_file(
    state: &AppState,
    user: &CurrentUser,
    set_id: &str,
    number: &str,
) -> AppResult<(String, Vec<u8>)> {
    let (_, path) = zip_file(state, user, set_id).await?;
    let stored = load_stored(state, set_id).await.ok_or_else(not_built)?;
    let n: u32 = number.trim().parse().map_err(|_| AppError::not_found())?;
    let pack = stored
        .build
        .packs
        .iter()
        .find(|p| p.number == n)
        .ok_or_else(AppError::not_found)?;
    let name = pack.file.clone();
    let entry = name.clone();
    let bytes = tokio::task::spawn_blocking(move || archive::read_entry(&path, &entry, MAX_PACK_BYTES))
        .await??
        .ok_or_else(not_built)?;
    Ok((name, bytes))
}

/// Remove the exports of sets that no longer exist.
pub async fn gc_exports(state: &AppState) -> AppResult<usize> {
    let root = state.config.exports_dir();
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let mut removed = 0;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_id(&name) || db::set_by_id(&state.db, &name).await?.is_some() {
            continue;
        }
        let lock = jobs::subject_lock(&format!("whatsapp:{name}"));
        let Ok(_guard) = lock.try_lock() else { continue };
        match tokio::fs::remove_dir_all(entry.path()).await {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!("removing stale export {}: {e}", entry.path().display()),
        }
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// The build job
// ---------------------------------------------------------------------------

/// One sticker made for WhatsApp, in set order (`pos`).
struct Made {
    pos: usize,
    item_id: String,
    name: String,
    emojis: Vec<String>,
    bytes: Vec<u8>,
    animated: bool,
    note: Option<String>,
}

pub(crate) fn probe_of(r: &ItemFull) -> stickpile_media::Probe {
    stickpile_media::Probe {
        format: r.format(),
        kind: r.kind(),
        width: r.width.max(0) as u32,
        height: r.height.max(0) as u32,
        frame_count: r.frame_count.map(|v| v.max(0) as u32),
        duration_ms: r.duration_ms.map(|v| v.max(0) as u64),
        fps: r.fps.map(|v| v as f32),
        has_alpha: r.has_alpha,
    }
}

/// 1 to 3 emoji: the item's real emoji (words typed into the field are
/// left out), else the default Telegram uses too.
fn emojis_for(r: &ItemFull) -> Vec<String> {
    let mut seen = HashSet::new();
    let list: Vec<String> = r
        .emojis()
        .into_iter()
        .map(|e| e.trim().to_string())
        .filter(|e| stickpile_telegram::is_emoji(e) && seen.insert(e.clone()))
        .take(wa::MAX_EMOJIS)
        .collect();
    if list.is_empty() {
        vec![stickpile_telegram::DEFAULT_EMOJI.to_string()]
    } else {
        list
    }
}

/// `s` cut to `max` characters.
fn cut(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

/// Sizes of the packs `len` stickers are split into: as few as possible,
/// as even as possible (31 makes 16 + 15, never 30 + 1).
fn pack_sizes(len: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let k = len.div_ceil(wa::PACK_MAX_STICKERS);
    let (base, extra) = (len / k, len % k);
    (0..k).map(|i| base + usize::from(i < extra)).collect()
}

enum Rendered {
    Static(Vec<u8>),
    Animated { bytes: Vec<u8>, cut: bool },
    Skip(String),
}

/// A still sticker made from the first frame (or a video's representative
/// frame) of the stored original.
async fn render_still(state: &AppState, abs: PathBuf, probe: stickpile_media::Probe) -> Result<Rendered, JobError> {
    let tools = state.tools.clone();
    tokio::task::spawn_blocking(move || {
        let frame = wa::first_frame(&abs, &probe, &tools)?;
        wa::still(&frame)
    })
    .await
    .map_err(retry)
    .map(|r| match r {
        Ok(Some(b)) => Rendered::Static(b),
        Ok(None) => Rendered::Skip("does not fit WhatsApp's 100 KB limit for static stickers".into()),
        Err(e) => Rendered::Skip(e.to_string()),
    })
}

async fn render(state: &AppState, r: &ItemFull, abs: PathBuf) -> Result<Rendered, JobError> {
    let probe = probe_of(r);
    match probe.kind {
        MediaKind::Still => render_still(state, abs, probe).await,
        MediaKind::Video if !state.ffmpeg_available => Ok(Rendered::Skip(
            "animated stickers from videos need ffmpeg on the server".into(),
        )),
        MediaKind::Animation | MediaKind::Video => {
            let tools = state.tools.clone();
            let (abs2, probe2) = (abs.clone(), probe.clone());
            let out = tokio::task::spawn_blocking(move || wa::animated(&abs2, &probe2, &tools))
                .await
                .map_err(retry)?;
            match out {
                Ok(wa::Animated::Webp { bytes, cut }) => Ok(Rendered::Animated { bytes, cut }),
                Ok(wa::Animated::NotAnimated) => render_still(state, abs, probe).await,
                Ok(wa::Animated::TooLarge) => Ok(Rendered::Skip(
                    "does not fit WhatsApp's 500 KB limit for animated stickers, even at 5 frames per second".into(),
                )),
                Err(e) => Ok(Rendered::Skip(e.to_string())),
            }
        }
    }
}

/// The tray icon of the first pack: the set's cover image or chosen cover
/// item, if it has one.
pub(crate) async fn cover_frame(
    state: &AppState,
    set: &SetRow,
    rows: &[ItemFull],
) -> Result<Option<image::RgbaImage>, JobError> {
    let path = if let Some(media_id) = &set.cover_media_id {
        cover::variant_row(&state.db, media_id, Variant::Original)
            .await?
            .map(|v| v.path)
    } else {
        match sets::chosen_cover(&state.db, &set.id).await? {
            Some(item) => match rows.iter().find(|r| r.id == item) {
                Some(r) => cover::variant_row(&state.db, &r.media_id, Variant::Thumb)
                    .await?
                    .map(|v| v.path),
                None => None,
            },
            None => None,
        }
    };
    let Some(path) = path else { return Ok(None) };
    let Some(abs) = files::resolve(&state.media_dir(), &path) else {
        return Ok(None);
    };
    let bytes = files::read(abs).await?;
    let limits = state.media_limits();
    tokio::task::spawn_blocking(move || stickpile_media::decode_frame(&bytes, &limits).ok())
        .await
        .map_err(retry)
}

/// `WhatsappExport` job.
pub async fn build_job(state: &AppState, job: &JobRow) -> JobResult {
    let p = job.payload();
    let set_id = p["set_id"]
        .as_str()
        .ok_or_else(|| JobError::Fatal("payload without set_id".into()))?
        .to_string();
    let lock = jobs::subject_lock(&format!("whatsapp:{set_id}"));
    let _guard = lock.lock().await;
    let Some(set) = db::set_by_id(&state.db, &set_id).await.map_err(retry)? else {
        return Ok(Some(json!({"skipped": "the set no longer exists"})));
    };
    let Some(publisher) = publisher(state, &set).await? else {
        return Ok(Some(json!({"skipped": "the owner no longer exists"})));
    };
    let out_dir = dir(state, &set.id).ok_or_else(|| JobError::Fatal("bad set id".into()))?;
    let rows = hydrate::fetch_by_set(&state.db, &set.id).await?;
    let fp = fingerprint(state, &set, &publisher, &rows).await?;
    if let Some(stored) = load_stored(state, &set.id).await {
        if stored.fingerprint == fp && out_dir.join(&stored.build.file).is_file() {
            let mut v = serde_json::to_value(&stored.build).map_err(retry)?;
            v["set_id"] = json!(set.id);
            v["reused"] = json!(true);
            return Ok(Some(v));
        }
    }

    let media_ids: Vec<String> = rows.iter().map(|r| r.media_id.clone()).collect();
    let variants = hydrate::variant_rows(&state.db, &media_ids).await?;
    let mut statics: Vec<Made> = Vec::new();
    let mut moving: Vec<Made> = Vec::new();
    let mut skipped: Vec<ExportSkip> = Vec::new();
    let skip = |r: &ItemFull, reason: String| ExportSkip {
        item_id: r.id.clone(),
        name: r.name.clone(),
        reason,
    };
    let mut originals: Vec<Option<PathBuf>> = Vec::with_capacity(rows.len());
    for (pos, r) in rows.iter().enumerate() {
        let original = variants
            .get(&r.media_id)
            .and_then(|vs| vs.iter().find(|v| v.variant() == Some(Variant::Original)))
            .and_then(|v| files::resolve(&state.media_dir(), &v.path));
        originals.push(original.clone());
        let Some(abs) = original else {
            skipped.push(skip(r, "the original file is missing".into()));
            continue;
        };
        let made = |bytes, animated, note| Made {
            pos,
            item_id: r.id.clone(),
            name: r.name.clone(),
            emojis: emojis_for(r),
            bytes,
            animated,
            note,
        };
        match render(state, r, abs).await? {
            Rendered::Static(b) => {
                let note = (r.kind() != MediaKind::Still)
                    .then(|| "every frame looks the same: sent as a still sticker".to_string());
                statics.push(made(b, false, note));
            }
            Rendered::Animated { bytes, cut } => {
                let note = cut.then(|| "cut to the first 10 seconds".to_string());
                moving.push(made(bytes, true, note));
            }
            Rendered::Skip(reason) => skipped.push(skip(r, reason)),
        }
    }

    // A pack holds at least 3 stickers, all static or all animated.
    if (1..wa::PACK_MIN_STICKERS).contains(&moving.len()) {
        for m in std::mem::take(&mut moving) {
            let r = &rows[m.pos];
            let Some(abs) = originals[m.pos].clone() else { continue };
            match render_still(state, abs, probe_of(r)).await? {
                Rendered::Static(bytes) => statics.push(Made {
                    bytes,
                    animated: false,
                    note: Some("sent as a still sticker: a WhatsApp pack needs at least 3 animated stickers".into()),
                    ..m
                }),
                Rendered::Skip(reason) => skipped.push(skip(r, reason)),
                Rendered::Animated { .. } => {}
            }
        }
        statics.sort_by_key(|m| m.pos);
    }
    if (1..wa::PACK_MIN_STICKERS).contains(&statics.len()) && !moving.is_empty() {
        for m in std::mem::take(&mut statics) {
            skipped.push(ExportSkip {
                item_id: m.item_id,
                name: m.name,
                reason: "a WhatsApp pack needs at least 3 stickers and cannot mix still and animated ones; \
                         this set has fewer than 3 still ones"
                    .into(),
            });
        }
    }
    let usable = statics.len() + moving.len();
    if statics.len() < wa::PACK_MIN_STICKERS && moving.len() < wa::PACK_MIN_STICKERS {
        return Err(JobError::Fatal(format!(
            "nothing to export: a WhatsApp pack needs at least {} stickers, and {usable} items of this set can \
             become WhatsApp stickers",
            wa::PACK_MIN_STICKERS
        )));
    }
    skipped.sort_by_key(|s| rows.iter().position(|r| r.id == s.item_id));

    // Packs: the still ones first, then the animated ones, each group split
    // evenly into packs of at most 30.
    let mut groups: Vec<Vec<Made>> = Vec::new();
    for mut group in [statics, moving] {
        for size in pack_sizes(group.len()) {
            let rest = group.split_off(size);
            groups.push(std::mem::replace(&mut group, rest));
        }
    }
    let total = groups.len();
    let cover = cover_frame(state, &set, &rows).await?;
    let slug = set.slug.clone();
    let title_base = cut(&set.name, wa::MAX_NAME_CHARS);
    let publisher = cut(&publisher, wa::MAX_NAME_CHARS);
    let limits = state.media_limits();

    let mut packs: Vec<(WhatsappPack, Vec<u8>)> = Vec::with_capacity(total);
    for (i, group) in groups.into_iter().enumerate() {
        let number = (i + 1) as u32;
        let (identifier, file, title) = if total == 1 {
            (slug.clone(), format!("{slug}.wastickers"), title_base.clone())
        } else {
            let suffix = format!(" ({number}/{total})");
            let room = wa::MAX_NAME_CHARS.saturating_sub(suffix.chars().count());
            (
                format!("{slug}-{number}"),
                format!("{slug}-{number}.wastickers"),
                format!("{}{suffix}", cut(&title_base, room)),
            )
        };
        let tray_source = if i == 0 { cover.clone() } else { None };
        let first = group[0].bytes.clone();
        let limits = limits.clone();
        let tray = tokio::task::spawn_blocking(move || {
            let frame = match tray_source {
                Some(f) => f,
                None => stickpile_media::decode_frame(&first, &limits)?,
            };
            wa::tray(&frame)
        })
        .await
        .map_err(retry)?
        .map_err(|e| JobError::Fatal(format!("making the tray icon: {e}")))?;
        let animated = group[0].animated;
        let mut version = Sha256::new();
        version.update(&tray);
        let mut stickers = Vec::with_capacity(group.len());
        let mut entries: Vec<(String, Vec<u8>)> = vec![
            ("title.txt".into(), title.clone().into_bytes()),
            ("author.txt".into(), publisher.clone().into_bytes()),
            (TRAY_FILE.into(), tray),
        ];
        for (k, m) in group.into_iter().enumerate() {
            let name = format!("{:02}.webp", k + 1);
            version.update(&m.bytes);
            version.update(m.emojis.join(" ").as_bytes());
            stickers.push(WhatsappSticker {
                item_id: m.item_id,
                name: m.name,
                file: name.clone(),
                emojis: m.emojis,
                animated: m.animated,
                size_bytes: m.bytes.len() as u64,
                note: m.note,
            });
            entries.push((name, m.bytes));
        }
        let bytes = tokio::task::spawn_blocking(move || {
            archive::zip_bytes(entries.iter().map(|(n, b)| (n.as_str(), b.as_slice())))
        })
        .await
        .map_err(retry)?
        .map_err(retry)?;
        let pack = WhatsappPack {
            number,
            identifier,
            title,
            publisher: publisher.clone(),
            animated,
            url: r::path(r::SET_WHATSAPP_PACK, &[&set.id, &number.to_string()]),
            file,
            tray_file: TRAY_FILE.into(),
            image_data_version: hex::encode(&version.finalize()[..8]),
            size_bytes: bytes.len() as u64,
            stickers,
        };
        packs.push((pack, bytes));
    }

    let (packs, datas): (Vec<WhatsappPack>, Vec<Vec<u8>>) = packs.into_iter().unzip();
    let contents = contents_json(&packs);
    let file = format!("{slug}-whatsapp.zip");
    let target = out_dir.join(&file);
    let dir2 = out_dir.clone();
    let names: Vec<String> = packs.iter().map(|p| p.file.clone()).collect();
    let size = tokio::task::spawn_blocking(move || -> std::io::Result<u64> {
        std::fs::create_dir_all(&dir2)?;
        let tmp = tempfile::Builder::new().prefix(".tmp-").tempfile_in(&dir2)?;
        let mut z = Zip::new(std::io::BufWriter::new(tmp.reopen()?));
        z.file("contents.json", contents.as_bytes())?;
        for (name, data) in names.iter().zip(&datas) {
            z.file(name, data)?;
        }
        let mut w = z.finish()?;
        w.flush()?;
        drop(w);
        tmp.as_file().sync_all()?;
        let size = tmp.as_file().metadata()?.len();
        tmp.persist(&target).map_err(|e| e.error)?;
        Ok(size)
    })
    .await
    .map_err(retry)?
    .map_err(retry)?;

    let build = WhatsappBuild {
        built_at: now(),
        set_version: set.version.max(0) as u64,
        url: r::path(r::SET_WHATSAPP_ZIP, &[&set.id]),
        file: file.clone(),
        size_bytes: size,
        packs,
        skipped,
    };
    let old = load_stored(state, &set.id).await;
    let manifest = serde_json::to_vec_pretty(&Stored {
        fingerprint: fp,
        build: build.clone(),
    })
    .map_err(retry)?;
    files::write_atomic(out_dir.join(MANIFEST), manifest).await?;
    if let Some(old) = old.filter(|o| o.build.file != file) {
        let _ = tokio::fs::remove_file(out_dir.join(&old.build.file)).await;
    }
    let mut v = serde_json::to_value(&build).map_err(retry)?;
    v["set_id"] = json!(set.id);
    v["reused"] = json!(false);
    Ok(Some(v))
}

/// `contents.json` in the layout of WhatsApp's sample app
/// (`Android/app/src/main/assets/contents.json`), plus `wastickers`: the
/// file that holds each pack's images.
fn contents_json(packs: &[WhatsappPack]) -> String {
    let packs: Vec<Value> = packs
        .iter()
        .map(|p| {
            json!({
                "identifier": p.identifier,
                "name": p.title,
                "publisher": p.publisher,
                "tray_image_file": p.tray_file,
                "image_data_version": p.image_data_version,
                "avoid_cache": false,
                "animated_sticker_pack": p.animated,
                "publisher_email": "",
                "publisher_website": "",
                "privacy_policy_website": "",
                "license_agreement_website": "",
                "wastickers": p.file,
                "stickers": p.stickers.iter().map(|s| json!({
                    "image_file": s.file,
                    "emojis": s.emojis,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&json!({
        "android_play_store_link": "",
        "ios_app_store_link": "",
        "sticker_packs": packs,
    }))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_are_split_evenly() {
        assert_eq!(pack_sizes(0), Vec::<usize>::new());
        assert_eq!(pack_sizes(3), vec![3]);
        assert_eq!(pack_sizes(30), vec![30]);
        assert_eq!(pack_sizes(31), vec![16, 15]);
        assert_eq!(pack_sizes(61), vec![21, 20, 20]);
        for n in 3..200 {
            let s = pack_sizes(n);
            assert_eq!(s.iter().sum::<usize>(), n);
            assert!(s
                .iter()
                .all(|k| (wa::PACK_MIN_STICKERS..=wa::PACK_MAX_STICKERS).contains(k)));
        }
    }

    #[test]
    fn names_are_cut_by_characters() {
        assert_eq!(cut("  Котики  ", 3), "Кот");
        assert_eq!(cut("cats", 128), "cats");
    }
}
