//! AI suggestions: the zero-shot path shared by the `Enrich` job and the
//! upload tray's suggestions ([`suggest`]), and applying them to items. AI
//! values only fill fields that are empty or still AI-sourced; anything the
//! user wrote or confirmed is never overwritten. Filled fields are marked `ai`.

use std::collections::HashSet;

use image::RgbaImage;
use stickpile_ai::{AiError, Suggestions};
use stickpile_api::AiSuggestions;

use crate::config::AiConfig;
use crate::db::{json_list, to_json_list, MediaRow};
use crate::error::{AppError, AppResult};
use crate::ids::now;
use crate::service::sets;
use crate::state::{Ai, AiStatus, AppState};

/// Largest image `POST /api/v1/ai/suggest` takes. CLIP looks at 224 × 224
/// pixels, so clients should send a small rendering (the upload tray sends
/// a PNG with the longest side 256 px, like the grid thumbnails).
pub const SUGGEST_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Decoded size limit for suggestions (below `limits.max_pixels` when that is
/// larger), so a small compressed file cannot make the server decode a huge
/// frame just to scale it down to 224 px.
pub const SUGGEST_MAX_PIXELS: u64 = 4096 * 4096;

/// Embed `frame` and pick keywords, emojis and a vibe with the configured
/// limits and the zero-shot thresholds. The one path behind both the
/// `Enrich` job and [`suggest`], so the tray suggests what enrichment would.
pub fn embed_and_suggest(ai: &Ai, cfg: &AiConfig, frame: &RgbaImage) -> Result<(Vec<f32>, Suggestions), AiError> {
    let embedding = ai.enricher.embed_image(frame)?;
    let suggestions = ai
        .zeroshot
        .suggest(&embedding, cfg.max_keywords as usize, cfg.max_emojis as usize);
    Ok((embedding, suggestions))
}

/// `Retry-After` while the model loads. Loading takes seconds (the
/// vocabulary is embedded at startup), so clients ask again soon rather
/// than wait long after it is ready.
pub const LOADING_RETRY_SECS: u64 = 2;

/// 503 unless the AI backend is ready (see [`not_ready`]).
pub fn require_ready(state: &AppState) -> AppResult<()> {
    match state.ai_status() {
        AiStatus::Ready if state.ai().is_some() => Ok(()),
        // Published just before the status, so this is only a race.
        AiStatus::Ready => Err(not_ready(AiStatus::Loading)),
        s => Err(not_ready(s)),
    }
}

/// Why AI cannot answer: `ai_loading` with `Retry-After` while the model
/// loads ("later"), `unavailable` without when AI is off or failed to load
/// ("not on this server").
pub fn not_ready(status: AiStatus) -> AppError {
    match status {
        AiStatus::Loading | AiStatus::Ready => AppError::ai_loading(
            "the AI model is still loading; try again in a few seconds",
            LOADING_RETRY_SECS,
        ),
        AiStatus::Failed => AppError::unavailable("the AI model could not be loaded on this server"),
        AiStatus::Off => AppError::unavailable("AI is turned off on this server"),
    }
}

/// Suggestions for one still image (PNG, JPEG, WebP or GIF; animations give
/// their first frame), without storing anything. Decoding and inference run
/// on the blocking pool. Callers check [`require_ready`], authentication and
/// the rate limit first.
pub async fn suggest(state: &AppState, bytes: Vec<u8>) -> AppResult<AiSuggestions> {
    let st = state.clone();
    let cfg = state.config.ai.clone();
    let mut limits = state.media_limits();
    limits.max_bytes = SUGGEST_MAX_BYTES;
    limits.max_pixels = limits.max_pixels.min(SUGGEST_MAX_PIXELS);
    tokio::task::spawn_blocking(move || {
        let ai = st.ai().ok_or_else(|| not_ready(st.ai_status()))?;
        let frame = stickpile_media::decode_frame(&bytes, &limits)?;
        let (_, s) = embed_and_suggest(ai, &cfg, &frame).map_err(|e| AppError::internal(format!("AI: {e}")))?;
        let fill = AiFill::from_suggestions(&s);
        Ok(AiSuggestions {
            keywords: fill.keywords,
            emojis: fill.emojis,
            vibe: fill.vibe,
        })
    })
    .await?
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AiFill {
    pub keywords: Vec<String>,
    pub emojis: Vec<String>,
    pub vibe: Option<String>,
}

impl AiFill {
    pub fn from_suggestions(s: &Suggestions) -> AiFill {
        AiFill {
            keywords: s.keywords.iter().map(|(k, _)| k.clone()).collect(),
            emojis: s.emojis.iter().map(|(e, _)| e.clone()).collect(),
            vibe: s.vibe.as_ref().map(|(v, _)| v.clone()),
        }
    }

    /// The last zero-shot output stored on a media row, if it was enriched.
    pub fn from_media(m: &MediaRow) -> Option<AiFill> {
        fn names(json: Option<&str>) -> Vec<String> {
            json.and_then(|j| serde_json::from_str::<Vec<(String, f32)>>(j).ok())
                .map(|v| v.into_iter().map(|(k, _)| k).collect())
                .unwrap_or_default()
        }
        if m.ai_keywords.is_none() && m.ai_emojis.is_none() && m.ai_vibe.is_none() {
            return None;
        }
        Some(AiFill {
            keywords: names(m.ai_keywords.as_deref()),
            emojis: names(m.ai_emojis.as_deref()),
            vibe: m.ai_vibe.clone(),
        })
    }
}

#[derive(sqlx::FromRow)]
struct Target {
    id: String,
    set_id: String,
    keywords: String,
    keywords_source: String,
    emojis: String,
    emojis_source: String,
    vibe: Option<String>,
    vibe_source: String,
}

/// Fill the items of `media_id` (or just `only_item`) from `fill`. Bumps the
/// version of every touched set. Returns the ids of changed items.
pub async fn apply(
    conn: &mut sqlx::SqliteConnection,
    media_id: &str,
    fill: &AiFill,
    only_item: Option<&str>,
) -> AppResult<Vec<String>> {
    let targets = sqlx::query_as::<_, Target>(
        "SELECT id, set_id, keywords, keywords_source, emojis, emojis_source, vibe, vibe_source FROM items \
         WHERE media_id = ? AND (? IS NULL OR id = ?)",
    )
    .bind(media_id)
    .bind(only_item)
    .bind(only_item)
    .fetch_all(&mut *conn)
    .await?;
    let mut changed = Vec::new();
    let mut sets_touched = HashSet::new();
    let t = now();
    for it in targets {
        let mut keywords = (it.keywords.clone(), it.keywords_source.clone());
        let mut emojis = (it.emojis.clone(), it.emojis_source.clone());
        let mut vibe = (it.vibe.clone(), it.vibe_source.clone());
        if !fill.keywords.is_empty() && (json_list(&it.keywords).is_empty() || it.keywords_source == "ai") {
            keywords = (to_json_list(&fill.keywords), "ai".into());
        }
        if !fill.emojis.is_empty() && (json_list(&it.emojis).is_empty() || it.emojis_source == "ai") {
            emojis = (to_json_list(&fill.emojis), "ai".into());
        }
        if fill.vibe.is_some() && (it.vibe.is_none() || it.vibe_source == "ai") {
            vibe = (fill.vibe.clone(), "ai".into());
        }
        let differs = keywords != (it.keywords.clone(), it.keywords_source.clone())
            || emojis != (it.emojis.clone(), it.emojis_source.clone())
            || vibe != (it.vibe.clone(), it.vibe_source.clone());
        if !differs {
            continue;
        }
        sqlx::query(
            "UPDATE items SET keywords = ?, keywords_source = ?, emojis = ?, emojis_source = ?, vibe = ?, \
             vibe_source = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&keywords.0)
        .bind(&keywords.1)
        .bind(&emojis.0)
        .bind(&emojis.1)
        .bind(&vibe.0)
        .bind(&vibe.1)
        .bind(t)
        .bind(&it.id)
        .execute(&mut *conn)
        .await?;
        sets_touched.insert(it.set_id);
        changed.push(it.id);
    }
    for set_id in sets_touched {
        sets::bump_version(conn, &set_id).await?;
    }
    Ok(changed)
}
