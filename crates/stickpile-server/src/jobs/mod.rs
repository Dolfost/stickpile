//! DB-backed job queue (`jobs` table). `media.workers` tokio workers claim
//! queued jobs atomically, woken by a [`tokio::sync::Notify`] on enqueue and
//! by polling. Failures are retried with exponential backoff up to
//! [`MAX_ATTEMPTS`]; [`JobError::Fatal`] fails immediately.
//!
//! Adding a job kind: add a handler module and a match arm in [`dispatch`].
//! Git and Telegram kinds are registered but fail with "not implemented yet"
//! until the integrations land.

pub mod media;

use std::time::Duration;

use serde_json::Value;
use sqlx::SqlitePool;
use stickpile_api::{Job, JobKind, JobStatus};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::{enum_str, parse_enum};
use crate::error::AppResult;
use crate::ids::{new_id, now};
use crate::state::AppState;

pub const MAX_ATTEMPTS: i64 = 3;
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const BASE_BACKOFF_SECS: i64 = 10;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct JobRow {
    pub id: String,
    pub kind: String,
    pub payload: String,
    pub subject: Option<String>,
    pub status: String,
    pub attempts: i64,
    pub error: Option<String>,
    pub result: Option<String>,
    pub owner_id: Option<String>,
    pub run_after: i64,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

pub const JOB_COLUMNS: &str =
    "id, kind, payload, subject, status, attempts, error, result, owner_id, run_after, created_at, started_at, finished_at";

impl JobRow {
    pub fn kind(&self) -> Option<JobKind> {
        parse_enum(&self.kind)
    }
    pub fn status(&self) -> JobStatus {
        parse_enum(&self.status).unwrap_or(JobStatus::Queued)
    }
    pub fn payload(&self) -> Value {
        serde_json::from_str(&self.payload).unwrap_or(Value::Null)
    }
    pub fn to_api(&self) -> Job {
        Job {
            id: self.id.clone(),
            kind: self.kind().unwrap_or(JobKind::Variants),
            status: self.status(),
            attempts: self.attempts.max(0) as u32,
            error: self.error.clone(),
            subject: self.subject.clone(),
            result: self.result.as_deref().and_then(|r| serde_json::from_str(r).ok()),
            created_at: self.created_at,
            started_at: self.started_at,
            finished_at: self.finished_at,
        }
    }
}

#[derive(Debug)]
pub enum JobError {
    /// Transient: retried with backoff.
    Retry(String),
    /// Permanent: fails the job now.
    Fatal(String),
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobError::Retry(m) | JobError::Fatal(m) => f.write_str(m),
        }
    }
}

impl From<crate::error::AppError> for JobError {
    fn from(e: crate::error::AppError) -> JobError {
        JobError::Retry(e.to_string())
    }
}

pub type JobResult = Result<Option<Value>, JobError>;

/// Enqueue a job and wake a worker.
pub async fn enqueue(
    state: &AppState,
    kind: JobKind,
    payload: Value,
    subject: Option<String>,
    owner_id: Option<&str>,
) -> AppResult<Job> {
    let row = insert(&state.db, kind, payload, subject, owner_id).await?;
    state.wake_workers();
    Ok(row.to_api())
}

pub async fn insert(
    db: &SqlitePool,
    kind: JobKind,
    payload: Value,
    subject: Option<String>,
    owner_id: Option<&str>,
) -> AppResult<JobRow> {
    let id = new_id();
    let t = now();
    sqlx::query(
        "INSERT INTO jobs (id, kind, payload, subject, status, owner_id, run_after, created_at) \
         VALUES (?, ?, ?, ?, 'queued', ?, ?, ?)",
    )
    .bind(&id)
    .bind(enum_str(&kind))
    .bind(payload.to_string())
    .bind(&subject)
    .bind(owner_id)
    .bind(t)
    .bind(t)
    .execute(db)
    .await?;
    Ok(get(db, &id).await?.expect("just inserted"))
}

pub async fn get(db: &SqlitePool, id: &str) -> AppResult<Option<JobRow>> {
    Ok(
        sqlx::query_as::<_, JobRow>(&format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?"))
            .bind(id)
            .fetch_optional(db)
            .await?,
    )
}

/// Is a job of `kind` for `subject` already queued or running?
pub async fn pending_exists(db: &SqlitePool, kind: JobKind, subject: &str) -> AppResult<bool> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE kind = ? AND subject = ? AND status IN ('queued', 'running')",
    )
    .bind(enum_str(&kind))
    .bind(subject)
    .fetch_one(db)
    .await?;
    Ok(n > 0)
}

pub async fn list(db: &SqlitePool, status: Option<JobStatus>, limit: i64) -> AppResult<Vec<Job>> {
    let rows = match status {
        Some(s) => {
            sqlx::query_as::<_, JobRow>(&format!(
                "SELECT {JOB_COLUMNS} FROM jobs WHERE status = ? ORDER BY created_at DESC, id LIMIT ?"
            ))
            .bind(enum_str(&s))
            .bind(limit)
            .fetch_all(db)
            .await?
        }
        None => {
            sqlx::query_as::<_, JobRow>(&format!(
                "SELECT {JOB_COLUMNS} FROM jobs ORDER BY created_at DESC, id LIMIT ?"
            ))
            .bind(limit)
            .fetch_all(db)
            .await?
        }
    };
    Ok(rows.iter().map(JobRow::to_api).collect())
}

/// Jobs left `running` by a crash go back to the queue.
pub async fn requeue_interrupted(db: &SqlitePool) -> AppResult<u64> {
    Ok(
        sqlx::query("UPDATE jobs SET status = 'queued', run_after = ? WHERE status = 'running'")
            .bind(now())
            .execute(db)
            .await?
            .rows_affected(),
    )
}

/// Delete finished jobs older than `max_age_secs`.
pub async fn purge_finished(db: &SqlitePool, max_age_secs: i64) -> AppResult<u64> {
    Ok(
        sqlx::query("DELETE FROM jobs WHERE status IN ('done', 'failed') AND finished_at < ?")
            .bind(now() - max_age_secs)
            .execute(db)
            .await?
            .rows_affected(),
    )
}

async fn claim(db: &SqlitePool) -> AppResult<Option<JobRow>> {
    let t = now();
    Ok(sqlx::query_as::<_, JobRow>(&format!(
        "UPDATE jobs SET status = 'running', started_at = ?, attempts = attempts + 1, error = NULL \
         WHERE id = (SELECT id FROM jobs WHERE status = 'queued' AND run_after <= ? ORDER BY run_after, created_at, id LIMIT 1) \
         RETURNING {JOB_COLUMNS}"
    ))
    .bind(t)
    .bind(t)
    .fetch_optional(db)
    .await?)
}

/// Run one job by kind. The integrations agent replaces the `not
/// implemented` arms with real handlers.
pub async fn dispatch(state: &AppState, job: &JobRow) -> JobResult {
    match job.kind() {
        Some(JobKind::Variants) => media::variants(state, job).await,
        Some(JobKind::Enrich) => media::enrich(state, job).await,
        Some(kind @ (JobKind::GitPull | JobKind::GitPush | JobKind::TelegramImport | JobKind::TelegramExport)) => Err(
            JobError::Fatal(format!("{} jobs are not implemented yet", enum_str(&kind))),
        ),
        None => Err(JobError::Fatal(format!("unknown job kind {:?}", job.kind))),
    }
}

async fn finish(db: &SqlitePool, job: &JobRow, outcome: JobResult) -> AppResult<()> {
    let t = now();
    match outcome {
        Ok(result) => {
            sqlx::query("UPDATE jobs SET status = 'done', finished_at = ?, result = ?, error = NULL WHERE id = ?")
                .bind(t)
                .bind(result.map(|r| r.to_string()))
                .bind(&job.id)
                .execute(db)
                .await?;
        }
        Err(JobError::Retry(msg)) if job.attempts < MAX_ATTEMPTS => {
            let backoff = BASE_BACKOFF_SECS << (job.attempts - 1).clamp(0, 10);
            tracing::warn!(job = %job.id, kind = %job.kind, attempt = job.attempts, "job failed, retrying in {backoff}s: {msg}");
            sqlx::query("UPDATE jobs SET status = 'queued', run_after = ?, error = ? WHERE id = ?")
                .bind(t + backoff)
                .bind(&msg)
                .bind(&job.id)
                .execute(db)
                .await?;
        }
        Err(e) => {
            tracing::warn!(job = %job.id, kind = %job.kind, "job failed: {e}");
            sqlx::query("UPDATE jobs SET status = 'failed', finished_at = ?, error = ? WHERE id = ?")
                .bind(t)
                .bind(e.to_string())
                .bind(&job.id)
                .execute(db)
                .await?;
        }
    }
    Ok(())
}

/// Claim and run every runnable job now (tests and the CLI use this to drain
/// the queue deterministically). Returns how many jobs ran.
pub async fn run_pending(state: &AppState) -> AppResult<usize> {
    let mut n = 0;
    while let Some(job) = claim(&state.db).await? {
        let outcome = dispatch(state, &job).await;
        finish(&state.db, &job, outcome).await?;
        n += 1;
    }
    Ok(n)
}

async fn worker(state: AppState, cancel: CancellationToken, n: u32) {
    tracing::debug!("job worker {n} started");
    loop {
        let notified = state.jobs_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        match claim(&state.db).await {
            Ok(Some(job)) => {
                let outcome = tokio::select! {
                    o = dispatch(&state, &job) => o,
                    _ = cancel.cancelled() => {
                        // Leave it `running`; requeued on next start.
                        break;
                    }
                };
                if let Err(e) = finish(&state.db, &job, outcome).await {
                    tracing::error!("recording job result: {e}");
                }
                continue;
            }
            Ok(None) => {}
            Err(e) => tracing::error!("claiming job: {e}"),
        }
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep(POLL_INTERVAL) => {}
            _ = cancel.cancelled() => break,
        }
    }
    tracing::debug!("job worker {n} stopped");
}

pub fn spawn_workers(state: &AppState, cancel: &CancellationToken) -> Vec<JoinHandle<()>> {
    (0..state.config.media.workers)
        .map(|n| tokio::spawn(worker(state.clone(), cancel.clone(), n)))
        .collect()
}
