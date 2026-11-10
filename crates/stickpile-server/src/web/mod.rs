//! Server-rendered web UI: maud templates plus htmx, no SPA and no
//! JavaScript toolchain. Every page works without JavaScript (forms post and
//! redirect); htmx swaps fragments in place when it is loaded, and
//! `assets/app.js` adds clipboard copy and drag-and-drop.
//!
//! How pages plug into the rest of the server:
//!
//! * Authentication: handlers take [`Ctx`] (an authenticated caller plus
//!   request facts) or [`AdminCtx`]. Anonymous visitors are redirected to
//!   `/login` (htmx requests get `401` with `HX-Redirect`) by
//!   [`html_errors`], which also renders every other error as an HTML page
//!   or, for htmx requests, as a message in the page's `#flash` region.
//! * CSRF: forms carry `<input type=hidden name=csrf_token>` (checked by the
//!   auth middleware for urlencoded bodies); htmx sends `X-CSRF-Token` from
//!   `hx-headers` on `<body>`. Multipart forms (uploads) put the token first
//!   and are verified by [`MultipartCaller`]. Login and registration have no
//!   session yet and call [`crate::security::check_same_origin`].
//! * Business logic and authorization: the same [`crate::service`]
//!   functions as the JSON API; UI-only queries live in
//!   [`crate::service::ui`].
//! * CSP: no inline scripts or styles anywhere (`style=` attributes
//!   included); styling is class-based (`assets/app.css`).

mod admin;
mod auth;
mod components;
mod home;
mod integrations;
mod items;
mod layout;
mod popular;
mod search;
mod sets;
mod settings;
mod stats;
mod upload;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Multipart, Request};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{from_fn, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use maud::{html, Markup};
use stickpile_api::ApiError;

use crate::auth::{AuthMethod, AuthOutcome, CurrentUser, CSRF_FORM_FIELD, CSRF_HEADER};
use crate::error::{code_for_status, AppError, AppResult};
use crate::security::check_same_origin;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let big = || DefaultBodyLimit::disable();
    Router::new()
        .route("/", get(home::page))
        // auth
        .route("/login", get(auth::login_page).post(auth::login_submit))
        .route("/login/totp", post(auth::login_totp))
        .route("/logout", post(auth::logout))
        .route("/register", get(auth::register_page).post(auth::register_submit))
        // sets
        .route("/sets", get(sets::list).post(sets::create))
        .route("/sets/new", get(sets::new_page))
        .route("/sets/{id}", get(sets::page))
        .route(
            "/sets/{id}/settings",
            get(sets::settings_page).post(sets::settings_submit),
        )
        .route("/sets/{id}/delete", get(sets::delete_page).post(sets::delete_submit))
        .route("/sets/{id}/items/{item}/move", post(sets::move_item))
        .route("/sets/{id}/upload", post(upload::upload_to_set).layer(big()))
        .route("/sets/{id}/import", post(upload::import_into).layer(big()))
        // items
        .route("/items/{id}", get(items::page).post(items::update_all))
        .route(
            "/items/{id}/field/{field}",
            get(items::field_view).post(items::field_save),
        )
        .route("/items/{id}/field/{field}/edit", get(items::field_edit))
        .route("/items/{id}/confirm/{field}", post(items::confirm))
        .route("/items/{id}/like", post(items::like))
        .route("/items/{id}/delete", get(items::delete_page).post(items::delete_submit))
        .route("/items/{id}/enrich", post(items::enrich))
        .route("/items/{id}/better", get(items::better))
        .route("/items/{id}/move", post(items::move_to_set))
        // upload, import, search, popular, stats
        .route("/upload", get(upload::page).post(upload::upload_any).layer(big()))
        .route("/import", post(upload::import_new).layer(big()))
        .route("/search", get(search::page))
        .route("/popular", get(popular::page))
        .route("/stats", get(stats::page))
        // settings
        .route("/settings", get(settings::page).post(settings::profile))
        .route("/settings/password", post(settings::password))
        .route("/settings/totp", post(settings::totp_begin))
        .route("/settings/totp/confirm", post(settings::totp_confirm))
        .route("/settings/totp/disable", post(settings::totp_disable))
        .route("/settings/tokens", post(settings::token_create))
        .route("/settings/tokens/{id}/revoke", post(settings::token_revoke))
        // admin
        .route("/admin", get(admin::overview))
        .route("/admin/users", get(admin::users).post(admin::create_user))
        .route("/admin/users/{id}", post(admin::update_user))
        .route(
            "/admin/users/{id}/delete",
            get(admin::delete_user_page).post(admin::delete_user),
        )
        .route(
            "/admin/invitations",
            get(admin::invitations).post(admin::create_invitation),
        )
        .route("/admin/invitations/{id}/delete", post(admin::delete_invitation))
        .route("/admin/audit", get(admin::audit))
        // jobs and integrations
        .route("/jobs/{id}", get(integrations::job_status))
        .merge(integrations::routes())
        .fallback(not_found)
        .layer(from_fn(html_errors))
}

async fn not_found() -> AppError {
    AppError::not_found()
}

// ---------------------------------------------------------------------------
// Request context
// ---------------------------------------------------------------------------

/// An authenticated page request: the caller plus what templates need to
/// know about the request itself.
pub struct Ctx {
    pub user: CurrentUser,
    /// The request came from htmx (`HX-Request`): answer with a fragment.
    pub htmx: bool,
    /// Path and query of this request (for `next` fields).
    pub here: String,
}

impl Ctx {
    pub fn csrf(&self) -> Option<&str> {
        self.user.csrf_token()
    }
    /// Hidden CSRF field for forms (nothing for tailnet callers).
    pub fn csrf_input(&self) -> Markup {
        layout::csrf_input(self.csrf())
    }
    pub fn id(&self) -> &str {
        self.user.id()
    }
    /// A full page around `body`.
    pub fn page(&self, title: &str, nav: layout::Nav, body: Markup) -> Markup {
        layout::page(Some(&self.user), title, nav, body)
    }
    /// `fragment` for htmx, otherwise a full page around it.
    pub fn page_or_fragment(&self, title: &str, nav: layout::Nav, fragment: Markup) -> Markup {
        if self.htmx {
            fragment
        } else {
            self.page(title, nav, fragment)
        }
    }
    /// After a successful form action: the fragment for htmx, a 303 to `next`
    /// (if local) or `fallback` otherwise.
    pub fn done(&self, fragment: Markup, next: Option<&str>, fallback: &str) -> Response {
        if self.htmx {
            fragment.into_response()
        } else {
            Redirect::to(safe_next(next).unwrap_or(fallback)).into_response()
        }
    }
    /// Navigate to `location`: `HX-Redirect` for htmx, 303 otherwise.
    pub fn redirect(&self, location: &str) -> Response {
        redirect(self.htmx, location)
    }
}

pub fn redirect(htmx: bool, location: &str) -> Response {
    if htmx {
        let mut r = StatusCode::OK.into_response();
        if let Ok(v) = HeaderValue::from_str(location) {
            r.headers_mut().insert("hx-redirect", v);
        }
        r
    } else {
        Redirect::to(location).into_response()
    }
}

/// An htmx request that wants a fragment. History restoration requests
/// (back button after `hx-push-url`) need the full page.
pub fn is_htmx(headers: &HeaderMap) -> bool {
    headers.get("hx-request").is_some_and(|v| v == "true")
        && headers.get("hx-history-restore-request").is_none_or(|v| v != "true")
}

/// The page the user is on: for htmx requests the page that made the
/// request (`HX-Current-URL`), so `next` fields in fragments point back to
/// it; otherwise this request's path and query.
fn here(parts: &Parts) -> String {
    if is_htmx(&parts.headers) {
        let current = parts
            .headers
            .get("hx-current-url")
            .and_then(|v| v.to_str().ok())
            .and_then(|u| u.split_once("://"))
            .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
            .map(|p| p.split('#').next().unwrap_or(p));
        if let Some(p) = safe_next(current) {
            return p.to_string();
        }
    }
    parts
        .uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".into())
}

impl<S: Send + Sync> FromRequestParts<S> for Ctx {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, s: &S) -> Result<Self, AppError> {
        let user = CurrentUser::from_request_parts(parts, s).await?;
        Ok(Ctx {
            user,
            htmx: is_htmx(&parts.headers),
            here: here(parts),
        })
    }
}

/// [`Ctx`] when signed in, `None` for anonymous visitors (and for invalid
/// credentials, which then just see the sign-in form). CSRF failures are
/// still errors.
pub struct MaybeCtx(pub Option<Ctx>);

impl<S: Send + Sync> FromRequestParts<S> for MaybeCtx {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, s: &S) -> Result<Self, AppError> {
        match Ctx::from_request_parts(parts, s).await {
            Ok(ctx) => Ok(MaybeCtx(Some(ctx))),
            Err(e) if e.status == StatusCode::UNAUTHORIZED => Ok(MaybeCtx(None)),
            Err(e) => Err(e),
        }
    }
}

/// [`Ctx`] of an admin; other users get 403.
pub struct AdminCtx(pub Ctx);

impl<S: Send + Sync> FromRequestParts<S> for AdminCtx {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, s: &S) -> Result<Self, AppError> {
        let ctx = Ctx::from_request_parts(parts, s).await?;
        if !ctx.user.is_admin() {
            return Err(AppError::forbidden("the admin panel needs the admin role"));
        }
        Ok(AdminCtx(ctx))
    }
}

impl std::ops::Deref for AdminCtx {
    type Target = Ctx;
    fn deref(&self) -> &Ctx {
        &self.0
    }
}

/// `next` if it is a local path (`/...`, not `//host` or `/\host`).
pub fn safe_next(next: Option<&str>) -> Option<&str> {
    let n = next?.trim();
    let ok = n.starts_with('/')
        && !n.starts_with("//")
        && !n.contains('\\')
        && n.len() <= 2000
        && !n.chars().any(char::is_control);
    ok.then_some(n)
}

// ---------------------------------------------------------------------------
// Multipart forms
// ---------------------------------------------------------------------------

/// Caller of a multipart form POST (uploads, imports).
///
/// A plain browser form cannot add the `X-CSRF-Token` header, and the auth
/// middleware only reads the token from urlencoded bodies. For cookie
/// sessions without the header the token must therefore be the first
/// multipart field: [`MultipartCaller::multipart`] checks it before any
/// file is read, after the same-origin check here. htmx and `app.js` send
/// the header and are checked by the middleware as usual.
pub struct MultipartCaller {
    pub ctx: Ctx,
    form_token: bool,
}

impl FromRequestParts<AppState> for MultipartCaller {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let outcome = parts
            .extensions
            .get::<AuthOutcome>()
            .cloned()
            .ok_or_else(|| AppError::unauthorized("authentication required"))?;
        let user = outcome
            .result?
            .ok_or_else(|| AppError::unauthorized("authentication required"))?;
        let form_token = match outcome.csrf {
            Ok(()) => false,
            Err(e) => {
                let session = matches!(user.method, AuthMethod::Session { .. });
                let multipart = parts
                    .headers
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("multipart/form-data"));
                if session && multipart && e.code == "csrf" && !parts.headers.contains_key(CSRF_HEADER) {
                    check_same_origin(state, &parts.headers)?;
                    true
                } else {
                    return Err(e);
                }
            }
        };
        Ok(MultipartCaller {
            ctx: Ctx {
                user,
                htmx: is_htmx(&parts.headers),
                here: here(parts),
            },
            form_token,
        })
    }
}

impl MultipartCaller {
    /// The request body as multipart, capped at `limits.max_request_mb`
    /// (413 beyond), with the form CSRF token verified when needed.
    pub async fn multipart(&self, state: &AppState, req: Request) -> AppResult<Multipart> {
        let limit = state.config.limits.max_request_mb * 1024 * 1024;
        let declared = req
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if declared.is_some_and(|n| n > limit) {
            return Err(AppError::too_large(format!(
                "request larger than {} MiB",
                state.config.limits.max_request_mb
            )));
        }
        let (parts, body) = req.into_parts();
        let body = Body::new(http_body_util::Limited::new(body, limit as usize));
        let mut mp = Multipart::from_request(Request::from_parts(parts, body), state)
            .await
            .map_err(|e| AppError::bad_request(e.body_text()))?;
        if self.form_token {
            let expected = self.ctx.csrf().unwrap_or_default().to_string();
            let mut field = mp
                .next_field()
                .await
                .map_err(crate::service::upload::multipart_error)?
                .ok_or_else(|| AppError::csrf("missing CSRF token"))?;
            if field.name() != Some(CSRF_FORM_FIELD) {
                return Err(AppError::csrf("missing CSRF token (it must be the first form field)"));
            }
            let token = crate::service::upload::field_text(&mut field, 256).await?;
            if !crate::ids::ct_eq(token.trim(), &expected) {
                return Err(AppError::csrf("CSRF token mismatch"));
            }
        }
        Ok(mp)
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Renders error responses of web routes as HTML: anonymous callers are sent
/// to `/login`; htmx requests get a message retargeted into `#flash`;
/// everything else gets an error page. Responses that already are HTML (a
/// form re-rendered with its error) pass through.
pub async fn html_errors(req: Request, next: Next) -> Response {
    let htmx = is_htmx(req.headers());
    let method = req.method().clone();
    let target = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".into());
    let viewer = req
        .extensions()
        .get::<AuthOutcome>()
        .and_then(|o| o.result.as_ref().ok().cloned().flatten());
    let resp = next.run(req).await;
    let status = resp.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return resp;
    }
    let is_html = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if is_html {
        return resp;
    }
    let (parts, body) = resp.into_parts();
    let bytes = axum::body::to_bytes(body, 16 * 1024).await.unwrap_or_default();
    let (code, message) = match serde_json::from_slice::<ApiError>(&bytes) {
        Ok(e) => (e.code, e.message),
        Err(_) => {
            let text = String::from_utf8_lossy(&bytes).trim().to_string();
            let text = if text.is_empty() {
                status.canonical_reason().unwrap_or("error").to_lowercase()
            } else {
                text
            };
            (code_for_status(status).to_string(), text)
        }
    };
    let mut out = if status == StatusCode::UNAUTHORIZED {
        login_redirect(htmx, &method, &target)
    } else if htmx {
        let mut r = (status, layout::flash_error(&message)).into_response();
        let h = r.headers_mut();
        h.insert("hx-retarget", HeaderValue::from_static("#flash"));
        h.insert("hx-reswap", HeaderValue::from_static("innerHTML"));
        r
    } else {
        (status, layout::error_page(viewer.as_ref(), status, &code, &message)).into_response()
    };
    if let Some(v) = parts.headers.get(header::RETRY_AFTER) {
        out.headers_mut().insert(header::RETRY_AFTER, v.clone());
    }
    out
}

fn login_redirect(htmx: bool, method: &Method, target: &str) -> Response {
    let location = if *method == Method::GET && target != "/" && safe_next(Some(target)).is_some() {
        format!(
            "/login?next={}",
            form_urlencoded::byte_serialize(target.as_bytes()).collect::<String>()
        )
    } else {
        "/login".to_string()
    };
    if htmx {
        let mut r = (
            StatusCode::UNAUTHORIZED,
            html! { p { "Your session has ended. " a href="/login" { "Sign in again" } "." } },
        )
            .into_response();
        r.headers_mut()
            .insert("hx-redirect", HeaderValue::from_static("/login"));
        r
    } else {
        Redirect::to(&location).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::safe_next;

    #[test]
    fn next_must_be_local() {
        assert_eq!(safe_next(Some("/sets/abc?x=1")), Some("/sets/abc?x=1"));
        assert_eq!(safe_next(Some("//evil.example")), None);
        assert_eq!(safe_next(Some("/\\evil.example")), None);
        assert_eq!(safe_next(Some("https://evil.example")), None);
        assert_eq!(safe_next(Some("/a\nb")), None);
        assert_eq!(safe_next(None), None);
    }
}
