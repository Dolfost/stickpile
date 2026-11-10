//! Keeps the in-memory search index in step with the database. Every write
//! path calls [`refresh_items`] / [`refresh_set`] / [`remove_items`] after
//! committing; [`load_all`] fills the index at startup.

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::{QueryBuilder, Sqlite};
use stickpile_search::Doc;

use crate::error::AppResult;
use crate::service::hydrate::{self, ItemFull, ITEM_FULL_SELECT};
use crate::state::AppState;

pub fn doc(f: &ItemFull, embedding: Option<Arc<[f32]>>) -> Doc {
    Doc {
        item_id: f.id.clone(),
        set_id: f.set_id.clone(),
        set_slug: f.set_slug.clone(),
        set_name: f.set_name.clone(),
        owner_id: f.owner_id.clone(),
        owner_username: f.owner_username.clone(),
        visibility: f.visibility(),
        name: f.name.clone(),
        keywords: f.keywords(),
        emojis: f.emojis(),
        vibe: f.vibe.clone(),
        tags: f.tags(),
        kind: f.kind(),
        format: f.format(),
        quality: f.quality.map(|q| q.clamp(0, 100) as u8),
        likes: f.likes.max(0) as u64,
        copies: f.copies.max(0) as u64,
        media_id: f.media_id.clone(),
        embedding,
        updated_at: f.updated_at,
    }
}

/// Embeddings of the current model for `media_ids` (`None` = all media).
async fn embeddings(state: &AppState, media_ids: Option<&[String]>) -> AppResult<HashMap<String, Arc<[f32]>>> {
    let Some(model) = state.ai_model_id() else {
        return Ok(HashMap::new());
    };
    let mut out = HashMap::new();
    let mut fetch = |rows: Vec<(String, Vec<u8>)>| {
        for (id, blob) in rows {
            let v = stickpile_ai::from_blob(&blob);
            if !v.is_empty() {
                out.insert(id, Arc::<[f32]>::from(v));
            }
        }
    };
    match media_ids {
        None => {
            let rows: Vec<(String, Vec<u8>)> =
                sqlx::query_as("SELECT id, embedding FROM media WHERE embedding IS NOT NULL AND embedding_model = ?")
                    .bind(model)
                    .fetch_all(&state.db)
                    .await?;
            fetch(rows);
        }
        Some(ids) => {
            for chunk in ids.chunks(500) {
                let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
                    "SELECT id, embedding FROM media WHERE embedding IS NOT NULL AND embedding_model = ",
                );
                qb.push_bind(model);
                qb.push(" AND id IN (");
                let mut sep = qb.separated(", ");
                for id in chunk {
                    sep.push_bind(id);
                }
                qb.push(")");
                fetch(qb.build_query_as().fetch_all(&state.db).await?);
            }
        }
    }
    Ok(out)
}

fn upsert_all(state: &AppState, rows: &[ItemFull], emb: &HashMap<String, Arc<[f32]>>) {
    let mut index = state.index.write().unwrap_or_else(|e| e.into_inner());
    for r in rows {
        index.upsert(doc(r, emb.get(&r.media_id).cloned()));
    }
}

/// Load every item into the (empty) index.
pub async fn load_all(state: &AppState) -> anyhow::Result<usize> {
    let rows = sqlx::query_as::<_, ItemFull>(ITEM_FULL_SELECT)
        .fetch_all(&state.db)
        .await?;
    let emb = embeddings(state, None).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    upsert_all(state, &rows, &emb);
    Ok(rows.len())
}

/// Re-read `ids` from the database; ids that no longer exist are removed.
pub async fn refresh_items(state: &AppState, ids: &[String]) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let rows = hydrate::fetch_by_ids(&state.db, ids).await?;
    let media_ids: Vec<String> = rows.iter().map(|r| r.media_id.clone()).collect();
    let emb = embeddings(state, Some(&media_ids)).await?;
    let present: std::collections::HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    {
        let mut index = state.index.write().unwrap_or_else(|e| e.into_inner());
        for id in ids {
            if !present.contains(id.as_str()) {
                index.remove(id);
            }
        }
    }
    upsert_all(state, &rows, &emb);
    Ok(())
}

/// Re-read every item of a set (after set renames, visibility changes, imports).
pub async fn refresh_set(state: &AppState, set_id: &str) -> AppResult<()> {
    let rows = hydrate::fetch_by_set(&state.db, set_id).await?;
    let media_ids: Vec<String> = rows.iter().map(|r| r.media_id.clone()).collect();
    let emb = embeddings(state, Some(&media_ids)).await?;
    {
        let mut index = state.index.write().unwrap_or_else(|e| e.into_inner());
        index.remove_set(set_id);
    }
    upsert_all(state, &rows, &emb);
    Ok(())
}

/// Attach stored embeddings of the current model to every indexed doc
/// (after the AI backend becomes ready).
pub async fn load_embeddings(state: &AppState) -> AppResult<usize> {
    let emb = embeddings(state, None).await?;
    let mut index = state.index.write().unwrap_or_else(|e| e.into_inner());
    for (media_id, v) in &emb {
        index.set_embedding(media_id, Some(v.clone()));
    }
    Ok(emb.len())
}

pub fn remove_items(state: &AppState, ids: &[String]) {
    let mut index = state.index.write().unwrap_or_else(|e| e.into_inner());
    for id in ids {
        index.remove(id);
    }
}

pub fn remove_set(state: &AppState, set_id: &str) {
    state
        .index
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .remove_set(set_id);
}

pub fn set_counters(state: &AppState, item_id: &str, likes: u64, copies: u64) {
    state
        .index
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .set_counters(item_id, likes, copies);
}

pub fn set_embedding(state: &AppState, media_id: &str, embedding: Option<Arc<[f32]>>) {
    state
        .index
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .set_embedding(media_id, embedding);
}
