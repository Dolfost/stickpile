//! The `recheck` job: recompute the parts its batch
//! ([`crate::service::recheck`]) asks for on one picture (payload
//! `{recheck_id, media_id}`).
//!
//! What each part does in mode `missing` (and `refresh`):
//!
//! - `quality`: the quality report when there is none (again, from the
//!   stored original, against the recommended size of the first set in
//!   scope showing the picture).
//! - `renditions`: the variants when they never finished or an older
//!   encoder made them, unless a `variants` job for them is queued (all
//!   again).
//! - `embedding`: CLIP when the picture has no embedding from the loaded
//!   model (again, always).
//! - `keywords`, `emojis`, `vibe`: when the picture has no embedding from
//!   the loaded model, it is made first, and its suggestions fill empty
//!   fields and replace the computer's values (they were another model's);
//!   with an embedding from this model, the stored suggestions fill empty
//!   fields only (the zero-shot is run again from the stored embedding and
//!   replaces the computer's values). [`ai_fill::apply_to`]: a person's
//!   value is never replaced.
//! - `text`: a picture the loaded reader never read is read, and the text
//!   fills texts that are not set and replaces the computer's; one it read
//!   gives its stored text to texts that are not set (read again; the text
//!   replaces the computer's, and an empty reading clears it,
//!   [`ai_fill::clear_ai_text`]). Only `text_source = 'ai'` rows.
//!
//! Only items in the batch's scope are written (a user's batch: the items
//! of their sets). Work on the picture itself (embedding, reading,
//! quality, renditions) is shared by every item showing it, whoever owns
//! it. Each part runs even when another failed; the job fails with every
//! part's error.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use stickpile_api::{JobKind, RecheckMode, RecheckPart};

use super::media;
use super::{JobError, JobResult, JobRow};
use crate::db::{self, MediaRow};
use crate::ids::now;
use crate::index;
use crate::service::ai::{self as ai_fill, AiFill, Overwrite};
use crate::service::recheck::{self as batch, RecheckRow};
use crate::service::sets;
use crate::state::{AiStatus, AppState};

fn field<'a>(payload: &'a Value, name: &str) -> Result<&'a str, JobError> {
    payload
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| JobError::Fatal(format!("payload without {name}")))
}

fn retry(e: impl std::fmt::Display) -> JobError {
    JobError::Retry(e.to_string())
}

/// Run one picture of a batch.
pub async fn run(state: &AppState, job: &JobRow) -> JobResult {
    let payload = job.payload();
    let recheck_id = field(&payload, "recheck_id")?;
    let media_id = field(&payload, "media_id")?;
    let Some(batch) = batch::row(&state.db, recheck_id).await.map_err(retry)? else {
        return Ok(Some(
            json!({ "skipped": "the recheck no longer exists", "changed": false }),
        ));
    };
    let Some(media) = db::media_by_id(&state.db, media_id).await.map_err(retry)? else {
        return Ok(Some(json!({ "skipped": "media no longer exists", "changed": false })));
    };
    // The batch's items showing the picture, oldest first, with their
    // set's recommended size.
    let items: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT i.id, s.recommended_px FROM items i JOIN sets s ON s.id = i.set_id WHERE i.media_id = ?1 \
         AND (?2 IS NULL OR s.owner_id = ?2) AND (?3 IS NULL OR i.set_id = ?3) ORDER BY i.created_at, i.id",
    )
    .bind(media_id)
    .bind(&batch.owner_id)
    .bind(&batch.set_id)
    .fetch_all(&state.db)
    .await
    .map_err(retry)?;
    let Some((_, recommended)) = items.first() else {
        return Ok(Some(
            json!({ "skipped": "no item of the recheck shows it any more", "changed": false }),
        ));
    };
    let recommended = recommended
        .map(|v| v as u32)
        .unwrap_or(state.config.media.default_recommended_px);

    let parts = batch.parts();
    let mode = batch.mode();
    let mut report = Map::new();
    let mut errors: Vec<(&str, JobError)> = Vec::new();
    let mut changed = false;
    let mut note = |name: &'static str, r: Result<(bool, Value), JobError>| match r {
        Ok((c, v)) => {
            changed |= c;
            report.insert(name.into(), v);
        }
        Err(e) => errors.push((name, e)),
    };
    if parts.contains(&RecheckPart::Quality) {
        note("quality", quality(state, &media, mode, recommended).await);
    }
    if parts.contains(&RecheckPart::Renditions) {
        note("renditions", renditions(state, &media, mode).await);
    }
    if parts.iter().any(|p| p.needs_clip()) {
        note("clip", clip(state, &media, &batch, &parts, mode).await);
    }
    if parts.contains(&RecheckPart::Text) {
        note("text", text(state, &media, &batch, mode).await);
    }
    if !errors.is_empty() {
        let retry = errors.iter().any(|(_, e)| matches!(e, JobError::Retry(_)));
        let msg = errors
            .iter()
            .map(|(name, e)| format!("{name}: {e}"))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(if retry {
            JobError::Retry(msg)
        } else {
            JobError::Fatal(msg)
        });
    }
    Ok(Some(
        json!({ "media_id": media_id, "changed": changed, "parts": report }),
    ))
}

/// The quality report, from the stored original as an upload computes it
/// ([`stickpile_media::ingest`]). A new score reaches the items' summaries
/// (`updated_at`) and the search index.
async fn quality(
    state: &AppState,
    media: &MediaRow,
    mode: RecheckMode,
    recommended_px: u32,
) -> Result<(bool, Value), JobError> {
    if mode != RecheckMode::Refresh && media.quality_report.is_some() {
        return Ok((false, json!({ "score": media.quality })));
    }
    let bytes = {
        let _guard = state.media_lock.read().await;
        media::read_original(state, &media.id).await?
    };
    let mut limits = state.media_limits();
    // The stored original passed these limits once; a lossless re-encode
    // may be larger than the upload was.
    limits.max_bytes = limits.max_bytes.max(bytes.len() as u64);
    let opts = stickpile_media::IngestOptions {
        limits,
        thumb_px: state.config.media.thumb_px,
        recommended_px,
    };
    let tools = state.tools.clone();
    let report = tokio::task::spawn_blocking(move || stickpile_media::ingest(&bytes, &opts, &tools).map(|i| i.quality))
        .await
        .map_err(retry)?
        .map_err(|e| match e {
            stickpile_media::MediaError::Ffmpeg(_) | stickpile_media::MediaError::Io(_) => {
                JobError::Retry(e.to_string())
            }
            e => JobError::Fatal(e.to_string()),
        })?;
    let report_json = serde_json::to_string(&report).ok();
    let score = i64::from(report.score);
    if report_json == media.quality_report && Some(score) == media.quality {
        return Ok((false, json!({ "score": score })));
    }
    let mut tx = sets::begin(&state.db).await?;
    sqlx::query("UPDATE media SET quality = ?, quality_report = ? WHERE id = ?")
        .bind(score)
        .bind(&report_json)
        .bind(&media.id)
        .execute(&mut *tx)
        .await
        .map_err(retry)?;
    // Summaries carry the score: let client caches pick it up.
    let ids: Vec<String> = sqlx::query_scalar("UPDATE items SET updated_at = ? WHERE media_id = ? RETURNING id")
        .bind(now())
        .bind(&media.id)
        .fetch_all(&mut *tx)
        .await
        .map_err(retry)?;
    tx.commit().await.map_err(retry)?;
    index::refresh_items(state, &ids).await?;
    Ok((true, json!({ "score": score, "was": media.quality })))
}

/// The renditions, made as the `variants` job makes them.
async fn renditions(state: &AppState, media: &MediaRow, mode: RecheckMode) -> Result<(bool, Value), JobError> {
    if mode != RecheckMode::Refresh {
        let (done, version): (bool, i64) =
            sqlx::query_as("SELECT variants_done, variants_version FROM media WHERE id = ?")
                .bind(&media.id)
                .fetch_one(&state.db)
                .await
                .map_err(retry)?;
        if done && version >= stickpile_media::variants_version(media.kind()) {
            return Ok((false, json!("current")));
        }
        // An upload's own job makes them.
        if super::pending_exists(&state.db, JobKind::Variants, &media.id).await? {
            return Ok((false, json!("queued")));
        }
    }
    let made = media::generate_variants(state, &media.id).await?;
    Ok((true, made.unwrap_or(Value::Null)))
}

/// `embedding`, `keywords`, `emojis` and `vibe` (see the module docs).
async fn clip(
    state: &AppState,
    media: &MediaRow,
    batch: &RecheckRow,
    parts: &[RecheckPart],
    mode: RecheckMode,
) -> Result<(bool, Value), JobError> {
    let Some(ai) = state.ai() else {
        return Err(match state.ai_status() {
            AiStatus::Loading => JobError::Retry("the AI model is still loading".into()),
            _ => JobError::Fatal("AI is not available on this server".into()),
        });
    };
    let model = ai.image_model_id().to_string();
    let refresh = mode == RecheckMode::Refresh;
    // The computer's values on the items are this model's.
    let current = media.embedding_model.as_deref() == Some(model.as_str());
    let want_embedding = parts.contains(&RecheckPart::Embedding);
    let want = |p: RecheckPart| parts.contains(&p);
    let fields = want(RecheckPart::Keywords) || want(RecheckPart::Emojis) || want(RecheckPart::Vibe);
    let stored: Option<Vec<u8>> = sqlx::query_scalar("SELECT embedding FROM media WHERE id = ?")
        .bind(&media.id)
        .fetch_one(&state.db)
        .await
        .map_err(retry)?;
    let stored = stored.filter(|_| current);

    let mut changed = false;
    let mut report = Map::new();
    let embed_now = (want_embedding && (refresh || !current)) || (fields && stored.is_none());
    let fill: Option<AiFill> = if embed_now {
        let e = media::embed_media(state, &media.id).await?;
        let blob = stickpile_ai::to_blob(&e.embedding);
        let new_vector = stored.as_deref() != Some(blob.as_slice());
        changed |= new_vector;
        let mut tx = sets::begin(&state.db).await?;
        media::store_embedding(&mut tx, &media.id, &e).await?;
        tx.commit().await.map_err(retry)?;
        index::set_embedding(state, &media.id, Some(Arc::from(e.embedding.as_slice())));
        report.insert("embedding".into(), json!({ "model": e.model, "changed": new_vector }));
        Some(AiFill::from_suggestions(&e.suggestions))
    } else if fields {
        let stored = stored.as_deref().expect("embedded with this model");
        let refreshed = if refresh { None } else { AiFill::from_media(media) };
        match refreshed {
            Some(f) => Some(f),
            None => {
                // The zero-shot again: the vocabulary or the limits may
                // have changed since (microseconds; no blocking pool).
                let s = ai.zeroshot.suggest(
                    &stickpile_ai::from_blob(stored),
                    state.config.ai.max_keywords as usize,
                    state.config.ai.max_emojis as usize,
                );
                let mut tx = sets::begin(&state.db).await?;
                media::store_suggestions(&mut tx, &media.id, &s).await?;
                tx.commit().await.map_err(retry)?;
                Some(AiFill::from_suggestions(&s))
            }
        }
    } else {
        None
    };
    if let (Some(fill), true) = (fill, fields) {
        let fill = AiFill {
            keywords: if want(RecheckPart::Keywords) {
                fill.keywords
            } else {
                Vec::new()
            },
            emojis: if want(RecheckPart::Emojis) {
                fill.emojis
            } else {
                Vec::new()
            },
            vibe: if want(RecheckPart::Vibe) { fill.vibe } else { None },
        };
        let overwrite = if refresh || !current {
            Overwrite::AiValues
        } else {
            Overwrite::EmptyOnly
        };
        let mut tx = sets::begin(&state.db).await?;
        let ids = ai_fill::apply_to(&mut tx, &media.id, &fill, batch.filter(), overwrite).await?;
        tx.commit().await.map_err(retry)?;
        index::refresh_items(state, &ids).await?;
        changed |= !ids.is_empty();
        report.insert("items_updated".into(), json!(ids.len()));
    }
    Ok((changed, Value::Object(report)))
}

/// `text` (see the module docs).
async fn text(
    state: &AppState,
    media: &MediaRow,
    batch: &RecheckRow,
    mode: RecheckMode,
) -> Result<(bool, Value), JobError> {
    let Some(reader) = state.text_reader().cloned() else {
        return Err(JobError::Fatal("this server has no text reader".into()));
    };
    let model = reader.model_id().to_string();
    let read_before = ai_fill::ocr_of(media, &model).map(str::to_string);
    let (text, fresh) = match (mode, read_before) {
        (RecheckMode::Missing, Some(t)) => (t, false),
        _ => match media::read_and_store(state, media, reader).await? {
            Ok(t) => (t, true),
            Err(e) => return Err(JobError::Fatal(format!("reading the text: {e}"))),
        },
    };
    let mut changed = fresh
        && (media.ocr_model.as_deref() != Some(model.as_str()) || media.ocr_text.as_deref() != Some(text.as_str()));
    let mut tx = sets::begin(&state.db).await?;
    let ids = if mode == RecheckMode::Refresh && text.is_empty() {
        ai_fill::clear_ai_text(&mut tx, &media.id, batch.filter()).await?
    } else {
        let overwrite = if fresh {
            Overwrite::AiValues
        } else {
            Overwrite::EmptyOnly
        };
        ai_fill::apply_text_to(&mut tx, &media.id, &text, batch.filter(), overwrite).await?
    };
    tx.commit().await.map_err(retry)?;
    index::refresh_items(state, &ids).await?;
    changed |= !ids.is_empty();
    Ok((
        changed,
        json!({ "ocr_model": model, "read": fresh, "text": text, "items_updated": ids.len() }),
    ))
}
