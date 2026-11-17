//! Rechecks (docs/ai.md, "Recheck"): batches that recompute metadata of
//! many pictures in the background. A user rechecks the items of the sets
//! they own, an admin every item on the server, either optionally one set;
//! the server itself starts server-wide ones at startup
//! ([`start_system_clip`], [`start_system_text`]).
//!
//! A batch is a `rechecks` row plus one `recheck` job per picture (media)
//! in scope, queued below every other job ([`crate::jobs::RECHECK_PRIORITY`]).
//! The job body is [`crate::jobs::recheck`]; when a job ends,
//! [`record_job`] counts it, and the batch is over when
//! `done + failed = total`. Jobs persist, so a restart continues the batch
//! (`jobs::requeue_interrupted`). Cancelling marks the batch `cancelled`
//! and deletes its queued jobs; one that runs finishes and is counted.
//! Asking for the same scope (owner, set, parts, mode) while it runs
//! returns the running batch.

use std::collections::HashSet;

use serde_json::{json, Value};
use sqlx::SqlitePool;
use stickpile_api::{
    JobKind, Recheck, RecheckFailure, RecheckMode, RecheckPart, RecheckScope, RecheckState, StartRecheck,
};

use crate::auth::CurrentUser;
use crate::authz;
use crate::db::{self, enum_str, parse_enum};
use crate::error::{AppError, AppResult};
use crate::ids::{new_id, now};
use crate::jobs::{self, JobRow};
use crate::service::ai::{self as ai_fill, ItemFilter};
use crate::service::sets;
use crate::state::{AiStatus, AppState};

/// Batches a list returns at most.
pub const LIST_LIMIT: i64 = 200;
/// Failures a batch lists at most.
pub const FAILURES_LIMIT: i64 = 1000;

const SELECT: &str = "SELECT r.id, r.scope, r.requested_by, r.owner_id, r.set_id, r.parts, r.mode, r.state, \
     r.total, r.done, r.failed, r.changed, r.created_at, r.finished_at, u.username AS requested_by_username \
     FROM rechecks r LEFT JOIN users u ON u.id = r.requested_by";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RecheckRow {
    pub id: String,
    pub scope: String,
    pub requested_by: Option<String>,
    pub owner_id: Option<String>,
    pub set_id: Option<String>,
    pub parts: String,
    pub mode: String,
    pub state: String,
    pub total: i64,
    pub done: i64,
    pub failed: i64,
    pub changed: i64,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub requested_by_username: Option<String>,
}

impl RecheckRow {
    pub fn parts(&self) -> Vec<RecheckPart> {
        serde_json::from_str(&self.parts).unwrap_or_default()
    }
    pub fn mode(&self) -> RecheckMode {
        parse_enum(&self.mode).unwrap_or(RecheckMode::Missing)
    }
    pub fn scope(&self) -> RecheckScope {
        parse_enum(&self.scope).unwrap_or(RecheckScope::Other)
    }
    pub fn state(&self) -> RecheckState {
        parse_enum(&self.state).unwrap_or(RecheckState::Other)
    }
    /// The items its jobs may write: the owner's (user scope), of the set.
    pub fn filter(&self) -> ItemFilter<'_> {
        ItemFilter {
            item: None,
            owner: self.owner_id.as_deref(),
            set: self.set_id.as_deref(),
        }
    }
    pub fn to_api(&self) -> Recheck {
        Recheck {
            id: self.id.clone(),
            scope: self.scope(),
            parts: self.parts(),
            mode: self.mode(),
            set_id: self.set_id.clone(),
            requested_by: self.requested_by_username.clone(),
            state: self.state(),
            total: self.total.max(0) as u64,
            done: self.done.max(0) as u64,
            failed: self.failed.max(0) as u64,
            changed: self.changed.max(0) as u64,
            created_at: self.created_at,
            finished_at: self.finished_at,
        }
    }
}

pub async fn row(db: &SqlitePool, id: &str) -> AppResult<Option<RecheckRow>> {
    Ok(sqlx::query_as::<_, RecheckRow>(&format!("{SELECT} WHERE r.id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await?)
}

// ---------------------------------------------------------------------------
// Starting
// ---------------------------------------------------------------------------

/// What a new batch covers.
#[derive(Debug, Clone)]
pub struct Spec {
    /// `User` (with `owner_id`) or `Server`.
    pub scope: RecheckScope,
    pub owner_id: Option<String>,
    /// `None`: the server itself.
    pub requested_by: Option<String>,
    pub set_id: Option<String>,
    /// Sorted, no repeats ([`parts`]).
    pub parts: Vec<RecheckPart>,
    pub mode: RecheckMode,
}

impl Spec {
    fn parts_json(&self) -> String {
        serde_json::to_string(&self.parts).unwrap_or_else(|_| "[]".into())
    }
}

/// Which pictures in scope get a job.
enum Pick {
    /// Those where a chosen part is missing ([`missing_sql`]).
    Missing,
    /// All of them.
    All,
    /// The server's own batches: this SQL condition on `media m`, minus the
    /// pictures a queued `enrich` job or a running batch of the server with
    /// the same parts covers.
    System(String),
}

/// The parts asked for, sorted and checked against what the server can do
/// now: none asked means every part it can recompute (the CLIP parts with a
/// loaded model, `text` with a text reader, `quality` and `renditions`
/// always). 400 for a part this server does not know, 503 for one it cannot
/// recompute (`ai_loading` while the model loads).
pub fn parts(state: &AppState, asked: &[RecheckPart]) -> AppResult<Vec<RecheckPart>> {
    if asked.contains(&RecheckPart::Other) {
        return Err(AppError::bad_request("unknown recheck part"));
    }
    let mut parts: Vec<RecheckPart> = if asked.is_empty() {
        let mut all = vec![RecheckPart::Quality, RecheckPart::Renditions];
        match state.ai_status() {
            AiStatus::Ready if state.ai().is_some() => {
                all.extend(RecheckPart::ALL.into_iter().filter(|p| p.needs_clip()));
            }
            // Ready soon: ask again rather than leave the CLIP parts out.
            AiStatus::Loading | AiStatus::Ready => return Err(ai_fill::not_ready(AiStatus::Loading)),
            AiStatus::Off | AiStatus::Failed => {}
        }
        if state.text_reader().is_some() {
            all.push(RecheckPart::Text);
        }
        all
    } else {
        if asked.iter().any(|p| p.needs_clip()) {
            ai_fill::require_ready(state)?;
        }
        if asked.contains(&RecheckPart::Text) && state.text_reader().is_none() {
            return Err(AppError::unavailable("this server does not read the words in pictures"));
        }
        asked.to_vec()
    };
    parts.sort();
    parts.dedup();
    Ok(parts)
}

fn mode(m: RecheckMode) -> AppResult<RecheckMode> {
    match m {
        RecheckMode::Other => Err(AppError::bad_request("unknown recheck mode")),
        m => Ok(m),
    }
}

/// `POST /api/v1/rechecks`: recheck the caller's own items (of one of
/// their sets, with `set_id`). Rate limited (`limits.recheck_per_hour`)
/// unless the same recheck runs: then that one is returned, with `false`.
pub async fn start_own(state: &AppState, viewer: &CurrentUser, req: StartRecheck) -> AppResult<(Recheck, bool)> {
    if let Some(set_id) = &req.set_id {
        authz::owned_set(state, viewer.id(), set_id).await?;
    }
    let spec = Spec {
        scope: RecheckScope::User,
        owner_id: Some(viewer.id().to_string()),
        requested_by: Some(viewer.id().to_string()),
        set_id: req.set_id,
        mode: mode(req.mode)?,
        parts: parts(state, &req.parts)?,
    };
    let pick = pick_for(spec.mode);
    match create(state, &spec, pick, || state.limiters.check_recheck(viewer.id())).await? {
        Created::New(row) => Ok((row.to_api(), true)),
        Created::Running(row) => Ok((row.to_api(), false)),
        Created::Nothing => Err(AppError::internal("no recheck was made")),
    }
}

/// `POST /api/v1/admin/rechecks`: recheck every item on the server (of one
/// set, with `set_id`), audited as `recheck.start`. The caller must be an
/// admin (the route's extractor checks).
pub async fn start_server(state: &AppState, admin: &CurrentUser, req: StartRecheck) -> AppResult<(Recheck, bool)> {
    if !admin.is_admin() {
        return Err(AppError::forbidden("admin role required"));
    }
    if let Some(set_id) = &req.set_id {
        if !crate::ids::is_id(set_id) || db::set_by_id(&state.db, set_id).await?.is_none() {
            return Err(AppError::not_found());
        }
    }
    let spec = Spec {
        scope: RecheckScope::Server,
        owner_id: None,
        requested_by: Some(admin.id().to_string()),
        set_id: req.set_id,
        mode: mode(req.mode)?,
        parts: parts(state, &req.parts)?,
    };
    let pick = pick_for(spec.mode);
    let row = match create(state, &spec, pick, || Ok(())).await? {
        Created::New(row) => row,
        Created::Running(row) => return Ok((row.to_api(), false)),
        Created::Nothing => return Err(AppError::internal("no recheck was made")),
    };
    db::audit(
        &state.db,
        Some(admin.id()),
        "recheck.start",
        Some(&row.id),
        admin.ip_string().as_deref(),
        Some(json!({
            "scope": "server",
            "parts": spec.parts,
            "mode": spec.mode,
            "set_id": spec.set_id,
            "total": row.total,
        })),
    )
    .await;
    Ok((row.to_api(), true))
}

fn pick_for(mode: RecheckMode) -> Pick {
    match mode {
        RecheckMode::Refresh => Pick::All,
        _ => Pick::Missing,
    }
}

/// The server's own recheck after the CLIP model is ready: every picture
/// whose embedding is missing or from another model gets its embedding,
/// keywords, emojis and vibe (mode `missing`; for such pictures the
/// computer's values are from another model, so they are replaced, as
/// re-enrichment always did; a person's never). Nothing to do: no batch.
pub async fn start_system_clip(state: &AppState) -> AppResult<Option<Recheck>> {
    if state.ai_model_id().is_none() {
        return Ok(None);
    }
    let spec = Spec {
        scope: RecheckScope::Server,
        owner_id: None,
        requested_by: None,
        set_id: None,
        parts: vec![
            RecheckPart::Keywords,
            RecheckPart::Emojis,
            RecheckPart::Vibe,
            RecheckPart::Embedding,
        ],
        mode: RecheckMode::Missing,
    };
    start_system(state, spec, "(m.embedding_model IS NULL OR m.embedding_model != ?3)").await
}

/// The server's own recheck of the item text with a text reader: every
/// picture the current reader never read gets `text` (mode `missing`).
/// Nothing to do: no batch.
pub async fn start_system_text(state: &AppState) -> AppResult<Option<Recheck>> {
    if state.text_reader().is_none() {
        return Ok(None);
    }
    let spec = Spec {
        scope: RecheckScope::Server,
        owner_id: None,
        requested_by: None,
        set_id: None,
        parts: vec![RecheckPart::Text],
        mode: RecheckMode::Missing,
    };
    start_system(state, spec, UNREAD_SQL).await
}

async fn start_system(state: &AppState, spec: Spec, condition: &str) -> AppResult<Option<Recheck>> {
    match create(state, &spec, Pick::System(condition.to_string()), || Ok(())).await? {
        Created::New(row) => Ok(Some(row.to_api())),
        Created::Running(_) | Created::Nothing => Ok(None),
    }
}

/// Media the current text reader (`?4`) has not read.
const UNREAD_SQL: &str = "(m.ocr_model IS NULL OR m.ocr_model != ?4 OR m.ocr_text IS NULL)";

/// Items of media `m` in the batch's scope (`?1` owner, `?2` set) that
/// also match `extra`.
fn scoped_item(extra: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM items i JOIN sets s ON s.id = i.set_id WHERE i.media_id = m.id \
         AND (?1 IS NULL OR s.owner_id = ?1) AND (?2 IS NULL OR i.set_id = ?2) AND {extra})"
    )
}

/// The SQL condition on `media m` for "something of `parts` is missing":
/// `?3` is the loaded image model, `?4` the text reader's model. Each
/// picture's job checks again; this only keeps pictures with nothing to do
/// out of the batch.
fn missing_sql(parts: &[RecheckPart]) -> String {
    let embedding = "(m.embedding_model IS NULL OR m.embedding_model != ?3)";
    let renditions = format!(
        "(m.variants_done = 0 OR m.variants_version < CASE m.kind WHEN 'still' THEN {} WHEN 'animation' THEN {} \
         ELSE {} END)",
        stickpile_media::variants_version(stickpile_api::MediaKind::Still),
        stickpile_media::variants_version(stickpile_api::MediaKind::Animation),
        stickpile_media::variants_version(stickpile_api::MediaKind::Video),
    );
    let conds: Vec<String> = parts
        .iter()
        .filter_map(|p| match p {
            RecheckPart::Embedding => Some(embedding.to_string()),
            // A field is missing when it is empty and the picture has a
            // suggestion for it, or when the computer's values are from
            // another model.
            RecheckPart::Keywords => Some(format!(
                "{embedding} OR ((m.ai_keywords IS NULL OR m.ai_keywords != '[]') AND {})",
                scoped_item("i.keywords = '[]'")
            )),
            RecheckPart::Emojis => Some(format!(
                "{embedding} OR ((m.ai_emojis IS NULL OR m.ai_emojis != '[]') AND {})",
                scoped_item("i.emojis = '[]'")
            )),
            RecheckPart::Vibe => Some(format!(
                "{embedding} OR ((m.ai_vibe IS NOT NULL OR m.ai_keywords IS NULL) AND {})",
                scoped_item("i.vibe IS NULL")
            )),
            RecheckPart::Text => Some(format!(
                "{UNREAD_SQL} OR (m.ocr_text != '' AND {})",
                scoped_item("i.text_source = 'ai' AND i.text = ''")
            )),
            RecheckPart::Quality => Some("m.quality_report IS NULL".to_string()),
            RecheckPart::Renditions => Some(renditions.clone()),
            RecheckPart::Other => None,
        })
        .collect();
    if conds.is_empty() {
        return "0".into();
    }
    conds.iter().map(|c| format!("({c})")).collect::<Vec<_>>().join(" OR ")
}

/// What [`create`] did.
enum Created {
    New(RecheckRow),
    /// The same batch runs; nothing was written.
    Running(RecheckRow),
    /// The server's own batch had nothing to do; nothing was written.
    Nothing,
}

/// Create the batch of `spec` and its jobs in one transaction, unless the
/// same one runs. `admit` runs before anything is written (the rate
/// limit). A person's batch with nothing to do is `done` at once; the
/// server's is not made.
async fn create(
    state: &AppState,
    spec: &Spec,
    pick: Pick,
    admit: impl FnOnce() -> AppResult<()>,
) -> AppResult<Created> {
    let parts_json = spec.parts_json();
    let mode = enum_str(&spec.mode);
    let scope = enum_str(&spec.scope);
    let mut tx = sets::begin(&state.db).await?;
    // The same scope running? (A server batch is the same whoever started it.)
    let running: Option<String> = sqlx::query_scalar(
        "SELECT id FROM rechecks WHERE state = 'running' AND scope = ? AND owner_id IS ? AND set_id IS ? \
         AND parts = ? AND mode = ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&scope)
    .bind(&spec.owner_id)
    .bind(&spec.set_id)
    .bind(&parts_json)
    .bind(&mode)
    .fetch_optional(&mut *tx)
    .await?;
    let running = match (&pick, running) {
        // The server's own batches add what a running one does not cover.
        (Pick::System(_), _) | (_, None) => None,
        (_, Some(id)) => Some(id),
    };
    if let Some(id) = running {
        drop(tx);
        let row = row(&state.db, &id).await?.ok_or_else(AppError::not_found)?;
        return Ok(Created::Running(row));
    }
    admit()?;

    let model = state.ai_model_id().unwrap_or("").to_string();
    let reader = state
        .text_reader()
        .map(|r| r.model_id().to_string())
        .unwrap_or_default();
    let in_scope = "EXISTS (SELECT 1 FROM items i JOIN sets s ON s.id = i.set_id WHERE i.media_id = m.id \
                    AND (?1 IS NULL OR s.owner_id = ?1) AND (?2 IS NULL OR i.set_id = ?2))";
    let condition = match &pick {
        Pick::Missing => missing_sql(&spec.parts),
        Pick::All => "1".to_string(),
        Pick::System(c) => c.clone(),
    };
    let mut media: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT m.id FROM media m WHERE {in_scope} AND ({condition}) ORDER BY m.created_at, m.id"
    ))
    .bind(&spec.owner_id)
    .bind(&spec.set_id)
    .bind(&model)
    .bind(&reader)
    .fetch_all(&mut *tx)
    .await?;
    if matches!(pick, Pick::System(_)) {
        let covered = covered_by_pending(&mut tx, &parts_json).await?;
        media.retain(|m| !covered.contains(m));
        if media.is_empty() {
            return Ok(Created::Nothing);
        }
    }

    let id = new_id();
    let t = now();
    let total = media.len() as i64;
    let (state_str, finished_at) = if total == 0 {
        ("done", Some(t))
    } else {
        ("running", None)
    };
    sqlx::query(
        "INSERT INTO rechecks (id, scope, requested_by, owner_id, set_id, parts, mode, state, total, created_at, \
         finished_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&scope)
    .bind(&spec.requested_by)
    .bind(&spec.owner_id)
    .bind(&spec.set_id)
    .bind(&parts_json)
    .bind(&mode)
    .bind(state_str)
    .bind(total)
    .bind(t)
    .bind(finished_at)
    .execute(&mut *tx)
    .await?;
    for media_id in &media {
        jobs::insert_with(
            &mut tx,
            JobKind::Recheck,
            &json!({ "recheck_id": id, "media_id": media_id }),
            Some(media_id),
            spec.owner_id.as_deref(),
            jobs::RECHECK_PRIORITY,
        )
        .await?;
    }
    tx.commit().await?;
    if total > 0 {
        state.wake_workers();
    }
    tracing::info!(
        recheck = %id,
        scope = %scope,
        parts = %parts_json,
        mode = %mode,
        by = spec.requested_by.as_deref().unwrap_or("the server"),
        "recheck of {total} pictures started"
    );
    let row = row(&state.db, &id).await?.ok_or_else(AppError::not_found)?;
    Ok(Created::New(row))
}

/// Media a queued or running `enrich` job, or a pending job of a running
/// batch of the server with the same parts, already covers.
async fn covered_by_pending(conn: &mut sqlx::SqliteConnection, parts_json: &str) -> AppResult<HashSet<String>> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT subject FROM jobs WHERE status IN ('queued', 'running') AND subject IS NOT NULL \
         AND (kind = 'enrich' OR (kind = 'recheck' AND json_extract(payload, '$.recheck_id') IN \
              (SELECT id FROM rechecks WHERE state = 'running' AND scope = 'server' AND requested_by IS NULL \
               AND parts = ?)))",
    )
    .bind(parts_json)
    .fetch_all(&mut *conn)
    .await?;
    Ok(ids.into_iter().collect())
}

// ---------------------------------------------------------------------------
// Reading and cancelling
// ---------------------------------------------------------------------------

/// A batch the viewer may see: their own, or any for an admin; 404 else.
async fn visible(state: &AppState, viewer: &CurrentUser, id: &str) -> AppResult<RecheckRow> {
    if !crate::ids::is_id(id) {
        return Err(AppError::not_found());
    }
    let r = row(&state.db, id).await?.ok_or_else(AppError::not_found)?;
    if viewer.is_admin() || r.owner_id.as_deref() == Some(viewer.id()) {
        Ok(r)
    } else {
        Err(AppError::not_found())
    }
}

/// `GET /api/v1/rechecks/{id}`.
pub async fn get(state: &AppState, viewer: &CurrentUser, id: &str) -> AppResult<Recheck> {
    Ok(visible(state, viewer, id).await?.to_api())
}

/// `GET /api/v1/rechecks`: the viewer's batches, and for an admin the
/// server-wide ones too, newest first.
pub async fn list(state: &AppState, viewer: &CurrentUser) -> AppResult<Vec<Recheck>> {
    let rows = sqlx::query_as::<_, RecheckRow>(&format!(
        "{SELECT} WHERE (r.scope = 'user' AND r.owner_id = ?) OR (? AND r.scope = 'server') \
         ORDER BY r.created_at DESC, r.rowid DESC LIMIT ?"
    ))
    .bind(viewer.id())
    .bind(viewer.is_admin())
    .bind(LIST_LIMIT)
    .fetch_all(&state.db)
    .await?;
    Ok(rows.iter().map(RecheckRow::to_api).collect())
}

/// `GET /api/v1/rechecks/{id}/failures`: the pictures that failed, oldest
/// first.
pub async fn failures(state: &AppState, viewer: &CurrentUser, id: &str) -> AppResult<Vec<RecheckFailure>> {
    let r = visible(state, viewer, id).await?;
    let rows: Vec<(String, Option<String>, String, i64)> = sqlx::query_as(
        "SELECT media_id, item_id, error, at FROM recheck_failures WHERE recheck_id = ? ORDER BY at, rowid LIMIT ?",
    )
    .bind(&r.id)
    .bind(FAILURES_LIMIT)
    .fetch_all(&state.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(media_id, item_id, error, at)| RecheckFailure {
            media_id,
            item_id,
            error,
            at,
        })
        .collect())
}

/// `POST /api/v1/rechecks/{id}/cancel`: its owner or an admin. A running
/// batch becomes `cancelled` and its queued jobs are deleted; a job that
/// runs now finishes and is counted. A batch that is over is returned as
/// it is. Cancelling a server-wide batch is audited (`recheck.cancel`).
pub async fn cancel(state: &AppState, viewer: &CurrentUser, id: &str) -> AppResult<Recheck> {
    let r = visible(state, viewer, id).await?;
    let mut tx = sets::begin(&state.db).await?;
    let stopped =
        sqlx::query("UPDATE rechecks SET state = 'cancelled', finished_at = ? WHERE id = ? AND state = 'running'")
            .bind(now())
            .bind(&r.id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    let dropped = if stopped > 0 {
        sqlx::query(
            "DELETE FROM jobs WHERE kind = 'recheck' AND status = 'queued' \
             AND json_extract(payload, '$.recheck_id') = ?",
        )
        .bind(&r.id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
    } else {
        0
    };
    tx.commit().await?;
    if stopped > 0 && r.scope() == RecheckScope::Server {
        db::audit(
            &state.db,
            Some(viewer.id()),
            "recheck.cancel",
            Some(&r.id),
            viewer.ip_string().as_deref(),
            Some(json!({ "dropped": dropped })),
        )
        .await;
    }
    Ok(row(&state.db, &r.id).await?.ok_or_else(AppError::not_found)?.to_api())
}

// ---------------------------------------------------------------------------
// Counting
// ---------------------------------------------------------------------------

/// Count a `recheck` job that ended in its batch: `done` (and `changed`
/// when its result says so) or `failed` with a `recheck_failures` row. The
/// batch is over when every picture is counted: `failed` if none was
/// rechecked, else `done`. A cancelled batch keeps counting what still ran
/// and stays `cancelled`. Called by the queue for every finished `recheck`
/// job, never for one that is retried.
pub async fn record_job(db: &SqlitePool, job: &JobRow, outcome: Result<Option<&Value>, &str>) -> AppResult<()> {
    let payload = job.payload();
    let Some(recheck_id) = payload.get("recheck_id").and_then(Value::as_str) else {
        return Ok(());
    };
    let media_id = payload.get("media_id").and_then(Value::as_str).unwrap_or_default();
    let t = now();
    let mut tx = sets::begin(db).await?;
    match outcome {
        Ok(result) => {
            let changed = result
                .and_then(|r| r.get("changed"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            sqlx::query(
                "UPDATE rechecks SET done = done + 1, changed = changed + ?2, \
                 finished_at = CASE WHEN state = 'running' AND done + 1 + failed >= total THEN ?3 ELSE finished_at END, \
                 state = CASE WHEN state = 'running' AND done + 1 + failed >= total THEN 'done' ELSE state END \
                 WHERE id = ?1",
            )
            .bind(recheck_id)
            .bind(i64::from(changed))
            .bind(t)
            .execute(&mut *tx)
            .await?;
        }
        Err(error) => {
            sqlx::query(
                "INSERT INTO recheck_failures (recheck_id, media_id, item_id, error, at) \
                 SELECT r.id, ?2, (SELECT i.id FROM items i JOIN sets s ON s.id = i.set_id WHERE i.media_id = ?2 \
                                   AND s.owner_id = r.owner_id AND (r.set_id IS NULL OR i.set_id = r.set_id) \
                                   ORDER BY i.created_at, i.id LIMIT 1), ?3, ?4 \
                 FROM rechecks r WHERE r.id = ?1",
            )
            .bind(recheck_id)
            .bind(media_id)
            .bind(error)
            .bind(t)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE rechecks SET failed = failed + 1, \
                 finished_at = CASE WHEN state = 'running' AND done + failed + 1 >= total THEN ?2 ELSE finished_at END, \
                 state = CASE WHEN state = 'running' AND done + failed + 1 >= total \
                              THEN (CASE WHEN done = 0 THEN 'failed' ELSE 'done' END) ELSE state END \
                 WHERE id = ?1",
            )
            .bind(recheck_id)
            .bind(t)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Delete batches that ended more than `max_age_secs` ago (with their
/// failures), with the finished jobs ([`crate::jobs::purge_finished`]).
pub async fn purge_finished(db: &SqlitePool, max_age_secs: i64) -> AppResult<u64> {
    Ok(
        sqlx::query("DELETE FROM rechecks WHERE state != 'running' AND finished_at < ?")
            .bind(now() - max_age_secs)
            .execute(db)
            .await?
            .rows_affected(),
    )
}
