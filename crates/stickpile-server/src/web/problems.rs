//! Problems: the "Problems" section at the bottom of a set page (owners
//! only; loaded with htmx after the page, and again after changes to the
//! items), the Problems tab (`/problems`: every own set, or the whole server
//! for admins) and the dismiss / restore / delete actions. Everything comes
//! from [`crate::service::problems`], which also does the authorization.
//!
//! Actions answer htmx with the re-rendered block of the set they changed
//! (`place=set`: the set page's block, `place=tab`: the set's group on the
//! tab, with the tab's filters), and plain forms with a redirect to `next`.

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Form;
use maud::{html, Markup};
use serde::Deserialize;
use stickpile_api::{
    MediaKind, Problem, ProblemAction, ProblemCounts, ProblemItem, ProblemKind, ProblemSeverity, SetProblems, Variant,
    Visibility,
};

use super::components::{file_url, item_url, plural, set_url, visibility_badge};
use super::layout::Nav;
use super::{safe_next, Ctx};
use crate::error::{AppError, AppResult};
use crate::i18n::{t, tf};
use crate::service::items as items_svc;
use crate::service::problems::{self as svc, Filter, ProblemScope};
use crate::service::{gitpack, telegram};
use crate::state::AppState;

/// View state of a problems list, from the query (GET) or the hidden fields
/// of an action form (POST).
#[derive(Debug, Default, Clone, Deserialize)]
pub struct View {
    /// `set` (the set page's block) or `tab` (a group on the Problems tab).
    #[serde(default)]
    place: String,
    /// `1`: list dismissed problems too.
    #[serde(default)]
    dismissed: String,
    /// The least severe to list (`error`, `warning`, `info`/empty).
    #[serde(default)]
    severity: String,
    #[serde(default)]
    kind: String,
    /// `mine` or `server` (tab only).
    #[serde(default)]
    scope: String,
    /// Where a plain form goes back to.
    #[serde(default)]
    next: String,
}

impl View {
    fn on_tab(&self) -> bool {
        self.place == "tab"
    }
    fn show_dismissed(&self) -> bool {
        self.dismissed == "1"
    }
    fn filter(&self) -> Filter {
        Filter {
            severity: ProblemSeverity::parse(&self.severity),
            kind: ProblemKind::parse(&self.kind),
            dismissed: self.show_dismissed(),
        }
    }
    fn scope(&self) -> ProblemScope {
        if self.scope == "server" {
            ProblemScope::Server
        } else {
            ProblemScope::Mine
        }
    }
    /// Hidden fields that carry this view through an action form.
    fn fields(&self, ctx: &Ctx) -> Markup {
        html! {
            (ctx.csrf_input())
            input type="hidden" name="place" value=(if self.on_tab() { "tab" } else { "set" });
            input type="hidden" name="dismissed" value=(self.dismissed);
            input type="hidden" name="severity" value=(self.severity);
            input type="hidden" name="kind" value=(self.kind);
            input type="hidden" name="scope" value=(self.scope);
            input type="hidden" name="next" value=(ctx.here);
        }
    }
}

pub fn severity_label(s: ProblemSeverity) -> &'static str {
    match s {
        ProblemSeverity::Error => t("Error"),
        ProblemSeverity::Warning => t("Warning"),
        ProblemSeverity::Info => t("Info"),
    }
}

pub fn kind_label(k: ProblemKind) -> &'static str {
    match k {
        ProblemKind::ExactDuplicate => t("Exact duplicates"),
        ProblemKind::NearDuplicate => t("Near-duplicates"),
        ProblemKind::Similar => t("Look alike (AI hint)"),
        ProblemKind::InOtherSet => t("Also in other sets"),
        ProblemKind::MissingFiles => t("Missing files"),
        ProblemKind::ProcessingFailed => t("Processing failed"),
        ProblemKind::LowResolution => t("Very small pictures"),
        ProblemKind::AiUnreviewed => t("AI suggestions to review"),
        ProblemKind::NotEmoji => t("Emoji entries that are not emoji"),
        ProblemKind::NoEmoji => t("No emoji"),
        ProblemKind::TelegramTooMany => t("Too many stickers for Telegram"),
        ProblemKind::TelegramUnsupported => t("Cannot become Telegram stickers"),
        ProblemKind::TelegramTrimmed => t("Cut short on Telegram"),
        ProblemKind::TelegramOutdated => t("Telegram pack out of date"),
        ProblemKind::TelegramFailed => t("Publishing to Telegram failed"),
        ProblemKind::GitFailed => t("Git sync failed"),
        ProblemKind::EmptySet => t("Empty set"),
    }
}

fn sev_class(s: ProblemSeverity) -> &'static str {
    match s {
        ProblemSeverity::Error => "sev-error",
        ProblemSeverity::Warning => "sev-warning",
        ProblemSeverity::Info => "sev-info",
    }
}

/// "2 errors · 1 warning · 3 info", or nothing.
fn count_line(c: &ProblemCounts) -> Markup {
    let parts: Vec<(u32, &str, &str, &str)> = [
        (c.error, "error", "errors", "sev-error"),
        (c.warning, "warning", "warnings", "sev-warning"),
        (c.info, "info", "info", "sev-info"),
    ]
    .into_iter()
    .filter(|p| p.0 > 0)
    .collect();
    html! {
        span.problem-counts {
            @for (i, (n, one, many, class)) in parts.iter().enumerate() {
                @if i > 0 { span.sep aria-hidden="true" { " · " } }
                span class=(format!("count {class}")) { (plural(u64::from(*n), one, many)) }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Set page section
// ---------------------------------------------------------------------------

/// The set page's "Problems" section. The list is fetched after the page
/// (htmx `load`) and again when the item grid or the upload form changed
/// something, so fixing an item on the page updates it. Without JavaScript
/// the link opens the same list as a page.
pub fn set_section(set_id: &str) -> Markup {
    let url = format!("/sets/{set_id}/problems");
    html! {
        section #problems .problems-section aria-labelledby="problems-head" {
            h2 #problems-head { (t("Problems")) }
            div #problems-block .problems-block hx-get=(url) hx-swap="innerHTML"
                hx-trigger="load, htmx:afterRequest from:#items-section delay:800ms, htmx:afterRequest from:.uploader delay:800ms" {
                p.muted { (t("Checking the set for problems…")) " " a href=(url) { (t("Open the list")) } }
            }
        }
    }
}

/// The contents of `#problems-block` (set page) or the standalone page.
fn set_block(ctx: &Ctx, state: &AppState, sp: &SetProblems, view: &View) -> Markup {
    let url = format!("/sets/{}/problems", sp.set.id);
    let toggle = if view.show_dismissed() {
        Some((url.clone(), t("Hide dismissed").to_string()))
    } else if sp.counts.dismissed > 0 {
        Some((
            format!("{url}?dismissed=1"),
            tf("Show {n} dismissed", &[("n", &sp.counts.dismissed)]),
        ))
    } else {
        None
    };
    html! {
        div.problems-summary {
            @if sp.counts.open() == 0 {
                p.problems-none { (t("No problems found.")) }
            } @else {
                p { (count_line(&sp.counts)) }
            }
            @if let Some((href, label)) = toggle {
                a.button.small href=(href) hx-get=(href) hx-target="#problems-block" hx-swap="innerHTML" { (label) }
            }
        }
        @if !sp.problems.is_empty() {
            div.problem-list { @for p in &sp.problems { (card(ctx, state, sp, p, view)) } }
        }
        p.hint {
            (t("Checked just now. Near-duplicates are compared picture by picture; “look alike” is an AI hint and \
                often fine. Mark anything intentional as “Not a problem”; it comes back if those items change."))
        }
    }
}

/// `GET /sets/{id}/problems`: the block for htmx, a page otherwise.
pub async fn set_page(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Query(mut view): Query<View>,
) -> AppResult<Response> {
    view.place = "set".into();
    let sp = svc::for_set(&state, &ctx.user, &id, view.filter()).await?;
    let block = set_block(&ctx, &state, &sp, &view);
    if ctx.htmx {
        return Ok(block.into_response());
    }
    let body = html! {
        p.crumbs { a href=(set_url(&sp.set.id)) { "← " (sp.set.name) } }
        h1 { (tf("Problems in “{set}”", &[("set", &sp.set.name)])) }
        div #problems-block .problems-block { (block) }
    };
    Ok(ctx.page(t("Problems"), Nav::Sets, body).into_response())
}

// ---------------------------------------------------------------------------
// One problem
// ---------------------------------------------------------------------------

fn thumb(it: &ProblemItem, size: u32) -> Markup {
    html! {
        @if it.hidden {
            span.problem-thumb.hidden-item title=(t("Item of a private set")) { (t("private")) }
        } @else {
            a.problem-thumb href=(item_url(&it.item_id)) tabindex="-1" {
                img src=(file_url(&it.item_id, Variant::Thumb)) alt="" loading="lazy" decoding="async"
                    width=(size) height=(size);
            }
        }
    }
}

fn item_name(it: &ProblemItem) -> Markup {
    html! {
        @if it.hidden {
            span.muted { (t("Private item")) }
        } @else {
            a href=(item_url(&it.item_id)) { (it.name) }
            @match it.kind {
                MediaKind::Still => {},
                MediaKind::Animation => { " " span.badge.kind { (t("anim")) } },
                MediaKind::Video => { " " span.badge.kind { (t("video")) } },
            }
        }
    }
}

/// Per-item link for the problem's action ("Edit emojis", ...).
fn item_action(sp: &SetProblems, p: &Problem, it: &ProblemItem) -> Markup {
    if it.hidden {
        return html! {};
    }
    let page = item_url(&it.item_id);
    html! {
        @match p.action {
            Some(ProblemAction::EditEmojis) if sp.can_edit => a.small href=(format!("{page}?edit=1#edit")) { (t("Edit emojis")) },
            Some(ProblemAction::ReviewAi) if sp.can_edit => a.small href=(page) { (t("Review")) },
            Some(ProblemAction::FindBetter) => a.small href=(page) { (t("Find a better version")) },
            _ => {},
        }
    }
}

/// The delete button of one item of a duplicate pair or group: deletes it
/// and re-renders the block (and drops its card from the item grid).
fn delete_button(sp: &SetProblems, it: &ProblemItem, view: &View) -> Markup {
    let url = format!("/sets/{}/problems/remove/{}", sp.set.id, it.item_id);
    let target = target(sp, view);
    // The view travels as values (the CSRF header comes from `hx-headers`).
    let vals = serde_json::json!({
        "place": if view.on_tab() { "tab" } else { "set" },
        "dismissed": view.dismissed,
        "severity": view.severity,
        "kind": view.kind,
        "scope": view.scope,
    })
    .to_string();
    html! {
        // Without JavaScript: the item's delete confirmation page.
        a.button.small.danger href=(format!("/items/{}/delete", it.item_id)) hx-post=(url) hx-vals=(vals)
            hx-confirm=(tf("Delete “{name}” from this set?", &[("name", &it.name)])) hx-target=(target.0) hx-swap=(target.1) {
            (t("Delete")) span.sr-only { " " (it.name) }
        }
    }
}

/// (hx-target, hx-swap) for actions in this view.
fn target(sp: &SetProblems, view: &View) -> (String, &'static str) {
    if view.on_tab() {
        (format!("#problems-set-{}", sp.set.id), "outerHTML")
    } else {
        ("#problems-block".into(), "innerHTML")
    }
}

fn card(ctx: &Ctx, state: &AppState, sp: &SetProblems, p: &Problem, view: &View) -> Markup {
    let pair = matches!(
        p.kind,
        ProblemKind::ExactDuplicate | ProblemKind::NearDuplicate | ProblemKind::Similar
    );
    let deletable = sp.can_edit && matches!(p.action, Some(ProblemAction::DeleteOne));
    let head_id = format!("problem-{}-{}", sp.set.id, p.id);
    let (tgt, swap) = target(sp, view);
    let set_id = &sp.set.id;
    html! {
        article id=(head_id) class=(format!("problem {}{}", sev_class(p.severity), if p.dismissed { " dismissed" } else { "" }))
            aria-labelledby=(format!("{head_id}-title")) {
            div.problem-head {
                span class=(format!("badge sev {}", sev_class(p.severity))) { (severity_label(p.severity)) }
                strong id=(format!("{head_id}-title")) { (kind_label(p.kind)) }
                @if p.dismissed { span.badge { (t("Dismissed")) } }
            }
            p.problem-msg { (p.message) }
            @if pair {
                div.problem-pair {
                    @for it in &p.items {
                        figure.problem-pick {
                            (thumb(it, 160))
                            figcaption {
                                span.name { (item_name(it)) }
                                @if let Some(d) = &it.detail { span.detail { (d) } }
                                @if deletable && !it.hidden { (delete_button(sp, it, view)) }
                            }
                        }
                    }
                }
                @if let (ProblemKind::NearDuplicate, Some(s)) = (p.kind, p.score) {
                    p.hint { (tf("Perceptual hash distance {d} of 64.", &[("d", &s)])) }
                }
                @if let (ProblemKind::Similar, Some(s)) = (p.kind, p.score) {
                    p.hint { (tf("AI similarity {s}. A hint, not a verdict.", &[("s", &format!("{s:.2}"))])) }
                }
            } @else if !p.items.is_empty() {
                ul.problem-items {
                    @for it in &p.items {
                        li {
                            (thumb(it, 40))
                            span.name { (item_name(it)) }
                            @if let Some(d) = &it.detail { span.detail { (d) } }
                            (item_action(sp, p, it))
                        }
                    }
                }
            }
            div.problem-actions {
                (set_action(ctx, state, sp, p))
                @if sp.can_dismiss {
                    @let (path, label) = if p.dismissed { ("restore", t("Show again")) } else { ("dismiss", t("Not a problem")) };
                    @let url = format!("/sets/{set_id}/problems/{}/{path}", p.id);
                    form.inline method="post" action=(url) hx-post=(url) hx-target=(tgt) hx-swap=(swap) {
                        (view.fields(ctx))
                        button.small type="submit" { (label) span.sr-only { ": " (kind_label(p.kind)) } }
                    }
                }
            }
        }
    }
}

/// Set-level fix for the problem: publish, pull, upload, open the set.
fn set_action(ctx: &Ctx, state: &AppState, sp: &SetProblems, p: &Problem) -> Markup {
    let set_id = &sp.set.id;
    html! {
        @match p.action {
            Some(ProblemAction::PublishTelegram) if sp.can_dismiss && telegram::enabled(state) => {
                a.button.small.primary href=(format!("/sets/{set_id}/telegram")) { (t("Publish to Telegram")) }
            },
            Some(ProblemAction::PullGit) if sp.can_dismiss && gitpack::enabled(state) => {
                form.inline method="post" action=(format!("/sets/{set_id}/pull")) {
                    (ctx.csrf_input())
                    button.small.primary type="submit" { (t("Pull again")) }
                }
            },
            Some(ProblemAction::AddItems) if sp.can_edit => {
                a.button.small.primary href=(format!("{}#upload-head", set_url(set_id))) { (t("Add items")) }
            },
            Some(ProblemAction::SplitSet) if sp.can_edit => {
                a.button.small href=(format!("{}#items-section", set_url(set_id))) { (t("Open the items")) }
                span.hint { " " (t("Move items to another set with “Move to” on their pages.")) }
            },
            _ => {},
        }
    }
}

// ---------------------------------------------------------------------------
// The Problems tab
// ---------------------------------------------------------------------------

/// One set's group on the tab (also the htmx answer to its actions).
fn tab_group(ctx: &Ctx, state: &AppState, sp: &SetProblems, view: &View) -> Markup {
    let private_other = sp.owner_id != ctx.id() && sp.set.visibility == Visibility::Private;
    html! {
        section.problem-set id=(format!("problems-set-{}", sp.set.id)) aria-labelledby=(format!("ps-{}", sp.set.id)) {
            div.problem-set-head {
                h2 id=(format!("ps-{}", sp.set.id)) {
                    @if private_other {
                        (sp.set.name)
                    } @else {
                        a href=(set_url(&sp.set.id)) { (sp.set.name) }
                    }
                }
                @if sp.owner_id != ctx.id() { span.muted { (tf("by {owner}", &[("owner", &sp.set.owner_username)])) } }
                (visibility_badge(sp.set.visibility))
                (count_line(&sp.counts))
            }
            @if sp.problems.is_empty() {
                p.muted { (t("Nothing left to show here.")) }
            } @else {
                div.problem-list { @for p in &sp.problems { (card(ctx, state, sp, p, view)) } }
            }
        }
    }
}

pub async fn tab(State(state): State<AppState>, ctx: Ctx, Query(mut view): Query<View>) -> AppResult<Response> {
    view.place = "tab".into();
    if view.scope == "server" && !ctx.user.is_admin() {
        return Err(AppError::forbidden(
            "the server-wide problems report needs the admin role",
        ));
    }
    let report = svc::report(&state, &ctx.user, view.scope(), view.filter()).await?;
    let server = view.scope() == ProblemScope::Server;
    let base = |scope: &str| {
        if scope == "server" {
            "/problems?scope=server"
        } else {
            "/problems"
        }
    };
    let body = html! {
        div.section-head { h1 { (t("Problems")) } }
        @if ctx.user.is_admin() {
            nav.tabs aria-label=(t("Whose sets")) {
                a href=(base("mine")) aria-current=[(!server).then_some("page")] { (t("My sets")) }
                a href=(base("server")) aria-current=[server.then_some("page")] { (t("Whole server")) }
            }
        }
        form.filters.problem-filters method="get" action="/problems" {
            @if server { input type="hidden" name="scope" value="server"; }
            span.filter-field {
                label for="pf-severity" { (t("Show")) }
                select #pf-severity name="severity" {
                    @for (value, label) in [("", t("Everything")), ("warning", t("Errors and warnings")), ("error", t("Errors only"))] {
                        option value=(value) selected[view.severity == value || (value.is_empty() && ProblemSeverity::parse(&view.severity).is_none_or(|s| s == ProblemSeverity::Info))] { (label) }
                    }
                }
            }
            span.filter-field {
                label for="pf-kind" { (t("Kind")) }
                select #pf-kind name="kind" {
                    option value="" selected[ProblemKind::parse(&view.kind).is_none()] { (t("All kinds")) }
                    @for k in ProblemKind::ALL {
                        option value=(k.as_str()) selected[view.kind == k.as_str()] { (kind_label(k)) }
                    }
                }
            }
            label.check {
                input type="checkbox" name="dismissed" value="1" checked[view.show_dismissed()];
                (t("Show dismissed"))
            }
            button type="submit" { (t("Apply")) }
        }
        p.problems-total {
            @let sets = plural(u64::from(report.sets_checked), "set", "sets");
            @if server {
                (tf("Checked {sets} on this server.", &[("sets", &sets)]))
            } @else {
                (tf("Checked {sets}.", &[("sets", &sets)]))
            }
            " "
            @if report.counts.open() == 0 {
                (t("No problems found."))
            } @else {
                (count_line(&report.counts)) "."
            }
            @if report.counts.dismissed > 0 { " " (tf("{n} dismissed.", &[("n", &report.counts.dismissed)])) }
        }
        @if server {
            p.hint {
                (t("Every set on the server. Items of other users' private sets are not shown, and only owners can \
                    dismiss problems."))
            }
        }
        @if report.sets.is_empty() && report.counts.open() > 0 {
            p.muted { (t("Nothing matches these filters.")) }
        }
        @for sp in &report.sets { (tab_group(&ctx, &state, sp, &view)) }
        p.hint {
            @if report.similar_available {
                (t("Near-duplicates are compared picture by picture. “Look alike” comes from the AI and is only a \
                    hint. Problems marked “Not a problem” stay hidden until the items change."))
            } @else {
                (t("Near-duplicates are compared picture by picture. “Look alike” comes from the AI (off on this \
                    server right now) and is only a hint. Problems marked “Not a problem” stay hidden until the items \
                    change."))
            }
        }
    };
    Ok(ctx.page(t("Problems"), Nav::Problems, body).into_response())
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// The re-rendered block (set page) or group (tab) after an action.
async fn after_action(state: &AppState, ctx: &Ctx, set_id: &str, view: &View, extra: Markup) -> AppResult<Response> {
    if !ctx.htmx {
        let back = safe_next(Some(&view.next)).map(str::to_string).unwrap_or_else(|| {
            if view.on_tab() {
                "/problems".into()
            } else {
                format!("/sets/{set_id}/problems")
            }
        });
        return Ok(ctx.redirect(&back));
    }
    let sp = svc::for_set(state, &ctx.user, set_id, view.filter()).await?;
    let block = if view.on_tab() {
        tab_group(ctx, state, &sp, view)
    } else {
        set_block(ctx, state, &sp, view)
    };
    Ok(html! { (block) (extra) }.into_response())
}

pub async fn dismiss(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, problem)): Path<(String, String)>,
    Form(view): Form<View>,
) -> AppResult<Response> {
    svc::dismiss(&state, &ctx.user, &id, &problem).await?;
    after_action(&state, &ctx, &id, &view, html! {}).await
}

pub async fn restore(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, problem)): Path<(String, String)>,
    Form(view): Form<View>,
) -> AppResult<Response> {
    svc::undismiss(&state, &ctx.user, &id, &problem).await?;
    after_action(&state, &ctx, &id, &view, html! {}).await
}

/// Delete one item of a duplicate pair or group (htmx; plain browsers use
/// the item's confirmation page). The item must be in this set.
pub async fn remove_item(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, item)): Path<(String, String)>,
    Form(view): Form<View>,
) -> AppResult<Response> {
    let detail = items_svc::detail(&state, &ctx.user, &item).await?;
    if detail.item.set_id != id {
        return Err(AppError::not_found());
    }
    items_svc::delete(&state, &ctx.user, &item).await?;
    // The set page's grid loses the card too.
    let extra = html! { div id=(format!("item-{item}")) hx-swap-oob="delete" {} };
    after_action(
        &state,
        &ctx,
        &id,
        &view,
        if view.on_tab() {
            html! {}
        } else {
            extra
        },
    )
    .await
}
