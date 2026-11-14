//! Problems report: what is wrong (or worth a look) in a set. For one set
//! (its owner, and admins read-only), for every set a user owns, and for
//! every set on the server (admins). Shared by the JSON API and the web UI
//! (the set page's "Problems" section and the Problems tab); docs/web.md and
//! docs/api.md ("Problems") describe each kind.
//!
//! Computed on every request, no report is cached: loading the facts of a
//! set is a handful of indexed queries, and detection is O(n²) over cheap
//! hashes and dot products within a set (see [`detect`]). The one expensive
//! input, the thumbnails' pixel signatures that confirm near-duplicates
//! ([`stickpile_media::signature`]), depends only on the media and is cached
//! by media id. Cross-set checks never compare all pairs: the same file in
//! two sets is found by grouping on the stored file's hash. The server scope
//! works through the sets in batches.
//!
//! Dismissals ("not a problem") are rows of `problem_dismissals` keyed by the
//! problem id, which hashes what makes the problem: when that changes the
//! problem gets a new id and shows up again.

pub mod detect;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use sqlx::{QueryBuilder, Sqlite};
use stickpile_api::{
    JobStatus, MediaKind, Problem, ProblemCounts, ProblemKind, ProblemReport, ProblemSeverity, SetProblems, SetSource,
    SetSummary, Variant,
};
use stickpile_media::signature::Signature;
use stickpile_telegram::PackMapping;

use self::detect::{Env, ItemFacts, JobFact, SetFacts};
use crate::auth::CurrentUser;
use crate::authz;
use crate::db::{self, i64_to_u64, json_list, parse_enum, SetRow, SET_COLUMNS};
use crate::error::{AppError, AppResult};
use crate::files;
use crate::ids::{is_id, now};
use crate::service::telegram;
use crate::state::{AppState, LruCache};

/// Sets loaded and checked together (server scope).
const BATCH: usize = 50;
/// Dismissals kept per set; adding one beyond drops the oldest.
const MAX_DISMISSALS: i64 = 500;
/// Pixel signatures kept in memory (about 12 KB each).
const SIGNATURE_CACHE: usize = 2048;

/// Which sets a report covers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProblemScope {
    /// The caller's own sets (default).
    #[default]
    Mine,
    /// Every set on the server (admins only).
    Server,
}

impl ProblemScope {
    pub fn as_str(self) -> &'static str {
        match self {
            ProblemScope::Mine => "mine",
            ProblemScope::Server => "server",
        }
    }
}

/// Which problems a report lists. Counts always cover every problem.
#[derive(Debug, Clone, Copy, Default)]
pub struct Filter {
    /// The least severe to list: `Error` lists errors only, `Warning`
    /// errors and warnings, `None` / `Info` everything.
    pub severity: Option<ProblemSeverity>,
    pub kind: Option<ProblemKind>,
    /// List dismissed problems too.
    pub dismissed: bool,
}

impl Filter {
    fn keeps(&self, p: &Problem) -> bool {
        (self.dismissed || !p.dismissed)
            && self.severity.is_none_or(|s| p.severity <= s)
            && self.kind.is_none_or(|k| p.kind == k)
    }
}

fn counts(problems: &[Problem]) -> ProblemCounts {
    let mut c = ProblemCounts::default();
    for p in problems {
        match (p.dismissed, p.severity) {
            (true, _) => c.dismissed += 1,
            (false, ProblemSeverity::Error) => c.error += 1,
            (false, ProblemSeverity::Warning) => c.warning += 1,
            (false, ProblemSeverity::Info) => c.info += 1,
        }
    }
    c
}

fn add(total: &mut ProblemCounts, c: &ProblemCounts) {
    total.error += c.error;
    total.warning += c.warning;
    total.info += c.info;
    total.dismissed += c.dismissed;
}

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

/// The problems of one set: for its owner, and read-only for admins (with
/// the items of other users' private sets withheld). Others: 404 when they
/// cannot read the set, 403 when they can.
pub async fn for_set(state: &AppState, viewer: &CurrentUser, set_id: &str, filter: Filter) -> AppResult<SetProblems> {
    let set = if viewer.is_admin() {
        if !is_id(set_id) {
            return Err(AppError::not_found());
        }
        db::set_by_id(&state.db, set_id)
            .await?
            .ok_or_else(AppError::not_found)?
    } else {
        let set = authz::readable_set(state, viewer.id(), set_id).await?;
        if !set.is_owner() {
            return Err(AppError::forbidden("only the owner of a set sees its problems"));
        }
        set.set
    };
    let mut out = check(state, viewer, vec![set], filter).await?;
    Ok(out.pop().expect("one set in, one out"))
}

/// Problems of every set in `scope`, grouped by set. `Server` needs the
/// admin role (403 otherwise).
pub async fn report(
    state: &AppState,
    viewer: &CurrentUser,
    scope: ProblemScope,
    filter: Filter,
) -> AppResult<ProblemReport> {
    let sets: Vec<SetRow> = match scope {
        ProblemScope::Mine => {
            sqlx::query_as(&format!(
                "SELECT {SET_COLUMNS} FROM sets WHERE owner_id = ? ORDER BY name COLLATE NOCASE, id"
            ))
            .bind(viewer.id())
            .fetch_all(&state.db)
            .await?
        }
        ProblemScope::Server => {
            if !viewer.is_admin() {
                return Err(AppError::forbidden(
                    "the server-wide problems report needs the admin role",
                ));
            }
            sqlx::query_as(&format!(
                "SELECT {SET_COLUMNS} FROM sets ORDER BY owner_id, name COLLATE NOCASE, id"
            ))
            .fetch_all(&state.db)
            .await?
        }
    };
    let sets_checked = sets.len() as u32;
    let mut total = ProblemCounts::default();
    let mut out = Vec::new();
    let mut rest = sets;
    while !rest.is_empty() {
        let batch: Vec<SetRow> = rest.drain(..rest.len().min(BATCH)).collect();
        for sp in check(state, viewer, batch, filter).await? {
            add(&mut total, &sp.counts);
            if !sp.problems.is_empty() {
                out.push(sp);
            }
        }
    }
    // Worst sets first, then by name.
    out.sort_by(|a, b| {
        let key = |s: &SetProblems| std::cmp::Reverse((s.counts.error, s.counts.warning, s.counts.info));
        key(a)
            .cmp(&key(b))
            .then_with(|| a.set.name.to_lowercase().cmp(&b.set.name.to_lowercase()))
    });
    Ok(ProblemReport {
        scope: scope.as_str().to_string(),
        sets: out,
        counts: total,
        sets_checked,
        similar_available: state.ai_model_id().is_some(),
        generated_at: now(),
    })
}

/// Mark a problem of an own set as "not a problem". It must be a current
/// problem of the set (404 otherwise: it changed or was fixed meanwhile).
pub async fn dismiss(state: &AppState, viewer: &CurrentUser, set_id: &str, problem_id: &str) -> AppResult<()> {
    let set = authz::owned_set(state, viewer.id(), set_id).await?;
    if !detect::valid_problem_id(problem_id) {
        return Err(AppError::not_found());
    }
    let all = Filter {
        dismissed: true,
        ..Filter::default()
    };
    let current = check(state, viewer, vec![set.set.clone()], all).await?;
    let Some(p) = current.iter().flat_map(|s| &s.problems).find(|p| p.id == problem_id) else {
        return Err(AppError::new(
            axum::http::StatusCode::NOT_FOUND,
            "not_found",
            "no such problem in this set (it may have changed or been fixed)",
        ));
    };
    let mut tx = crate::service::sets::begin(&state.db).await?;
    sqlx::query(
        "INSERT INTO problem_dismissals (set_id, problem_id, kind, dismissed_by, created_at) VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(set_id, problem_id) DO NOTHING",
    )
    .bind(&set.id)
    .bind(problem_id)
    .bind(p.kind.as_str())
    .bind(viewer.id())
    .bind(now())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM problem_dismissals WHERE set_id = ? AND problem_id NOT IN \
         (SELECT problem_id FROM problem_dismissals WHERE set_id = ? ORDER BY created_at DESC, rowid DESC LIMIT ?)",
    )
    .bind(&set.id)
    .bind(&set.id)
    .bind(MAX_DISMISSALS)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Undo [`dismiss`] (idempotent).
pub async fn undismiss(state: &AppState, viewer: &CurrentUser, set_id: &str, problem_id: &str) -> AppResult<()> {
    let set = authz::owned_set(state, viewer.id(), set_id).await?;
    if !detect::valid_problem_id(problem_id) {
        return Err(AppError::not_found());
    }
    sqlx::query("DELETE FROM problem_dismissals WHERE set_id = ? AND problem_id = ?")
        .bind(&set.id)
        .bind(problem_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Checking a batch of sets
// ---------------------------------------------------------------------------

/// Problems of `sets` (authorization already done), in the same order.
async fn check(
    state: &AppState,
    viewer: &CurrentUser,
    sets: Vec<SetRow>,
    filter: Filter,
) -> AppResult<Vec<SetProblems>> {
    let env = Env {
        telegram: telegram::enabled(state),
        ffmpeg: state.ffmpeg_available,
    };
    let facts = load(state, &sets, env.telegram).await?;
    let dismissed = dismissals(state, &sets).await?;
    let usernames = usernames(state, &sets).await?;

    // Candidates of every set, then the signatures they need in one go.
    let cands: Vec<Vec<detect::Candidate>> = facts.iter().map(|f| detect::candidates(&f.facts.items)).collect();
    let mut needed: Vec<(String, String)> = Vec::new();
    let mut seen = HashSet::new();
    for (f, c) in facts.iter().zip(&cands) {
        for it in detect::signatures_needed(&f.facts.items, c) {
            if seen.insert(it.media_id.clone()) {
                if let Some(path) = f.thumb_paths.get(&it.media_id) {
                    needed.push((it.media_id.clone(), path.clone()));
                }
            }
        }
    }
    let sigs = signatures(state, needed).await;
    let member_sets = authz::member_set_ids(&state.db, viewer.id()).await?;

    let mut out = Vec::with_capacity(sets.len());
    for ((set, f), c) in sets.iter().zip(&facts).zip(&cands) {
        let mut problems = detect::detect(&f.facts, c, &sigs, &env);
        for p in &mut problems {
            p.dismissed = dismissed.contains(&(set.id.clone(), p.id.clone()));
        }
        let readable = authz::can_read_set(viewer.id(), set, member_sets.contains(&set.id));
        if !readable {
            for p in &mut problems {
                for it in &mut p.items {
                    it.name.clear();
                    it.detail = None;
                    it.hidden = true;
                }
            }
        }
        let c = counts(&problems);
        problems.retain(|p| filter.keeps(p));
        out.push(SetProblems {
            set: summary(
                set,
                usernames.get(&set.owner_id).cloned().unwrap_or_default(),
                f,
                readable,
            ),
            owner_id: set.owner_id.clone(),
            problems,
            counts: c,
            can_dismiss: set.owner_id == viewer.id(),
            can_edit: set.owner_id == viewer.id() && !set.read_only(),
        });
    }
    Ok(out)
}

fn summary(set: &SetRow, owner_username: String, f: &Loaded, readable: bool) -> SetSummary {
    let items = &f.facts.items;
    let cover = set
        .cover_item_id
        .as_ref()
        .filter(|c| items.iter().any(|it| &it.id == *c))
        .cloned()
        .or_else(|| items.first().map(|it| it.id.clone()));
    // The set's own cover image wins over its items (service::cover). The
    // URL is unversioned here (no media facts loaded), so it is served
    // with `no-cache` rather than cached for good.
    let cover_url = match (&set.cover_media_id, &cover) {
        (Some(_), _) => Some(stickpile_api::routes::path(
            stickpile_api::routes::SET_COVER_FILE,
            &[&set.id, Variant::Thumb.as_str()],
        )),
        (None, Some(item)) => Some(stickpile_api::routes::path(
            stickpile_api::routes::ITEM_FILE,
            &[item, Variant::Thumb.as_str()],
        )),
        (None, None) => None,
    };
    SetSummary {
        id: set.id.clone(),
        slug: set.slug.clone(),
        name: set.name.clone(),
        description: if readable {
            set.description.clone()
        } else {
            String::new()
        },
        owner_username,
        visibility: set.visibility(),
        source: parse_enum(&set.source).unwrap_or(SetSource::Local),
        item_count: items.len() as u64,
        cover_item_id: cover.filter(|_| readable),
        cover_url: cover_url.filter(|_| readable),
        updated_at: set.updated_at,
    }
}

async fn usernames(state: &AppState, sets: &[SetRow]) -> AppResult<HashMap<String, String>> {
    let owners: HashSet<&str> = sets.iter().map(|s| s.owner_id.as_str()).collect();
    let mut out = HashMap::new();
    for owner in owners {
        if let Some(u) = db::user_by_id(&state.db, owner).await? {
            out.insert(u.id, u.username);
        }
    }
    Ok(out)
}

async fn dismissals(state: &AppState, sets: &[SetRow]) -> AppResult<HashSet<(String, String)>> {
    let mut out = HashSet::new();
    for chunk in sets.chunks(BATCH) {
        let mut qb: QueryBuilder<Sqlite> =
            QueryBuilder::new("SELECT set_id, problem_id FROM problem_dismissals WHERE set_id IN (");
        let mut sep = qb.separated(", ");
        for s in chunk {
            sep.push_bind(&s.id);
        }
        qb.push(")");
        let rows: Vec<(String, String)> = qb.build_query_as().fetch_all(&state.db).await?;
        out.extend(rows);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Loading facts
// ---------------------------------------------------------------------------

/// Facts of one set plus where its thumbnails are.
struct Loaded {
    facts: SetFacts,
    /// media id -> thumbnail path (relative to the media dir).
    thumb_paths: HashMap<String, String>,
}

#[derive(sqlx::FromRow)]
struct ItemRow {
    id: String,
    set_id: String,
    position: i64,
    name: String,
    name_source: String,
    keywords: String,
    keywords_source: String,
    emojis: String,
    emojis_source: String,
    vibe: Option<String>,
    vibe_source: String,
    created_at: i64,
    media_id: String,
    sha256: String,
    original_sha256: String,
    kind: String,
    width: i64,
    height: i64,
    duration_ms: Option<i64>,
    quality: Option<i64>,
    phash: i64,
    dhash: i64,
    variants_done: bool,
}

#[derive(sqlx::FromRow)]
struct JobRowLite {
    id: String,
    subject: Option<String>,
    status: String,
    error: Option<String>,
    finished_at: Option<i64>,
}

impl JobRowLite {
    fn fact(&self) -> JobFact {
        JobFact {
            id: self.id.clone(),
            status: parse_enum(&self.status).unwrap_or(JobStatus::Queued),
            error: self.error.clone(),
            finished_at: self.finished_at,
        }
    }
}

fn ai_fields(r: &ItemRow) -> Vec<&'static str> {
    let mut out = Vec::new();
    if r.name_source == "ai" && !r.name.trim().is_empty() {
        out.push("name");
    }
    if r.keywords_source == "ai" && !json_list(&r.keywords).is_empty() {
        out.push("keywords");
    }
    if r.emojis_source == "ai" && !json_list(&r.emojis).is_empty() {
        out.push("emojis");
    }
    if r.vibe_source == "ai" && r.vibe.is_some() {
        out.push("vibe");
    }
    out
}

/// `SELECT ... WHERE <column> IN (<ids>)` over chunks of `ids`.
async fn rows_in<T>(state: &AppState, select: &str, column: &str, ids: &[String], tail: &str) -> AppResult<Vec<T>>
where
    T: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin,
{
    let mut out = Vec::new();
    for chunk in ids.chunks(400) {
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(select);
        qb.push(format!(" {column} IN ("));
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(id);
        }
        qb.push(") ");
        qb.push(tail);
        out.extend(qb.build_query_as::<T>().fetch_all(&state.db).await?);
    }
    Ok(out)
}

async fn load(state: &AppState, sets: &[SetRow], telegram_on: bool) -> AppResult<Vec<Loaded>> {
    let set_ids: Vec<String> = sets.iter().map(|s| s.id.clone()).collect();
    let rows: Vec<ItemRow> = rows_in(
        state,
        "SELECT i.id, i.set_id, i.position, i.name, i.name_source, i.keywords, i.keywords_source, i.emojis, \
         i.emojis_source, i.vibe, i.vibe_source, i.created_at, i.media_id, m.sha256, m.original_sha256, m.kind, \
         m.width, m.height, m.duration_ms, m.quality, m.phash, m.dhash, m.variants_done \
         FROM items i JOIN media m ON m.id = i.media_id WHERE",
        "i.set_id",
        &set_ids,
        "ORDER BY i.set_id, i.position, i.id",
    )
    .await?;
    let media_ids: Vec<String> = rows
        .iter()
        .map(|r| r.media_id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    // Variants (and where the thumbnails are).
    let vrows: Vec<(String, String, String)> = rows_in(
        state,
        "SELECT media_id, variant, path FROM media_variants WHERE",
        "media_id",
        &media_ids,
        "",
    )
    .await?;
    let mut variants: HashMap<String, Vec<Variant>> = HashMap::new();
    let mut thumbs: HashMap<String, String> = HashMap::new();
    for (m, v, path) in vrows {
        if let Some(v) = Variant::parse(&v) {
            if v == Variant::Thumb {
                thumbs.insert(m.clone(), path);
            }
            variants.entry(m).or_default().push(v);
        }
    }

    // The newest Variants job of media that are not done.
    let pending: Vec<String> = rows
        .iter()
        .filter(|r| !r.variants_done)
        .map(|r| r.media_id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut variant_jobs: HashMap<String, JobFact> = HashMap::new();
    let jobs: Vec<JobRowLite> = rows_in(
        state,
        "SELECT id, subject, status, error, finished_at FROM jobs WHERE kind = 'variants' AND",
        "subject",
        &pending,
        "ORDER BY created_at, rowid",
    )
    .await?;
    for j in jobs {
        if let Some(s) = j.subject.clone() {
            variant_jobs.insert(s, j.fact());
        }
    }

    // Telegram: the newest export job per set (and the newest that worked),
    // and the sticker mapping of bound sets.
    let mut last_export: HashMap<String, JobFact> = HashMap::new();
    let mut last_done: HashMap<String, i64> = HashMap::new();
    let mut mappings: HashMap<String, PackMapping> = HashMap::new();
    if telegram_on {
        let jobs: Vec<JobRowLite> = rows_in(
            state,
            "SELECT id, subject, status, error, finished_at FROM jobs WHERE kind = 'telegram_export' AND",
            "subject",
            &set_ids,
            "ORDER BY created_at, rowid",
        )
        .await?;
        for j in jobs {
            let Some(s) = j.subject.clone() else { continue };
            if j.status == "done" {
                if let Some(t) = j.finished_at {
                    last_done.insert(s.clone(), t);
                }
            }
            last_export.insert(s, j.fact());
        }
        let bound: Vec<String> = sets
            .iter()
            .filter(|s| s.telegram_pack.is_some())
            .map(|s| s.id.clone())
            .collect();
        let maps: Vec<(String, Option<String>)> =
            rows_in(state, "SELECT id, telegram_mapping FROM sets WHERE", "id", &bound, "").await?;
        for (id, m) in maps {
            let m = m.and_then(|m| serde_json::from_str(&m).ok()).unwrap_or_default();
            mappings.insert(id, m);
        }
    }

    // Embeddings of the current model, from the search index.
    let embeddings: HashMap<String, Arc<[f32]>> = {
        let index = state.index.read().unwrap_or_else(|e| e.into_inner());
        rows.iter()
            .filter_map(|r| {
                index
                    .get(&r.id)
                    .and_then(|d| d.embedding.clone())
                    .map(|e| (r.id.clone(), e))
            })
            .collect()
    };

    let earlier = earlier_copies(state, sets).await?;

    let mut by_set: HashMap<String, Vec<ItemFacts>> = HashMap::new();
    for r in &rows {
        let facts = ItemFacts {
            id: r.id.clone(),
            name: r.name.clone(),
            position: r.position,
            created_at: r.created_at,
            media_id: r.media_id.clone(),
            sha256: r.sha256.clone(),
            original_sha256: r.original_sha256.clone(),
            kind: parse_enum(&r.kind).unwrap_or(MediaKind::Still),
            width: r.width.max(0) as u32,
            height: r.height.max(0) as u32,
            duration_ms: r.duration_ms.map(|d| d.max(0) as u64),
            quality: r.quality.map(|q| q.clamp(0, 100) as u8),
            phash: i64_to_u64(r.phash),
            dhash: i64_to_u64(r.dhash),
            emojis: json_list(&r.emojis),
            ai_fields: ai_fields(r),
            variants: variants.get(&r.media_id).cloned().unwrap_or_default(),
            variants_done: r.variants_done,
            variants_job: if r.variants_done {
                None
            } else {
                variant_jobs.get(&r.media_id).cloned()
            },
            embedding: embeddings.get(&r.id).cloned(),
            earlier_in: earlier.get(&r.id).cloned().unwrap_or_default(),
        };
        by_set.entry(r.set_id.clone()).or_default().push(facts);
    }
    Ok(sets
        .iter()
        .map(|s| {
            let items = by_set.remove(&s.id).unwrap_or_default();
            let thumb_paths = items
                .iter()
                .filter_map(|it| thumbs.get(&it.media_id).map(|p| (it.media_id.clone(), p.clone())))
                .collect();
            Loaded {
                facts: SetFacts {
                    id: s.id.clone(),
                    version: s.version,
                    updated_at: s.updated_at,
                    git_error: (s.source == "git").then(|| s.git_last_error.clone()).flatten(),
                    telegram_pack: s.telegram_pack.clone(),
                    telegram_mapping: mappings.remove(&s.id),
                    last_export: last_export.remove(&s.id),
                    last_export_done_at: last_done.get(&s.id).copied(),
                    items,
                },
                thumb_paths,
            }
        })
        .collect())
}

/// item id -> (set id, set name) of the owner's other sets that got the same
/// file earlier. Grouped by the stored file's hash in SQL: only files in more
/// than one set of an owner come back.
async fn earlier_copies(state: &AppState, sets: &[SetRow]) -> AppResult<HashMap<String, Vec<(String, String)>>> {
    let owners: HashSet<&str> = sets.iter().map(|s| s.owner_id.as_str()).collect();
    let wanted: HashSet<&str> = sets.iter().map(|s| s.id.as_str()).collect();
    let mut out: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for owner in owners {
        let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
            "SELECT i.id, i.set_id, s.name, m.original_sha256, i.created_at \
             FROM items i JOIN sets s ON s.id = i.set_id JOIN media m ON m.id = i.media_id \
             WHERE s.owner_id = ? AND m.original_sha256 IN ( \
               SELECT m2.original_sha256 FROM items i2 JOIN sets s2 ON s2.id = i2.set_id \
               JOIN media m2 ON m2.id = i2.media_id WHERE s2.owner_id = ? \
               GROUP BY m2.original_sha256 HAVING COUNT(DISTINCT i2.set_id) > 1) \
             ORDER BY m.original_sha256, i.created_at, i.rowid",
        )
        .bind(owner)
        .bind(owner)
        .fetch_all(&state.db)
        .await?;
        let mut group: Vec<&(String, String, String, String, i64)> = Vec::new();
        let mut flush = |group: &mut Vec<&(String, String, String, String, i64)>| {
            for (n, (item, set, ..)) in group.iter().enumerate() {
                if !wanted.contains(set.as_str()) {
                    continue;
                }
                let mut sets_before: Vec<(String, String)> = Vec::new();
                for (_, other_set, other_name, ..) in &group[..n] {
                    if other_set != set && !sets_before.iter().any(|(id, _)| id == other_set) {
                        sets_before.push((other_set.clone(), other_name.clone()));
                    }
                }
                if !sets_before.is_empty() {
                    out.insert(item.clone(), sets_before);
                }
            }
            group.clear();
        };
        for row in &rows {
            if group.first().is_some_and(|g| g.3 != row.3) {
                flush(&mut group);
            }
            group.push(row);
        }
        flush(&mut group);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Pixel signatures
// ---------------------------------------------------------------------------

fn signature_cache() -> &'static Mutex<LruCache<Arc<Signature>>> {
    static CACHE: OnceLock<Mutex<LruCache<Arc<Signature>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(LruCache::new(SIGNATURE_CACHE)))
}

/// Signatures of (media id, thumbnail path): from the cache, else decoded
/// on the blocking pool. Thumbnails that cannot be read are left out (their
/// pairs are then not reported as near-duplicates).
async fn signatures(state: &AppState, needed: Vec<(String, String)>) -> HashMap<String, Arc<Signature>> {
    let mut out = HashMap::new();
    let mut missing = Vec::new();
    {
        let mut cache = signature_cache().lock().unwrap_or_else(|e| e.into_inner());
        for (media, path) in needed {
            match cache.get(&media) {
                Some(s) => {
                    out.insert(media, s);
                }
                None => missing.push((media, path)),
            }
        }
    }
    if missing.is_empty() {
        return out;
    }
    let dir = state.media_dir();
    let limits = state.media_limits();
    let made = tokio::task::spawn_blocking(move || {
        missing
            .into_iter()
            .filter_map(|(media, rel)| {
                let abs = files::resolve(&dir, &rel)?;
                let bytes = std::fs::read(&abs)
                    .map_err(|e| tracing::debug!("problems: reading {}: {e}", abs.display()))
                    .ok()?;
                let sig = Signature::of_thumbnail(&bytes, &limits)
                    .map_err(|e| tracing::debug!("problems: decoding {}: {e}", abs.display()))
                    .ok()?;
                Some((media, Arc::new(sig)))
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    let mut cache = signature_cache().lock().unwrap_or_else(|e| e.into_inner());
    for (media, sig) in made {
        cache.put(media.clone(), sig.clone());
        out.insert(media, sig);
    }
    out
}

#[cfg(test)]
mod tests;
