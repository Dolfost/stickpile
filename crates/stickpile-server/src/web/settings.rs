//! Account settings: profile (display name, Telegram user id), password,
//! TOTP enrolment (QR code as inline SVG) and API tokens.

use std::fmt::Write as _;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Form;
use maud::{html, Markup};
use serde::Deserialize;
use stickpile_api::{ChangePassword, UpdateMe};

use super::components::{opt_time, time_el, trusted};
use super::layout::{self, Nav};
use super::Ctx;
use crate::auth::AuthMethod;
use crate::db::UserRow;
use crate::error::{AppError, AppResult};
use crate::ids::now;
use crate::service::{account, telegram, ui};
use crate::state::AppState;

/// QR code of `data` as an SVG (dark modules on white, 4-module quiet
/// zone), built from the module matrix: no `style` attributes.
pub fn qr_svg(data: &str) -> Option<String> {
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let w = code.width();
    let colors = code.to_colors();
    let quiet = 4;
    let size = w + 2 * quiet;
    let mut d = String::new();
    for y in 0..w {
        for x in 0..w {
            if colors[y * w + x] == qrcode::Color::Dark {
                let _ = write!(d, "M{},{}h1v1h-1z", x + quiet, y + quiet);
            }
        }
    }
    Some(format!(
        r##"<svg class="qr" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" width="220" height="220" role="img" aria-label="QR code for your authenticator app" shape-rendering="crispEdges"><rect width="{size}" height="{size}" fill="#ffffff"/><path d="{d}" fill="#000000"/></svg>"##
    ))
}

fn notice_for(done: Option<&str>) -> Option<&'static str> {
    Some(match done? {
        "profile" => "Profile saved.",
        "password" => "Password changed. Your other web sessions were signed out.",
        "totp-on" => "Two-step verification is on.",
        "totp-off" => "Two-step verification is off.",
        "revoked" => "Token revoked.",
        _ => return None,
    })
}

#[derive(Debug, Default, Deserialize)]
pub struct DoneQuery {
    #[serde(default)]
    done: Option<String>,
}

/// Which section an error belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Profile,
    Password,
    Totp,
    Tokens,
}

async fn render(
    state: &AppState,
    ctx: &Ctx,
    notice: Option<&str>,
    error: Option<(Section, &str)>,
) -> AppResult<Markup> {
    let me: UserRow = ui::me(state, &ctx.user).await?;
    let tokens = account::list_tokens(state, &ctx.user).await?;
    let err = |s: Section| {
        html! { @if let Some((sec, msg)) = error { @if sec == s { (layout::alert("error", msg)) } } }
    };
    let tailnet = matches!(ctx.user.method, AuthMethod::Tailscale { .. });
    let t = now();
    let body = html! {
        h1 { "Settings" }
        @if let Some(n) = notice { (layout::alert("success", n)) }
        section #profile aria-labelledby="s-profile" {
            h2 #s-profile { "Profile" }
            (err(Section::Profile))
            dl.kv {
                dt { "Username" } dd { code { (me.username) } }
                dt { "Role" } dd { (me.role) }
                @if let Some(l) = &me.tailscale_login { dt { "Tailnet login" } dd { code { (l) } } }
            }
            form.stack.narrow method="post" action="/settings" {
                (ctx.csrf_input())
                div.field-row {
                    label for="s-display" { "Display name" }
                    input #s-display type="text" name="display_name" value=(me.display_name) maxlength="100";
                }
                div.field-row {
                    label for="s-tg" { "Telegram user id" }
                    input #s-tg type="text" inputmode="numeric" pattern="[0-9]*" name="telegram_user_id"
                        value=(me.telegram_user_id.map(|v| v.to_string()).unwrap_or_default()) aria-describedby="s-tg-hint";
                    p.hint #s-tg-hint {
                        "Numeric id of your Telegram account; sticker packs you publish belong to it. "
                        "Ask @userinfobot in Telegram for it. Empty clears it."
                        @if !telegram::enabled(state) { " (Telegram publishing is not enabled on this server.)" }
                    }
                }
                button.primary type="submit" { "Save profile" }
            }
        }
        section #password aria-labelledby="s-password" {
            h2 #s-password { "Password" }
            (err(Section::Password))
            @if tailnet && me.password_hash.is_none() {
                p.muted { "You sign in through the tailnet. Set a password to also sign in from elsewhere or to create tokens with the CLI's login." }
            }
            form.stack.narrow method="post" action="/settings/password" {
                (ctx.csrf_input())
                input.sr-only type="text" name="username" value=(me.username) autocomplete="username" tabindex="-1" aria-hidden="true";
                @if me.password_hash.is_some() {
                    div.field-row {
                        label for="s-cur" { "Current password" }
                        input #s-cur type="password" name="current_password" required autocomplete="current-password";
                    }
                }
                div.field-row {
                    label for="s-new" { "New password" }
                    input #s-new type="password" name="new_password" required minlength="8" autocomplete="new-password";
                }
                div.field-row {
                    label for="s-new2" { "Repeat the new password" }
                    input #s-new2 type="password" name="new_password2" required minlength="8" autocomplete="new-password";
                }
                button.primary type="submit" { "Change password" }
                p.hint { "Changing it signs out your other browser sessions. API tokens keep working; revoke them below if needed." }
            }
        }
        section #totp aria-labelledby="s-totp" {
            h2 #s-totp { "Two-step verification (TOTP)" }
            (err(Section::Totp))
            @if me.totp_secret.is_some() {
                p { "On. Signing in with a password also needs a code from your authenticator app." }
                form.inline-row method="post" action="/settings/totp/disable" {
                    (ctx.csrf_input())
                    label for="s-totp-off" { "Current code" }
                    input #s-totp-off type="text" name="code" required inputmode="numeric" pattern="[0-9 ]{6,7}" maxlength="7" autocomplete="one-time-code";
                    button.danger type="submit" { "Turn off" }
                }
            } @else {
                p { "Off. Add a code from an authenticator app to password sign-ins." }
                form.inline method="post" action="/settings/totp" {
                    (ctx.csrf_input())
                    button type="submit" { "Set up two-step verification" }
                }
            }
        }
        section #tokens aria-labelledby="s-tokens" {
            h2 #s-tokens { "API tokens" }
            p { "For the CLI, the desktop client and scripts. A token acts as you; keep it secret." }
            (err(Section::Tokens))
            @if tokens.is_empty() {
                p.muted { "No tokens." }
            } @else {
                table.data {
                    thead { tr {
                        th scope="col" { "Name" } th scope="col" { "Token" } th scope="col" { "Created" }
                        th scope="col" { "Last used" } th scope="col" { "Expires" } th scope="col" { span.sr-only { "Actions" } }
                    } }
                    tbody {
                        @for tok in &tokens {
                            @let expired = tok.expires_at.is_some_and(|e| e <= t);
                            tr.revoked[tok.revoked || expired] {
                                th scope="row" { (tok.name) }
                                td { code { (tok.prefix) "…" } }
                                td { (time_el(tok.created_at)) }
                                td { (opt_time(tok.last_used_at)) }
                                td {
                                    @match tok.expires_at { Some(e) => (time_el(e)), None => span.muted { "never" } }
                                }
                                td {
                                    @if tok.revoked { span.muted { "revoked" } }
                                    @else if expired { span.muted { "expired" } }
                                    @else {
                                        form.inline method="post" action=(format!("/settings/tokens/{}/revoke", tok.id)) {
                                            (ctx.csrf_input())
                                            button.small.danger type="submit" { "Revoke" span.sr-only { " " (tok.name) } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            form.inline-row method="post" action="/settings/tokens" {
                (ctx.csrf_input())
                label for="s-tok-name" { "Name" }
                input #s-tok-name type="text" name="name" required maxlength="100" placeholder="laptop desktop client";
                label for="s-tok-exp" { "Expires" }
                select #s-tok-exp name="expires_days" {
                    option value="" { "never" }
                    option value="30" { "in 30 days" }
                    option value="90" { "in 90 days" }
                    option value="365" { "in a year" }
                }
                button.primary type="submit" { "Create token" }
            }
        }
    };
    Ok(ctx.page("Settings", Nav::Settings, body))
}

pub async fn page(State(state): State<AppState>, ctx: Ctx, Query(q): Query<DoneQuery>) -> AppResult<Markup> {
    render(&state, &ctx, notice_for(q.done.as_deref()), None).await
}

async fn failed(state: &AppState, ctx: &Ctx, section: Section, e: AppError) -> AppResult<Response> {
    if e.status.is_server_error() {
        return Err(e);
    }
    let mut msg = e.message.clone();
    if let Some(first) = msg.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    let page = render(state, ctx, None, Some((section, &msg))).await?;
    let mut resp = (e.status, page).into_response();
    if let Some(s) = e.retry_after {
        if let Ok(v) = s.to_string().parse() {
            resp.headers_mut().insert(axum::http::header::RETRY_AFTER, v);
        }
    }
    Ok(resp)
}

#[derive(Debug, Default, Deserialize)]
pub struct ProfileForm {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    telegram_user_id: String,
}

pub async fn profile(State(state): State<AppState>, ctx: Ctx, Form(f): Form<ProfileForm>) -> AppResult<Response> {
    let tg = f.telegram_user_id.trim();
    let telegram_user_id = if tg.is_empty() {
        None
    } else {
        match tg.parse::<i64>() {
            Ok(v) => Some(v),
            Err(_) => {
                return failed(
                    &state,
                    &ctx,
                    Section::Profile,
                    AppError::bad_request("the Telegram user id must be a number"),
                )
                .await
            }
        }
    };
    let req = UpdateMe {
        display_name: Some(f.display_name.trim().to_string()),
        telegram_user_id: Some(telegram_user_id),
    };
    match account::update_me(&state, &ctx.user, req).await {
        Ok(_) => Ok(ctx.redirect("/settings?done=profile")),
        Err(e) => failed(&state, &ctx, Section::Profile, e).await,
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct PasswordForm {
    #[serde(default)]
    current_password: Option<String>,
    #[serde(default)]
    new_password: String,
    #[serde(default)]
    new_password2: String,
}

pub async fn password(State(state): State<AppState>, ctx: Ctx, Form(f): Form<PasswordForm>) -> AppResult<Response> {
    if f.new_password != f.new_password2 {
        return failed(
            &state,
            &ctx,
            Section::Password,
            AppError::bad_request("the new passwords do not match"),
        )
        .await;
    }
    let req = ChangePassword {
        current_password: f.current_password.filter(|p| !p.is_empty()),
        new_password: f.new_password,
    };
    match account::change_password(&state, &ctx.user, req).await {
        Ok(()) => Ok(ctx.redirect("/settings?done=password")),
        Err(e) => failed(&state, &ctx, Section::Password, e).await,
    }
}

fn enrol_page(ctx: &Ctx, secret: &str, otpauth_url: &str, error: Option<&str>) -> Markup {
    let qr = qr_svg(otpauth_url);
    let grouped: String = secret
        .chars()
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ");
    let body = html! {
        p.crumbs { a href="/settings#totp" { "← Settings" } }
        h1 { "Set up two-step verification" }
        @if let Some(e) = error { (layout::alert("error", e)) }
        ol.steps {
            li {
                "Scan this code with an authenticator app (Aegis, 2FAS, Google Authenticator, …)."
                @if let Some(svg) = qr { div.qr-box { (trusted(svg)) } }
            }
            li {
                "Or enter the key by hand: " code.secret { (grouped) }
                " (time-based, 6 digits, 30 seconds)."
            }
            li {
                "Enter the code the app shows to finish:"
                form.inline-row method="post" action="/settings/totp/confirm" {
                    (ctx.csrf_input())
                    label for="totp-code" { "Code" }
                    input #totp-code type="text" name="code" required inputmode="numeric" pattern="[0-9 ]{6,7}" maxlength="7"
                        autocomplete="one-time-code" autofocus;
                    button.primary type="submit" { "Turn on" }
                }
            }
        }
        p.muted { "Until you confirm, sign-in works as before. Starting again replaces this key." }
    };
    ctx.page("Two-step verification", Nav::Settings, body)
}

pub async fn totp_begin(State(state): State<AppState>, ctx: Ctx) -> AppResult<Response> {
    match account::totp_begin(&state, &ctx.user).await {
        Ok(setup) => Ok(enrol_page(&ctx, &setup.secret, &setup.otpauth_url, None).into_response()),
        Err(e) => failed(&state, &ctx, Section::Totp, e).await,
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct CodeForm {
    #[serde(default)]
    code: String,
}

fn clean_code(c: &str) -> String {
    c.chars().filter(|c| !c.is_whitespace()).collect()
}

pub async fn totp_confirm(State(state): State<AppState>, ctx: Ctx, Form(f): Form<CodeForm>) -> AppResult<Response> {
    match account::totp_confirm(&state, &ctx.user, &clean_code(&f.code)).await {
        Ok(_) => Ok(ctx.redirect("/settings?done=totp-on")),
        Err(e) if e.code == "bad_request" => {
            // Show the same pending key again with the error.
            let me = ui::me(&state, &ctx.user).await?;
            match me.totp_pending_secret.as_deref() {
                Some(secret) => {
                    let url = crate::auth::totp::otpauth_url(secret, &state.config.auth.totp_issuer, &me.username)?;
                    Ok((
                        StatusCode::BAD_REQUEST,
                        enrol_page(
                            &ctx,
                            secret,
                            &url,
                            Some("That code is not valid. Check the time on your phone and try again."),
                        ),
                    )
                        .into_response())
                }
                None => failed(&state, &ctx, Section::Totp, e).await,
            }
        }
        Err(e) => failed(&state, &ctx, Section::Totp, e).await,
    }
}

pub async fn totp_disable(State(state): State<AppState>, ctx: Ctx, Form(f): Form<CodeForm>) -> AppResult<Response> {
    match account::totp_disable(&state, &ctx.user, &clean_code(&f.code)).await {
        Ok(_) => Ok(ctx.redirect("/settings?done=totp-off")),
        Err(e) => failed(&state, &ctx, Section::Totp, e).await,
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct TokenForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    expires_days: String,
}

pub async fn token_create(State(state): State<AppState>, ctx: Ctx, Form(f): Form<TokenForm>) -> AppResult<Response> {
    let expires_at = match f.expires_days.trim() {
        "" => None,
        d => match d.parse::<i64>() {
            Ok(days) if (1..=3650).contains(&days) => Some(now() + days * 86_400),
            _ => {
                return failed(&state, &ctx, Section::Tokens, AppError::bad_request("invalid expiry")).await;
            }
        },
    };
    let created = match account::create_token(&state, &ctx.user, f.name.trim(), expires_at).await {
        Ok(c) => c,
        Err(e) => return failed(&state, &ctx, Section::Tokens, e).await,
    };
    let body = html! {
        p.crumbs { a href="/settings#tokens" { "← Settings" } }
        h1 { "New API token" }
        section.secret-box {
            p { "Token " strong { (created.info.name) } ". " strong { "Copy it now: it is not shown again." } }
            p { code.secret.copyable { (created.token) } }
            p.hint {
                "Use it as " code { "Authorization: Bearer <token>" } ", or paste it into the CLI or desktop client settings."
            }
        }
        p { a.button href="/settings#tokens" { "Done" } }
    };
    Ok(ctx.page("New API token", Nav::Settings, body).into_response())
}

pub async fn token_revoke(State(state): State<AppState>, ctx: Ctx, Path(id): Path<String>) -> AppResult<Response> {
    account::revoke_token(&state, &ctx.user, &id).await?;
    Ok(ctx.redirect("/settings?done=revoked#tokens"))
}

#[cfg(test)]
mod tests {
    use super::qr_svg;

    #[test]
    fn qr_is_plain_svg() {
        let svg = qr_svg("otpauth://totp/stickpile:alice?secret=JBSWY3DPEHPK3PXP&issuer=stickpile").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("<path d=\"M"));
        assert!(!svg.contains("style"));
    }
}
