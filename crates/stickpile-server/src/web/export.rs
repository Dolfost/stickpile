//! The export page of a set, `/sets/{id}/export` (docs/web.md "Export and
//! publish"):
//!
//! * Download: the set as ZIP, TAR or TAR.GZ (the API's export routes), and
//!   the Signal-ready zip, for anyone who can read the set.
//! * WhatsApp: build the set's `.wastickers` packs and download them, also
//!   for anyone who can read the set.
//! * Publish: one row per platform the server is configured for, with
//!   where the set is published, when, the last result (with what was
//!   left out or changed) and the platform's options, prefilled with what
//!   the set was published with last; tick several and publish them with
//!   one button. The owner only.
//!
//! The forms post and redirect back here; background jobs are shown with
//! the job fragment, which polls itself with htmx (without JavaScript the
//! page refreshes itself while one runs).

use std::collections::{BTreeMap, HashMap};

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use maud::{html, Markup};
use serde::Deserialize;
use serde_json::Value;
use stickpile_api::routes as r;
use stickpile_api::{Job, Publication, PublishRequest, Set, WhatsappStatus};

use super::components::{fmt_bytes, opt_time, plural, set_url};
use super::integrations::{job_fragment, job_running};
use super::layout::{self, Nav};
use super::Ctx;
use crate::error::AppResult;
use crate::i18n::{t, t_dyn, tf};
use crate::service::publish::PublicationRow;
use crate::service::{publish, sets, signal, ui, whatsapp};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sets/{id}/export", get(page))
        .route("/sets/{id}/export/whatsapp", post(whatsapp_build))
        .route("/sets/{id}/export/publish", post(publish_submit))
}

fn export_url(id: &str) -> String {
    format!("/sets/{id}/export")
}

#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    /// Publish jobs to show, comma-separated (after the form redirects).
    #[serde(default)]
    jobs: Option<String>,
}

/// What a render of the page needs besides the set.
struct View<'a> {
    whatsapp: WhatsappStatus,
    /// `None` for anyone but the owner.
    publications: Option<Vec<Publication>>,
    /// The owner's stored publish state, by platform (option defaults).
    stored: HashMap<String, PublicationRow>,
    jobs: Vec<Job>,
    error: Option<String>,
    form: &'a HashMap<String, String>,
}

pub async fn page(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> AppResult<Markup> {
    let set = sets::detail(&state, &ctx.user, &id).await?.set;
    let mut jobs = Vec::new();
    for job_id in q
        .jobs
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .take(8)
    {
        if let Ok(job) = ui::job(&state, &ctx.user, job_id).await {
            jobs.push(job);
        }
    }
    let empty = HashMap::new();
    let (publications, stored) = owner_publications(&state, &ctx, &set).await?;
    let view = View {
        whatsapp: whatsapp::status(&state, &ctx.user, &set.id).await?,
        publications,
        stored,
        jobs,
        error: None,
        form: &empty,
    };
    Ok(render(&state, &ctx, &set, view))
}

async fn owner_publications(
    state: &AppState,
    ctx: &Ctx,
    set: &Set,
) -> AppResult<(Option<Vec<Publication>>, HashMap<String, PublicationRow>)> {
    if set.owner_id != ctx.id() {
        return Ok((None, HashMap::new()));
    }
    Ok((
        Some(publish::list(state, &ctx.user, &set.id).await?),
        publish::stored_all(&state.db, &set.id).await?,
    ))
}

fn render(state: &AppState, ctx: &Ctx, set: &Set, view: View<'_>) -> Markup {
    let running = view.jobs.iter().any(job_running) || view.whatsapp.job.as_ref().is_some_and(job_running);
    let head = html! {
        @if running { noscript { meta http-equiv="refresh" content="3"; } }
    };
    let body = html! {
        p.crumbs { a href=(set_url(&set.id)) { "← " (set.name) } }
        h1 { (tf("Export {name}", &[("name", &set.name)])) }
        (downloads(state, set))
        (whatsapp_section(state, ctx, set, &view.whatsapp))
        @if let Some(publications) = &view.publications {
            (publish_section(state, ctx, set, publications, &view))
        }
    };
    layout::page_with_head(Some(&ctx.user), t("Export"), Nav::Sets, head, body)
}

fn downloads(state: &AppState, set: &Set) -> Markup {
    let link = |route: &str| r::path(route, &[&set.id]);
    html! {
        section #download aria-labelledby="download-head" {
            h2 #download-head { (t("Download")) }
            p {
                (t("The set's files and its set.toml, as stickpile exports and imports them. Each format holds the same files."))
            }
            p.actions {
                a.button.primary href=(link(r::SET_EXPORT_ZIP)) download { "ZIP (.zip)" }
                " "
                a.button href=(link(r::SET_EXPORT)) download { "TAR (.tar)" }
                " "
                a.button href=(link(r::SET_EXPORT_TGZ)) download { "TAR.GZ (.tar.gz)" }
            }
            p.muted.small {
                (t("RAR is not offered: making RAR archives needs the proprietary rar program. Everything that opens RAR opens ZIP too."))
            }
            @if set.item_count > 0 {
                h3 #signal-zip { (t("Signal-ready zip")) }
                p {
                    (t("Every item as a 512x512 Signal sticker, numbered in order, with emoji.txt and a manifest.json for signal-cli. In Signal Desktop: File, Create/upload sticker pack, then drag the images in and pick the emoji emoji.txt lists."))
                    @if !signal::configured(state) {
                        " " (t("This server does not publish to Signal itself, so this is the way to make a Signal pack."))
                    }
                }
                p.actions {
                    a.button href=(signal::zip_url(&set.id)) download { (t("Signal-ready zip")) " (.zip)" }
                }
                p.muted.small {
                    (t("Made when you download it, which takes a while for a large set; items that cannot become Signal stickers are listed in left-out.txt."))
                }
            }
        }
    }
}

fn whatsapp_section(state: &AppState, ctx: &Ctx, set: &Set, status: &WhatsappStatus) -> Markup {
    let action = format!("/sets/{}/export/whatsapp", set.id);
    let back = export_url(&set.id);
    html! {
        section #whatsapp aria-labelledby="whatsapp-head" {
            h2 #whatsapp-head { (t("WhatsApp sticker packs")) }
            p {
                (t("Every item becomes a 512x512 WebP sticker. Sticker apps such as Sticker Maker add .wastickers packs to WhatsApp."))
                " " (t("A pack holds 3 to 30 stickers, all still or all animated, so a larger set is split into several packs."))
            }
            @if !state.ffmpeg_available {
                p.muted.small { (t("Videos are left out: animated stickers from videos need ffmpeg on the server.")) }
            }
            @if let Some(build) = &status.build {
                @if !status.current {
                    (layout::alert("info", t("The set changed after these packs were built; build them again to update them.")))
                }
                p.muted.small { (tf("Built {time}.", &[("time", &super::components::fmt_time(build.built_at))])) }
                ul.link-list {
                    @for pack in &build.packs {
                        @let n = pack.stickers.len() as u64;
                        li {
                            a href=(pack.url) download { (pack.file) }
                            ": " (pack.title) ", "
                            @if pack.animated {
                                (plural(n, "animated sticker", "animated stickers"))
                            } @else {
                                (plural(n, "still sticker", "still stickers"))
                            }
                            ", " (fmt_bytes(pack.size_bytes))
                        }
                    }
                }
                p { a.button href=(build.url) download { (t("Download all packs")) " (.zip, " (fmt_bytes(build.size_bytes)) ")" } }
                @let notes: Vec<_> = build.packs.iter().flat_map(|p| p.stickers.iter()).filter(|s| s.note.is_some()).collect();
                @let changed = notes.len() as u64;
                @let left_out = build.skipped.len() as u64;
                @if !notes.is_empty() {
                    details {
                        summary { (plural(changed, "sticker was changed to fit", "stickers were changed to fit")) }
                        ul { @for s in &notes { li { (s.name) ": " (t_dyn(s.note.as_deref().unwrap_or(""))) } } }
                    }
                }
                @if !build.skipped.is_empty() {
                    details open {
                        summary { (plural(left_out, "item was left out", "items were left out")) }
                        ul { @for s in &build.skipped { li { (s.name) ": " (t_dyn(&s.reason)) } } }
                    }
                }
            }
            form.inline method="post" action=(action) hx-post=(action) hx-target="#wa-job" hx-swap="innerHTML" {
                (ctx.csrf_input())
                button.primary[status.build.is_none()] type="submit" {
                    @if status.build.is_some() { (t("Build the packs again")) } @else { (t("Build WhatsApp packs")) }
                }
            }
            div #wa-job aria-live="polite" {
                @if let Some(job) = &status.job { (job_fragment(job, Some(&back))) }
            }
        }
    }
}

fn publish_section(state: &AppState, ctx: &Ctx, set: &Set, publications: &[Publication], view: &View<'_>) -> Markup {
    let configured: Vec<&Publication> = publications.iter().filter(|p| p.configured).collect();
    let action = format!("/sets/{}/export/publish", set.id);
    let back = export_url(&set.id);
    let only_one = configured.len() == 1;
    html! {
        section #publish aria-labelledby="publish-head" {
            h2 #publish-head { (t("Publish")) }
            @if configured.is_empty() {
                p.muted { (t("No platform to publish to is set up on this server.")) }
            } @else {
                p { (t("Publishing creates the pack on the platform the first time and updates it later. Tick several platforms to publish to all of them at once.")) }
                @if let Some(e) = &view.error { (layout::alert("error", e)) }
                form method="post" action=(action) {
                    (ctx.csrf_input())
                    table.data {
                        thead { tr {
                            th scope="col" { (t("Platform")) }
                            th scope="col" { (t("Published as")) }
                            th scope="col" { (t("Last publish")) }
                            th scope="col" { (t("Options")) }
                        } }
                        tbody {
                            @for p in &configured {
                                (publication_row(state, ctx, p, view, only_one, &back))
                            }
                        }
                    }
                    p.actions {
                        button.primary type="submit" { (t("Publish to the selected platforms")) }
                    }
                }
            }
            div #publish-jobs aria-live="polite" {
                @for job in &view.jobs { (job_fragment(job, Some(&back))) }
            }
        }
    }
}

/// What was left out of, or changed for, the last publish (the job
/// result's `skipped` and `notes`), each with the reason.
fn result_details(result: &Value) -> Markup {
    let list = |key: &str| result.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    let skipped = list("skipped");
    let notes = list("notes");
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    html! {
        @if !notes.is_empty() {
            details {
                summary { (plural(notes.len() as u64, "sticker was changed to fit", "stickers were changed to fit")) }
                ul { @for n in &notes { li { (text(n, "name")) ": " (t_dyn(&text(n, "note"))) } } }
            }
        }
        @if !skipped.is_empty() {
            details {
                summary { (plural(skipped.len() as u64, "item was left out", "items were left out")) }
                ul { @for s in &skipped { li { (text(s, "name")) ": " (t_dyn(&text(s, "reason"))) } } }
            }
        }
    }
}

fn publication_row(
    state: &AppState,
    ctx: &Ctx,
    p: &Publication,
    view: &View<'_>,
    only_one: bool,
    back: &str,
) -> Markup {
    let form = view.form;
    let platform = publish::platform(&p.platform);
    let defaults: HashMap<&str, String> = platform
        .map(|pl| pl.option_values(view.stored.get(&p.platform)))
        .unwrap_or_default()
        .into_iter()
        .collect();
    let check = format!("platform.{}", p.platform);
    let checked = if form.is_empty() {
        only_one || p.target.is_some()
    } else {
        form.get(&check).is_some()
    };
    let check_id = format!("pub-{}", p.platform);
    html! {
        tr {
            th scope="row" {
                label.check for=(check_id) {
                    input #(check_id) type="checkbox" name=(check) value="on" checked[checked];
                    " " (p.name)
                }
            }
            td {
                @match (&p.target, &p.url) {
                    (Some(target), Some(url)) if url.starts_with("https://") => {
                        a href=(url) rel="noopener noreferrer" target="_blank" { (target) }
                    }
                    (Some(target), _) => { (target) }
                    _ => { span.muted { (t("not yet")) } }
                }
            }
            td {
                @if let Some(ts) = p.published_at {
                    (opt_time(Some(ts)))
                    @if let (Some(result), Some(pl)) = (&p.last_result, platform) {
                        br; span.small { (pl.summary(result)) }
                        (result_details(result))
                    }
                } @else {
                    (opt_time(None))
                }
                @if p.last_status == Some(stickpile_api::JobStatus::Failed) {
                    @if let Some(e) = &p.last_error {
                        p.error-text { (tf("The last publish failed: {error}", &[("error", e)])) }
                    }
                }
                @if let Some(job) = &p.job { (job_fragment(job, Some(back))) }
            }
            td {
                @if let Some(pl) = platform {
                    @if let Some(note) = pl.notice(state, &ctx.user) { p.hint { (note) } }
                    @for o in pl.options() {
                        @let name = format!("{}.{}", p.platform, o.key);
                        @let field_id = format!("opt-{}-{}", p.platform, o.key);
                        @let hint_id = format!("{field_id}-hint");
                        @let value = if form.is_empty() {
                            defaults.get(o.key).map(String::as_str).unwrap_or("")
                        } else {
                            form.get(&name).map(String::as_str).unwrap_or("")
                        };
                        div.field-row {
                            label for=(field_id) { (o.label) }
                            @if o.choices.is_empty() {
                                input #(field_id) type="text" name=(name) maxlength=(o.max_chars)
                                    value=(value)
                                    aria-describedby=[o.hint.is_some().then_some(hint_id.as_str())] spellcheck="false";
                            } @else {
                                select #(field_id) name=(name) aria-describedby=[o.hint.is_some().then_some(hint_id.as_str())] {
                                    @for (i, (v, label)) in o.choices.iter().enumerate() {
                                        option value=(v) selected[*v == value || (i == 0 && value.is_empty())] { (label) }
                                    }
                                }
                            }
                            @if let Some(h) = o.hint { p.hint #(hint_id) { (h) } }
                        }
                    }
                    button type="submit" name="only" value=(p.platform) { (tf("Publish to {platform} only", &[("platform", &p.name)])) }
                }
            }
        }
    }
}

/// The publish request a submitted form means: the platform of the row's
/// own button, else every ticked one; each with its non-empty options.
fn request_from_form(form: &HashMap<String, String>) -> PublishRequest {
    let platforms: Vec<String> = match form.get("only").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(only) => vec![only.to_string()],
        None => publish::PLATFORMS
            .iter()
            .filter(|p| form.contains_key(&format!("platform.{}", p.id())))
            .map(|p| p.id().to_string())
            .collect(),
    };
    let mut options = BTreeMap::new();
    for id in &platforms {
        let Some(p) = publish::platform(id) else { continue };
        let mut obj = serde_json::Map::new();
        for o in p.options() {
            if let Some(v) = form
                .get(&format!("{id}.{}", o.key))
                .map(|v| v.trim())
                .filter(|v| !v.is_empty())
            {
                obj.insert(o.key.to_string(), Value::String(v.to_string()));
            }
        }
        options.insert(id.clone(), Value::Object(obj));
    }
    PublishRequest { platforms, options }
}

pub async fn publish_submit(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> AppResult<Response> {
    let req = request_from_form(&form);
    match publish::publish(&state, &ctx.user, &id, req).await {
        Ok(done) => {
            let ids: Vec<&str> = done.jobs.iter().map(|j| j.id.as_str()).collect();
            Ok(ctx.redirect(&format!("{}?jobs={}#publish", export_url(&id), ids.join(","))))
        }
        Err(e) if e.status.is_client_error() && e.code != "not_found" && e.code != "forbidden" => {
            let set = sets::detail(&state, &ctx.user, &id).await?.set;
            let (publications, stored) = owner_publications(&state, &ctx, &set).await?;
            let view = View {
                whatsapp: whatsapp::status(&state, &ctx.user, &set.id).await?,
                publications,
                stored,
                jobs: Vec::new(),
                error: Some(t_dyn(&e.message)),
                form: &form,
            };
            Ok((e.status, render(&state, &ctx, &set, view)).into_response())
        }
        Err(e) => Err(e),
    }
}

pub async fn whatsapp_build(State(state): State<AppState>, ctx: Ctx, Path(id): Path<String>) -> AppResult<Response> {
    let job = whatsapp::build(&state, &ctx.user, &id).await?;
    let back = export_url(&id);
    if ctx.htmx {
        Ok(job_fragment(&job, Some(&back)).into_response())
    } else {
        Ok(ctx.redirect(&format!("{back}#whatsapp")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_to_request() {
        let form: HashMap<String, String> = [
            ("platform.telegram", "on"),
            ("telegram.pack_name", " cats "),
            ("telegram.title", ""),
            ("csrf_token", "x"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let req = request_from_form(&form);
        assert_eq!(req.platforms, ["telegram"]);
        assert_eq!(req.options["telegram"], serde_json::json!({"pack_name": "cats"}));
        let mut only = form.clone();
        only.remove("platform.telegram");
        only.insert("only".into(), "telegram".into());
        assert_eq!(request_from_form(&only).platforms, ["telegram"]);
        only.remove("only");
        assert!(request_from_form(&only).platforms.is_empty());
    }
}
