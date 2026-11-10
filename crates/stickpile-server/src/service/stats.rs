//! Usage events (idempotent copy reports), popular lists and per-user stats.

use std::collections::HashMap;

use serde::Deserialize;
use stickpile_api::{Popular, PopularEntry, TargetApp, UsageAck, UsageBatch, UserStats};

use crate::auth::CurrentUser;
use crate::authz::{self, SEARCHABLE_SQL};
use crate::db;
use crate::error::{AppError, AppResult};
use crate::ids::now;
use crate::index;
use crate::service::{hydrate, sets};
use crate::state::AppState;

pub const MAX_EVENTS: usize = 1000;
const YEAR: i64 = 365 * 86_400;
const DAY: i64 = 86_400;
const WEEK: i64 = 7 * DAY;

fn valid_event_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Record copy events. Idempotent by event id; events for items that do not
/// exist or that the caller cannot read are rejected; `copied_at` is clamped
/// to `[now - 1 year, now + 1 day]`.
pub async fn record_usage(state: &AppState, viewer: &CurrentUser, batch: UsageBatch) -> AppResult<UsageAck> {
    if batch.events.len() > MAX_EVENTS {
        return Err(AppError::bad_request(format!("at most {MAX_EVENTS} events per batch")));
    }
    let t = now();
    let mut ack = UsageAck {
        accepted: 0,
        duplicates: 0,
        rejected: Vec::new(),
    };
    let mut set_cache: HashMap<String, Option<db::SetRow>> = HashMap::new();
    let mut counted: HashMap<String, ()> = HashMap::new();
    let mut tx = sets::begin(&state.db).await?;
    for ev in batch.events {
        if !valid_event_id(&ev.id) || !crate::ids::is_id(&ev.item_id) {
            ack.rejected.push(ev.id);
            continue;
        }
        let set_id: Option<String> = sqlx::query_scalar("SELECT set_id FROM items WHERE id = ?")
            .bind(&ev.item_id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(set_id) = set_id else {
            ack.rejected.push(ev.id);
            continue;
        };
        if !set_cache.contains_key(&set_id) {
            let s = sqlx::query_as::<_, db::SetRow>(&format!("SELECT {} FROM sets WHERE id = ?", db::SET_COLUMNS))
                .bind(&set_id)
                .fetch_optional(&mut *tx)
                .await?;
            set_cache.insert(set_id.clone(), s);
        }
        let allowed = set_cache
            .get(&set_id)
            .and_then(|s| s.as_ref())
            .is_some_and(|s| authz::can_report_usage(viewer.id(), s));
        if !allowed {
            ack.rejected.push(ev.id);
            continue;
        }
        let copied_at = ev.copied_at.clamp(t - YEAR, t + DAY);
        let client = ev.client.as_deref().map(|c| c.chars().take(64).collect::<String>());
        let n = sqlx::query(
            "INSERT OR IGNORE INTO usage_events (id, user_id, item_id, set_id, target, variant, client, copied_at, received_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&ev.id)
        .bind(viewer.id())
        .bind(&ev.item_id)
        .bind(&set_id)
        .bind(ev.target.map(|t| t.as_str()))
        .bind(ev.variant.map(|v| v.as_str()))
        .bind(client)
        .bind(copied_at)
        .bind(t)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if n == 0 {
            ack.duplicates += 1;
        } else {
            ack.accepted += 1;
            sqlx::query("UPDATE items SET copies = copies + 1 WHERE id = ?")
                .bind(&ev.item_id)
                .execute(&mut *tx)
                .await?;
            counted.insert(ev.item_id.clone(), ());
        }
    }
    tx.commit().await?;
    for item_id in counted.keys() {
        let (likes, copies): (i64, i64) = sqlx::query_as("SELECT likes, copies FROM items WHERE id = ?")
            .bind(item_id)
            .fetch_one(&state.db)
            .await?;
        index::set_counters(state, item_id, likes.max(0) as u64, copies.max(0) as u64);
    }
    Ok(ack)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PopularScope {
    /// Everything the caller can search: own items and public items (default).
    #[default]
    Server,
    /// The caller's own items.
    Mine,
    /// One set (`set=<id>`, any set the caller can read).
    Set,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    Day,
    #[default]
    Week,
    Month,
    All,
}

impl Window {
    fn since(self, t: i64) -> i64 {
        match self {
            Window::Day => t - DAY,
            Window::Week => t - WEEK,
            Window::Month => t - 30 * DAY,
            Window::All => i64::MIN,
        }
    }
}

/// Smoothing constant for the trending ratio.
const TREND_K: f64 = 2.0;

/// Trending score: copies in the last 7 days vs the weekly average of the 4
/// weeks before, smoothed: `(c7 + k) / (avg + k)`. Only items above their
/// average trend.
pub fn trend_score(c7: i64, c28: i64) -> Option<f64> {
    let avg = c28 as f64 / 4.0;
    (c7 > 0 && c7 as f64 > avg).then(|| (c7 as f64 + TREND_K) / (avg + TREND_K))
}

/// Three separate signals (never blended): most copied in the window, most
/// liked (all-time), trending (last 7 days vs the 4 weeks before).
pub async fn popular(
    state: &AppState,
    viewer: &CurrentUser,
    scope: PopularScope,
    set_id: Option<&str>,
    window: Window,
    limit: Option<i64>,
) -> AppResult<Popular> {
    let limit = limit.unwrap_or(20).clamp(1, 100);
    let (cond, bind): (String, String) = match scope {
        PopularScope::Server => (SEARCHABLE_SQL.to_string(), viewer.id().to_string()),
        PopularScope::Mine => ("s.owner_id = ?".into(), viewer.id().to_string()),
        PopularScope::Set => {
            let sid = set_id.ok_or_else(|| AppError::bad_request("scope=set needs set=<id>"))?;
            let set = authz::readable_set(state, viewer.id(), sid).await?;
            ("i.set_id = ?".into(), set.id)
        }
    };
    let t = now();
    let copied: Vec<(String, i64)> = sqlx::query_as(&format!(
        "SELECT e.item_id, COUNT(*) AS n FROM usage_events e JOIN items i ON i.id = e.item_id \
         JOIN sets s ON s.id = i.set_id WHERE e.copied_at >= ? AND {cond} \
         GROUP BY e.item_id ORDER BY n DESC, e.item_id LIMIT ?"
    ))
    .bind(window.since(t))
    .bind(&bind)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;
    let liked: Vec<(String, i64)> = sqlx::query_as(&format!(
        "SELECT i.id, i.likes FROM items i JOIN sets s ON s.id = i.set_id WHERE i.likes > 0 AND {cond} \
         ORDER BY i.likes DESC, i.id LIMIT ?"
    ))
    .bind(&bind)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;
    let t7 = t - WEEK;
    let t35 = t - 5 * WEEK;
    let windowed: Vec<(String, i64, i64)> = sqlx::query_as(&format!(
        "SELECT e.item_id, SUM(CASE WHEN e.copied_at >= ? THEN 1 ELSE 0 END) AS c7, \
         SUM(CASE WHEN e.copied_at < ? THEN 1 ELSE 0 END) AS c28 \
         FROM usage_events e JOIN items i ON i.id = e.item_id JOIN sets s ON s.id = i.set_id \
         WHERE e.copied_at >= ? AND {cond} GROUP BY e.item_id"
    ))
    .bind(t7)
    .bind(t7)
    .bind(t35)
    .bind(&bind)
    .fetch_all(&state.db)
    .await?;
    let mut trending: Vec<(String, i64, f64)> = windowed
        .into_iter()
        .filter_map(|(id, c7, c28)| trend_score(c7, c28).map(|s| (id, c7, s)))
        .collect();
    trending.sort_by(|a, b| b.2.total_cmp(&a.2).then(b.1.cmp(&a.1)).then_with(|| a.0.cmp(&b.0)));
    trending.truncate(limit as usize);

    let mut all_ids: Vec<String> = copied.iter().map(|c| c.0.clone()).collect();
    all_ids.extend(liked.iter().map(|c| c.0.clone()));
    all_ids.extend(trending.iter().map(|c| c.0.clone()));
    all_ids.sort();
    all_ids.dedup();
    let summaries = hydrate::summaries_by_id(&state.db, &all_ids).await?;
    let entries = |list: Vec<(String, i64)>| -> Vec<PopularEntry> {
        list.into_iter()
            .filter_map(|(id, v)| {
                summaries.get(&id).map(|s| PopularEntry {
                    item: s.clone(),
                    value: v.max(0) as u64,
                })
            })
            .collect()
    };
    Ok(Popular {
        most_copied: entries(copied),
        most_liked: entries(liked),
        trending: entries(trending.into_iter().map(|(id, c7, _)| (id, c7)).collect()),
    })
}

/// Storage of a user's library: every distinct media referenced by their
/// items, with all its variants.
pub async fn user_storage(db: &sqlx::SqlitePool, user_id: &str) -> AppResult<u64> {
    let v: Option<i64> = sqlx::query_scalar(
        "SELECT SUM(v.size_bytes) FROM media_variants v WHERE v.media_id IN \
         (SELECT DISTINCT i.media_id FROM items i JOIN sets s ON s.id = i.set_id WHERE s.owner_id = ?)",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    Ok(v.unwrap_or(0).max(0) as u64)
}

pub async fn user_stats(state: &AppState, viewer: &CurrentUser) -> AppResult<UserStats> {
    let db = &state.db;
    let uid = viewer.id();
    let t = now();
    let scalar = |sql: &'static str| sqlx::query_scalar::<_, i64>(sql);
    let sets_n = scalar("SELECT COUNT(*) FROM sets WHERE owner_id = ?")
        .bind(uid)
        .fetch_one(db)
        .await?;
    let items_n = scalar("SELECT COUNT(*) FROM items i JOIN sets s ON s.id = i.set_id WHERE s.owner_id = ?")
        .bind(uid)
        .fetch_one(db)
        .await?;
    let copies_total = scalar("SELECT COUNT(*) FROM usage_events WHERE user_id = ?")
        .bind(uid)
        .fetch_one(db)
        .await?;
    let copies_7d = scalar("SELECT COUNT(*) FROM usage_events WHERE user_id = ? AND copied_at >= ?")
        .bind(uid)
        .bind(t - WEEK)
        .fetch_one(db)
        .await?;
    let likes_given = scalar("SELECT COUNT(*) FROM ratings WHERE user_id = ?")
        .bind(uid)
        .fetch_one(db)
        .await?;
    let likes_received = scalar(
        "SELECT COUNT(*) FROM ratings r JOIN items i ON i.id = r.item_id JOIN sets s ON s.id = i.set_id \
         WHERE s.owner_id = ? AND r.user_id != ?",
    )
    .bind(uid)
    .bind(uid)
    .fetch_one(db)
    .await?;
    // The caller's most copied items (by the caller), among items they can still read.
    let top: Vec<(String, i64)> = sqlx::query_as(
        "SELECT e.item_id, COUNT(*) AS n FROM usage_events e JOIN items i ON i.id = e.item_id \
         JOIN sets s ON s.id = i.set_id WHERE e.user_id = ? AND (s.owner_id = ? OR s.visibility != 'private') \
         GROUP BY e.item_id ORDER BY n DESC, e.item_id LIMIT 10",
    )
    .bind(uid)
    .bind(uid)
    .fetch_all(db)
    .await?;
    let ids: Vec<String> = top.iter().map(|t| t.0.clone()).collect();
    let summaries = hydrate::summaries_by_id(db, &ids).await?;
    let top_items = top
        .into_iter()
        .filter_map(|(id, n)| {
            summaries.get(&id).map(|s| stickpile_api::PopularEntry {
                item: s.clone(),
                value: n as u64,
            })
        })
        .collect();
    let targets: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT target, COUNT(*) AS n FROM usage_events WHERE user_id = ? AND target IS NOT NULL \
         GROUP BY target ORDER BY n DESC, target",
    )
    .bind(uid)
    .fetch_all(db)
    .await?;
    let top_targets = targets
        .into_iter()
        .filter_map(|(tgt, n)| tgt.and_then(|s| TargetApp::parse(&s)).map(|tg| (tg, n.max(0) as u64)))
        .collect();
    // Copies per day for the last 30 days (UTC days), oldest first.
    let today = t.div_euclid(DAY);
    let first = today - 29;
    let days: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT copied_at / 86400 AS d, COUNT(*) FROM usage_events WHERE user_id = ? AND copied_at >= ? GROUP BY d",
    )
    .bind(uid)
    .bind(first * DAY)
    .fetch_all(db)
    .await?;
    let mut daily = vec![0u64; 30];
    for (d, n) in days {
        if (first..=today).contains(&d) {
            daily[(d - first) as usize] += n.max(0) as u64;
        }
    }
    Ok(UserStats {
        sets: sets_n.max(0) as u64,
        items: items_n.max(0) as u64,
        storage_bytes: user_storage(db, uid).await?,
        copies_total: copies_total.max(0) as u64,
        copies_last_7d: copies_7d.max(0) as u64,
        likes_given: likes_given.max(0) as u64,
        likes_received: likes_received.max(0) as u64,
        top_items,
        top_targets,
        daily_copies: daily,
    })
}

#[cfg(test)]
mod tests {
    use super::trend_score;

    #[test]
    fn trending_needs_growth() {
        assert!(trend_score(0, 0).is_none());
        assert!(trend_score(1, 8).is_none(), "below the weekly average of 2");
        assert!(trend_score(3, 8).is_some());
        assert!(trend_score(10, 4).unwrap() > trend_score(3, 4).unwrap());
    }
}
