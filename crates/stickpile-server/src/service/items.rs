//! Items: detail, edit/confirm/move, delete, likes, "find a better version",
//! re-enrichment and the client-cache sync feed.

use base64::Engine;
use serde_json::json;
use stickpile_api::{Item, ItemDetail, ItemPage, ItemSummary, Job, JobKind, LikeState, UpdateItem, Visibility};
use stickpile_media::hash::hamming;

use crate::auth::CurrentUser;
use crate::authz::{self, SEARCHABLE_SQL};
use crate::db::{i64_to_u64, source_str, to_json_list};
use crate::error::{AppError, AppResult};
use crate::ids::now;
use crate::index;
use crate::jobs;
use crate::service::sets::{self, Audience};
use crate::service::upload::{clean_list, clean_name, clean_vibe, own_hashes, stem_for};
use crate::service::{hydrate, upload};
use crate::state::AppState;

pub async fn detail(state: &AppState, viewer: &CurrentUser, item_id: &str) -> AppResult<ItemDetail> {
    let r = authz::readable_item(state, viewer.id(), item_id).await?;
    let row = hydrate::fetch_one(&state.db, &r.item_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let item = hydrate::items(&state.db, viewer.id(), std::slice::from_ref(&row))
        .await?
        .pop()
        .expect("one item");
    let set = sets::summary(&state.db, &row.set_id).await?;
    let quality: Option<stickpile_api::QualityReport> =
        sqlx::query_scalar::<_, Option<String>>("SELECT quality_report FROM media WHERE id = ?")
            .bind(&row.media_id)
            .fetch_one(&state.db)
            .await?
            .and_then(|q| serde_json::from_str(&q).ok());
    let pool = own_hashes(&state.db, viewer.id()).await?;
    let near = upload::near_duplicates(state, &pool, &row.id, &row.media_id, row.phash_u64()).await?;
    let incompatible = hydrate::incompatibilities(&row, &item.media.variants);
    Ok(ItemDetail {
        item,
        set,
        quality,
        near_duplicates: near,
        incompatible,
    })
}

pub async fn get_item(state: &AppState, viewer: &str, item_id: &str) -> AppResult<Item> {
    let row = hydrate::fetch_one(&state.db, item_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(hydrate::items(&state.db, viewer, std::slice::from_ref(&row))
        .await?
        .pop()
        .expect("one item"))
}

const CONFIRMABLE: [&str; 4] = ["name", "keywords", "emojis", "vibe"];

/// PATCH an item: edited fields become `user`-sourced; `confirm` flips the
/// listed fields to `user` without changing values; `set_id` moves the item
/// to the end of another set the caller can edit.
pub async fn update(state: &AppState, viewer: &CurrentUser, item_id: &str, req: UpdateItem) -> AppResult<Item> {
    let r = authz::editable_item(state, viewer.id(), item_id).await?;
    for c in &req.confirm {
        if !CONFIRMABLE.contains(&c.as_str()) {
            return Err(AppError::bad_request(format!(
                "cannot confirm {c:?}; use one of {CONFIRMABLE:?}"
            )));
        }
    }
    let name = req.name.as_deref().map(clean_name).transpose()?;
    if name.as_deref() == Some("") {
        return Err(AppError::bad_request("name must not be empty"));
    }
    let keywords = req.keywords.as_deref().map(|k| clean_list(k, "keywords")).transpose()?;
    let emojis = req.emojis.as_deref().map(|e| clean_list(e, "emojis")).transpose()?;
    let tags = req.tags.as_deref().map(|t| clean_list(t, "tags")).transpose()?;
    let vibe = match &req.vibe {
        None => None,
        Some(v) => Some(clean_vibe(v.as_deref())?),
    };
    let target = match req.set_id.as_deref() {
        Some(sid) if sid != r.set.id => Some(authz::editable_set(state, viewer.id(), sid).await?),
        _ => None,
    };

    let mut tx = sets::begin(&state.db).await?;
    #[derive(sqlx::FromRow)]
    struct Cur {
        name: String,
        filename: String,
    }
    let cur = sqlx::query_as::<_, Cur>("SELECT name, filename FROM items WHERE id = ?")
        .bind(item_id)
        .fetch_one(&mut *tx)
        .await?;
    let mut sets_: Vec<String> = Vec::new();
    let mut changed = false;
    let t = now();
    if let Some(n) = &name {
        let stem = if cur.filename == stem_for(&cur.name) {
            stem_for(n)
        } else {
            cur.filename.clone()
        };
        sqlx::query("UPDATE items SET name = ?, name_source = 'user', filename = ? WHERE id = ?")
            .bind(n)
            .bind(stem)
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
        changed = true;
    }
    if let Some(k) = &keywords {
        sqlx::query("UPDATE items SET keywords = ?, keywords_source = 'user' WHERE id = ?")
            .bind(to_json_list(k))
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
        changed = true;
    }
    if let Some(e) = &emojis {
        sqlx::query("UPDATE items SET emojis = ?, emojis_source = 'user' WHERE id = ?")
            .bind(to_json_list(e))
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
        changed = true;
    }
    if let Some(v) = &vibe {
        sqlx::query("UPDATE items SET vibe = ?, vibe_source = 'user' WHERE id = ?")
            .bind(v)
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
        changed = true;
    }
    if let Some(tg) = &tags {
        sqlx::query("UPDATE items SET tags = ? WHERE id = ?")
            .bind(to_json_list(tg))
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
        changed = true;
    }
    for field in &req.confirm {
        let col = format!("{field}_source");
        // `field` is one of CONFIRMABLE (checked above), so this is not injectable.
        sqlx::query(&format!("UPDATE items SET {col} = ? WHERE id = ?"))
            .bind(source_str(stickpile_api::FieldSource::User))
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
        changed = true;
    }
    if let Some(target) = &target {
        let res = sqlx::query(
            "UPDATE items SET set_id = ?, position = (SELECT COALESCE(MAX(position) + 1, 0) FROM items WHERE set_id = ?) \
             WHERE id = ?",
        )
        .bind(&target.id)
        .bind(&target.id)
        .bind(item_id)
        .execute(&mut *tx)
        .await;
        match res {
            Ok(_) => {}
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(AppError::conflict("the target set already contains this media"));
            }
            Err(e) => return Err(e.into()),
        }
        let (old_vis, new_vis) = (r.set.visibility(), target.visibility());
        if old_vis == Visibility::Public && new_vis != Visibility::Public {
            sets::tombstone(&mut tx, &[item_id.to_string()], &r.set.owner_id, Audience::Others).await?;
        } else if new_vis == Visibility::Public {
            sqlx::query("DELETE FROM item_tombstones WHERE item_id = ? AND audience = 'others'")
                .bind(item_id)
                .execute(&mut *tx)
                .await?;
        }
        sets::renumber(&mut tx, &r.set.id).await?;
        sets_.push(target.id.clone());
        changed = true;
    }
    if changed {
        sqlx::query("UPDATE items SET updated_at = ? WHERE id = ?")
            .bind(t)
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
        sets_.push(r.set.id.clone());
        for s in &sets_ {
            sets::bump_version(&mut tx, s).await?;
        }
    }
    tx.commit().await?;
    index::refresh_items(state, &[item_id.to_string()]).await?;
    get_item(state, viewer.id(), item_id).await
}

pub async fn delete(state: &AppState, viewer: &CurrentUser, item_id: &str) -> AppResult<()> {
    let r = authz::editable_item(state, viewer.id(), item_id).await?;
    let mut tx = sets::begin(&state.db).await?;
    sets::tombstone(
        &mut tx,
        &[item_id.to_string()],
        &r.set.owner_id,
        Audience::for_removal(r.set.visibility()),
    )
    .await?;
    sqlx::query("DELETE FROM items WHERE id = ?")
        .bind(item_id)
        .execute(&mut *tx)
        .await?;
    sets::renumber(&mut tx, &r.set.id).await?;
    sets::bump_version(&mut tx, &r.set.id).await?;
    tx.commit().await?;
    index::remove_items(state, &[item_id.to_string()]);
    Ok(())
}

async fn like_state(state: &AppState, viewer: &str, item_id: &str) -> AppResult<LikeState> {
    let (likes, copies): (i64, i64) = sqlx::query_as("SELECT likes, copies FROM items WHERE id = ?")
        .bind(item_id)
        .fetch_one(&state.db)
        .await?;
    let liked: Option<i64> = sqlx::query_scalar("SELECT 1 FROM ratings WHERE user_id = ? AND item_id = ?")
        .bind(viewer)
        .bind(item_id)
        .fetch_optional(&state.db)
        .await?;
    index::set_counters(state, item_id, likes.max(0) as u64, copies.max(0) as u64);
    Ok(LikeState {
        liked: liked.is_some(),
        likes: likes.max(0) as u64,
    })
}

/// Like (idempotent). Allowed on every item the caller can read.
pub async fn like(state: &AppState, viewer: &CurrentUser, item_id: &str) -> AppResult<LikeState> {
    let r = authz::readable_item(state, viewer.id(), item_id).await?;
    if !authz::can_like(viewer.id(), &r.set) {
        return Err(AppError::not_found());
    }
    let mut tx = sets::begin(&state.db).await?;
    let n = sqlx::query("INSERT OR IGNORE INTO ratings (user_id, item_id, created_at) VALUES (?, ?, ?)")
        .bind(viewer.id())
        .bind(item_id)
        .bind(now())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if n > 0 {
        sqlx::query("UPDATE items SET likes = (SELECT COUNT(*) FROM ratings WHERE item_id = ?) WHERE id = ?")
            .bind(item_id)
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    like_state(state, viewer.id(), item_id).await
}

/// Unlike (idempotent).
pub async fn unlike(state: &AppState, viewer: &CurrentUser, item_id: &str) -> AppResult<LikeState> {
    let r = authz::readable_item(state, viewer.id(), item_id).await?;
    if !authz::can_like(viewer.id(), &r.set) {
        return Err(AppError::not_found());
    }
    let mut tx = sets::begin(&state.db).await?;
    let n = sqlx::query("DELETE FROM ratings WHERE user_id = ? AND item_id = ?")
        .bind(viewer.id())
        .bind(item_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if n > 0 {
        sqlx::query("UPDATE items SET likes = (SELECT COUNT(*) FROM ratings WHERE item_id = ?) WHERE id = ?")
            .bind(item_id)
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    like_state(state, viewer.id(), item_id).await
}

/// Items the caller can search (own + public) whose pHash is within the
/// near-duplicate distance and whose quality is higher, best first.
pub async fn better(state: &AppState, viewer: &CurrentUser, item_id: &str) -> AppResult<Vec<ItemSummary>> {
    let r = authz::readable_item(state, viewer.id(), item_id).await?;
    let (phash, quality): (i64, Option<i64>) = sqlx::query_as("SELECT phash, quality FROM media WHERE id = ?")
        .bind(&r.media_id)
        .fetch_one(&state.db)
        .await?;
    let rows: Vec<(String, String, i64, Option<i64>)> = sqlx::query_as(&format!(
        "SELECT i.id, i.media_id, m.phash, m.quality FROM items i JOIN sets s ON s.id = i.set_id \
         JOIN media m ON m.id = i.media_id WHERE {SEARCHABLE_SQL} AND m.quality IS NOT NULL"
    ))
    .bind(viewer.id())
    .fetch_all(&state.db)
    .await?;
    let max = state.config.media.near_duplicate_distance;
    let base = quality.unwrap_or(-1);
    let mut cands: Vec<(String, i64)> = rows
        .into_iter()
        .filter(|(id, mid, p, q)| {
            id != item_id
                && *mid != r.media_id
                && q.is_some_and(|q| q > base)
                && hamming(i64_to_u64(phash), i64_to_u64(*p)) <= max
        })
        .map(|(id, _, _, q)| (id, q.unwrap_or(0)))
        .collect();
    cands.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    cands.truncate(50);
    let ids: Vec<String> = cands.iter().map(|c| c.0.clone()).collect();
    let mut map = hydrate::summaries_by_id(&state.db, &ids).await?;
    Ok(ids.iter().filter_map(|id| map.remove(id)).collect())
}

/// Re-run AI enrichment for an item's media (owner only).
pub async fn enrich(state: &AppState, viewer: &CurrentUser, item_id: &str) -> AppResult<Job> {
    let r = authz::editable_item(state, viewer.id(), item_id).await?;
    match state.ai_status() {
        crate::state::AiStatus::Ready => {}
        crate::state::AiStatus::Loading => {
            return Err(AppError::unavailable(
                "the AI model is still loading; try again shortly",
            ));
        }
        _ => return Err(AppError::unavailable("AI enrichment is not available on this server")),
    }
    jobs::enqueue(
        state,
        JobKind::Enrich,
        json!({"media_id": r.media_id, "item_id": item_id}),
        Some(r.media_id.clone()),
        Some(viewer.id()),
    )
    .await
}

// ---------------------------------------------------------------------------
// Client cache sync
// ---------------------------------------------------------------------------

pub const SYNC_DEFAULT_LIMIT: i64 = 500;
pub const SYNC_MAX_LIMIT: i64 = 2000;

fn encode_cursor(updated_at: i64, id: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{updated_at}:{id}"))
}

fn decode_cursor(c: &str) -> AppResult<(i64, String)> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(c)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .ok_or_else(|| AppError::bad_request("invalid cursor"))?;
    let (t, id) = raw
        .split_once(':')
        .ok_or_else(|| AppError::bad_request("invalid cursor"))?;
    Ok((
        t.parse().map_err(|_| AppError::bad_request("invalid cursor"))?,
        id.to_string(),
    ))
}

/// Every item the caller may search (own + public, never others' unlisted),
/// ordered by (`updated_at`, id), with `updated_at >= updated_since`.
/// `removed` (first page only, when `updated_since` is given) lists items
/// the caller could see before and no longer can: deleted items, items of
/// deleted sets, and other users' items whose set stopped being public.
pub async fn sync_page(
    state: &AppState,
    viewer: &CurrentUser,
    updated_since: Option<i64>,
    cursor: Option<&str>,
    limit: Option<i64>,
) -> AppResult<ItemPage> {
    let server_time = now();
    let limit = limit.unwrap_or(SYNC_DEFAULT_LIMIT).clamp(1, SYNC_MAX_LIMIT);
    let since = updated_since.unwrap_or(i64::MIN);
    let (after_t, after_id) = match cursor {
        Some(c) => decode_cursor(c)?,
        None => (i64::MIN, String::new()),
    };
    let rows = sqlx::query_as::<_, hydrate::ItemFull>(&format!(
        "{} WHERE {SEARCHABLE_SQL} AND i.updated_at >= ? AND (i.updated_at > ? OR (i.updated_at = ? AND i.id > ?)) \
         ORDER BY i.updated_at, i.id LIMIT ?",
        hydrate::ITEM_FULL_SELECT
    ))
    .bind(viewer.id())
    .bind(since)
    .bind(after_t)
    .bind(after_t)
    .bind(&after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    let more = rows.len() as i64 > limit;
    let rows = &rows[..rows.len().min(limit as usize)];
    let next_cursor = if more {
        rows.last().map(|r| encode_cursor(r.updated_at, &r.id))
    } else {
        None
    };
    let items = hydrate::summaries(&state.db, rows).await?;
    let removed = match (updated_since, cursor) {
        (Some(since), None) => {
            let mut ids: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT item_id FROM item_tombstones WHERE removed_at >= ? AND \
                 ((owner_id = ? AND audience IN ('owner', 'all')) OR (owner_id != ? AND audience IN ('others', 'all'))) \
                 AND item_id NOT IN (SELECT i.id FROM items i JOIN sets s ON s.id = i.set_id WHERE (s.owner_id = ? OR s.visibility = 'public'))",
            )
            .bind(since)
            .bind(viewer.id())
            .bind(viewer.id())
            .bind(viewer.id())
            .fetch_all(&state.db)
            .await?;
            ids.sort();
            ids
        }
        _ => Vec::new(),
    };
    Ok(ItemPage {
        items,
        removed,
        next_cursor,
        server_time,
    })
}

/// Purge tombstones older than `max_age_secs` (clients that have not synced
/// for that long should do a full resync).
pub async fn purge_tombstones(db: &sqlx::SqlitePool, max_age_secs: i64) -> AppResult<u64> {
    Ok(sqlx::query("DELETE FROM item_tombstones WHERE removed_at < ?")
        .bind(now() - max_age_secs)
        .execute(db)
        .await?
        .rows_affected())
}
