//! Authentication (§6, §9).
//!
//! [`auth_middleware`] resolves the caller once per request and stores the
//! outcome in the request extensions; the [`CurrentUser`], [`MaybeUser`] and
//! [`AdminUser`] extractors read it. Precedence:
//!
//! 1. `Authorization: Bearer spk_...` — if the header is present it is the
//!    only credential considered; an invalid token is a 401 (no fallback).
//! 2. Tailnet identity (`Tailscale-User-Login`) — only on requests from the
//!    trusted proxy; an unknown login without auto-create falls through.
//! 3. The `stickpile_session` cookie.
//!
//! CSRF: cookie-authenticated unsafe requests (POST/PUT/PATCH/DELETE) need
//! `X-CSRF-Token` (or a `csrf_token` field in an urlencoded form body) equal to
//! the session's token, plus a same-origin `Origin`/`Sec-Fetch-Site`. Tailnet
//! identity is ambient too (the proxy attaches it to any request from the
//! device, including a cross-site form post), so tailscale-authenticated unsafe
//! requests must pass the same-origin check. Bearer requests are exempt.

pub mod login;
pub mod password;
pub mod session;
pub mod tailscale;
pub mod tokens;
pub mod totp;

use std::net::IpAddr;

use axum::body::Body;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

use crate::db::UserRow;
use crate::error::{AppError, AppResult};
use crate::security::{check_same_origin, client_ip, from_trusted_proxy, is_unsafe_method, Peer};
use crate::state::AppState;

pub const CSRF_HEADER: &str = "x-csrf-token";
pub const CSRF_FORM_FIELD: &str = "csrf_token";
/// Largest urlencoded body buffered to look for `csrf_token`.
const FORM_BUFFER_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub enum AuthMethod {
    Bearer { token_id: String },
    Tailscale { login: String },
    Session { session_id: String, csrf_token: String },
}

/// The authenticated caller.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub user: UserRow,
    pub method: AuthMethod,
    pub ip: Option<IpAddr>,
}

impl CurrentUser {
    pub fn id(&self) -> &str {
        &self.user.id
    }
    pub fn is_admin(&self) -> bool {
        self.user.is_admin()
    }
    /// The session's CSRF token, for forms and `hx-headers` in templates.
    /// `None` for bearer and tailnet callers (they need none).
    pub fn csrf_token(&self) -> Option<&str> {
        match &self.method {
            AuthMethod::Session { csrf_token, .. } => Some(csrf_token),
            _ => None,
        }
    }
    /// Session id (sha256 of the cookie), for logout.
    pub fn session_id(&self) -> Option<&str> {
        match &self.method {
            AuthMethod::Session { session_id, .. } => Some(session_id),
            _ => None,
        }
    }
    pub fn ip_string(&self) -> Option<String> {
        self.ip.map(|i| i.to_string())
    }
}

/// Stored in the request extensions by [`auth_middleware`].
#[derive(Clone)]
pub struct AuthOutcome {
    pub result: Result<Option<CurrentUser>, AppError>,
    /// `Err` if this is an unsafe request that failed the CSRF/origin check.
    pub csrf: Result<(), AppError>,
}

/// Client IP (after trusted-proxy `X-Forwarded-For` handling).
#[derive(Debug, Clone, Copy)]
pub struct ClientIp(pub Option<IpAddr>);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.extensions.get::<ClientIp>().copied().unwrap_or(ClientIp(None)))
    }
}

impl<S: Send + Sync> FromRequestParts<S> for CurrentUser {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, AppError> {
        let outcome = parts.extensions.get::<AuthOutcome>().cloned();
        let Some(outcome) = outcome else {
            return Err(AppError::unauthorized("authentication required"));
        };
        let user = outcome
            .result?
            .ok_or_else(|| AppError::unauthorized("authentication required"))?;
        outcome.csrf?;
        Ok(user)
    }
}

/// Optional authentication: `None` for anonymous callers; invalid
/// credentials and CSRF failures are still errors.
pub struct MaybeUser(pub Option<CurrentUser>);

impl<S: Send + Sync> FromRequestParts<S> for MaybeUser {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, AppError> {
        let Some(outcome) = parts.extensions.get::<AuthOutcome>().cloned() else {
            return Ok(MaybeUser(None));
        };
        match outcome.result? {
            Some(u) => {
                outcome.csrf?;
                Ok(MaybeUser(Some(u)))
            }
            None => Ok(MaybeUser(None)),
        }
    }
}

/// An authenticated admin; authenticated non-admins get 403.
pub struct AdminUser(pub CurrentUser);

impl<S: Send + Sync> FromRequestParts<S> for AdminUser {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, s: &S) -> Result<Self, AppError> {
        let u = CurrentUser::from_request_parts(parts, s).await?;
        if !u.is_admin() {
            return Err(AppError::forbidden("admin role required"));
        }
        Ok(AdminUser(u))
    }
}

/// Resolves the caller for every request (see the module docs).
pub async fn auth_middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if path.starts_with("/static/") || path == stickpile_api::routes::HEALTH {
        return next.run(req).await;
    }
    let peer = Peer::from_request(&req);
    let ip = client_ip(&state, &peer, req.headers());
    let (mut req, outcome, refreshed) = resolve(&state, req, &peer, ip).await;
    req.extensions_mut().insert(ClientIp(ip));
    req.extensions_mut().insert(outcome);
    let mut resp = next.run(req).await;
    if let Some(cookie) = refreshed {
        let already = resp
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|v| v.as_bytes().starts_with(session::COOKIE_NAME.as_bytes()));
        if !already {
            resp.headers_mut().append(header::SET_COOKIE, cookie);
        }
    }
    resp
}

fn parse_bearer(v: &HeaderValue) -> Option<&str> {
    let s = v.to_str().ok()?;
    let (scheme, token) = s.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

fn outcome(result: AppResult<Option<CurrentUser>>, csrf: AppResult<()>) -> AuthOutcome {
    AuthOutcome { result, csrf }
}

async fn resolve(
    state: &AppState,
    req: Request,
    peer: &Peer,
    ip: Option<IpAddr>,
) -> (Request, AuthOutcome, Option<HeaderValue>) {
    let ip_s = ip.map(|i| i.to_string());
    let unsafe_method = is_unsafe_method(req.method());

    // 1. Bearer token.
    if let Some(v) = req.headers().get(header::AUTHORIZATION) {
        let result = match parse_bearer(v) {
            Some(secret) => match tokens::authenticate(&state.db, secret).await {
                Ok(Some((tok, user))) => Ok(Some(CurrentUser {
                    user,
                    method: AuthMethod::Bearer { token_id: tok.id },
                    ip,
                })),
                Ok(None) => Err(AppError::unauthorized("invalid, expired or revoked token")),
                Err(e) => Err(e),
            },
            None => Err(AppError::unauthorized("unsupported Authorization scheme (use Bearer)")),
        };
        return (req, outcome(result, Ok(())), None);
    }

    // 2. Tailnet identity from the trusted proxy.
    if from_trusted_proxy(state, peer, req.headers()) {
        if let Some(id) = tailscale::identity(req.headers()) {
            match tailscale::resolve(state, &id, ip_s.as_deref()).await {
                Ok(tailscale::Mapped::User(user)) => {
                    let csrf = if unsafe_method {
                        check_same_origin(state, req.headers())
                    } else {
                        Ok(())
                    };
                    let cu = CurrentUser {
                        user: *user,
                        method: AuthMethod::Tailscale { login: id.login },
                        ip,
                    };
                    return (req, outcome(Ok(Some(cu)), csrf), None);
                }
                Ok(tailscale::Mapped::Disabled) => {
                    return (
                        req,
                        outcome(Err(AppError::unauthorized("account disabled")), Ok(())),
                        None,
                    );
                }
                Ok(tailscale::Mapped::Unknown) => {}
                Err(e) => return (req, outcome(Err(e), Ok(())), None),
            }
        }
    }

    // 3. Session cookie.
    let Some(secret) = session::secret_from_cookies(req.headers()) else {
        return (req, outcome(Ok(None), Ok(())), None);
    };
    let resolved = match session::resolve(state, &secret).await {
        Ok(Some(r)) => r,
        Ok(None) => return (req, outcome(Ok(None), Ok(())), None),
        Err(e) => return (req, outcome(Err(e), Ok(())), None),
    };
    let expected = resolved.session.csrf_token.clone();
    let (req, csrf) = if unsafe_method {
        check_csrf(state, req, &expected).await
    } else {
        (req, Ok(()))
    };
    let cu = CurrentUser {
        user: resolved.user,
        method: AuthMethod::Session {
            session_id: resolved.session.id,
            csrf_token: expected,
        },
        ip,
    };
    (req, outcome(Ok(Some(cu)), csrf), resolved.refreshed_cookie)
}

/// Token from `X-CSRF-Token`, else from a `csrf_token` field of an urlencoded
/// body (buffered up to 64 KiB and put back), then the same-origin check.
async fn check_csrf(state: &AppState, req: Request, expected: &str) -> (Request, AppResult<()>) {
    let header_token = req
        .headers()
        .get(CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let (req, token) = match header_token {
        Some(t) => (req, Some(t)),
        None if is_urlencoded(&req) => {
            let (parts, body) = req.into_parts();
            match axum::body::to_bytes(body, FORM_BUFFER_LIMIT).await {
                Ok(bytes) => {
                    let token = form_urlencoded::parse(&bytes)
                        .find(|(k, _)| k == CSRF_FORM_FIELD)
                        .map(|(_, v)| v.into_owned());
                    (Request::from_parts(parts, Body::from(bytes)), token)
                }
                Err(_) => {
                    let req = Request::from_parts(parts, Body::empty());
                    return (req, Err(AppError::too_large("form body too large")));
                }
            }
        }
        None => (req, None),
    };
    let result = match token {
        Some(t) if crate::ids::ct_eq(&t, expected) => check_same_origin(state, req.headers()),
        Some(_) => Err(AppError::csrf("CSRF token mismatch")),
        None => Err(AppError::csrf("missing CSRF token (X-CSRF-Token header)")),
    };
    (req, result)
}

fn is_urlencoded(req: &Request) -> bool {
    req.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.trim_start()
                .to_ascii_lowercase()
                .starts_with("application/x-www-form-urlencoded")
        })
}
