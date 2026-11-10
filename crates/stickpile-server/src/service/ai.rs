//! Applying AI suggestions to items. AI values only fill fields that are
//! empty or still AI-sourced; anything the user wrote or confirmed is never
//! overwritten. Filled fields are marked `ai`.

use std::collections::HashSet;

use stickpile_ai::Suggestions;

use crate::db::{json_list, to_json_list, MediaRow};
use crate::error::AppResult;
use crate::ids::now;
use crate::service::sets;

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
