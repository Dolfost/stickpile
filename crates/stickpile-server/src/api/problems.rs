//! Problems report endpoints: per set, for all own sets (or the whole server
//! for admins), and dismissals.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use stickpile_api::routes as r;
use stickpile_api::{ApiError, ProblemKind, ProblemReport, ProblemSeverity, SetProblems};
use utoipa::IntoParams;

use super::empty;
use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::service::problems::{self as svc, Filter, ProblemScope};
use crate::state::AppState;

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FilterQuery {
    /// The least severe problems to list: `error` (errors only), `warning`
    /// (errors and warnings) or `info` (everything, the default).
    pub severity: Option<String>,
    /// Only this kind (`near_duplicate`, `no_emoji`, ...).
    pub kind: Option<String>,
    /// Also list dismissed problems (default false).
    pub dismissed: Option<bool>,
}

impl FilterQuery {
    pub fn filter(&self) -> AppResult<Filter> {
        let severity = match self.severity.as_deref().filter(|s| !s.is_empty()) {
            Some(s) => Some(
                ProblemSeverity::parse(s)
                    .ok_or_else(|| AppError::bad_request("severity must be error, warning or info"))?,
            ),
            None => None,
        };
        let kind = match self.kind.as_deref().filter(|s| !s.is_empty()) {
            Some(k) => Some(ProblemKind::parse(k).ok_or_else(|| AppError::bad_request(format!("unknown kind {k:?}")))?),
            None => None,
        };
        Ok(Filter {
            severity,
            kind,
            dismissed: self.dismissed.unwrap_or(false),
        })
    }
}

/// Problems of one set: its owner, and admins (read-only; items of other
/// users' private sets are `hidden`).
#[utoipa::path(get, path = r::SET_PROBLEMS, tag = "problems",
    params(("id" = String, Path, description = "set id"), FilterQuery),
    responses((status = 200, body = SetProblems), (status = 400, body = ApiError), (status = 403, body = ApiError),
              (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn for_set(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> AppResult<Json<SetProblems>> {
    Ok(Json(svc::for_set(&state, &user, &id, q.filter()?).await?))
}

/// Query of [`report`]: [`FilterQuery`] plus the scope. (Not flattened:
/// serde_urlencoded cannot parse booleans inside a flattened struct.)
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReportQuery {
    /// `mine` (default) or `server` (every set; admins only, 403 otherwise).
    pub scope: Option<ProblemScope>,
    /// As for a set's problems.
    pub severity: Option<String>,
    pub kind: Option<String>,
    pub dismissed: Option<bool>,
}

/// Problems of every own set, or of every set on the server (admins).
#[utoipa::path(get, path = r::PROBLEMS, tag = "problems", params(ReportQuery),
    responses((status = 200, body = ProblemReport), (status = 400, body = ApiError), (status = 403, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn report(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ReportQuery>,
) -> AppResult<Json<ProblemReport>> {
    let filter = FilterQuery {
        severity: q.severity,
        kind: q.kind,
        dismissed: q.dismissed,
    }
    .filter()?;
    Ok(Json(
        svc::report(&state, &user, q.scope.unwrap_or_default(), filter).await?,
    ))
}

/// Mark a current problem of an own set as "not a problem" (idempotent).
#[utoipa::path(put, path = r::SET_PROBLEM_DISMISSAL, tag = "problems",
    params(("id" = String, Path, description = "set id"), ("problem" = String, Path, description = "problem id")),
    responses((status = 204), (status = 403, body = ApiError), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn dismiss(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, problem)): Path<(String, String)>,
) -> AppResult<Response> {
    svc::dismiss(&state, &user, &id, &problem).await?;
    Ok(empty(StatusCode::NO_CONTENT))
}

/// Show a dismissed problem again (idempotent).
#[utoipa::path(delete, path = r::SET_PROBLEM_DISMISSAL, tag = "problems",
    params(("id" = String, Path, description = "set id"), ("problem" = String, Path, description = "problem id")),
    responses((status = 204), (status = 403, body = ApiError), (status = 404, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn undismiss(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, problem)): Path<(String, String)>,
) -> AppResult<Response> {
    svc::undismiss(&state, &user, &id, &problem).await?;
    Ok(empty(StatusCode::NO_CONTENT))
}
