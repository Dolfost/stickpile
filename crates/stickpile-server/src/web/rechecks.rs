//! Rechecks in the web UI ([`crate::service::recheck`]): "Recheck my
//! stickers" in the settings (also reached from a set page, with the set
//! chosen), "Recheck every sticker" in the admin panel, and the page of one
//! recheck (`/rechecks/{id}`): its progress, which polls itself with htmx
//! while it runs (a meta refresh without JavaScript), a cancel button, the
//! result and the pictures that failed.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Form;
use maud::{html, Markup};
use stickpile_api::{Recheck, RecheckFailure, RecheckMode, RecheckPart, RecheckScope, RecheckState, StartRecheck};

use super::components::{item_url, time_el};
use super::layout::{self, Nav};
use super::{settings, AdminCtx, Ctx};
use crate::db;
use crate::error::{AppError, AppResult};
use crate::i18n::{t, t_dyn, tf};
use crate::service::recheck as svc;
use crate::service::sets::{self, SetScope};
use crate::service::ui;
use crate::state::{AiStatus, AppState};

/// A part's name in the request's language.
pub fn part_label(p: RecheckPart) -> &'static str {
    match p {
        RecheckPart::Keywords => t("Keywords"),
        RecheckPart::Emojis => t("Emojis"),
        RecheckPart::Vibe => t("Vibe"),
        RecheckPart::Text => t("Text in the picture"),
        RecheckPart::Embedding => t("Search by meaning"),
        RecheckPart::Quality => t("Quality score"),
        RecheckPart::Renditions => t("Renditions"),
        RecheckPart::Other => t("other"),
    }
}

fn part_hint(p: RecheckPart) -> &'static str {
    match p {
        RecheckPart::Keywords | RecheckPart::Emojis | RecheckPart::Vibe => t("suggested by the AI from the picture"),
        RecheckPart::Text => t("the words written in the picture, read by the computer"),
        RecheckPart::Embedding => t("what the search compares a query with"),
        RecheckPart::Quality => t("the quality report on the item page"),
        RecheckPart::Renditions => t("the files made for Telegram, Discord and the other chat apps"),
        RecheckPart::Other => "",
    }
}

fn mode_label(m: RecheckMode) -> &'static str {
    match m {
        RecheckMode::Missing => t("Only what is missing"),
        RecheckMode::Refresh => t("Everything again"),
        RecheckMode::Other => t("other"),
    }
}

fn mode_hint(m: RecheckMode) -> &'static str {
    match m {
        RecheckMode::Missing => t(
            "empty fields, pictures the current AI model or text reader never looked at, missing quality reports and renditions",
        ),
        RecheckMode::Refresh => t("replaces what the computer gave before; never what you wrote or confirmed"),
        RecheckMode::Other => "",
    }
}

fn state_label(s: RecheckState) -> &'static str {
    match s {
        RecheckState::Running => t("running"),
        RecheckState::Done => t("done"),
        RecheckState::Cancelled => t("cancelled"),
        RecheckState::Failed => t("failed"),
        RecheckState::Other => t("other"),
    }
}

/// CSS class of the progress box (the job box's colours).
fn state_class(s: RecheckState) -> &'static str {
    match s {
        RecheckState::Done => "job job-done",
        RecheckState::Failed => "job job-failed",
        _ => "job",
    }
}

fn parts_text(parts: &[RecheckPart]) -> String {
    parts.iter().map(|p| part_label(*p)).collect::<Vec<_>>().join(", ")
}

fn recheck_url(id: &str) -> String {
    format!("/rechecks/{id}")
}

/// The parts this server can recompute now, and why others are missing.
fn available(state: &AppState) -> (Vec<RecheckPart>, Vec<&'static str>) {
    let mut parts = Vec::new();
    let mut notes = Vec::new();
    match state.ai_status() {
        AiStatus::Ready if state.ai().is_some() => {
            parts.extend(RecheckPart::ALL.into_iter().filter(|p| p.needs_clip()));
        }
        AiStatus::Loading | AiStatus::Ready => notes.push(t(
            "The AI model is still loading: keywords, emojis, vibe and search by meaning can be rechecked in a few seconds (reload the page).",
        )),
        AiStatus::Off | AiStatus::Failed => notes.push(t(
            "AI is off on this server, so keywords, emojis, vibe and search by meaning cannot be rechecked.",
        )),
    }
    if state.text_reader().is_some() {
        parts.push(RecheckPart::Text);
    } else {
        notes.push(t(
            "This server does not read the words in pictures, so the text cannot be rechecked.",
        ));
    }
    parts.push(RecheckPart::Quality);
    parts.push(RecheckPart::Renditions);
    parts.sort();
    (parts, notes)
}

/// What a start form shows chosen: what was sent, after an error; else
/// every part on offer and `missing`.
#[derive(Debug, Clone, Default)]
pub struct Chosen {
    pub parts: Option<Vec<RecheckPart>>,
    pub mode: Option<RecheckMode>,
    /// The set id typed in (the admin form).
    pub set: Option<String>,
}

impl Chosen {
    /// The parts (`part.<name>` boxes) and the mode a submitted form chose.
    fn from_form(form: &HashMap<String, String>) -> Chosen {
        let parts = RecheckPart::ALL
            .into_iter()
            .filter(|p| form.contains_key(&format!("part.{}", p.as_str())))
            .collect();
        let mode = match form.get("mode").map(|m| m.trim()) {
            Some("refresh") => RecheckMode::Refresh,
            Some("missing") | Some("") | None => RecheckMode::Missing,
            Some(_) => RecheckMode::Other,
        };
        Chosen {
            parts: Some(parts),
            mode: Some(mode),
            set: form
                .get("set_id")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        }
    }

    fn part(&self, p: RecheckPart) -> bool {
        self.parts.as_ref().is_none_or(|ps| ps.contains(&p))
    }

    fn mode(&self, m: RecheckMode) -> bool {
        self.mode.unwrap_or(RecheckMode::Missing) == m
    }
}

/// The parts and the mode a submitted form asks for; none ticked is an
/// error.
fn request_from_form(form: &HashMap<String, String>) -> AppResult<(Vec<RecheckPart>, RecheckMode)> {
    let chosen = Chosen::from_form(form);
    let parts = chosen.parts.unwrap_or_default();
    if parts.is_empty() {
        return Err(AppError::bad_request("choose at least one thing to recheck"));
    }
    Ok((parts, chosen.mode.unwrap_or_default()))
}

/// The part boxes and the mode radios of both start forms.
fn choices(state: &AppState, prefix: &str, chosen: &Chosen) -> Markup {
    let (parts, notes) = available(state);
    html! {
        fieldset.field-row {
            legend { (t("What to recheck")) }
            @for p in &parts {
                @let id = format!("{prefix}-part-{}", p.as_str());
                label.radio for=(id) {
                    input #(id) type="checkbox" name=(format!("part.{}", p.as_str())) value="on" checked[chosen.part(*p)];
                    " " strong { (part_label(*p)) } ": " (part_hint(*p))
                }
            }
            @for n in &notes { p.hint { (n) } }
        }
        fieldset.field-row {
            legend { (t("How")) }
            @for m in [RecheckMode::Missing, RecheckMode::Refresh] {
                label.radio {
                    input type="radio" name="mode" value=(m.as_str()) checked[chosen.mode(m)];
                    " " strong { (mode_label(m)) } ": " (mode_hint(m))
                }
            }
        }
    }
}

/// A table of rechecks, newest first, each linking to its page.
fn batch_table(list: &[Recheck], show_by: bool) -> Markup {
    html! {
        div.table-wrap { table.data {
            thead { tr {
                th scope="col" { (t("Started")) }
                @if show_by { th scope="col" { (t("By")) } }
                th scope="col" { (t("What")) }
                th scope="col" { (t("Mode")) }
                th scope="col" { (t("Status")) }
                th scope="col" { (t("Pictures")) }
                th scope="col" { (t("Changed")) }
                th scope="col" { (t("Failed")) }
            } }
            tbody {
                @for r in list {
                    tr {
                        th scope="row" { a href=(recheck_url(&r.id)) { (time_el(r.created_at)) } }
                        @if show_by { td { (started_by(r)) } }
                        td { (parts_text(&r.parts)) }
                        td { (mode_label(r.mode)) }
                        td { (state_label(r.state)) }
                        td { (r.done + r.failed) " / " (r.total) }
                        td { (r.changed) }
                        td { (r.failed) }
                    }
                }
            }
        } }
    }
}

fn started_by(r: &Recheck) -> String {
    match &r.requested_by {
        Some(u) => u.clone(),
        None => t("the server").to_string(),
    }
}

// ---------------------------------------------------------------------------
// Settings: Recheck my stickers
// ---------------------------------------------------------------------------

/// The settings section: the start form (with `set` chosen, from a set
/// page; what was sent after an error) and the viewer's recent rechecks.
/// `error` is the form's error.
pub async fn settings_section(
    state: &AppState,
    ctx: &Ctx,
    set: Option<&str>,
    chosen: &Chosen,
    error: Option<&str>,
) -> AppResult<Markup> {
    let own = sets::list(state, &ctx.user, SetScope::Mine).await?;
    let recent: Vec<Recheck> = svc::list(state, &ctx.user)
        .await?
        .into_iter()
        .filter(|r| r.scope == RecheckScope::User)
        .take(5)
        .collect();
    Ok(html! {
        section #recheck aria-labelledby="s-recheck" {
            h2 #s-recheck { (t("Recheck my stickers")) }
            p {
                (t("Let the computer look at your stickers again in the background: their keywords, emojis, vibe, the words in the picture, search by meaning, the quality score and the renditions."))
                " " strong { (t("What you wrote or confirmed is never changed.")) }
            }
            @if let Some(e) = error { (layout::alert("error", e)) }
            form.stack.narrow method="post" action="/settings/recheck" {
                (ctx.csrf_input())
                (choices(state, "s", chosen))
                div.field-row {
                    label for="s-recheck-set" { (t("Stickers")) }
                    select #s-recheck-set name="set_id" {
                        option value="" selected[set.is_none()] { (t("All my sets")) }
                        @for s in &own {
                            option value=(s.id) selected[set == Some(s.id.as_str())] { (s.name) }
                        }
                    }
                }
                button.primary type="submit" { (t("Start the recheck")) }
                p.hint {
                    (tf("You can start {n} rechecks an hour; asking for one that is running shows it.", &[("n", &state.config.limits.recheck_per_hour)]))
                }
            }
            @if !recent.is_empty() {
                h3 { (t("Recent rechecks")) }
                (batch_table(&recent, false))
            }
        }
    })
}

/// `POST /settings/recheck`: start, then the recheck's page; errors come
/// back in the settings form (429 and 503 with their `Retry-After`).
pub async fn start_own(
    State(state): State<AppState>,
    ctx: Ctx,
    Form(form): Form<HashMap<String, String>>,
) -> AppResult<Response> {
    let set_id = form.get("set_id").map(|s| s.trim()).filter(|s| !s.is_empty());
    let started = match request_from_form(&form) {
        Ok((parts, mode)) => {
            let req = StartRecheck {
                parts,
                mode,
                set_id: set_id.map(str::to_string),
            };
            svc::start_own(&state, &ctx.user, req).await
        }
        Err(e) => Err(e),
    };
    match started {
        Ok((r, _)) => Ok(ctx.redirect(&recheck_url(&r.id))),
        Err(e) => settings::recheck_failed(&state, &ctx, set_id, &Chosen::from_form(&form), e).await,
    }
}

// ---------------------------------------------------------------------------
// One recheck
// ---------------------------------------------------------------------------

/// What the recheck covers, as a sentence.
async fn scope_text(state: &AppState, r: &Recheck) -> AppResult<String> {
    let set_name = match &r.set_id {
        Some(id) => db::set_by_id(&state.db, id).await?.map(|s| s.name),
        None => None,
    };
    Ok(match (r.scope, set_name) {
        (RecheckScope::Server, Some(name)) => tf("Every sticker of the set \"{set}\".", &[("set", &name)]),
        (RecheckScope::Server, None) => t("Every sticker on the server.").to_string(),
        (_, Some(name)) => tf("The stickers of your set \"{set}\".", &[("set", &name)]),
        (_, None) => t("The stickers of all your sets.").to_string(),
    })
}

/// The progress box: counters and state, a cancel button while it runs
/// (then it polls itself every 2 s), the result and the failures after.
fn progress(ctx: &Ctx, r: &Recheck, failures: &[RecheckFailure], names: &HashMap<String, String>) -> Markup {
    let running = r.state == RecheckState::Running;
    let url = recheck_url(&r.id);
    let cancel = format!("{url}/cancel");
    let checked = r.done + r.failed;
    let fields = r.parts.iter().any(|p| {
        matches!(
            p,
            RecheckPart::Keywords | RecheckPart::Emojis | RecheckPart::Vibe | RecheckPart::Text
        )
    });
    html! {
        div #recheck-progress class=(state_class(r.state)) role="status" aria-live="polite"
            hx-get=[running.then_some(url.as_str())] hx-trigger=[running.then_some("every 2s")] hx-swap="outerHTML" {
            p { strong { (state_label(r.state)) } @if running { span.spinner aria-hidden="true" {} } }
            @if r.total > 0 {
                progress max=(r.total) value=(checked) aria-label=(t("Pictures checked")) { (checked) " / " (r.total) }
            }
            dl.tiles {
                div.tile { dt { (t("Pictures")) } dd { (r.total) } }
                div.tile { dt { (t("Checked")) } dd { (checked) } }
                div.tile { dt { (t("Changed")) } dd { (r.changed) } }
                div.tile { dt { (t("Failed")) } dd { (r.failed) } }
            }
            @if running {
                form.inline method="post" action=(cancel) hx-post=(cancel) hx-target="#recheck-progress" hx-swap="outerHTML" {
                    (ctx.csrf_input())
                    button.danger type="submit" { (t("Cancel the recheck")) }
                }
                p.hint { (t("You can leave this page: the recheck goes on in the background.")) }
            } @else {
                @if r.state == RecheckState::Done && r.total == 0 {
                    p { (t("Nothing was missing: every picture already had what you asked for.")) }
                } @else if r.state == RecheckState::Done {
                    p { (t("Finished.")) }
                } @else if r.state == RecheckState::Cancelled {
                    p { (t("Cancelled. Pictures that were already checked keep their new values.")) }
                } @else if r.state == RecheckState::Failed {
                    p { (t("Every picture failed; see the errors below.")) }
                }
                @if r.changed > 0 && fields {
                    p {
                        (t("New keywords, emojis, vibes and texts are AI suggestions: on each sticker they carry the AI mark and a Confirm button until you confirm or change them."))
                    }
                }
                @if !failures.is_empty() {
                    h2 #recheck-failures { (t("Failed pictures")) }
                    div.table-wrap { table.data aria-labelledby="recheck-failures" {
                        thead { tr {
                            th scope="col" { (t("Sticker")) }
                            th scope="col" { (t("Error")) }
                            th scope="col" { (t("When")) }
                        } }
                        tbody {
                            @for f in failures {
                                tr {
                                    th scope="row" {
                                        @if let Some(id) = &f.item_id {
                                            a href=(item_url(id)) { (names.get(id).map(String::as_str).unwrap_or(id.as_str())) }
                                        } @else {
                                            (t("picture")) " " code { (f.media_id) }
                                        }
                                    }
                                    td.error-text { (f.error) }
                                    td { (time_el(f.at)) }
                                }
                            }
                        }
                    } }
                }
            }
        }
    }
}

async fn progress_for(state: &AppState, ctx: &Ctx, r: &Recheck) -> AppResult<Markup> {
    let (failures, names) = if r.state == RecheckState::Running || r.failed == 0 {
        (Vec::new(), HashMap::new())
    } else {
        let failures = svc::failures(state, &ctx.user, &r.id).await?;
        let ids: Vec<String> = failures.iter().filter_map(|f| f.item_id.clone()).collect();
        let names = ui::own_item_names(state, &ctx.user, &ids).await?;
        (failures, names)
    };
    Ok(progress(ctx, r, &failures, &names))
}

/// `GET /rechecks/{id}`: the recheck's page; the progress box alone for htmx.
pub async fn page(State(state): State<AppState>, ctx: Ctx, Path(id): Path<String>) -> AppResult<Markup> {
    let r = svc::get(&state, &ctx.user, &id).await?;
    let fragment = progress_for(&state, &ctx, &r).await?;
    if ctx.htmx {
        return Ok(fragment);
    }
    let (back, back_label) = if r.scope == RecheckScope::Server {
        ("/admin/rechecks", t("Rechecks"))
    } else {
        ("/settings#recheck", t("Settings"))
    };
    let scope = scope_text(&state, &r).await?;
    let head = html! {
        @if r.state == RecheckState::Running { noscript { meta http-equiv="refresh" content="3"; } }
    };
    let body = html! {
        p.crumbs { a href=(back) { "\u{2190} " (back_label) } }
        h1 { (t("Recheck")) }
        p { (scope) }
        dl.kv {
            dt { (t("What")) } dd { (parts_text(&r.parts)) }
            dt { (t("How")) } dd { (mode_label(r.mode)) ": " (mode_hint(r.mode)) }
            dt { (t("Started by")) } dd { (started_by(&r)) }
            dt { (t("Started")) } dd { (time_el(r.created_at)) }
            @if let Some(at) = r.finished_at { dt { (t("Ended")) } dd { (time_el(at)) } }
        }
        (fragment)
    };
    Ok(layout::page_with_head(
        Some(&ctx.user),
        t("Recheck"),
        if r.scope == RecheckScope::Server {
            Nav::Admin
        } else {
            Nav::Settings
        },
        head,
        body,
    ))
}

/// `POST /rechecks/{id}/cancel` (its owner or an admin): the progress box
/// for htmx, else back to the recheck's page.
pub async fn cancel(State(state): State<AppState>, ctx: Ctx, Path(id): Path<String>) -> AppResult<Response> {
    let r = svc::cancel(&state, &ctx.user, &id).await?;
    if ctx.htmx {
        return Ok(progress_for(&state, &ctx, &r).await?.into_response());
    }
    Ok(ctx.redirect(&recheck_url(&r.id)))
}

// ---------------------------------------------------------------------------
// Admin: Recheck every sticker
// ---------------------------------------------------------------------------

fn admin_body(state: &AppState, ctx: &AdminCtx, list: &[Recheck], chosen: &Chosen, error: Option<&str>) -> Markup {
    html! {
        section aria-labelledby="a-recheck" {
            h2 #a-recheck { (t("Recheck every sticker")) }
            p {
                (t("Look at every sticker on the server again in the background, whoever owns it. Values people wrote or confirmed are never changed. Starting one is recorded in the audit log."))
            }
            @if let Some(e) = error { (layout::alert("error", e)) }
            form.stack.narrow method="post" action="/admin/rechecks" {
                (ctx.csrf_input())
                (choices(state, "a", chosen))
                div.field-row {
                    label for="a-recheck-set" { (t("Only one set (optional)")) }
                    input #a-recheck-set type="text" name="set_id" value=(chosen.set.as_deref().unwrap_or_default())
                        maxlength="26" spellcheck="false"
                        autocapitalize="none" aria-describedby="a-recheck-set-hint";
                    p.hint #a-recheck-set-hint { (t("The set's id, from its address (/sets/<id>). Empty: every set.")) }
                }
                button.primary type="submit" { (t("Start the recheck")) }
            }
        }
        section aria-labelledby="a-recheck-list" {
            h2 #a-recheck-list { (t("Server-wide rechecks")) }
            p.hint {
                (t("Including those the server starts by itself: after a change of the AI model, and for pictures its text reader never read."))
            }
            @if list.is_empty() {
                p.muted { (t("None yet.")) }
            } @else {
                (batch_table(list, true))
            }
        }
    }
}

async fn server_batches(state: &AppState, ctx: &AdminCtx) -> AppResult<Vec<Recheck>> {
    Ok(svc::list(state, &ctx.user)
        .await?
        .into_iter()
        .filter(|r| r.scope == RecheckScope::Server)
        .collect())
}

/// `GET /admin/rechecks`.
pub async fn admin_page(State(state): State<AppState>, ctx: AdminCtx) -> AppResult<Markup> {
    let list = server_batches(&state, &ctx).await?;
    let body = admin_body(&state, &ctx, &list, &Chosen::default(), None);
    Ok(super::admin::page_for(
        &ctx,
        t("Rechecks"),
        super::admin::Tab::Rechecks,
        body,
    ))
}

/// `POST /admin/rechecks`: start a server-wide recheck (audited by the
/// service), then its page; errors come back in the form.
pub async fn admin_start(
    State(state): State<AppState>,
    ctx: AdminCtx,
    Form(form): Form<HashMap<String, String>>,
) -> AppResult<Response> {
    let set_id = form.get("set_id").map(|s| s.trim()).filter(|s| !s.is_empty());
    let started = match request_from_form(&form) {
        Ok((parts, mode)) => {
            let req = StartRecheck {
                parts,
                mode,
                set_id: set_id.map(str::to_string),
            };
            svc::start_server(&state, &ctx.user, req).await
        }
        Err(e) => Err(e),
    };
    match started {
        Ok((r, _)) => Ok(ctx.redirect(&recheck_url(&r.id))),
        Err(e) if e.status.is_client_error() || e.status == StatusCode::SERVICE_UNAVAILABLE => {
            let list = server_batches(&state, &ctx).await?;
            let msg = settings::capitalize(&t_dyn(&e.message));
            let page = super::admin::page_for(
                &ctx,
                t("Rechecks"),
                super::admin::Tab::Rechecks,
                admin_body(&state, &ctx, &list, &Chosen::from_form(&form), Some(&msg)),
            );
            Ok(settings::with_retry_after(
                (e.status, page).into_response(),
                e.retry_after,
            ))
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_to_request() {
        let form: HashMap<String, String> = [("part.keywords", "on"), ("part.text", "on"), ("mode", "refresh")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(
            request_from_form(&form).unwrap(),
            (vec![RecheckPart::Keywords, RecheckPart::Text], RecheckMode::Refresh)
        );
        let none: HashMap<String, String> = [("mode".to_string(), "missing".to_string())].into_iter().collect();
        assert!(request_from_form(&none).is_err());
        let odd: HashMap<String, String> = [("part.quality", "on"), ("mode", "deep")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(request_from_form(&odd).unwrap().1, RecheckMode::Other);
    }
}
