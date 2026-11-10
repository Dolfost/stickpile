//! Health, search, usage, popular, stats, profiles, jobs.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use stickpile_api::profiles::{all_profiles, TargetProfile};
use stickpile_api::routes as r;
use stickpile_api::{ApiError, Job, Popular, SearchResults, TargetApp, UsageAck, UsageBatch, UserStats};
use utoipa::IntoParams;

use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::jobs;
use crate::service::search::{self, SearchParams};
use crate::service::stats::{self, PopularScope, Window};
use crate::state::AppState;

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct Health {
    pub status: &'static str,
    pub version: &'static str,
}

/// Liveness probe (unauthenticated).
#[utoipa::path(get, path = r::HEALTH, tag = "misc", responses((status = 200, body = Health)))]
pub async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchQuery {
    /// Query in the search syntax (`human e:😂 set:cats q>70 ...`).
    #[serde(default)]
    pub q: String,
    /// Default 50, max 200.
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    /// Target app; hits that cannot be sent there get `incompatible`.
    pub target: Option<String>,
}

/// Hybrid search over the caller's items and public items.
#[utoipa::path(get, path = r::SEARCH, tag = "search", params(SearchQuery),
    responses((status = 200, body = SearchResults), (status = 400, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn search(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SearchQuery>,
) -> AppResult<Json<SearchResults>> {
    let target = match q.target.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => Some(TargetApp::parse(t).ok_or_else(|| AppError::bad_request(format!("unknown target {t:?}")))?),
        None => None,
    };
    if q.q.len() > 1000 {
        return Err(AppError::bad_request("query too long"));
    }
    let params = SearchParams {
        q: q.q,
        limit: q.limit,
        offset: q.offset,
        target,
    };
    Ok(Json(search::search(&state, &user, params).await?))
}

/// Report copy events (idempotent by event id).
#[utoipa::path(post, path = r::USAGE, tag = "stats", request_body = UsageBatch,
    responses((status = 200, body = UsageAck), (status = 400, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn usage(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(batch): Json<UsageBatch>,
) -> AppResult<Json<UsageAck>> {
    Ok(Json(stats::record_usage(&state, &user, batch).await?))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PopularQuery {
    /// `server` (default: own + public items), `mine`, or `set`.
    pub scope: Option<PopularScope>,
    /// Set id for `scope=set`.
    pub set: Option<String>,
    /// Window of `most_copied`: `day`, `week` (default), `month`, `all`.
    pub window: Option<Window>,
    /// Entries per list, default 20, max 100.
    pub limit: Option<i64>,
}

/// Most copied, most liked and trending items, as three separate lists.
#[utoipa::path(get, path = r::POPULAR, tag = "stats", params(PopularQuery),
    responses((status = 200, body = Popular), (status = 404, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn popular(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PopularQuery>,
) -> AppResult<Json<Popular>> {
    Ok(Json(
        stats::popular(
            &state,
            &user,
            q.scope.unwrap_or_default(),
            q.set.as_deref(),
            q.window.unwrap_or_default(),
            q.limit,
        )
        .await?,
    ))
}

/// The caller's statistics.
#[utoipa::path(get, path = r::STATS_ME, tag = "stats", responses((status = 200, body = UserStats)),
    security(("bearer" = []), ("session" = [])))]
pub async fn stats_me(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<UserStats>> {
    Ok(Json(stats::user_stats(&state, &user).await?))
}

/// What each target chat app accepts.
#[utoipa::path(get, path = r::PROFILES, tag = "misc", responses((status = 200, body = Vec<TargetProfile>)),
    security(("bearer" = []), ("session" = [])))]
pub async fn profiles(_user: CurrentUser) -> Json<Vec<TargetProfile>> {
    Json(all_profiles())
}

/// A background job (its owner or an admin).
#[utoipa::path(get, path = r::JOB, tag = "misc", params(("id" = String, Path, description = "job id")),
    responses((status = 200, body = Job), (status = 404, body = ApiError)), security(("bearer" = []), ("session" = [])))]
pub async fn job(State(state): State<AppState>, user: CurrentUser, Path(id): Path<String>) -> AppResult<Json<Job>> {
    let job = jobs::get(&state.db, &id).await?.ok_or_else(AppError::not_found)?;
    if job.owner_id.as_deref() != Some(user.id()) && !user.is_admin() {
        return Err(AppError::not_found());
    }
    Ok(Json(job.to_api()))
}
