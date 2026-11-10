//! Git packs (docs/git-packs.md): sets bound to a remote repository.
//!
//! * [`create`] validates the URL against `git.allowed_schemes`, creates the
//!   set (`source = git`, a random webhook secret) and enqueues the first
//!   `GitPull`.
//! * `GitPull` ([`run_pull`]): `stickpile_gitpack::sync` into
//!   `<data_dir>/gitpacks/<set id>`, then [`transfer::import_bundle_into`]
//!   makes the set equal to the set directory (skipped when the commit did not
//!   move). Read-only packs are imported even though users cannot edit them.
//! * `GitPush` ([`run_push`], push mode): export the set into the checkout,
//!   commit and push. Every version bump of a push-mode set (re)schedules a
//!   debounced push ([`schedule_push`], called from `sets::bump_version`).
//!   A rejected push re-syncs, re-exports and retries once.
//! * Conflicts: `sets.git_synced_version` is the version that matches
//!   `git_last_commit`. If the set has local edits that are not upstream yet
//!   (version != synced version), a pull pushes them instead of importing:
//!   stickpile's database is the source of truth, and upstream changes made
//!   in the meantime stay in the repository history.
//! * [`schedule_due`] (every minute) enqueues pulls per
//!   `git.pull_interval_minutes`; [`webhook`] enqueues one on a verified
//!   forge webhook. A queued pull is never duplicated.
//!
//! All git work is blocking libgit2 and runs on the blocking pool. Jobs of
//! one set are serialised by a per-set lock (they share the checkout).

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{HeaderMap, StatusCode};
use serde_json::json;
use stickpile_api::{routes, CreateGitPack, GitPackCreated, Job, JobKind, Visibility};
use stickpile_gitpack::{GitAuth, GitPackError};

use crate::auth::CurrentUser;
use crate::authz;
use crate::config::read_secret;
use crate::db::{self, enum_str, SetRow};
use crate::error::{AppError, AppResult};
use crate::ids::{is_id, new_id, now, random_base32};
use crate::jobs::{self, JobError, JobResult, JobRow, JOB_COLUMNS};
use crate::service::sets::{self, NewSet};
use crate::service::transfer;
use crate::state::AppState;

/// Edits within this many seconds are pushed together.
pub const PUSH_DEBOUNCE_SECS: i64 = 5;
/// How often the scheduler looks for packs that are due for a pull.
pub const SCHEDULER_TICK_SECS: u64 = 60;
const DISABLED: &str = "git packs are disabled on this server (git.enabled = false)";

/// Git packs are on (`git.enabled`).
pub fn enabled(state: &AppState) -> bool {
    state.config.git.enabled
}

fn disabled() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "not_found", DISABLED)
}

/// `<data_dir>/gitpacks/<set id>`.
pub fn checkout_dir(state: &AppState, set_id: &str) -> Option<PathBuf> {
    is_id(set_id).then(|| state.config.gitpacks_dir().join(set_id))
}

/// Public webhook URL of a set.
pub fn webhook_url(state: &AppState, set_id: &str) -> String {
    format!(
        "{}{}",
        state.config.server.public_url.trim_end_matches('/'),
        routes::path(routes::GIT_WEBHOOK, &[set_id])
    )
}

// ---------------------------------------------------------------------------
// Input validation
// ---------------------------------------------------------------------------

/// A branch name we hand to libgit2 (which validates again): no options,
/// no revision syntax, no whitespace.
fn clean_branch(b: Option<String>) -> AppResult<Option<String>> {
    let Some(b) = b.map(|b| b.trim().to_string()).filter(|b| !b.is_empty()) else {
        return Ok(None);
    };
    let ok = b.len() <= 255
        && !b.starts_with(['-', '/', '.'])
        && !b.ends_with(['/', '.'])
        && !b.ends_with(".lock")
        && !b.contains("..")
        && !b.contains("//")
        && !b.contains("@{")
        && b.chars()
            .all(|c| !c.is_control() && !c.is_whitespace() && !"~^:?*[\\".contains(c));
    if ok {
        Ok(Some(b))
    } else {
        Err(AppError::bad_request(format!("invalid branch name {b:?}")))
    }
}

/// A relative directory inside the repository: no `..`, no hidden
/// components (so never `.git`), no backslashes or control characters.
fn clean_subdir(s: Option<String>) -> AppResult<Option<String>> {
    let Some(s) = s else { return Ok(None) };
    let t = s.trim().trim_matches('/');
    if t.is_empty() || t == "." {
        return Ok(None);
    }
    let bad = || {
        AppError::bad_request(
            "subdir must be a relative path inside the repository (no `..`, hidden components or backslashes)",
        )
    };
    if t.len() > 1024 {
        return Err(bad());
    }
    let mut parts = Vec::new();
    for c in t.split('/') {
        if c.is_empty() || c == "." {
            continue;
        }
        if c.starts_with('.') || c.contains('\\') || c.chars().any(char::is_control) || c.len() > 255 {
            return Err(bad());
        }
        parts.push(c);
    }
    Ok((!parts.is_empty()).then(|| parts.join("/")))
}

/// Last path segment of a remote URL without `.git` (`…/cat-stickers.git`
/// -> `cat-stickers`; works for scp-like `git@host:owner/repo.git` too).
fn repo_name(url: &str) -> String {
    let path = url.split("://").nth(1).unwrap_or(url);
    path.trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("")
        .trim_end_matches(".git")
        .to_string()
}

// ---------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------

/// Bind a new set to a repository and enqueue its first pull. The webhook
/// secret is only returned here.
pub async fn create(state: &AppState, user: &CurrentUser, req: CreateGitPack) -> AppResult<GitPackCreated> {
    if !enabled(state) {
        return Err(disabled());
    }
    let url = req.url.trim().to_string();
    stickpile_gitpack::validate_url(&url, &state.config.git.allowed_schemes)
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    let branch = clean_branch(req.branch)?;
    let subdir = clean_subdir(req.subdir)?;
    let repo = repo_name(&url);
    let slug = match req.slug.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(s) => s,
        None => stickpile_setfmt::slugify(subdir.as_deref().and_then(|s| s.rsplit('/').next()).unwrap_or(&repo)),
    };
    sets::validate_slug(&slug)?;
    let name = if repo.trim().is_empty() || sets::validate_name(&repo).is_err() {
        slug.clone()
    } else {
        repo
    };
    let row = sets::insert(
        &state.db,
        user.id(),
        NewSet {
            slug,
            name,
            description: String::new(),
            visibility: Visibility::Private,
            aspect_ratio: None,
            recommended_px: None,
            version: 1,
            extra_toml: None,
        },
    )
    .await?;
    let secret = random_base32(20);
    sqlx::query(
        "UPDATE sets SET source = 'git', git_url = ?, git_branch = ?, git_subdir = ?, git_mode = ?, \
         git_webhook_secret = ? WHERE id = ?",
    )
    .bind(&url)
    .bind(&branch)
    .bind(&subdir)
    .bind(enum_str(&req.mode))
    .bind(&secret)
    .bind(&row.id)
    .execute(&state.db)
    .await?;
    db::audit(
        &state.db,
        Some(user.id()),
        "gitpack.create",
        Some(&row.id),
        user.ip_string().as_deref(),
        Some(
            json!({"url": stickpile_gitpack::redact_url(&url), "branch": branch, "subdir": subdir,
                    "mode": enum_str(&req.mode)}),
        ),
    )
    .await;
    let job = enqueue_pull(state, &row.id, &row.owner_id).await?;
    let row = db::set_by_id(&state.db, &row.id)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(GitPackCreated {
        set: sets::load_api(&state.db, &row, user.id()).await?,
        webhook_url: webhook_url(state, &row.id),
        webhook_secret: secret,
        job,
    })
}

/// Re-pull a git pack now (the owner only, read-only packs included).
pub async fn pull_now(state: &AppState, user: &CurrentUser, set_id: &str) -> AppResult<Job> {
    if !enabled(state) {
        return Err(disabled());
    }
    let set = authz::owned_set(state, user.id(), set_id).await?;
    if set.source != "git" {
        return Err(AppError::bad_request("this set is not a git pack"));
    }
    enqueue_pull(state, &set.id, &set.owner_id).await
}

/// Enqueue a pull unless one is already queued (a running pull does not
/// count: upstream may have moved after it fetched).
pub async fn enqueue_pull(state: &AppState, set_id: &str, owner_id: &str) -> AppResult<Job> {
    let queued = sqlx::query_as::<_, JobRow>(&format!(
        "SELECT {JOB_COLUMNS} FROM jobs WHERE kind = 'git_pull' AND subject = ? AND status = 'queued' LIMIT 1"
    ))
    .bind(set_id)
    .fetch_optional(&state.db)
    .await?;
    if let Some(job) = queued {
        return Ok(job.to_api());
    }
    jobs::enqueue(
        state,
        JobKind::GitPull,
        json!({ "set_id": set_id }),
        Some(set_id.to_string()),
        Some(owner_id),
    )
    .await
}

/// Check the webhook credentials: `X-Stickpile-Webhook-Secret` or GitLab's
/// `X-Gitlab-Token` (the secret itself), or `X-Hub-Signature-256` /
/// `X-Gitea-Signature` / `X-Forgejo-Signature` (HMAC-SHA256 of the raw
/// body). All comparisons are constant time. No header -> 401, a header
/// that does not verify -> 403.
pub fn verify_webhook(secret: &str, headers: &HeaderMap, body: &[u8]) -> AppResult<()> {
    let secret = secret.as_bytes();
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let mut presented = false;
    for name in ["x-stickpile-webhook-secret", "x-gitlab-token"] {
        if let Some(v) = header(name) {
            presented = true;
            if stickpile_gitpack::verify_token(secret, v) {
                return Ok(());
            }
        }
    }
    for name in ["x-hub-signature-256", "x-gitea-signature", "x-forgejo-signature"] {
        if let Some(v) = header(name) {
            presented = true;
            if stickpile_gitpack::verify_hub_signature(secret, body, v) {
                return Ok(());
            }
        }
    }
    if presented {
        Err(AppError::forbidden("webhook secret or signature does not match"))
    } else {
        Err(AppError::unauthorized(
            "webhook secret or signature required (X-Stickpile-Webhook-Secret, X-Gitlab-Token or X-Hub-Signature-256)",
        ))
    }
}

/// The unauthenticated forge webhook: rate-limited per client IP, then the
/// secret or signature is verified and a pull enqueued. Unknown sets (and
/// sets that are not git packs) are 404.
pub async fn webhook(
    state: &AppState,
    set_id: &str,
    ip: Option<IpAddr>,
    headers: &HeaderMap,
    body: &[u8],
) -> AppResult<Job> {
    if !enabled(state) {
        return Err(disabled());
    }
    state.limiters.check_webhook(ip)?;
    if !is_id(set_id) {
        return Err(AppError::not_found());
    }
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT owner_id, git_webhook_secret FROM sets WHERE id = ? AND source = 'git'")
            .bind(set_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((owner_id, Some(secret))) = row else {
        return Err(AppError::not_found());
    };
    verify_webhook(&secret, headers, body)?;
    enqueue_pull(state, set_id, &owner_id).await
}

// ---------------------------------------------------------------------------
// Scheduling
// ---------------------------------------------------------------------------

/// Called in the transaction of every version bump: for a push-mode git
/// pack, (re)schedule a `GitPush` [`PUSH_DEBOUNCE_SECS`] from now. Workers
/// poll, so no wake-up is needed.
pub async fn schedule_push(conn: &mut sqlx::SqliteConnection, set_id: &str) -> AppResult<()> {
    let owner: Option<String> =
        sqlx::query_scalar("SELECT owner_id FROM sets WHERE id = ? AND source = 'git' AND git_mode = 'push'")
            .bind(set_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(owner) = owner else { return Ok(()) };
    let t = now();
    let moved =
        sqlx::query("UPDATE jobs SET run_after = ? WHERE kind = 'git_push' AND subject = ? AND status = 'queued'")
            .bind(t + PUSH_DEBOUNCE_SECS)
            .bind(set_id)
            .execute(&mut *conn)
            .await?
            .rows_affected();
    if moved == 0 {
        sqlx::query(
            "INSERT INTO jobs (id, kind, payload, subject, status, owner_id, run_after, created_at) \
             VALUES (?, 'git_push', ?, ?, 'queued', ?, ?, ?)",
        )
        .bind(new_id())
        .bind(json!({ "set_id": set_id }).to_string())
        .bind(set_id)
        .bind(&owner)
        .bind(t + PUSH_DEBOUNCE_SECS)
        .bind(t)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Enqueue a pull for every git pack whose last pull attempt is older than
/// `git.pull_interval_minutes` (0 = never) and that has none queued or
/// running. Returns how many were queued.
pub async fn schedule_due(state: &AppState) -> AppResult<usize> {
    let interval = i64::from(state.config.git.pull_interval_minutes) * 60;
    if !enabled(state) || interval == 0 {
        return Ok(0);
    }
    let due: Vec<(String, String)> = sqlx::query_as(
        "SELECT s.id, s.owner_id FROM sets s WHERE s.source = 'git' \
         AND (s.git_last_pull_at IS NULL OR s.git_last_pull_at <= ?) \
         AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.kind = 'git_pull' AND j.subject = s.id \
                         AND j.status IN ('queued', 'running'))",
    )
    .bind(now() - interval)
    .fetch_all(&state.db)
    .await?;
    for (set_id, owner) in &due {
        jobs::insert(
            &state.db,
            JobKind::GitPull,
            json!({ "set_id": set_id }),
            Some(set_id.clone()),
            Some(owner),
        )
        .await?;
    }
    if !due.is_empty() {
        state.wake_workers();
    }
    Ok(due.len())
}

/// Remove checkouts whose set no longer exists (or is no longer a git pack).
pub async fn gc_checkouts(state: &AppState) -> AppResult<usize> {
    let root = state.config.gitpacks_dir();
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let mut removed = 0;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_id(&name) {
            continue;
        }
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM sets WHERE id = ? AND source = 'git'")
            .bind(&name)
            .fetch_optional(&state.db)
            .await?;
        if exists.is_some() {
            continue;
        }
        let lock = set_lock(&name);
        let Ok(_guard) = lock.try_lock() else { continue };
        match tokio::fs::remove_dir_all(entry.path()).await {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!("removing stale checkout {}: {e}", entry.path().display()),
        }
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

/// Per-set lock shared by pull and push jobs (and the checkout GC).
fn set_lock(set_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    jobs::subject_lock(&format!("git:{set_id}"))
}

fn retry(e: impl std::fmt::Display) -> JobError {
    JobError::Retry(e.to_string())
}

/// Errors that will not go away by retrying fail the job at once.
fn classify(e: GitPackError) -> JobError {
    match e {
        GitPackError::Url(_) | GitPackError::TooLarge(_) => JobError::Fatal(e.to_string()),
        GitPackError::Git(_) | GitPackError::Io(_) => JobError::Retry(e.to_string()),
    }
}

/// App errors from importing/exporting: client errors are permanent.
fn app_error(e: AppError) -> JobError {
    if e.status.is_client_error() {
        JobError::Fatal(e.message)
    } else {
        JobError::Retry(e.to_string())
    }
}

/// Credentials from `[git]` (secret files are read on every job, so a
/// rotated token is picked up without a restart).
fn git_auth(state: &AppState) -> Result<GitAuth, JobError> {
    let cfg = &state.config.git;
    let https_token = match &cfg.https_token_file {
        Some(p) => Some(
            read_secret(p)
                .map_err(|e| JobError::Fatal(format!("reading git.https_token_file {}: {e}", p.display())))?,
        )
        .filter(|t| !t.trim().is_empty()),
        None => None,
    };
    Ok(GitAuth {
        ssh_key: cfg.ssh_key_file.clone(),
        https_username: cfg.https_username.clone(),
        https_token,
    })
}

fn payload_set_id(job: &JobRow) -> Result<String, JobError> {
    job.payload()
        .get("set_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| JobError::Fatal("payload without set_id".into()))
}

async fn sync(
    state: &AppState,
    set: &SetRow,
    dest: &Path,
    auth: &GitAuth,
) -> Result<stickpile_gitpack::Checkout, JobError> {
    let url = set
        .git_url
        .clone()
        .ok_or_else(|| JobError::Fatal("git pack without URL".into()))?;
    stickpile_gitpack::validate_url(&url, &state.config.git.allowed_schemes).map_err(classify)?;
    let branch = set.git_branch.clone();
    let max = state.config.git.max_repo_mb.saturating_mul(1024 * 1024);
    let (dest, auth) = (dest.to_path_buf(), auth.clone());
    tokio::task::spawn_blocking(move || stickpile_gitpack::sync(&url, branch.as_deref(), &dest, &auth, max))
        .await
        .map_err(retry)?
        .map_err(classify)
}

/// Record a failed attempt (time + error) so the UI can show it and the
/// scheduler does not retry before the next interval.
async fn record_failure(state: &AppState, set_id: &str, pulled: bool, e: &JobError) {
    let res = if pulled {
        sqlx::query("UPDATE sets SET git_last_pull_at = ?, git_last_error = ? WHERE id = ?")
            .bind(now())
            .bind(e.to_string())
            .bind(set_id)
            .execute(&state.db)
            .await
    } else {
        sqlx::query("UPDATE sets SET git_last_error = ? WHERE id = ?")
            .bind(e.to_string())
            .bind(set_id)
            .execute(&state.db)
            .await
    };
    if let Err(err) = res {
        tracing::error!("recording git error of set {set_id}: {err}");
    }
}

struct SyncState {
    synced_version: Option<i64>,
}

async fn sync_state(state: &AppState, set_id: &str) -> Result<SyncState, JobError> {
    let synced_version: Option<i64> = sqlx::query_scalar("SELECT git_synced_version FROM sets WHERE id = ?")
        .bind(set_id)
        .fetch_one(&state.db)
        .await
        .map_err(retry)?;
    Ok(SyncState { synced_version })
}

async fn owner_actor(state: &AppState, set: &SetRow) -> Result<Option<CurrentUser>, JobError> {
    Ok(db::user_by_id(&state.db, &set.owner_id)
        .await
        .map_err(retry)?
        .map(CurrentUser::background))
}

fn set_dir(dest: &Path, set: &SetRow) -> PathBuf {
    match &set.git_subdir {
        Some(s) => dest.join(s),
        None => dest.to_path_buf(),
    }
}

/// `GitPull` job.
pub async fn pull_job(state: &AppState, job: &JobRow) -> JobResult {
    let set_id = payload_set_id(job)?;
    run_pull(state, &set_id).await
}

/// `GitPush` job.
pub async fn push_job(state: &AppState, job: &JobRow) -> JobResult {
    let set_id = payload_set_id(job)?;
    run_push(state, &set_id).await
}

/// Fetch upstream and import it (or, with unpushed local edits in push
/// mode, push them).
pub async fn run_pull(state: &AppState, set_id: &str) -> JobResult {
    if !enabled(state) {
        return Err(JobError::Fatal(DISABLED.into()));
    }
    let lock = set_lock(set_id);
    let _guard = lock.lock().await;
    let Some(set) = db::set_by_id(&state.db, set_id).await.map_err(retry)? else {
        return Ok(Some(json!({"skipped": "the set no longer exists"})));
    };
    if set.source != "git" {
        return Err(JobError::Fatal("the set is not a git pack".into()));
    }
    let Some(actor) = owner_actor(state, &set).await? else {
        return Ok(Some(json!({"skipped": "the owner no longer exists"})));
    };
    let dest = checkout_dir(state, &set.id).ok_or_else(|| JobError::Fatal("bad set id".into()))?;
    let auth = git_auth(state)?;
    let co = match sync(state, &set, &dest, &auth).await {
        Ok(co) => co,
        Err(e) => {
            record_failure(state, &set.id, true, &e).await;
            return Err(e);
        }
    };
    let push_mode = set.git_mode.as_deref() == Some("push");
    let synced = sync_state(state, &set.id).await?.synced_version;
    let dirty = synced.is_some_and(|v| v != set.version);

    if co.commit.is_empty() {
        // Empty remote (push mode into a fresh repository).
        if push_mode && dirty {
            return push_inner(state, &set, &actor, &dest, &auth).await;
        }
        sqlx::query(
            "UPDATE sets SET git_last_pull_at = ?, git_last_error = NULL, git_last_commit = NULL, \
             git_synced_version = COALESCE(git_synced_version, version) WHERE id = ?",
        )
        .bind(now())
        .bind(&set.id)
        .execute(&state.db)
        .await
        .map_err(retry)?;
        return Ok(Some(json!({"commit": null, "empty": true})));
    }
    let first = synced.is_none() || set.git_last_commit.is_none();
    if !first && push_mode && dirty {
        // Local edits win; whatever upstream did meanwhile stays in its history.
        return push_inner(state, &set, &actor, &dest, &auth).await;
    }
    if !first && set.git_last_commit.as_deref() == Some(co.commit.as_str()) {
        sqlx::query("UPDATE sets SET git_last_pull_at = ?, git_last_error = NULL WHERE id = ?")
            .bind(now())
            .bind(&set.id)
            .execute(&state.db)
            .await
            .map_err(retry)?;
        return Ok(Some(json!({"commit": co.commit, "changed": false})));
    }

    let dir = set_dir(&dest, &set);
    let read = {
        let dir = dir.clone();
        tokio::task::spawn_blocking(move || stickpile_setfmt::read_dir(&dir))
            .await
            .map_err(retry)?
    };
    let mut bundle = match read {
        Ok(b) => b,
        Err(e) => {
            let e = match &e {
                stickpile_setfmt::SetFmtError::Io { .. } if !dir.is_dir() => JobError::Fatal(format!(
                    "{} not found in the repository",
                    set.git_subdir.as_deref().unwrap_or("the set directory")
                )),
                _ => JobError::Fatal(format!("reading the set directory: {e}")),
            };
            record_failure(state, &set.id, true, &e).await;
            return Err(e);
        }
    };
    // Without a `name` in set.toml the directory name is used, which for the
    // repository root is the checkout's set id: keep the current name then.
    if dir
        .file_name()
        .is_some_and(|n| n.to_string_lossy() == bundle.manifest.name)
    {
        bundle.manifest.name = set.name.clone();
    }
    let upstream_version = bundle.manifest.version;
    if (upstream_version as i64) < set.version {
        tracing::info!(
            set = %set.id,
            "upstream version {upstream_version} is lower than the stored version {}; importing anyway",
            set.version
        );
    }
    let report = match transfer::import_bundle_into(state, &actor, bundle, Some(set.clone())).await {
        Ok(r) => r,
        Err(e) => {
            let e = app_error(e);
            record_failure(state, &set.id, true, &e).await;
            return Err(e);
        }
    };
    sqlx::query(
        "UPDATE sets SET git_last_commit = ?, git_last_pull_at = ?, git_last_error = NULL, \
         git_synced_version = version WHERE id = ?",
    )
    .bind(&co.commit)
    .bind(now())
    .bind(&set.id)
    .execute(&state.db)
    .await
    .map_err(retry)?;
    let version = report.set.as_ref().map(|s| s.version);
    Ok(Some(json!({
        "commit": co.commit,
        "changed": true,
        "version": version,
        "upstream_version": upstream_version,
        "created": report.created,
        "updated": report.updated,
        "unchanged": report.unchanged,
        "removed": report.removed,
        "errors": report.errors,
    })))
}

/// Push local edits of a push-mode pack.
pub async fn run_push(state: &AppState, set_id: &str) -> JobResult {
    if !enabled(state) {
        return Err(JobError::Fatal(DISABLED.into()));
    }
    let lock = set_lock(set_id);
    let _guard = lock.lock().await;
    let Some(set) = db::set_by_id(&state.db, set_id).await.map_err(retry)? else {
        return Ok(Some(json!({"skipped": "the set no longer exists"})));
    };
    if set.source != "git" || set.git_mode.as_deref() != Some("push") {
        return Ok(Some(json!({"skipped": "not a push-mode git pack"})));
    }
    match sync_state(state, &set.id).await?.synced_version {
        // Never overwrite a repository we have not imported yet.
        None => return Ok(Some(json!({"skipped": "waiting for the first pull"}))),
        Some(v) if v == set.version => return Ok(Some(json!({"skipped": "nothing to push"}))),
        Some(_) => {}
    }
    let Some(actor) = owner_actor(state, &set).await? else {
        return Ok(Some(json!({"skipped": "the owner no longer exists"})));
    };
    let dest = checkout_dir(state, &set.id).ok_or_else(|| JobError::Fatal("bad set id".into()))?;
    let auth = git_auth(state)?;
    if let Err(e) = sync(state, &set, &dest, &auth).await {
        record_failure(state, &set.id, false, &e).await;
        return Err(e);
    }
    push_inner(state, &set, &actor, &dest, &auth).await
}

/// Export into the (freshly synced) checkout, commit and push. A rejected
/// push re-syncs, re-exports and tries once more, then fails the job.
async fn push_inner(state: &AppState, set: &SetRow, actor: &CurrentUser, dest: &Path, auth: &GitAuth) -> JobResult {
    let cfg = &state.config.git;
    let mut last_err = String::new();
    for attempt in 0..2 {
        if attempt > 0 {
            tracing::info!(set = %set.id, "push failed ({last_err}); re-syncing and retrying once");
            if let Err(e) = sync(state, set, dest, auth).await {
                record_failure(state, &set.id, false, &e).await;
                return Err(e);
            }
        }
        let (_, bundle) = transfer::export_bundle(state, actor, &set.id)
            .await
            .map_err(app_error)?;
        let version = bundle.manifest.version;
        let message = format!("stickpile: update {} (version {version})", bundle.manifest.name);
        let dir = set_dir(dest, set);
        let (repo, auth2, name, email) = (
            dest.to_path_buf(),
            auth.clone(),
            cfg.author_name.clone(),
            cfg.author_email.clone(),
        );
        let res = tokio::task::spawn_blocking(move || {
            stickpile_setfmt::write_dir(&dir, &bundle).map_err(|e| e.to_string())?;
            stickpile_gitpack::commit_and_push(&repo, &message, &name, &email, &auth2).map_err(|e| e.to_string())
        })
        .await
        .map_err(retry)?;
        match res {
            Ok(co) => {
                sqlx::query(
                    "UPDATE sets SET git_last_commit = NULLIF(?, ''), git_synced_version = ?, git_last_error = NULL \
                     WHERE id = ?",
                )
                .bind(&co.commit)
                .bind(version as i64)
                .bind(&set.id)
                .execute(&state.db)
                .await
                .map_err(retry)?;
                return Ok(Some(json!({
                    "pushed": true,
                    "committed": co.changed,
                    "commit": co.commit,
                    "version": version,
                })));
            }
            Err(e) => last_err = e,
        }
    }
    let e = JobError::Fatal(format!("push failed: {last_err}"));
    record_failure(state, &set.id, false, &e).await;
    Err(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subdirs() {
        assert_eq!(clean_subdir(None).unwrap(), None);
        assert_eq!(clean_subdir(Some(" / ".into())).unwrap(), None);
        assert_eq!(
            clean_subdir(Some("/packs//cats/".into())).unwrap().as_deref(),
            Some("packs/cats")
        );
        assert_eq!(
            clean_subdir(Some("./packs/./cats".into())).unwrap().as_deref(),
            Some("packs/cats")
        );
        for bad in ["../x", "a/../b", ".git", "a/.hidden", "a\\b", "a\u{1}b"] {
            assert!(clean_subdir(Some(bad.into())).is_err(), "{bad}");
        }
    }

    #[test]
    fn branches() {
        assert_eq!(clean_branch(Some(" main ".into())).unwrap().as_deref(), Some("main"));
        assert_eq!(
            clean_branch(Some("feature/x-1".into())).unwrap().as_deref(),
            Some("feature/x-1")
        );
        assert_eq!(clean_branch(Some("".into())).unwrap(), None);
        for bad in ["-x", "a..b", "a b", "x.lock", "a:b", "@{-1}x", "a~1", "/a", "a/"] {
            assert!(clean_branch(Some(bad.into())).is_err(), "{bad}");
        }
    }

    #[test]
    fn repo_names() {
        assert_eq!(repo_name("https://codeberg.org/alice/cat-stickers.git"), "cat-stickers");
        assert_eq!(repo_name("https://github.com/alice/cats/"), "cats");
        assert_eq!(repo_name("git@github.com:alice/dogs.git"), "dogs");
        assert_eq!(repo_name("file:///srv/repos/memes"), "memes");
    }

    #[test]
    fn webhook_verification() {
        let mut h = HeaderMap::new();
        assert_eq!(
            verify_webhook("s3cret", &h, b"{}").unwrap_err().status,
            StatusCode::UNAUTHORIZED
        );
        h.insert("x-stickpile-webhook-secret", "wrong".parse().unwrap());
        assert_eq!(
            verify_webhook("s3cret", &h, b"{}").unwrap_err().status,
            StatusCode::FORBIDDEN
        );
        h.insert("x-stickpile-webhook-secret", "s3cret".parse().unwrap());
        assert!(verify_webhook("s3cret", &h, b"{}").is_ok());
        let mut h = HeaderMap::new();
        h.insert("x-gitlab-token", "s3cret".parse().unwrap());
        assert!(verify_webhook("s3cret", &h, b"{}").is_ok());
        // An empty secret never verifies.
        assert!(verify_webhook("", &h, b"{}").is_err());
    }
}
