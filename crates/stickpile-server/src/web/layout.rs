//! Page shell: `<head>`, top navigation, flash region, footer, error pages.

use axum::http::StatusCode;
use maud::{html, Markup, DOCTYPE};
use stickpile_api::Theme;

use super::components::{icon, Icon};
use crate::assets;
use crate::auth::{AuthMethod, CurrentUser};

/// htmx settings (CSP-compatible: no injected `<style>`, no eval, no inline
/// scripts from responses, same-origin requests only, no history snapshots
/// in localStorage). Error responses (4xx/5xx) are swapped too: the server
/// retargets them into `#flash`.
const HTMX_CONFIG: &str = r#"{"includeIndicatorStyles":false,"allowEval":false,"allowScriptTags":false,"selfRequestsOnly":true,"historyCacheSize":0,"defaultSwapStyle":"outerHTML","responseHandling":[{"code":"204","swap":false},{"code":"[23]..","swap":true},{"code":"[45]..","swap":true,"error":true},{"code":"...","swap":false}]}"#;

/// Which top-level section is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Home,
    Sets,
    Upload,
    Search,
    Popular,
    Stats,
    Problems,
    Settings,
    Admin,
    None,
}

pub fn csrf_input(token: Option<&str>) -> Markup {
    html! {
        @if let Some(t) = token {
            input type="hidden" name="csrf_token" value=(t);
        }
    }
}

fn nav_link(href: &str, label: &str, this: Nav, active: Nav) -> Markup {
    html! {
        li {
            @if this == active {
                a href=(href) aria-current="page" { (label) }
            } @else {
                a href=(href) { (label) }
            }
        }
    }
}

/// A full HTML page. `user` is `None` on the login/register pages and on
/// error pages for anonymous visitors.
pub fn page(user: Option<&CurrentUser>, title: &str, active: Nav, body: Markup) -> Markup {
    page_with_head(user, title, active, html! {}, body)
}

/// [`page`] with extra `<head>` content (e.g. a `<noscript>` refresh).
pub fn page_with_head(user: Option<&CurrentUser>, title: &str, active: Nav, head: Markup, body: Markup) -> Markup {
    let csrf = user.and_then(|u| u.csrf_token());
    let hx_headers = csrf.map(|t| serde_json::json!({ "X-CSRF-Token": t }).to_string());
    // ---- theme (begin) -------------------------------------------------
    // The signed-in user's theme is rendered into the page, so it opens in
    // the right colours; "system" leaves it to `prefers-color-scheme`.
    // prefs.js applies a theme remembered in localStorage on signed-out
    // pages.
    let theme = user.map(|u| u.user.theme());
    let data_theme = match theme {
        Some(Theme::Light) => Some("light"),
        Some(Theme::Dark) => Some("dark"),
        _ => None,
    };
    let color_scheme = data_theme.unwrap_or("light dark");
    // ---- theme (end) ---------------------------------------------------
    html! {
        (DOCTYPE)
        html lang="en" data-theme=[data_theme] {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content=(color_scheme);
                @if let Some(t) = theme {
                    meta name="stickpile-theme" content=(t.as_str());
                }
                meta name="htmx-config" content=(HTMX_CONFIG);
                title { (title) " · stickpile" }
                link rel="icon" type="image/svg+xml" href=(assets::url("favicon.svg"));
                // Not deferred: applies remembered display preferences before
                // the first paint (and the theme on signed-out pages).
                script src=(assets::url("prefs.js")) {}
                link rel="stylesheet" href=(assets::url("app.css"));
                script src=(assets::url("htmx.min.js")) defer {}
                script src=(assets::url("app.js")) defer {}
                // Before upload.js, which attaches the picker to the tray. The
                // data is fetched on the first open.
                script src=(assets::url("emoji.js")) data-emoji-data=(assets::url("emoji.json")) defer {}
                script src=(assets::url("upload.js")) defer {}
                (head)
            }
            body hx-headers=[hx_headers] data-csrf=[csrf] {
                a.skip href="#main" { "Skip to content" }
                header.topbar {
                    a.brand href="/" { "stickpile" }
                    @if let Some(u) = user {
                        nav aria-label="Main" {
                            ul.nav {
                                (nav_link("/", "Home", Nav::Home, active))
                                (nav_link("/sets", "Sets", Nav::Sets, active))
                                (nav_link("/upload", "Upload", Nav::Upload, active))
                                (nav_link("/search", "Search", Nav::Search, active))
                                (nav_link("/popular", "Popular", Nav::Popular, active))
                                (nav_link("/stats", "Stats", Nav::Stats, active))
                                (nav_link("/problems", "Problems", Nav::Problems, active))
                                (nav_link("/settings", "Settings", Nav::Settings, active))
                                @if u.is_admin() {
                                    (nav_link("/admin", "Admin", Nav::Admin, active))
                                }
                            }
                        }
                        form.topsearch role="search" method="get" action="/search" {
                            label.sr-only for="topsearch-q" { "Search" }
                            input #topsearch-q type="search" name="q" placeholder="Search…" autocomplete="off";
                        }
                    }
                    // The right-hand group: the theme switch, then a divider
                    // and the account, so the switch is not pressed for
                    // "Sign out" (on narrow screens the switch moves up to
                    // the brand's line).
                    div.top-end {
                        (theme_switch(user, csrf))
                        @if let Some(u) = user {
                            div.who {
                                span.whoami title=(format!("Signed in as {}", u.user.username)) {
                                    (u.user.display_name)
                                    @if let AuthMethod::Tailscale { login } = &u.method {
                                        " " span.badge.tailnet title=(format!("Signed in through the tailnet as {login}")) { "tailnet" }
                                    }
                                }
                                @if matches!(u.method, AuthMethod::Session { .. }) {
                                    form.inline method="post" action="/logout" {
                                        (csrf_input(csrf))
                                        button.link type="submit" { "Sign out" }
                                    }
                                }
                            }
                        }
                    }
                }
                div #flash role="status" aria-live="polite" {}
                main #main tabindex="-1" {
                    (body)
                }
                footer.footer {
                    "stickpile " (env!("CARGO_PKG_VERSION"))
                    " · " a href="/api/v1/openapi.json" { "REST API" }
                }
            }
        }
    }
}

// ---- theme switch (begin) -------------------------------------------------

/// The top bar's light/dark switch: a toggle button named "Dark theme"
/// (`aria-pressed`). Signed in, it is a form that saves the other theme
/// (`POST /settings/theme`, then back to the page); with prefs.js it flips
/// the theme in effect (also under "system") at once and saves it with
/// `PATCH /api/v1/me`. Signed out it needs prefs.js (hidden without) and is
/// remembered in localStorage only.
fn theme_switch(user: Option<&CurrentUser>, csrf: Option<&str>) -> Markup {
    let dark = user.is_some_and(|u| u.user.theme() == Theme::Dark);
    let button = html! {
        button.icon-btn.theme-toggle type=(if user.is_some() { "submit" } else { "button" }) hidden[user.is_none()]
            data-theme-toggle="" aria-pressed=(if dark { "true" } else { "false" })
            title=(if dark { "Switch to the light theme" } else { "Switch to the dark theme" }) {
            (icon(Icon::Moon)) (icon(Icon::Sun)) span.sr-only { "Dark theme" }
        }
    };
    html! {
        @if user.is_some() {
            form.theme-switch method="post" action="/settings/theme" {
                (csrf_input(csrf))
                input type="hidden" name="theme" value=(if dark { "light" } else { "dark" });
                (button)
            }
        } @else {
            div.theme-switch { (button) }
        }
    }
}

// ---- theme switch (end) ---------------------------------------------------

/// A message for the `#flash` region (htmx errors).
pub fn flash_error(message: &str) -> Markup {
    html! {
        div.alert.error role="alert" {
            (message)
            " "
            button.link.dismiss type="button" data-dismiss="" { "Dismiss" }
        }
    }
}

pub fn alert(kind: &str, message: &str) -> Markup {
    html! {
        div class=(format!("alert {kind}")) role=(if kind == "error" { "alert" } else { "status" }) { (message) }
    }
}

/// Error page for browser requests.
pub fn error_page(user: Option<&CurrentUser>, status: StatusCode, code: &str, message: &str) -> Markup {
    let (title, explain): (&str, &str) = match status {
        StatusCode::NOT_FOUND => (
            "Not found",
            "This page does not exist, or you do not have access to it.",
        ),
        StatusCode::FORBIDDEN if code == "csrf" => (
            "Request blocked",
            "The form was submitted from another site or has expired. Reload the page and try again.",
        ),
        StatusCode::FORBIDDEN => ("Not allowed", message),
        StatusCode::METHOD_NOT_ALLOWED => ("Not allowed", "This address does not accept that kind of request."),
        StatusCode::TOO_MANY_REQUESTS => ("Too many requests", "Slow down a little and try again in a minute."),
        s if s.is_server_error() => (
            "Something went wrong",
            "The server could not complete the request. The details are in the server log.",
        ),
        _ => ("Request failed", message),
    };
    let body = html! {
        section.error-page {
            p.status { (status.as_u16()) }
            h1 { (title) }
            p { (explain) }
            p {
                a href="/" { "Go to the start page" }
                @if user.is_none() {
                    " or " a href="/login" { "sign in" }
                }
                "."
            }
        }
    };
    page(user, title, Nav::None, body)
}
