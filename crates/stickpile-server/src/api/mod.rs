//! JSON REST API (`/api/v1`, paths from [`stickpile_api::routes`]). Handlers
//! are thin: extract, call a [`crate::service`] function, render. Each one
//! carries a `#[utoipa::path]` and is listed in [`openapi::ApiDoc`].

pub mod admin;
pub mod ai;
pub mod auth;
pub mod collab;
pub mod integrations;
pub mod items;
pub mod misc;
pub mod openapi;
pub mod problems;
pub mod sets;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use stickpile_api::routes as r;

use crate::error::{code_for_status, AppError};
use crate::state::AppState;

/// Every API route. Upload and import routes get the request size limit
/// `limits.max_request_mb`, AI suggestions their image limit plus room for
/// the multipart framing; everything else the default 2 MiB.
pub fn routes(state: &AppState) -> Router<AppState> {
    let big = DefaultBodyLimit::max((state.config.limits.max_request_mb * 1024 * 1024) as usize);
    Router::new()
        .route(r::HEALTH, get(misc::health))
        .route(r::OPENAPI, get(openapi::openapi_json))
        // auth & account
        .route(r::AUTH_TOKEN, post(auth::token))
        .route(r::AUTH_REGISTER, post(auth::register))
        .route(r::ME, get(auth::me).patch(auth::update_me))
        .route(r::ME_PASSWORD, put(auth::change_password))
        .route(r::ME_TOTP, post(auth::totp_begin))
        .route(r::ME_TOTP_CONFIRM, post(auth::totp_confirm))
        .route(r::ME_TOTP_DISABLE, post(auth::totp_disable))
        .route(r::TOKENS, get(auth::list_tokens).post(auth::create_token))
        .route(r::TOKEN, axum::routing::delete(auth::revoke_token))
        // sets
        .route(r::SETS, get(sets::list).post(sets::create))
        .route(r::SETS_IMPORT, post(sets::import).layer(big))
        .route(r::SETS_GITPACK, post(integrations::create_gitpack))
        .route(r::SET, get(sets::get).patch(sets::update).delete(sets::delete))
        .route(r::SET_ORDER, put(sets::reorder))
        .route(r::SET_ITEMS, post(sets::upload).layer(big))
        .route(r::SET_EXPORT, get(sets::export))
        .route(r::SET_COVER, put(sets::put_cover).delete(sets::delete_cover).layer(big))
        .route(r::SET_COVER_FILE, get(sets::cover_file))
        .route(r::SET_MEMBERS, get(collab::list_members))
        .route(r::SET_MEMBER, put(collab::put_member).delete(collab::delete_member))
        .route(r::SET_PROPOSALS, post(collab::propose_set))
        .route(r::SET_PROPOSAL_ITEMS, post(collab::propose_items).layer(big))
        .route(r::SET_PROPOSAL_COVER, post(collab::propose_cover).layer(big))
        .route(r::SET_PULL, post(integrations::pull))
        .route(r::GIT_WEBHOOK, post(integrations::git_webhook))
        .route(r::SET_TELEGRAM_EXPORT, post(integrations::telegram_export))
        .route(r::TELEGRAM_IMPORT, post(integrations::telegram_import))
        // problems
        .route(r::SET_PROBLEMS, get(problems::for_set))
        .route(
            r::SET_PROBLEM_DISMISSAL,
            put(problems::dismiss).delete(problems::undismiss),
        )
        .route(r::PROBLEMS, get(problems::report))
        // items
        .route(r::ITEMS, get(items::sync))
        .route(r::ITEM, get(items::get).patch(items::update).delete(items::delete))
        .route(r::ITEM_FILE, get(items::file))
        .route(r::ITEM_LIKE, put(items::like).delete(items::unlike))
        .route(r::ITEM_BETTER, get(items::better))
        .route(r::ITEM_ENRICH, post(items::enrich))
        .route(r::ITEM_PROPOSALS, post(collab::propose_item))
        // proposals
        .route(r::PROPOSALS, get(collab::list))
        .route(r::PROPOSAL, get(collab::get))
        .route(r::PROPOSAL_ACCEPT, post(collab::accept))
        .route(r::PROPOSAL_DECLINE, post(collab::decline))
        .route(r::PROPOSAL_WITHDRAW, post(collab::withdraw))
        .route(r::PROPOSAL_THUMB, get(collab::thumb))
        // AI suggestions: one small image, nothing stored.
        .route(
            r::AI_SUGGEST,
            post(ai::suggest).layer(DefaultBodyLimit::max(
                (crate::service::ai::SUGGEST_MAX_BYTES + 64 * 1024) as usize,
            )),
        )
        // search, usage, stats
        .route(r::SEARCH, get(misc::search))
        .route(r::USAGE, post(misc::usage))
        .route(r::POPULAR, get(misc::popular))
        .route(r::STATS_ME, get(misc::stats_me))
        .route(r::PROFILES, get(misc::profiles))
        .route(r::JOB, get(misc::job))
        // admin
        .route(r::ADMIN_USERS, get(admin::list_users).post(admin::create_user))
        .route(
            r::ADMIN_USER,
            axum::routing::patch(admin::update_user).delete(admin::delete_user),
        )
        .route(
            r::ADMIN_INVITATIONS,
            get(admin::list_invitations).post(admin::create_invitation),
        )
        .route(r::ADMIN_INVITATION, axum::routing::delete(admin::delete_invitation))
        .route(r::ADMIN_AUDIT, get(admin::audit))
        .route(r::ADMIN_STORAGE, get(admin::storage))
        .route(r::ADMIN_JOBS, get(admin::jobs))
        .route("/api/{*rest}", axum::routing::any(api_not_found))
}

async fn api_not_found() -> AppError {
    AppError::not_found()
}

/// Turn error responses produced outside our handlers (axum extractor
/// rejections, 405s, body-limit 413s) into [`stickpile_api::ApiError`] JSON
/// for `/api/` paths.
pub async fn normalize_errors(req: Request, next: Next) -> Response {
    let is_api = req.uri().path().starts_with("/api/");
    let resp = next.run(req).await;
    let status = resp.status();
    if !is_api || !(status.is_client_error() || status.is_server_error()) {
        return resp;
    }
    let is_json = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if is_json {
        return resp;
    }
    let (parts, body) = resp.into_parts();
    let text = axum::body::to_bytes(body, 4096)
        .await
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .unwrap_or_default();
    let message = if text.is_empty() {
        status.canonical_reason().unwrap_or("error").to_lowercase()
    } else {
        text
    };
    let mut out = AppError::new(status, code_for_status(status), message).into_response();
    for (k, v) in parts.headers.iter() {
        if k != header::CONTENT_TYPE && k != header::CONTENT_LENGTH {
            out.headers_mut().insert(k.clone(), v.clone());
        }
    }
    out
}

/// `Content-Disposition` value with an ASCII-safe filename.
pub fn disposition(kind: &str, filename: &str) -> HeaderValue {
    let safe: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    HeaderValue::from_str(&format!("{kind}; filename=\"{safe}\"")).unwrap_or(HeaderValue::from_static("inline"))
}

pub fn empty(status: StatusCode) -> Response {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = status;
    r
}
