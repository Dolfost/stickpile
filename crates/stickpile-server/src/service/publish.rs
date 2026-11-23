//! Publishing a set to chat platforms, one or several at once
//! (docs/web.md "Export and publish", docs/api.md "Publishing").
//!
//! Every platform is a [`Platform`] listed in [`PLATFORMS`]: Telegram
//! (here), Discord ([`crate::service::discord`]) and Signal
//! ([`crate::service::signal`]). A new one is added the same way: implement
//! the trait, add the value to [`PLATFORMS`], add the job kind and its arm
//! in [`crate::jobs::dispatch`]. A platform provides:
//!
//! * its id (the API's name for it), its display name and whether this
//!   server is configured for it;
//! * [`Platform::prepare`]: check a publish request (who, which set, the
//!   platform's options) and return the payload of the job that does the
//!   work. Nothing is queued there, so a request for several platforms
//!   fails as a whole before any job exists. The registry queues a job of
//!   [`Platform::job_kind`] with the set id as its subject;
//! * where the set is published now ([`Platform::published`]): Telegram
//!   reads its own binding (`sets.telegram_pack`); other platforms keep
//!   theirs in `set_publications` (migration 0012) with [`bind`] and
//!   [`stored`];
//! * the options it takes ([`Platform::options`], text fields or choices
//!   on the export page, prefilled by [`Platform::option_values`]) and a
//!   one-line summary of a job result ([`Platform::summary`]).
//!
//! When a job of a platform's kind finishes, the job queue calls
//! [`record_job`], which stores the outcome in `set_publications`: when the
//! set was last published, that result, and the last error. They outlive
//! the job rows, which are deleted after 30 days.
//!
//! Only the set's owner may publish or see the publish state, as for the
//! Telegram export (admins get no implicit access to others' sets).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use serde_json::Value;
use sqlx::SqlitePool;
use stickpile_api::{Job, JobKind, JobStatus, Publication, PublishJobs, PublishRequest, TelegramExport};

use crate::auth::CurrentUser;
use crate::authz;
use crate::db::{self, enum_str, SetRow};
use crate::error::{AppError, AppResult};
use crate::i18n::{t, tf};
use crate::ids::now;
use crate::jobs::{self, JobRow, JOB_COLUMNS};
use crate::service::discord::DiscordPlatform;
use crate::service::signal::SignalPlatform;
use crate::service::telegram;
use crate::state::AppState;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One option a platform takes: a field on the export page and a key of
/// the platform's object in [`PublishRequest::options`].
#[derive(Debug, Clone)]
pub struct PlatformOption {
    pub key: &'static str,
    /// Label in the request's language.
    pub label: &'static str,
    pub hint: Option<&'static str>,
    pub max_chars: usize,
    /// (value, label) of a choice; empty for a text field.
    pub choices: Vec<(&'static str, &'static str)>,
}

/// A row of `set_publications`.
#[derive(Debug, Clone, Default, sqlx::FromRow)]
pub struct PublicationRow {
    pub set_id: String,
    pub platform: String,
    pub target: Option<String>,
    pub url: Option<String>,
    /// Platform-specific JSON.
    pub state: Option<String>,
    pub published_at: Option<i64>,
    pub last_result: Option<String>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub last_finished_at: Option<i64>,
    pub last_job_id: Option<String>,
}

impl PublicationRow {
    pub fn state_json(&self) -> Option<Value> {
        self.state.as_deref().and_then(|s| serde_json::from_str(s).ok())
    }
}

/// A platform sets can be published to. See the module docs.
pub trait Platform: Send + Sync {
    /// Stable id used in the API and the database: `telegram`.
    fn id(&self) -> &'static str;
    /// Name for people: `Telegram`.
    fn name(&self) -> &'static str;
    /// The job that publishes (subject: the set id; payload: `set_id`
    /// plus whatever [`Platform::prepare`] returns).
    fn job_kind(&self) -> JobKind;
    /// This server is set up for the platform (credentials readable, ...).
    fn configured(&self, state: &AppState) -> bool;
    /// The options the platform takes, labelled in the request's language.
    fn options(&self) -> Vec<PlatformOption>;
    /// Check that `user` may publish set `set_id` with `options` (a JSON
    /// object, or null for the defaults) and return the job payload, which
    /// must contain `set_id`. Errors are the API's (400 for bad options).
    fn prepare<'a>(
        &'a self,
        state: &'a AppState,
        user: &'a CurrentUser,
        set_id: &'a str,
        options: &'a Value,
    ) -> BoxFuture<'a, AppResult<Value>>;
    /// What the set is published as and its link: by default the
    /// `target` and `url` stored with [`bind`].
    fn published(&self, set: &SetRow, row: Option<&PublicationRow>) -> (Option<String>, Option<String>) {
        let _ = set;
        row.map(|r| (r.target.clone(), r.url.clone())).unwrap_or_default()
    }
    /// One line about a finished publish, from its job result, in the
    /// request's language.
    fn summary(&self, result: &Value) -> String;
    /// A note for `user` next to the platform's options on the export
    /// page, such as a setting a publish needs, in the request's language.
    fn notice(&self, state: &AppState, user: &CurrentUser) -> Option<&'static str> {
        let _ = (state, user);
        None
    }
    /// Values to prefill the options with on the export page (key, value),
    /// e.g. what the set was last published with.
    fn option_values(&self, row: Option<&PublicationRow>) -> Vec<(&'static str, String)> {
        let _ = row;
        Vec::new()
    }
}

/// Telegram: a sticker pack the bot owns (docs/telegram.md).
pub struct TelegramPlatform;

impl Platform for TelegramPlatform {
    fn id(&self) -> &'static str {
        "telegram"
    }

    fn name(&self) -> &'static str {
        "Telegram"
    }

    fn job_kind(&self) -> JobKind {
        JobKind::TelegramExport
    }

    fn configured(&self, state: &AppState) -> bool {
        telegram::enabled(state)
    }

    fn options(&self) -> Vec<PlatformOption> {
        vec![
            PlatformOption {
                key: "pack_name",
                label: t("Pack short name (optional)"),
                hint: Some(t("Letters, digits and _, e.g. my_cats; _by_<bot name> is added for you. Empty: the bound pack, or one derived from the slug.")),
                max_chars: 64,
                choices: Vec::new(),
            },
            PlatformOption {
                key: "title",
                label: t("Pack title (optional)"),
                hint: None,
                max_chars: 64,
                choices: Vec::new(),
            },
        ]
    }

    fn prepare<'a>(
        &'a self,
        state: &'a AppState,
        user: &'a CurrentUser,
        set_id: &'a str,
        options: &'a Value,
    ) -> BoxFuture<'a, AppResult<Value>> {
        Box::pin(async move {
            let req: TelegramExport = if options.is_null() {
                TelegramExport {
                    pack_name: None,
                    title: None,
                }
            } else {
                serde_json::from_value(options.clone()).map_err(|e| {
                    AppError::bad_request(format!("options.telegram: {e} (pack_name and title, both optional)"))
                })?
            };
            telegram::prepare_export(state, user, set_id, req).await
        })
    }

    fn published(&self, set: &SetRow, _row: Option<&PublicationRow>) -> (Option<String>, Option<String>) {
        match &set.telegram_pack {
            Some(p) => (Some(p.clone()), Some(telegram::pack_url(p))),
            None => (None, None),
        }
    }

    fn summary(&self, r: &Value) -> String {
        let n = |k: &str| r.get(k).and_then(Value::as_u64).unwrap_or(0);
        let skipped = r.get("skipped").and_then(Value::as_array).map_or(0, Vec::len);
        tf(
            "{exported} stickers: {added} added, {removed} removed, {skipped} left out",
            &[
                ("exported", &n("exported")),
                ("added", &n("added")),
                ("removed", &n("removed")),
                ("skipped", &skipped),
            ],
        )
    }

    fn notice(&self, state: &AppState, user: &CurrentUser) -> Option<&'static str> {
        (user.user.telegram_user_id.is_none() && state.config.telegram.default_owner_user_id.is_none()).then(|| {
            t("Telegram packs belong to a Telegram user. Set your Telegram user id in Settings, unless the administrator configured a default owner.")
        })
    }
}

/// Every platform the server knows, in the order pages list them.
pub static PLATFORMS: &[&dyn Platform] = &[&TelegramPlatform, &DiscordPlatform, &SignalPlatform];

pub fn platform(id: &str) -> Option<&'static dyn Platform> {
    PLATFORMS.iter().copied().find(|p| p.id() == id)
}

/// The platform whose publish jobs are of `kind`.
pub fn for_job(kind: JobKind) -> Option<&'static dyn Platform> {
    PLATFORMS.iter().copied().find(|p| p.job_kind() == kind)
}

/// The platforms this server is configured for.
pub fn configured(state: &AppState) -> Vec<&'static dyn Platform> {
    PLATFORMS.iter().copied().filter(|p| p.configured(state)).collect()
}

/// Every `set_publications` row of a set, by platform.
pub async fn stored_all(db: &SqlitePool, set_id: &str) -> AppResult<HashMap<String, PublicationRow>> {
    let rows: Vec<PublicationRow> = sqlx::query_as("SELECT * FROM set_publications WHERE set_id = ?")
        .bind(set_id)
        .fetch_all(db)
        .await?;
    Ok(rows.into_iter().map(|r| (r.platform.clone(), r)).collect())
}

/// The stored publication of `set_id` on `platform`, if any.
pub async fn stored(db: &SqlitePool, set_id: &str, platform: &str) -> AppResult<Option<PublicationRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM set_publications WHERE set_id = ? AND platform = ?")
            .bind(set_id)
            .bind(platform)
            .fetch_optional(db)
            .await?,
    )
}

/// Store what `set_id` is published as on `platform` (target, link and
/// platform-specific state), for platforms that keep it here. `None`
/// clears a field. A set that no longer exists is left alone.
pub async fn bind(
    db: &SqlitePool,
    set_id: &str,
    platform: &str,
    target: Option<&str>,
    url: Option<&str>,
    state: Option<&Value>,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO set_publications (set_id, platform, target, url, state) \
         SELECT ?1, ?2, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM sets WHERE id = ?1) \
         ON CONFLICT(set_id, platform) DO UPDATE SET \
         target = excluded.target, url = excluded.url, state = excluded.state",
    )
    .bind(set_id)
    .bind(platform)
    .bind(target)
    .bind(url)
    .bind(state.map(Value::to_string))
    .execute(db)
    .await?;
    Ok(())
}

/// Note a finished job in `set_publications` if it is a platform's publish
/// job (called by the job queue for done and failed jobs, not for retries).
/// A job that found its set or owner gone (`{"skipped": "..."}`) is not a
/// publish.
pub async fn record_job(
    db: &SqlitePool,
    job: &JobRow,
    status: JobStatus,
    result: Option<&Value>,
    error: Option<&str>,
) -> AppResult<()> {
    let Some(platform) = job.kind().and_then(for_job) else {
        return Ok(());
    };
    let payload = job.payload();
    let Some(set_id) = payload["set_id"].as_str().or(job.subject.as_deref()) else {
        return Ok(());
    };
    if result.and_then(|r| r.get("skipped")).is_some_and(Value::is_string) {
        return Ok(());
    }
    let done = status == JobStatus::Done;
    let t = now();
    sqlx::query(
        "INSERT INTO set_publications \
         (set_id, platform, published_at, last_result, last_status, last_error, last_finished_at, last_job_id) \
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8 WHERE EXISTS (SELECT 1 FROM sets WHERE id = ?1) \
         ON CONFLICT(set_id, platform) DO UPDATE SET \
         published_at = COALESCE(excluded.published_at, published_at), \
         last_result = COALESCE(excluded.last_result, last_result), \
         last_status = excluded.last_status, last_error = excluded.last_error, \
         last_finished_at = excluded.last_finished_at, last_job_id = excluded.last_job_id",
    )
    .bind(set_id)
    .bind(platform.id())
    .bind(done.then_some(t))
    .bind(if done { result.map(Value::to_string) } else { None })
    .bind(enum_str(&status))
    .bind(error)
    .bind(t)
    .bind(&job.id)
    .execute(db)
    .await?;
    Ok(())
}

/// The newest queued or running job of `kind` for `set_id`.
async fn pending_job(db: &SqlitePool, kind: JobKind, set_id: &str) -> AppResult<Option<Job>> {
    let row: Option<JobRow> = sqlx::query_as(&format!(
        "SELECT {JOB_COLUMNS} FROM jobs WHERE kind = ? AND subject = ? AND status IN ('queued', 'running') \
         ORDER BY created_at DESC, id DESC LIMIT 1"
    ))
    .bind(enum_str(&kind))
    .bind(set_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| r.to_api()))
}

/// The publication of `set` on `platform` for the API and the pages.
fn publication(
    p: &dyn Platform,
    state: &AppState,
    set: &SetRow,
    row: Option<&PublicationRow>,
    job: Option<Job>,
) -> Publication {
    let (target, url) = p.published(set, row);
    Publication {
        platform: p.id().to_string(),
        name: p.name().to_string(),
        configured: p.configured(state),
        target,
        url,
        published_at: row.and_then(|r| r.published_at),
        last_status: row
            .and_then(|r| r.last_status.as_deref())
            .and_then(db::parse_enum::<JobStatus>),
        last_finished_at: row.and_then(|r| r.last_finished_at),
        last_error: row.and_then(|r| r.last_error.clone()),
        last_result: row
            .and_then(|r| r.last_result.as_deref())
            .and_then(|s| serde_json::from_str(s).ok()),
        job,
        // Filled by the platforms that split a set into parts.
        parts: Vec::new(),
    }
}

/// Where the caller's set is published, one entry per platform the server
/// knows (configured or not). Owner only.
pub async fn list(state: &AppState, user: &CurrentUser, set_id: &str) -> AppResult<Vec<Publication>> {
    let set = authz::owned_set(state, user.id(), set_id).await?;
    let rows = stored_all(&state.db, &set.id).await?;
    let mut out = Vec::with_capacity(PLATFORMS.len());
    for p in PLATFORMS {
        let job = pending_job(&state.db, p.job_kind(), &set.id).await?;
        out.push(publication(*p, state, &set, rows.get(p.id()), job));
    }
    Ok(out)
}

/// Queue one publish job per platform of `req`, in its order (duplicates
/// once). Owner only. Every platform is checked before any job is queued:
/// an unknown or unconfigured platform, or bad options for one, is a 400
/// and nothing is published.
pub async fn publish(
    state: &AppState,
    user: &CurrentUser,
    set_id: &str,
    req: PublishRequest,
) -> AppResult<PublishJobs> {
    let set = authz::owned_set(state, user.id(), set_id).await?;
    let mut chosen: Vec<&'static dyn Platform> = Vec::new();
    for id in &req.platforms {
        let id = id.trim();
        let Some(p) = platform(id) else {
            let known: Vec<&str> = PLATFORMS.iter().map(|p| p.id()).collect();
            return Err(AppError::bad_request(format!(
                "unknown platform {id:?}; this server knows {}",
                known.join(", ")
            )));
        };
        if !p.configured(state) {
            return Err(AppError::bad_request(format!(
                "{} is not configured on this server",
                p.name()
            )));
        }
        if !chosen.iter().any(|c| c.id() == p.id()) {
            chosen.push(p);
        }
    }
    if chosen.is_empty() {
        return Err(AppError::bad_request("choose at least one platform to publish to"));
    }
    let mut payloads = Vec::with_capacity(chosen.len());
    for p in &chosen {
        let options = req.options.get(p.id()).cloned().unwrap_or(Value::Null);
        payloads.push(p.prepare(state, user, &set.id, &options).await?);
    }
    let mut out = Vec::with_capacity(chosen.len());
    for (p, payload) in chosen.iter().zip(payloads) {
        out.push(jobs::enqueue(state, p.job_kind(), payload, Some(set.id.clone()), Some(user.id())).await?);
    }
    Ok(PublishJobs { jobs: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platforms_have_unique_ids_and_job_kinds() {
        for (i, a) in PLATFORMS.iter().enumerate() {
            for b in &PLATFORMS[i + 1..] {
                assert_ne!(a.id(), b.id());
                assert_ne!(a.job_kind(), b.job_kind());
            }
            assert!(platform(a.id()).is_some());
            assert_eq!(for_job(a.job_kind()).map(|p| p.id()), Some(a.id()));
        }
        assert!(platform("telegram").is_some());
        assert!(platform("discord").is_some());
        assert!(platform("signal").is_some());
        assert!(platform("myspace").is_none());
        assert!(for_job(JobKind::Variants).is_none());
    }

    #[test]
    fn telegram_summary_counts() {
        let s = TelegramPlatform.summary(&serde_json::json!({
            "exported": 5, "added": 2, "removed": 1, "skipped": [{"item_id": "x"}]
        }));
        assert_eq!(s, "5 stickers: 2 added, 1 removed, 1 left out");
    }
}
