//! Popular items: most copied, most liked and trending side by side, never
//! blended into one score. Scope (server / mine / one set) and window
//! selectors; the window applies to "most copied" only.

use axum::extract::{Query, State};
use maud::{html, Markup};
use serde::Deserialize;
use stickpile_api::{Popular, PopularEntry};

use super::components::{item_url, kind_badge, plural, quality_badge, set_url, thumb};
use super::layout::Nav;
use super::Ctx;
use crate::error::AppResult;
use crate::i18n::{t, tf};
use crate::service::stats::{self, PopularScope, Window};
use crate::service::ui;
use crate::state::AppState;

#[derive(Debug, Default, Deserialize)]
pub struct PopularQuery {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    set: Option<String>,
}

/// One list; `count` says an entry's value with its unit (`3 copies`).
fn column(title: &str, hint: &str, entries: &[PopularEntry], count: impl Fn(u64) -> String) -> Markup {
    html! {
        section.pop-col aria-label=(title) {
            h2 { (title) }
            p.hint { (hint) }
            @if entries.is_empty() {
                p.muted { (t("Nothing yet.")) }
            } @else {
                ol.pop-list {
                    @for e in entries {
                        li {
                            a.mini-thumb href=(item_url(&e.item.id)) tabindex="-1" aria-hidden="true" {
                                (thumb(&e.item.id, &e.item.name, e.item.width, e.item.height))
                            }
                            div {
                                a href=(item_url(&e.item.id)) { (e.item.name) }
                                " " (kind_badge(e.item.kind, e.item.format)) (quality_badge(e.item.quality))
                                p.meta {
                                    strong { (count(e.value)) }
                                    " · " a href=(set_url(&e.item.set_id)) { (e.item.set_name) }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The three lists; `copied_hint` says the window of "Most copied".
fn columns(p: &Popular, copied_hint: &str) -> Markup {
    html! {
        div #popular-cols.pop-cols {
            (column(t("Most copied"), copied_hint, &p.most_copied, |n| plural(n, "copy", "copies")))
            (column(t("Most liked"), t("All-time likes."), &p.most_liked, |n| plural(n, "like", "likes")))
            (column(
                t("Trending"),
                t("Copied more this week than the 4 weeks before."),
                &p.trending,
                |n| plural(n, "copy this week", "copies this week"),
            ))
        }
    }
}

pub async fn page(State(state): State<AppState>, ctx: Ctx, Query(q): Query<PopularQuery>) -> AppResult<Markup> {
    let (scope, scope_key) = match q.scope.as_deref() {
        Some("mine") => (PopularScope::Mine, "mine"),
        Some("set") if q.set.is_some() => (PopularScope::Set, "set"),
        _ => (PopularScope::Server, "server"),
    };
    let (window, window_key, copied_hint) = match q.window.as_deref() {
        Some("day") => (Window::Day, "day", t("Copies in the last day.")),
        Some("month") => (Window::Month, "month", t("Copies in the last 30 days.")),
        Some("all") => (Window::All, "all", t("Copies of all time.")),
        _ => (Window::Week, "week", t("Copies in the last 7 days.")),
    };
    let set_id = if scope == PopularScope::Set {
        q.set.clone()
    } else {
        None
    };
    let popular = stats::popular(&state, &ctx.user, scope, set_id.as_deref(), window, Some(20)).await?;
    let cols = columns(&popular, copied_hint);
    if ctx.htmx {
        return Ok(cols);
    }
    let set_name = match &set_id {
        Some(id) => Some(ui::readable_set(&state, &ctx.user, id).await?.set.name),
        None => None,
    };
    let body = html! {
        div.section-head {
            h1 {
                @if let Some(n) = &set_name { (tf("Popular in {set}", &[("set", n)])) } @else { (t("Popular")) }
            }
        }
        form.filters method="get" action="/popular" hx-get="/popular" hx-target="#popular-cols" hx-swap="outerHTML"
            hx-push-url="true" hx-trigger="change, submit" {
            @if let Some(id) = &set_id { input type="hidden" name="set" value=(id); }
            label for="pop-scope" { (t("Scope")) }
            select #pop-scope name="scope" {
                option value="server" selected[scope_key == "server"] { (t("Whole server (yours + public)")) }
                option value="mine" selected[scope_key == "mine"] { (t("My items")) }
                @if set_id.is_some() { option value="set" selected[scope_key == "set"] { (t("This set")) } }
            }
            label for="pop-window" { (t("Copies in")) }
            select #pop-window name="window" {
                option value="day" selected[window_key == "day"] { (t("the last day")) }
                option value="week" selected[window_key == "week"] { (t("the last week")) }
                option value="month" selected[window_key == "month"] { (t("the last 30 days")) }
                option value="all" selected[window_key == "all"] { (t("all time")) }
            }
            button type="submit" { (t("Show")) }
        }
        p.muted.small { (t("Three separate signals; they are never combined into one score.")) }
        (cols)
    };
    Ok(ctx.page(t("Popular"), Nav::Popular, body))
}
