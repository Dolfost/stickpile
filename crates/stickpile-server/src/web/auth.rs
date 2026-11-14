//! Sign in (password, then TOTP when the account needs it), sign out and
//! self-service registration.
//!
//! Login and registration posts have no session yet, so they are protected
//! by the same-origin check ([`check_same_origin`]) plus `SameSite=Lax`.
//! Between the password and the TOTP step the server keeps the pending
//! login in memory under a random one-time token (5 minutes, 5 attempts);
//! the password never goes back to the browser.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{FromRequestParts, Query, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use maud::{html, Markup};
use serde::Deserialize;
use stickpile_api::RegisterRequest;

use super::layout::{self, Nav};
use super::{safe_next, Ctx};
use crate::auth::login::{self, LoginPurpose};
use crate::auth::{session, AuthMethod, AuthOutcome, ClientIp, CurrentUser};
use crate::config::Registration;
use crate::db::UserRow;
use crate::error::{AppError, AppResult};
use crate::i18n::{t, t_dyn};
use crate::ids::{random_base32, sha256_hex};
use crate::security::check_same_origin;
use crate::state::AppState;

/// The caller if authenticated; never fails (unlike `MaybeUser`, which
/// rejects invalid credentials and would bounce between error and login).
pub struct OptionalUser(pub Option<CurrentUser>);

impl<S: Send + Sync> FromRequestParts<S> for OptionalUser {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(OptionalUser(
            parts
                .extensions
                .get::<AuthOutcome>()
                .and_then(|o| o.result.as_ref().ok().cloned().flatten()),
        ))
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct NextQuery {
    #[serde(default)]
    pub next: Option<String>,
}

// ---------------------------------------------------------------------------
// Pending TOTP logins
// ---------------------------------------------------------------------------

const PENDING_TTL: Duration = Duration::from_secs(300);
const PENDING_MAX_ATTEMPTS: u8 = 5;
const PENDING_CAP: usize = 10_000;

struct Pending {
    username: String,
    password: String,
    next: String,
    created: Instant,
    attempts: u8,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn pending_insert(p: Pending) -> String {
    let token = random_base32(32);
    let mut map = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|_, v| v.created.elapsed() < PENDING_TTL);
    if map.len() >= PENDING_CAP {
        if let Some(oldest) = map.iter().min_by_key(|(_, v)| v.created).map(|(k, _)| k.clone()) {
            map.remove(&oldest);
        }
    }
    map.insert(token.clone(), p);
    token
}

/// (username, password, next) of a live pending login.
fn pending_get(token: &str) -> Option<(String, String, String)> {
    let map = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    map.get(token)
        .filter(|p| p.created.elapsed() < PENDING_TTL && p.attempts < PENDING_MAX_ATTEMPTS)
        .map(|p| (p.username.clone(), p.password.clone(), p.next.clone()))
}

fn pending_remove(token: &str) {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).remove(token);
}

/// Count a failed code; false once the attempts are used up.
fn pending_fail(token: &str) -> bool {
    let mut map = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    match map.get_mut(token) {
        Some(p) => {
            p.attempts += 1;
            if p.attempts >= PENDING_MAX_ATTEMPTS {
                map.remove(token);
                false
            } else {
                true
            }
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

pub fn login_form(username: &str, next: &str, error: Option<&str>) -> Markup {
    let body = html! {
        section.auth-box {
            h1 { (t("Sign in")) }
            @if let Some(e) = error { (layout::alert("error", e)) }
            form.stack method="post" action="/login" {
                input type="hidden" name="next" value=(next);
                div.field-row {
                    label for="login-username" { (t("Username")) }
                    input #login-username type="text" name="username" value=(username) required
                        autocomplete="username" autocapitalize="none" spellcheck="false" maxlength="64" autofocus;
                }
                div.field-row {
                    label for="login-password" { (t("Password")) }
                    input #login-password type="password" name="password" required autocomplete="current-password";
                }
                button.primary type="submit" { (t("Sign in")) }
            }
            p.muted {
                (t("On the tailnet? Open stickpile through its tailnet address and you are signed in automatically."))
                " " a href="/register" { (t("Create an account")) } " " (t("if the server allows it."))
            }
            // No email here: the ways back in are the tailnet, an admin, or
            // the operator's command (docs/deployment.md).
            details.forgot {
                summary { (t("Forgot your password?")) }
                ul {
                    li {
                        (t("If your account has a tailnet login, open stickpile through its tailnet address from one of your own devices: you are signed in, and Settings sets a new password without the old one."))
                    }
                    li { (t("Otherwise ask an administrator: the admin panel resets passwords.")) }
                    li {
                        (t("Whoever runs the server can set one with")) " "
                        code { "stickpile-server reset-password <username> --password-file <file>" } "."
                    }
                }
            }
        }
    };
    layout::page(None, t("Sign in"), Nav::None, body)
}

fn totp_form(token: &str, error: Option<&str>) -> Markup {
    let body = html! {
        section.auth-box {
            h1 { (t("Two-step verification")) }
            @if let Some(e) = error { (layout::alert("error", e)) }
            p { (t("Enter the 6-digit code from your authenticator app.")) }
            form.stack method="post" action="/login/totp" {
                input type="hidden" name="token" value=(token);
                div.field-row {
                    label for="login-totp" { (t("Code")) }
                    input #login-totp type="text" name="code" required inputmode="numeric" pattern="[0-9 ]{6,7}"
                        maxlength="7" autocomplete="one-time-code" autofocus;
                }
                button.primary type="submit" { (t("Verify")) }
            }
            p.muted { a href="/login" { (t("Start over")) } }
        }
    };
    layout::page(None, t("Two-step verification"), Nav::None, body)
}

fn html_status(status: StatusCode, markup: Markup) -> Response {
    (status, markup).into_response()
}

fn local_next(next: Option<&str>) -> String {
    safe_next(next)
        .filter(|n| !n.starts_with("/login") && !n.starts_with("/logout"))
        .unwrap_or("/")
        .to_string()
}

pub async fn login_page(OptionalUser(user): OptionalUser, Query(q): Query<NextQuery>) -> Response {
    let next = local_next(q.next.as_deref());
    if user.is_some() {
        return Redirect::to(&next).into_response();
    }
    login_form("", &next, None).into_response()
}

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    next: Option<String>,
}

/// Create a web session and redirect to `next`; replaces a session cookie
/// the browser already had.
async fn start_session(
    state: &AppState,
    user: &UserRow,
    ip: Option<std::net::IpAddr>,
    headers: &HeaderMap,
    next: &str,
) -> AppResult<Response> {
    if let Some(old) = session::secret_from_cookies(headers) {
        session::destroy(&state.db, &sha256_hex(old.as_bytes())).await?;
    }
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok());
    let ip_s = ip.map(|i| i.to_string());
    let created = session::create(state, user, ip_s.as_deref(), ua).await?;
    let mut resp = Redirect::to(next).into_response();
    resp.headers_mut().insert(header::SET_COOKIE, created.cookie);
    Ok(resp)
}

pub async fn login_submit(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Form(f): Form<LoginForm>,
) -> AppResult<Response> {
    check_same_origin(&state, &headers)?;
    let next = local_next(f.next.as_deref());
    match login::password_login(&state, &f.username, &f.password, None, ip, LoginPurpose::WebSession).await {
        Ok(user) => start_session(&state, &user, ip, &headers, &next).await,
        Err(e) if e.code == "totp_required" => {
            let token = pending_insert(Pending {
                username: f.username.clone(),
                password: f.password.clone(),
                next,
                created: Instant::now(),
                attempts: 0,
            });
            Ok(totp_form(&token, None).into_response())
        }
        Err(e) if e.status.is_server_error() => Err(e),
        Err(e) => {
            let msg = if e.code == "rate_limited" {
                t("Too many sign-in attempts. Wait a minute and try again.")
            } else {
                t("Wrong username or password.")
            };
            Ok(html_status(e.status, login_form(&f.username, &next, Some(msg))))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TotpForm {
    #[serde(default)]
    token: String,
    #[serde(default)]
    code: String,
}

pub async fn login_totp(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Form(f): Form<TotpForm>,
) -> AppResult<Response> {
    check_same_origin(&state, &headers)?;
    let Some((username, password, next)) = pending_get(&f.token) else {
        return Ok(html_status(
            StatusCode::UNAUTHORIZED,
            login_form("", "/", Some(t("The sign-in expired. Please start again."))),
        ));
    };
    let code: String = f.code.chars().filter(|c| !c.is_whitespace()).collect();
    match login::password_login(&state, &username, &password, Some(&code), ip, LoginPurpose::WebSession).await {
        Ok(user) => {
            pending_remove(&f.token);
            start_session(&state, &user, ip, &headers, &next).await
        }
        Err(e) if e.status.is_server_error() => Err(e),
        Err(e) if e.code == "rate_limited" => Ok(html_status(
            e.status,
            totp_form(&f.token, Some(t("Too many attempts. Wait a minute and try again."))),
        )),
        Err(e) => {
            if pending_fail(&f.token) {
                Ok(html_status(
                    e.status,
                    totp_form(&f.token, Some(t("That code is not valid."))),
                ))
            } else {
                Ok(html_status(
                    e.status,
                    login_form(&username, &next, Some(t("Too many wrong codes. Please sign in again."))),
                ))
            }
        }
    }
}

pub async fn logout(State(state): State<AppState>, ctx: Ctx) -> AppResult<Response> {
    match &ctx.user.method {
        AuthMethod::Session { session_id, .. } => {
            session::destroy(&state.db, session_id).await?;
            let mut resp = ctx.redirect("/login");
            resp.headers_mut()
                .insert(header::SET_COOKIE, session::clear_cookie_header(&state));
            Ok(resp)
        }
        // Tailnet and token callers have no session to end.
        _ => Ok(ctx.redirect("/")),
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct RegisterQuery {
    #[serde(default)]
    code: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RegisterForm {
    #[serde(default)]
    username: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    password2: String,
    #[serde(default)]
    invite_code: String,
}

fn register_form(policy: Registration, min_pw: usize, f: &RegisterForm, error: Option<&str>) -> Markup {
    let body = html! {
        section.auth-box {
            h1 { (t("Create an account")) }
            @if policy == Registration::Closed {
                (layout::alert("info", t("Registration is closed on this server. Ask an administrator for an account or an invitation.")))
                p { a href="/login" { (t("Sign in")) } }
            } @else {
                @if let Some(e) = error { (layout::alert("error", e)) }
                form.stack method="post" action="/register" {
                    @if policy == Registration::Invite {
                        div.field-row {
                            label for="reg-code" { (t("Invitation code")) }
                            input #reg-code type="text" name="invite_code" value=(f.invite_code) required
                                autocomplete="off" spellcheck="false";
                        }
                    }
                    div.field-row {
                        label for="reg-username" { (t("Username")) }
                        input #reg-username type="text" name="username" value=(f.username) required minlength="2"
                            maxlength="32" pattern="[A-Za-z0-9][A-Za-z0-9_.\\-]{1,31}" autocomplete="username"
                            autocapitalize="none" spellcheck="false" aria-describedby="reg-username-hint";
                        p.hint #reg-username-hint { (t("2–32 letters, digits, _ . or -, starting with a letter or digit.")) }
                    }
                    div.field-row {
                        label for="reg-display" { (t("Display name (optional)")) }
                        input #reg-display type="text" name="display_name" value=(f.display_name) maxlength="100"
                            autocomplete="nickname";
                    }
                    div.field-row {
                        label for="reg-password" { (t("Password")) }
                        input #reg-password type="password" name="password" required minlength=(min_pw)
                            autocomplete="new-password" aria-describedby="reg-password-hint";
                        p.hint #reg-password-hint { (t("At least 8 characters.")) }
                    }
                    div.field-row {
                        label for="reg-password2" { (t("Repeat the password")) }
                        input #reg-password2 type="password" name="password2" required minlength=(min_pw)
                            autocomplete="new-password";
                    }
                    button.primary type="submit" { (t("Create account")) }
                }
                p.muted { (t("Already have an account?")) " " a href="/login" { (t("Sign in")) } }
            }
        }
    };
    layout::page(None, t("Create an account"), Nav::None, body)
}

pub async fn register_page(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    Query(q): Query<RegisterQuery>,
) -> Response {
    if user.is_some() {
        return Redirect::to("/").into_response();
    }
    let policy = state.config.auth.registration;
    let f = RegisterForm {
        invite_code: q.code.unwrap_or_default(),
        ..Default::default()
    };
    let status = if policy == Registration::Closed {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::OK
    };
    (
        status,
        register_form(policy, state.config.auth.min_password_length, &f, None),
    )
        .into_response()
}

pub async fn register_submit(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Form(f): Form<RegisterForm>,
) -> AppResult<Response> {
    check_same_origin(&state, &headers)?;
    let policy = state.config.auth.registration;
    if f.password != f.password2 {
        return Ok(html_status(
            StatusCode::BAD_REQUEST,
            register_form(
                policy,
                state.config.auth.min_password_length,
                &f,
                Some(t("The passwords do not match.")),
            ),
        ));
    }
    let req = RegisterRequest {
        username: f.username.trim().to_string(),
        password: f.password.clone(),
        display_name: Some(f.display_name.trim().to_string()).filter(|d| !d.is_empty()),
        invite_code: Some(f.invite_code.trim().to_string()).filter(|c| !c.is_empty()),
    };
    match login::register(&state, req, ip).await {
        Ok(user) => start_session(&state, &user, ip, &headers, "/").await,
        Err(e) if e.status.is_server_error() => Err(e),
        Err(e) => {
            let msg = register_message(&e);
            let f = RegisterForm {
                password: String::new(),
                password2: String::new(),
                ..f
            };
            Ok(html_status(
                e.status,
                register_form(policy, state.config.auth.min_password_length, &f, Some(&msg)),
            ))
        }
    }
}

fn register_message(e: &AppError) -> String {
    match e.code {
        "conflict" => t("That username is taken.").to_string(),
        "rate_limited" => t("Too many attempts. Wait a minute and try again.").to_string(),
        _ => {
            // In the request's language when the catalog has the message.
            let mut m = super::settings::capitalize(&t_dyn(&e.message));
            if !m.ends_with('.') {
                m.push('.');
            }
            m
        }
    }
}
