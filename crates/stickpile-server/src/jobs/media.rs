//! `Variants` and `Enrich` job handlers.

use std::sync::Arc;

use serde_json::json;
use stickpile_api::Variant;

use super::{JobError, JobResult, JobRow};
use crate::db::{self, VariantRow};
use crate::files;
use crate::ids::{now, sha256_hex};
use crate::index;
use crate::service::ai::{self as ai_fill, AiFill};
use crate::service::sets;
use crate::state::AppState;

fn media_id(job: &JobRow) -> Result<String, JobError> {
    job.payload()
        .get("media_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| JobError::Fatal("payload without media_id".into()))
}

async fn variant_row(state: &AppState, media_id: &str, variant: Variant) -> Result<Option<VariantRow>, JobError> {
    sqlx::query_as::<_, VariantRow>(
        "SELECT media_id, variant, mime, width, height, size_bytes, sha256, path FROM media_variants \
         WHERE media_id = ? AND variant = ?",
    )
    .bind(media_id)
    .bind(variant.as_str())
    .fetch_optional(&state.db)
    .await
    .map_err(|e| JobError::Retry(e.to_string()))
}

/// Generate every non-thumb variant of a media item and record them.
pub async fn variants(state: &AppState, job: &JobRow) -> JobResult {
    let media_id = media_id(job)?;
    generate_variants(state, &media_id).await
}

/// The body of the `Variants` job, callable directly (the Telegram export
/// renders missing sticker variants itself instead of waiting for the queue).
pub async fn generate_variants(state: &AppState, media_id: &str) -> JobResult {
    let media_id = media_id.to_string();
    let _guard = state.media_lock.read().await;
    let Some(media) = db::media_by_id(&state.db, &media_id)
        .await
        .map_err(|e| JobError::Retry(e.to_string()))?
    else {
        return Ok(Some(json!({"skipped": "media no longer exists"})));
    };
    let original = variant_row(state, &media_id, Variant::Original)
        .await?
        .ok_or_else(|| JobError::Fatal("original file missing".into()))?;
    let abs = files::resolve(&state.media_dir(), &original.path).ok_or_else(|| JobError::Fatal("bad path".into()))?;
    let probe = media.probe();
    let opts = stickpile_media::VariantOptions {
        discord_gif_max_bytes: state.config.media.discord_gif_max_mb * 1024 * 1024,
    };
    let tools = state.tools.clone();
    let renditions =
        tokio::task::spawn_blocking(move || stickpile_media::generate_variants(&abs, &probe, &opts, &tools))
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?
            // Variants that cannot meet their constraints are skipped by the
            // media crate; an error means ffmpeg (or I/O) failed: retry.
            .map_err(|e| JobError::Retry(e.to_string()))?;
    let mut made = Vec::new();
    for r in renditions {
        if matches!(r.variant, Variant::Original | Variant::Thumb) {
            continue;
        }
        let rel = files::variant_rel_path(&media.sha256, r.variant, media.format());
        let abs = files::resolve(&state.media_dir(), &rel).ok_or_else(|| JobError::Fatal("bad path".into()))?;
        let sha = sha256_hex(&r.bytes);
        let size = r.bytes.len() as i64;
        files::write_atomic(abs, r.bytes).await?;
        sqlx::query(
            "INSERT OR REPLACE INTO media_variants (media_id, variant, mime, width, height, size_bytes, sha256, path) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&media_id)
        .bind(r.variant.as_str())
        .bind(r.mime)
        .bind(r.width as i64)
        .bind(r.height as i64)
        .bind(size)
        .bind(&sha)
        .bind(&rel)
        .execute(&state.db)
        .await
        .map_err(|e| JobError::Retry(e.to_string()))?;
        made.push(r.variant.as_str());
    }
    let t = now();
    sqlx::query("UPDATE media SET variants_done = 1 WHERE id = ?")
        .bind(&media_id)
        .execute(&state.db)
        .await
        .map_err(|e| JobError::Retry(e.to_string()))?;
    // Summaries list variants: let client caches pick up the new ones.
    sqlx::query("UPDATE items SET updated_at = ? WHERE media_id = ?")
        .bind(t)
        .bind(&media_id)
        .execute(&state.db)
        .await
        .map_err(|e| JobError::Retry(e.to_string()))?;
    Ok(Some(json!({ "variants": made })))
}

/// Embed the representative frame (decoded from the stored thumbnail), store
/// the embedding and zero-shot suggestions, and fill AI fields of the items.
pub async fn enrich(state: &AppState, job: &JobRow) -> JobResult {
    let media_id = media_id(job)?;
    if state.ai_status() == crate::state::AiStatus::Loading {
        return Err(JobError::Retry("the AI model is still loading".into()));
    }
    if state.ai().is_none() {
        return Err(JobError::Fatal(
            "AI enrichment is not available (ai.enabled = false or the model failed to load)".into(),
        ));
    }
    let Some(_media) = db::media_by_id(&state.db, &media_id)
        .await
        .map_err(|e| JobError::Retry(e.to_string()))?
    else {
        return Ok(Some(json!({"skipped": "media no longer exists"})));
    };
    let thumb = variant_row(state, &media_id, Variant::Thumb)
        .await?
        .ok_or_else(|| JobError::Fatal("thumbnail missing".into()))?;
    let abs = files::resolve(&state.media_dir(), &thumb.path).ok_or_else(|| JobError::Fatal("bad path".into()))?;
    let bytes = tokio::fs::read(&abs)
        .await
        .map_err(|e| JobError::Retry(format!("reading thumbnail: {e}")))?;
    let st = state.clone();
    let cfg = state.config.ai.clone();
    let (embedding, suggestions, model) = tokio::task::spawn_blocking(move || {
        let ai = st.ai().expect("checked");
        let frame =
            stickpile_media::decode_frame(&bytes, &st.media_limits()).map_err(|e| JobError::Fatal(e.to_string()))?;
        let (emb, sugg) = ai_fill::embed_and_suggest(ai, &cfg, &frame).map_err(|e| JobError::Retry(e.to_string()))?;
        Ok::<_, JobError>((emb, sugg, ai.image_model_id().to_string()))
    })
    .await
    .map_err(|e| JobError::Retry(e.to_string()))??;

    let kw_json = serde_json::to_string(&suggestions.keywords).unwrap_or_else(|_| "[]".into());
    let em_json = serde_json::to_string(&suggestions.emojis).unwrap_or_else(|_| "[]".into());
    let fill = AiFill::from_suggestions(&suggestions);
    let mut tx = sets::begin(&state.db).await?;
    sqlx::query(
        "UPDATE media SET embedding = ?, embedding_model = ?, ai_keywords = ?, ai_emojis = ?, ai_vibe = ? WHERE id = ?",
    )
    .bind(stickpile_ai::to_blob(&embedding))
    .bind(&model)
    .bind(&kw_json)
    .bind(&em_json)
    .bind(&fill.vibe)
    .bind(&media_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| JobError::Retry(e.to_string()))?;
    let changed = ai_fill::apply(&mut tx, &media_id, &fill, None).await?;
    tx.commit().await.map_err(|e| JobError::Retry(e.to_string()))?;

    index::set_embedding(state, &media_id, Some(Arc::from(embedding)));
    index::refresh_items(state, &changed).await?;
    Ok(Some(json!({
        "model": model,
        "keywords": fill.keywords,
        "emojis": fill.emojis,
        "vibe": fill.vibe,
        "items_updated": changed.len(),
    })))
}

/// Enqueue `Enrich` for every referenced media without an embedding from the
/// current image model (new installs, model changes, uploads made while the
/// model was loading). Returns how many jobs were queued.
pub async fn enqueue_missing_enrichment(state: &AppState) -> crate::error::AppResult<usize> {
    let Some(model) = state.ai_model_id().map(str::to_string) else {
        return Ok(0);
    };
    let ids: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT m.id, (SELECT s.owner_id FROM items i JOIN sets s ON s.id = i.set_id WHERE i.media_id = m.id LIMIT 1) \
         FROM media m WHERE (m.embedding_model IS NULL OR m.embedding_model != ?) \
         AND EXISTS (SELECT 1 FROM items i WHERE i.media_id = m.id) \
         AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.kind = 'enrich' AND j.subject = m.id AND j.status IN ('queued', 'running'))",
    )
    .bind(&model)
    .fetch_all(&state.db)
    .await?;
    for (media_id, owner) in &ids {
        super::insert(
            &state.db,
            stickpile_api::JobKind::Enrich,
            json!({ "media_id": media_id }),
            Some(media_id.clone()),
            owner.as_deref(),
        )
        .await?;
    }
    if !ids.is_empty() {
        state.wake_workers();
    }
    Ok(ids.len())
}
