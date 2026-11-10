//! Hybrid search: the query syntax from stickpile-api, the in-memory index
//! from stickpile-search, the AI text embedding (cached) and vocabulary-based
//! query expansion.

use std::collections::HashSet;
use std::sync::Arc;

use stickpile_api::query::{Is, Query};
use stickpile_api::{SearchHit, SearchResults, TargetApp};
use stickpile_search::{SearchOptions, Viewer};

use crate::auth::CurrentUser;
use crate::authz;
use crate::error::AppResult;
use crate::service::hydrate;
use crate::state::AppState;

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 200;
/// Weight of vibe-synonym expansions (vocabulary, no model needed).
pub const VIBE_EXPANSION_WEIGHT: f32 = 0.6;
/// Weight of zero-shot related-term expansions (needs the model).
pub const RELATED_EXPANSION_WEIGHT: f32 = 0.4;
/// Zero-shot related terms added per query. CLIP text-text cosines are high
/// even for unrelated words (docs/ai.md), so only the top few are used and
/// no absolute threshold applies.
pub const RELATED_TERMS: usize = 2;
/// Minimum cosine for an image-only match.
pub const MIN_SIMILARITY: f32 = 0.2;

#[derive(Debug, Clone, Default)]
pub struct SearchParams {
    pub q: String,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub target: Option<TargetApp>,
}

/// Text embedding of `text`, from the LRU cache or the model.
async fn text_embedding(state: &AppState, text: &str) -> Option<Arc<[f32]>> {
    state.ai()?;
    let key = text.to_string();
    if let Some(v) = state
        .text_embeddings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
    {
        return Some(v);
    }
    let st = state.clone();
    let k = key.clone();
    let res = tokio::task::spawn_blocking(move || st.ai().map(|ai| ai.enricher.embed_text(&k))).await;
    match res {
        Ok(Some(Ok(v))) => {
            let v: Arc<[f32]> = Arc::from(v);
            state
                .text_embeddings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .put(key, v.clone());
            Some(v)
        }
        Ok(Some(Err(e))) => {
            tracing::warn!("query embedding failed: {e}");
            None
        }
        _ => None,
    }
}

pub async fn search(state: &AppState, viewer: &CurrentUser, params: SearchParams) -> AppResult<SearchResults> {
    let query = Query::parse(&params.q);
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = params.offset.unwrap_or(0).min(100_000);
    let text = query.text.join(" ");

    let mut expansions: Vec<(String, f32)> = Vec::new();
    let mut push = |term: &str, w: f32| {
        if !query.text.iter().any(|t| t == term) && !expansions.iter().any(|(t, _)| t == term) {
            expansions.push((term.to_string(), w));
        }
    };
    for word in query.text.iter().flat_map(|t| t.split_whitespace()) {
        for vibe in state.vocab.vibes_for_word(word) {
            push(vibe, VIBE_EXPANSION_WEIGHT);
        }
    }
    let embedding = if text.is_empty() {
        None
    } else {
        text_embedding(state, &text).await
    };
    if let (Some(emb), Some(ai)) = (&embedding, state.ai()) {
        for (term, _score) in ai.zeroshot.related_terms(emb, RELATED_TERMS) {
            push(&term, RELATED_EXPANSION_WEIGHT);
        }
    }
    let semantic = state.ai().is_some() && (text.is_empty() || embedding.is_some());

    let liked: HashSet<String> = if query.is.any.contains(&Is::Liked) || query.is.none.contains(&Is::Liked) {
        sqlx::query_scalar("SELECT item_id FROM ratings WHERE user_id = ?")
            .bind(viewer.id())
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .collect()
    } else {
        HashSet::new()
    };
    let opts = SearchOptions {
        limit,
        offset,
        text_embedding: embedding.map(|e| e.to_vec()),
        expansions,
        min_similarity: MIN_SIMILARITY,
    };
    let v = Viewer {
        user_id: viewer.id().to_string(),
        liked,
    };
    let st = state.clone();
    let q2 = query.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let index = st.index.read().unwrap_or_else(|e| e.into_inner());
        index.search(&q2, &v, &opts)
    })
    .await?;

    let ids: Vec<String> = outcome.hits.iter().map(|h| h.item_id.clone()).collect();
    let rows = hydrate::fetch_by_ids(&state.db, &ids).await?;
    let vars = hydrate::variants(&state.db, &rows.iter().map(|r| r.media_id.clone()).collect::<Vec<_>>()).await?;
    let by_id: std::collections::HashMap<&str, &hydrate::ItemFull> = rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut hits = Vec::with_capacity(outcome.hits.len());
    for h in &outcome.hits {
        let Some(row) = by_id.get(h.item_id.as_str()) else {
            continue;
        };
        // Defence in depth: never return what the caller may not search.
        if !authz::searchable(viewer.id(), &row.owner_id, row.visibility()) {
            continue;
        }
        let variants = vars.get(&row.media_id).cloned().unwrap_or_default();
        let incompatible = params.target.and_then(|t| hydrate::incompatibility(row, &variants, t));
        let matched = h
            .matched
            .iter()
            .map(|m| {
                if *m == "expansion" {
                    "semantic".to_string()
                } else {
                    m.to_string()
                }
            })
            .collect::<Vec<_>>();
        hits.push(SearchHit {
            item: hydrate::to_summary(row, variants),
            score: h.score,
            matched,
            incompatible,
        });
    }
    Ok(SearchResults {
        hits,
        total: outcome.total as u64,
        query: query.to_string(),
        semantic,
    })
}
