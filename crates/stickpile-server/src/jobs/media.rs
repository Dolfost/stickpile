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
    sqlx::query("UPDATE media SET variants_done = 1, variants_version = ? WHERE id = ?")
        .bind(stickpile_media::variants_version(media.kind()))
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

/// Queue a `Variants` job for every media row whose renditions were made by
/// an older [`stickpile_media::variants_version`] (and none is pending yet),
/// so encoder changes reach stored items. Rows whose variants never finished
/// are left alone: their own job is pending or failed. Returns how many jobs
/// were queued; the workers pick them up once they run.
pub async fn requeue_outdated_variants(pool: &sqlx::SqlitePool) -> crate::error::AppResult<u64> {
    let rows: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT id, kind, variants_version FROM media WHERE variants_done = 1")
            .fetch_all(pool)
            .await?;
    let mut queued = 0;
    for (id, kind, version) in rows {
        let Some(kind) = db::parse_enum::<stickpile_api::MediaKind>(&kind) else {
            continue;
        };
        if version >= stickpile_media::variants_version(kind)
            || super::pending_exists(pool, super::JobKind::Variants, &id).await?
        {
            continue;
        }
        super::insert(
            pool,
            super::JobKind::Variants,
            json!({ "media_id": id }),
            Some(id.clone()),
            None,
        )
        .await?;
        queued += 1;
    }
    Ok(queued)
}

/// Embed the representative frame (decoded from the stored thumbnail), store
/// the embedding and zero-shot suggestions, and fill AI fields of the items.
/// Then, with a text reader, read the words in the stored original unless
/// this reader read them before ([`read_words`]).
pub async fn enrich(state: &AppState, job: &JobRow) -> JobResult {
    let media_id = media_id(job)?;
    // A download takes minutes, more than the retries wait: meanwhile do
    // what is loaded. The server's rechecks fill in the rest once the
    // models are ready (`app::finish_ai`, `app::publish_text_reader`).
    let downloading = state.model_download().is_some();
    if state.ai_status() == crate::state::AiStatus::Loading && !downloading {
        return Err(JobError::Retry("the AI model is still loading".into()));
    }
    if state.ai().is_none() && state.text_reader().is_none() && downloading {
        return Ok(Some(
            json!({"skipped": "the AI models are downloading; the server's recheck reads this picture once they are ready"}),
        ));
    }
    if state.ai().is_none() && state.text_reader().is_none() {
        return Err(JobError::Fatal(
            "AI enrichment is not available (ai.enabled = false or the model failed to load)".into(),
        ));
    }
    let Some(media) = db::media_by_id(&state.db, &media_id)
        .await
        .map_err(|e| JobError::Retry(e.to_string()))?
    else {
        return Ok(Some(json!({"skipped": "media no longer exists"})));
    };
    let mut result = match state.ai() {
        Some(_) => embed(state, &media_id).await?,
        None => json!({}),
    };
    if let Some(reader) = state.text_reader() {
        let words = read_words(state, &media, reader.clone()).await?;
        if let (Some(obj), serde_json::Value::Object(w)) = (result.as_object_mut(), words) {
            obj.extend(w);
        }
    }
    Ok(Some(result))
}

/// The CLIP part of [`enrich`].
async fn embed(state: &AppState, media_id: &str) -> Result<serde_json::Value, JobError> {
    let embedded = embed_media(state, media_id).await?;
    let fill = AiFill::from_suggestions(&embedded.suggestions);
    let mut tx = sets::begin(&state.db).await?;
    store_embedding(&mut tx, media_id, &embedded).await?;
    let changed = ai_fill::apply(&mut tx, media_id, &fill, None).await?;
    tx.commit().await.map_err(|e| JobError::Retry(e.to_string()))?;

    index::set_embedding(state, media_id, Some(Arc::from(embedded.embedding)));
    index::refresh_items(state, &changed).await?;
    Ok(json!({
        "model": embedded.model,
        "keywords": fill.keywords,
        "emojis": fill.emojis,
        "vibe": fill.vibe,
        "items_updated": changed.len(),
    }))
}

/// A picture's CLIP embedding with its zero-shot suggestions.
pub struct Embedded {
    pub embedding: Vec<f32>,
    pub suggestions: stickpile_ai::Suggestions,
    /// The image model that made it.
    pub model: String,
}

/// Embed the representative frame of `media_id` (decoded from its stored
/// thumbnail) and pick its suggestions, on the blocking pool. Needs the AI
/// backend ([`AppState::ai`]).
pub async fn embed_media(state: &AppState, media_id: &str) -> Result<Embedded, JobError> {
    let thumb = variant_row(state, media_id, Variant::Thumb)
        .await?
        .ok_or_else(|| JobError::Fatal("thumbnail missing".into()))?;
    let abs = files::resolve(&state.media_dir(), &thumb.path).ok_or_else(|| JobError::Fatal("bad path".into()))?;
    let bytes = tokio::fs::read(&abs)
        .await
        .map_err(|e| JobError::Retry(format!("reading thumbnail: {e}")))?;
    let st = state.clone();
    let cfg = state.config.ai.clone();
    tokio::task::spawn_blocking(move || {
        let ai = st
            .ai()
            .ok_or_else(|| JobError::Retry("the AI model is not loaded".into()))?;
        let frame =
            stickpile_media::decode_frame(&bytes, &st.media_limits()).map_err(|e| JobError::Fatal(e.to_string()))?;
        let (embedding, suggestions) =
            ai_fill::embed_and_suggest(ai, &cfg, &frame).map_err(|e| JobError::Retry(e.to_string()))?;
        Ok::<_, JobError>(Embedded {
            embedding,
            suggestions,
            model: ai.image_model_id().to_string(),
        })
    })
    .await
    .map_err(|e| JobError::Retry(e.to_string()))?
}

/// Store an embedding and its suggestions on the media row (the caller
/// updates the search index after the commit).
pub async fn store_embedding(conn: &mut sqlx::SqliteConnection, media_id: &str, e: &Embedded) -> Result<(), JobError> {
    store_suggestions(conn, media_id, &e.suggestions).await?;
    sqlx::query("UPDATE media SET embedding = ?, embedding_model = ? WHERE id = ?")
        .bind(stickpile_ai::to_blob(&e.embedding))
        .bind(&e.model)
        .bind(media_id)
        .execute(&mut *conn)
        .await
        .map_err(|e| JobError::Retry(e.to_string()))?;
    Ok(())
}

/// Store the last zero-shot output on the media row (what new items of the
/// same picture get, [`AiFill::from_media`]).
pub async fn store_suggestions(
    conn: &mut sqlx::SqliteConnection,
    media_id: &str,
    s: &stickpile_ai::Suggestions,
) -> Result<(), JobError> {
    let kw_json = serde_json::to_string(&s.keywords).unwrap_or_else(|_| "[]".into());
    let em_json = serde_json::to_string(&s.emojis).unwrap_or_else(|_| "[]".into());
    sqlx::query("UPDATE media SET ai_keywords = ?, ai_emojis = ?, ai_vibe = ? WHERE id = ?")
        .bind(&kw_json)
        .bind(&em_json)
        .bind(s.vibe.as_ref().map(|(v, _)| v.clone()))
        .bind(media_id)
        .execute(&mut *conn)
        .await
        .map_err(|e| JobError::Retry(e.to_string()))?;
    Ok(())
}

/// The OCR part of [`enrich`]: the words of the stored, sanitised original
/// (a still, or an animation's first, middle and last frames), stored on
/// the media with the reader's model id, unless that reader read them
/// before (a file is read once, however many items it is in); then the
/// items whose text is still the computer's get them
/// ([`ai_fill::apply_text`]). Reading runs on the blocking pool. A failed
/// read fails nothing: it is logged and reported as `ocr_error`, and the
/// media stays unread for a later run.
async fn read_words(
    state: &AppState,
    media: &db::MediaRow,
    reader: Arc<dyn stickpile_ai::TextReader>,
) -> Result<serde_json::Value, JobError> {
    let model = reader.model_id().to_string();
    let text = match ai_fill::ocr_of(media, &model) {
        Some(t) => t.to_string(),
        None => match read_and_store(state, media, reader).await? {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(media = %media.id, "reading the text failed: {e}");
                return Ok(json!({ "ocr_model": model, "ocr_error": e }));
            }
        },
    };
    let mut tx = sets::begin(&state.db).await?;
    let changed = ai_fill::apply_text(&mut tx, &media.id, &text, None).await?;
    tx.commit().await.map_err(|e| JobError::Retry(e.to_string()))?;
    index::refresh_items(state, &changed).await?;
    Ok(json!({ "ocr_model": model, "text": text, "text_items_updated": changed.len() }))
}

/// Read the words of a media's stored, sanitised original with `reader`
/// (on the blocking pool) and store the result with the reader's model id,
/// whatever was stored before. `Ok(Err(message))` when reading failed
/// (nothing is stored then).
pub async fn read_and_store(
    state: &AppState,
    media: &db::MediaRow,
    reader: Arc<dyn stickpile_ai::TextReader>,
) -> Result<Result<String, String>, JobError> {
    let model = reader.model_id().to_string();
    let original = variant_row(state, &media.id, Variant::Original)
        .await?
        .ok_or_else(|| JobError::Fatal("original file missing".into()))?;
    let abs = files::resolve(&state.media_dir(), &original.path).ok_or_else(|| JobError::Fatal("bad path".into()))?;
    let probe = media.probe();
    let tools = state.tools.clone();
    let read = tokio::task::spawn_blocking(move || {
        let frames = stickpile_media::text_frames(&abs, &probe, &tools).map_err(|e| e.to_string())?;
        ai_fill::read_text(reader.as_ref(), &frames).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| JobError::Retry(e.to_string()))?;
    if let Ok(t) = &read {
        sqlx::query("UPDATE media SET ocr_text = ?, ocr_model = ? WHERE id = ?")
            .bind(t)
            .bind(&model)
            .bind(&media.id)
            .execute(&state.db)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?;
    }
    Ok(read)
}

/// Read the stored, sanitised original of a media row (the caller holds
/// `state.media_lock` shared).
pub async fn read_original(state: &AppState, media_id: &str) -> Result<Vec<u8>, JobError> {
    let original = variant_row(state, media_id, Variant::Original)
        .await?
        .ok_or_else(|| JobError::Fatal("original file missing".into()))?;
    let abs = files::resolve(&state.media_dir(), &original.path).ok_or_else(|| JobError::Fatal("bad path".into()))?;
    tokio::fs::read(&abs)
        .await
        .map_err(|e| JobError::Retry(format!("reading the original: {e}")))
}
