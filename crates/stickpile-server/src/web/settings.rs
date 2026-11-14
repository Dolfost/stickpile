//! Account settings: profile (display name, Telegram user id), theme,
//! password, TOTP enrolment (QR code as inline SVG) and API tokens.

use std::fmt::Write as _;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Form;
use maud::{html, Markup};
use serde::Deserialize;
use stickpile_api::{ChangePassword, Theme, UpdateMe};

use super::components::{opt_time, time_el, trusted};
use super::layout::{self, Nav};
use super::{safe_next, Ctx};
use crate::auth::AuthMethod;
use crate::db::UserRow;
use crate::error::{AppError, AppResult};
use crate::i18n::{t, t_dyn, tf};
use crate::ids::now;
use crate::service::users::USERNAME_PATTERN;
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
    let label = html! { (t("QR code for your authenticator app")) }.into_string();
    Some(format!(
        r##"<svg class="qr" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" width="220" height="220" role="img" aria-label="{label}" shape-rendering="crispEdges"><rect width="{size}" height="{size}" fill="#ffffff"/><path d="{d}" fill="#000000"/></svg>"##
    ))
}

fn notice_for(done: Option<&str>) -> Option<&'static str> {
    Some(match done? {
        "profile" => t("Profile saved."),
        "theme" => t("Theme saved."),
        "language" => t("Language saved."),
        "password" => t("Password changed. Your other web sessions were signed out."),
        "totp-on" => t("Two-step verification is on."),
        "totp-off" => t("Two-step verification is off."),
        "revoked" => t("Token revoked."),
        _ => return None,
    })
}

/// A role (`admin`, `user`) in the request's language.
fn role_label(role: &str) -> &str {
    match role {
        "admin" => t("admin"),
        "user" => t("user"),
        other => other,
    }
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
    Appearance,
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
    let now_ts = now();
    let body = html! {
        h1 { (t("Settings")) }
        @if let Some(n) = notice { (layout::alert("success", n)) }
        section #profile aria-labelledby="s-profile" {
            h2 #s-profile { (t("Profile")) }
            (err(Section::Profile))
            dl.kv {
                dt { (t("Role")) } dd { (role_label(&me.role)) }
                @if let Some(l) = &me.tailscale_login { dt { (t("Tailnet login")) } dd { code { (l) } } }
            }
            form.stack.narrow method="post" action="/settings" {
                (ctx.csrf_input())
                div.field-row {
                    label for="s-username" { (t("Username")) }
                    input #s-username type="text" name="username" value=(me.username) required
                        minlength="2" maxlength="32" pattern=(USERNAME_PATTERN) autocomplete="username"
                        spellcheck="false" autocapitalize="none" aria-describedby="s-username-hint";
                    p.hint #s-username-hint {
                        (tf("Shown as the owner of your sets (“by {username}”), matched by", &[("username", &me.username)]))
                        " " code { "by:" } " " (t("in search, and used to sign in with a password.")) " "
                        (t("2–32 letters, digits,")) " " code { "_" } ", " code { "." } " " (t("or")) " " code { "-" } "."
                    }
                }
                div.field-row {
                    label for="s-display" { (t("Display name")) }
                    input #s-display type="text" name="display_name" value=(me.display_name) maxlength="100";
                }
                div.field-row {
                    label for="s-tg" { (t("Telegram user id")) }
                    input #s-tg type="text" inputmode="numeric" pattern="[0-9]*" name="telegram_user_id"
                        value=(me.telegram_user_id.map(|v| v.to_string()).unwrap_or_default()) aria-describedby="s-tg-hint";
                    p.hint #s-tg-hint {
                        (t("Numeric id of your Telegram account; sticker packs you publish belong to it."))
                        " " (t("Ask @userinfobot in Telegram for it. Empty clears it."))
                        @if !telegram::enabled(state) { " " (t("(Telegram publishing is not enabled on this server.)")) }
                    }
                }
                button.primary type="submit" { (t("Save profile")) }
            }
        }
        section #appearance aria-labelledby="s-appearance" {
            h2 #s-appearance { (t("Appearance")) }
            (err(Section::Appearance))
            form.stack.narrow method="post" action="/settings/theme" {
                (ctx.csrf_input())
                input type="hidden" name="next" value="/settings?done=theme#appearance";
                fieldset.field-row aria-describedby="s-theme-hint" {
                    legend { (t("Theme")) }
                    @for (theme, label, hint) in [
                        (Theme::System, t("System"), t("follow the light or dark setting of your device")),
                        (Theme::Light, t("Light"), t("always light")),
                        (Theme::Dark, t("Dark"), t("always dark")),
                    ] {
                        label.radio {
                            input type="radio" name="theme" value=(theme.as_str()) checked[me.theme() == theme];
                            " " strong { (label) } " — " (hint)
                        }
                    }
                    p.hint #s-theme-hint {
                        (t("Saved with your account, so it follows you to other devices."))
                        " " (t("The moon or sun button in the top bar switches between light and dark at once."))
                    }
                }
                button.primary type="submit" { (t("Save theme")) }
            }
            form.stack.narrow method="post" action="/settings/language" {
                (ctx.csrf_input())
                div.field-row {
                    label for="s-language" { (t("Language")) }
                    select #s-language name="language" aria-describedby="s-language-hint" {
                        option value="" selected[me.lang().is_none()] { (t("Automatic (your browser's language)")) }
                        @for l in crate::i18n::Lang::ALL {
                            option value=(l.code()) selected[me.lang() == Some(l)] lang=(l.code()) { (l.native_name()) }
                        }
                    }
                    p.hint #s-language-hint {
                        (t("Saved with your account: the desktop client follows it unless it has a language of its own."))
                    }
                }
                button.primary type="submit" { (t("Save language")) }
            }
        }
        section #password aria-labelledby="s-password" {
            h2 #s-password { (t("Password")) }
            (err(Section::Password))
            @if tailnet && me.password_hash.is_none() {
                p.muted { (t("You sign in through the tailnet. Set a password to also sign in from elsewhere or to create tokens with the CLI's login.")) }
            } @else if tailnet {
                p.muted { (t("You are signed in through the tailnet, so the current password is not needed to set a new one.")) }
            }
            form.stack.narrow method="post" action="/settings/password" {
                (ctx.csrf_input())
                input.sr-only type="text" name="username" value=(me.username) autocomplete="username" tabindex="-1" aria-hidden="true";
                @if me.password_hash.is_some() && !tailnet {
                    div.field-row {
                        label for="s-cur" { (t("Current password")) }
                        input #s-cur type="password" name="current_password" required autocomplete="current-password";
                    }
                }
                div.field-row {
                    label for="s-new" { (t("New password")) }
                    input #s-new type="password" name="new_password" required minlength=(state.config.auth.min_password_length) autocomplete="new-password";
                }
                div.field-row {
                    label for="s-new2" { (t("Repeat the new password")) }
                    input #s-new2 type="password" name="new_password2" required minlength=(state.config.auth.min_password_length) autocomplete="new-password";
                }
                button.primary type="submit" { (t("Change password")) }
                p.hint { (t("Changing it signs out your other browser sessions. API tokens keep working; revoke them below if needed.")) }
            }
        }
        section #totp aria-labelledby="s-totp" {
            h2 #s-totp { (t("Two-step verification (TOTP)")) }
            (err(Section::Totp))
            @if me.totp_secret.is_some() {
                p { (t("On. Signing in with a password also needs a code from your authenticator app.")) }
                form.inline-row method="post" action="/settings/totp/disable" {
                    (ctx.csrf_input())
                    label for="s-totp-off" { (t("Current code")) }
                    input #s-totp-off type="text" name="code" required inputmode="numeric" pattern="[0-9 ]{6,7}" maxlength="7" autocomplete="one-time-code";
                    button.danger type="submit" { (t("Turn off")) }
                }
            } @else {
                p { (t("Off. Add a code from an authenticator app to password sign-ins.")) }
                form.inline method="post" action="/settings/totp" {
                    (ctx.csrf_input())
                    button type="submit" { (t("Set up two-step verification")) }
                }
            }
        }
        section #tokens aria-labelledby="s-tokens" {
            h2 #s-tokens { (t("API tokens")) }
            p { (t("For the CLI, the desktop client and scripts. A token acts as you; keep it secret.")) }
            (err(Section::Tokens))
            @if tokens.is_empty() {
                p.muted { (t("No tokens.")) }
            } @else {
                div.table-wrap { table.data {
                    thead { tr {
                        th scope="col" { (t("Name")) } th scope="col" { (t("Token")) } th scope="col" { (t("Created")) }
                        th scope="col" { (t("Last used")) } th scope="col" { (t("Expires")) } th scope="col" { span.sr-only { (t("Actions")) } }
                    } }
                    tbody {
                        @for tok in &tokens {
                            @let expired = tok.expires_at.is_some_and(|e| e <= now_ts);
                            tr.revoked[tok.revoked || expired] {
                                th scope="row" { (tok.name) }
                                td { code { (tok.prefix) "…" } }
                                td { (time_el(tok.created_at)) }
                                td { (opt_time(tok.last_used_at)) }
                                td {
                                    @match tok.expires_at { Some(e) => (time_el(e)), None => span.muted { (t("never")) } }
                                }
                                td {
                                    @if tok.revoked { span.muted { (t("revoked")) } }
                                    @else if expired { span.muted { (t("expired")) } }
                                    @else {
                                        form.inline method="post" action=(format!("/settings/tokens/{}/revoke", tok.id)) {
                                            (ctx.csrf_input())
                                            button.small.danger type="submit" { (t("Revoke")) span.sr-only { " " (tok.name) } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } }
            }
            // A panel of its own, apart from the list above.
            form.token-form method="post" action="/settings/tokens" aria-labelledby="s-tok-new" {
                h3 #s-tok-new { (t("New token")) }
                (ctx.csrf_input())
                div.inline-row {
                    label for="s-tok-name" { (t("Name")) }
                    input #s-tok-name type="text" name="name" required maxlength="100" placeholder=(t("laptop desktop client"));
                    label for="s-tok-exp" { (t("Expires")) }
                    select #s-tok-exp name="expires_days" {
                        option value="" { (t("never")) }
                        option value="30" { (t("in 30 days")) }
                        option value="90" { (t("in 90 days")) }
                        option value="365" { (t("in a year")) }
                    }
                    button.primary type="submit" { (t("Create token")) }
                }
            }
        }
    };
    Ok(ctx.page(t("Settings"), Nav::Settings, body))
}

pub async fn page(State(state): State<AppState>, ctx: Ctx, Query(q): Query<DoneQuery>) -> AppResult<Markup> {
    render(&state, &ctx, notice_for(q.done.as_deref()), None).await
}

async fn failed(state: &AppState, ctx: &Ctx, section: Section, e: AppError) -> AppResult<Response> {
    if e.status.is_server_error() {
        return Err(e);
    }
    // In the request's language when the catalog has the message.
    let msg = capitalize(&t_dyn(&e.message));
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
    username: String,
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
    let username = f.username.trim();
    let req = UpdateMe {
        username: (!username.is_empty()).then(|| username.to_string()),
        display_name: Some(f.display_name.trim().to_string()),
        telegram_user_id: Some(telegram_user_id),
        theme: None,
        language: None,
    };
    match account::update_me(&state, &ctx.user, req).await {
        Ok(_) => Ok(ctx.redirect("/settings?done=profile")),
        Err(e) => failed(&state, &ctx, Section::Profile, e).await,
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct ThemeForm {
    #[serde(default)]
    theme: String,
    #[serde(default)]
    next: Option<String>,
}

/// Path and query of a same-site `Referer` (the page a form was sent from).
fn referer_path(headers: &HeaderMap) -> Option<String> {
    let referer = headers.get(header::REFERER)?.to_str().ok()?;
    let (_, rest) = referer.split_once("://")?;
    let path = &rest[rest.find('/')?..];
    safe_next(Some(path.split('#').next().unwrap_or(path))).map(str::to_string)
}

/// The theme, from the settings form or the top bar's switch without
/// JavaScript (with it, the switch saves through `PATCH /api/v1/me`). Goes
/// back to `next`, else to the page the switch was on.
pub async fn theme(
    State(state): State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    Form(f): Form<ThemeForm>,
) -> AppResult<Response> {
    let Some(theme) = Theme::parse(f.theme.trim()) else {
        let e = AppError::bad_request("the theme must be system, light or dark");
        return failed(&state, &ctx, Section::Appearance, e).await;
    };
    let req = UpdateMe {
        theme: Some(theme),
        ..Default::default()
    };
    if let Err(e) = account::update_me(&state, &ctx.user, req).await {
        return failed(&state, &ctx, Section::Appearance, e).await;
    }
    let back = safe_next(f.next.as_deref())
        .map(str::to_string)
        .or_else(|| referer_path(&headers))
        .unwrap_or_else(|| "/settings?done=theme#appearance".into());
    Ok(ctx.redirect(&back))
}

#[derive(Debug, Default, Deserialize)]
pub struct LanguageForm {
    #[serde(default)]
    language: String,
}

/// The language from the settings form (empty: automatic).
pub async fn language(State(state): State<AppState>, ctx: Ctx, Form(f): Form<LanguageForm>) -> AppResult<Response> {
    let code = f.language.trim();
    let req = UpdateMe {
        language: Some((!code.is_empty()).then(|| code.to_string())),
        ..Default::default()
    };
    if let Err(e) = account::update_me(&state, &ctx.user, req).await {
        return failed(&state, &ctx, Section::Appearance, e).await;
    }
    Ok(ctx.redirect("/settings?done=language#appearance"))
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

/// `s` with its first letter in upper case (any script).
pub(super) fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
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
        p.crumbs { a href="/settings#totp" { "← " (t("Settings")) } }
        h1 { (t("Set up two-step verification")) }
        @if let Some(e) = error { (layout::alert("error", e)) }
        ol.steps {
            li {
                (t("Scan this code with an authenticator app (Aegis, 2FAS, Google Authenticator, …)."))
                @if let Some(svg) = qr { div.qr-box { (trusted(svg)) } }
            }
            li {
                (t("Or enter the key by hand:")) " " code.secret { (grouped) }
                " " (t("(time-based, 6 digits, 30 seconds)."))
            }
            li {
                (t("Enter the code the app shows to finish:"))
                form.inline-row method="post" action="/settings/totp/confirm" {
                    (ctx.csrf_input())
                    label for="totp-code" { (t("Code")) }
                    input #totp-code type="text" name="code" required inputmode="numeric" pattern="[0-9 ]{6,7}" maxlength="7"
                        autocomplete="one-time-code" autofocus;
                    button.primary type="submit" { (t("Turn on")) }
                }
            }
        }
        p.muted { (t("Until you confirm, sign-in works as before. Starting again replaces this key.")) }
    };
    ctx.page(t("Two-step verification"), Nav::Settings, body)
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
                            Some(t("That code is not valid. Check the time on your phone and try again.")),
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
        p.crumbs { a href="/settings#tokens" { "← " (t("Settings")) } }
        h1 { (t("New API token")) }
        section.secret-box {
            p { (t("Token")) " " strong { (created.info.name) } ". " strong { (t("Copy it now: it is not shown again.")) } }
            p { code.secret.copyable { (created.token) } }
            p.hint {
                (t("Use it as")) " " code { "Authorization: Bearer <token>" } ", " (t("or paste it into the CLI or desktop client settings."))
            }
        }
        p { a.button href="/settings#tokens" { (t("Done")) } }
    };
    Ok(ctx.page(t("New API token"), Nav::Settings, body).into_response())
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
