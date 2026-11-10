//! Queries only the web UI needs: recently copied items, upload targets, job
//! queue overview and job lookup with the API's visibility rule.

use stickpile_api::{ItemSummary, Job, JobStatus, SetSummary};

use crate::auth::CurrentUser;
use crate::authz;
use crate::db::{self, SetRow, SET_COLUMNS};
use crate::error::{AppError, AppResult};
use crate::jobs;
use crate::service::{hydrate, sets};
use crate::state::AppState;

/// Items the viewer copied most recently (distinct, newest first), among
/// items they can still read.
pub async fn recently_copied(state: &AppState, viewer: &CurrentUser, limit: i64) -> AppResult<Vec<ItemSummary>> {
    let ids: Vec<(String, i64)> = sqlx::query_as(
        "SELECT e.item_id, MAX(e.copied_at) AS last FROM usage_events e JOIN items i ON i.id = e.item_id \
         JOIN sets s ON s.id = i.set_id WHERE e.user_id = ? AND (s.owner_id = ? OR s.visibility != 'private') \
         GROUP BY e.item_id ORDER BY last DESC, e.item_id LIMIT ?",
    )
    .bind(viewer.id())
    .bind(viewer.id())
    .bind(limit.clamp(1, 200))
    .fetch_all(&state.db)
    .await?;
    let ids: Vec<String> = ids.into_iter().map(|(id, _)| id).collect();
    let mut map = hydrate::summaries_by_id(&state.db, &ids).await?;
    Ok(ids.iter().filter_map(|id| map.remove(id)).collect())
}

/// The viewer's sets they can add items to (own sets except read-only git
/// packs), by name.
pub async fn editable_sets(state: &AppState, viewer: &CurrentUser) -> AppResult<Vec<SetSummary>> {
    let rows = sqlx::query_as::<_, SetRow>(&format!(
        "SELECT {SET_COLUMNS} FROM sets WHERE owner_id = ? ORDER BY name COLLATE NOCASE, id"
    ))
    .bind(viewer.id())
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::new();
    for row in rows.iter().filter(|r| authz::can_edit_set(viewer.id(), r)) {
        out.push(sets::summary(&state.db, &row.id).await?);
    }
    Ok(out)
}

/// One row of the job queue overview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobCount {
    pub kind: String,
    pub status: String,
    pub count: u64,
}

/// Jobs grouped by kind and status (admin overview).
pub async fn job_counts(state: &AppState) -> AppResult<Vec<JobCount>> {
    let rows: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT kind, status, COUNT(*) FROM jobs GROUP BY kind, status ORDER BY kind, status")
            .fetch_all(&state.db)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(kind, status, n)| JobCount {
            kind,
            status,
            count: n.max(0) as u64,
        })
        .collect())
}

/// Most recent failed jobs (admin overview).
pub async fn recent_failures(state: &AppState, limit: i64) -> AppResult<Vec<Job>> {
    jobs::list(&state.db, Some(JobStatus::Failed), limit.clamp(1, 1000)).await
}

/// A job, visible to its owner and to admins (404 otherwise), like
/// `GET /api/v1/jobs/{id}`.
pub async fn job(state: &AppState, viewer: &CurrentUser, id: &str) -> AppResult<Job> {
    if !crate::ids::is_id(id) {
        return Err(AppError::not_found());
    }
    let job = jobs::get(&state.db, id).await?.ok_or_else(AppError::not_found)?;
    if job.owner_id.as_deref() != Some(viewer.id()) && !viewer.is_admin() {
        return Err(AppError::not_found());
    }
    Ok(job.to_api())
}

/// The raw set row for a set the viewer may read (404 otherwise): the item
/// page needs the edit rule, which summaries do not carry.
pub async fn readable_set_row(state: &AppState, viewer: &CurrentUser, set_id: &str) -> AppResult<SetRow> {
    authz::readable_set(state, viewer.id(), set_id).await
}

/// Whether the viewer may edit the set (owner, not a read-only git pack).
pub fn can_edit(viewer: &CurrentUser, set: &SetRow) -> bool {
    authz::can_edit_set(viewer.id(), set)
}

/// Re-read the viewer's own user row (fresh TOTP state for the settings page).
pub async fn me(state: &AppState, viewer: &CurrentUser) -> AppResult<db::UserRow> {
    db::user_by_id(&state.db, viewer.id())
        .await?
        .ok_or_else(|| AppError::unauthorized("account no longer exists"))
}
