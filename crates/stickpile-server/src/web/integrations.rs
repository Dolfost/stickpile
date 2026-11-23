//! Git packs, Telegram import/export, Discord and Signal import and
//! background job status.
//!
//! Job status is a fragment that polls itself with htmx
//! (`hx-trigger="every 2s"`, swapping its own outerHTML) until the job is
//! done or failed; the replacement then has no trigger and polling stops.
//! Without JavaScript the job page refreshes itself (`<noscript>` meta
//! refresh) while the job runs.

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use maud::{html, Markup};
use serde::Deserialize;
use stickpile_api::{
    CreateGitPack, DiscordImport, GitMode, Job, JobKind, JobStatus, Set, SignalImport, TelegramExport, TelegramImport,
    Visibility,
};

use super::components::{fmt_time, set_url};
use super::layout::{self, Nav};
use super::{safe_next, Ctx};
use crate::error::{AppError, AppResult};
use crate::i18n::{t, t_dyn, tf};
use crate::service::{discord, gitpack, publish, sets, signal, telegram, ui};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/gitpacks/new", get(gitpack_page).post(gitpack_create))
        .route("/sets/{id}/pull", post(pull))
        .route("/telegram/import", get(telegram_import_page).post(telegram_import))
        .route("/sets/{id}/telegram", get(telegram_export_page).post(telegram_export))
        .route("/discord/import", get(discord_import_page).post(discord_import))
        .route("/signal/import", get(signal_import_page).post(signal_import))
}

// ---------------------------------------------------------------------------
// Links on other pages
// ---------------------------------------------------------------------------

/// Extra buttons on the sets list.
pub fn set_list_links(state: &AppState) -> Markup {
    html! {
        @if gitpack::enabled(state) { a.button href="/gitpacks/new" { (t("Add a git pack")) } }
        @if telegram::enabled(state) { a.button href="/telegram/import" { (t("Import a Telegram pack")) } }
        @if discord::enabled(state) { a.button href="/discord/import" { (t("Import from Discord")) } }
        @if signal::enabled(state) { a.button href="/signal/import" { (t("Import a Signal pack")) } }
    }
}

/// Extra section on the upload page.
pub fn upload_page_links(state: &AppState) -> Markup {
    let git = gitpack::enabled(state);
    let tg = telegram::enabled(state);
    let dc = discord::enabled(state);
    let sg = signal::enabled(state);
    html! {
        @if git || tg || dc || sg {
            section aria-labelledby="up-more" {
                h2 #up-more { (t("More sources")) }
                ul.link-list {
                    @if git { li { a href="/gitpacks/new" { (t("Subscribe to a git repository as a set")) } } }
                    @if tg { li { a href="/telegram/import" { (t("Import a Telegram sticker pack")) } } }
                    @if dc { li { a href="/discord/import" { (t("Import the stickers or emoji of a Discord server")) } } }
                    @if sg { li { a href="/signal/import" { (t("Import a Signal sticker pack")) } } }
                }
            }
        }
    }
}

/// Owner actions on the set page: "Pull now" for git packs, "Publish to
/// Telegram" (to the export page's publish section) when the integration
/// is on, else "Publish" when another platform is.
pub fn set_actions(state: &AppState, ctx: &Ctx, set: &Set) -> Markup {
    let pull_url = format!("/sets/{}/pull", set.id);
    html! {
        @if set.git.is_some() && gitpack::enabled(state) {
            form.inline method="post" action=(pull_url) hx-post=(pull_url) hx-target="#set-job" hx-swap="innerHTML" {
                (ctx.csrf_input())
                button type="submit" { (t("Pull now")) }
            }
        }
        @if telegram::enabled(state) && set.item_count > 0 {
            a.button href=(format!("/sets/{}/export#publish", set.id)) {
                @if set.telegram_pack.is_some() { (t("Update Telegram pack")) } @else { (t("Publish to Telegram")) }
            }
        } @else if set.item_count > 0 && !publish::configured(state).is_empty() {
            a.button href=(format!("/sets/{}/export#publish", set.id)) { (t("Publish")) }
        }
        div #set-job aria-live="polite" {}
    }
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

fn kind_label(k: JobKind) -> &'static str {
    match k {
        JobKind::Variants => t("Generating variants"),
        JobKind::Enrich => t("AI suggestions"),
        JobKind::GitPull => t("Git pull"),
        JobKind::GitPush => t("Git push"),
        JobKind::TelegramImport => t("Telegram import"),
        JobKind::TelegramExport => t("Telegram export"),
        JobKind::WhatsappExport => t("WhatsApp packs"),
        JobKind::DiscordImport => t("Discord import"),
        JobKind::DiscordExport => t("Discord export"),
        JobKind::SignalImport => t("Signal import"),
        JobKind::SignalExport => t("Signal export"),
        JobKind::Recheck => t("Recheck"),
        JobKind::Other => t("Job"),
    }
}

/// [`status_label`] in the request's language (the label itself is a CSS
/// class too).
fn status_text(s: JobStatus) -> &'static str {
    match s {
        JobStatus::Queued => t("queued"),
        JobStatus::Running => t("running"),
        JobStatus::Done => t("done"),
        JobStatus::Failed => t("failed"),
    }
}

fn status_label(s: JobStatus) -> &'static str {
    match s {
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::Done => "done",
        JobStatus::Failed => "failed",
    }
}

/// The link to what a job published or imported from: "Open in
/// <platform>".
fn open_label(kind: JobKind) -> String {
    match crate::service::publish::for_job(kind) {
        Some(p) => tf("Open in {platform}", &[("platform", &p.name())]),
        None if kind == JobKind::DiscordImport => tf("Open in {platform}", &[("platform", &"Discord")]),
        None => t("Open in Telegram").to_string(),
    }
}

pub fn job_running(job: &Job) -> bool {
    matches!(job.status, JobStatus::Queued | JobStatus::Running)
}

/// Status of one job; polls itself while the job is queued or running.
pub fn job_fragment(job: &Job, back: Option<&str>) -> Markup {
    let running = job_running(job);
    let back = safe_next(back);
    let poll = format!(
        "/jobs/{}{}",
        job.id,
        back.map(|b| format!(
            "?back={}",
            form_urlencoded::byte_serialize(b.as_bytes()).collect::<String>()
        ))
        .unwrap_or_default()
    );
    let result_set = job
        .result
        .as_ref()
        .and_then(|r| r.get("set_id"))
        .and_then(|v| v.as_str());
    let result_url = job
        .result
        .as_ref()
        .and_then(|r| r.get("url"))
        .and_then(|v| v.as_str())
        .filter(|u| u.starts_with("https://"));
    html! {
        div class=(format!("job job-{}", status_label(job.status))) id=(format!("job-{}", job.id)) role="status"
            hx-get=[running.then_some(poll.as_str())] hx-trigger=[running.then_some("every 2s")] hx-swap="outerHTML" {
            strong { (kind_label(job.kind)) } ": " (status_text(job.status))
            @if running { span.spinner aria-hidden="true" {} }
            @if job.attempts > 1 { " " (tf("(attempt {n})", &[("n", &job.attempts)])) }
            @if let Some(e) = &job.error { p.error-text { (e) } }
            @if job.status == JobStatus::Done {
                @if let Some(skipped) = job.result.as_ref().and_then(|r| r.get("skipped")).and_then(|v| v.as_str()) {
                    p.muted { (skipped) }
                }
                p.job-links {
                    @if let Some(sid) = result_set { a href=(set_url(sid)) { (t("Open the set")) } " " }
                    @if let Some(u) = result_url { a href=(u) rel="noopener noreferrer" target="_blank" { (open_label(job.kind)) } " " }
                    @if let Some(b) = back { a href=(b) { (t("Reload to see the changes")) } }
                }
            }
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct JobQuery {
    #[serde(default)]
    back: Option<String>,
}

pub async fn job_status(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Query(q): Query<JobQuery>,
) -> AppResult<Markup> {
    let job = ui::job(&state, &ctx.user, &id).await?;
    let fragment = job_fragment(&job, q.back.as_deref());
    if ctx.htmx {
        return Ok(fragment);
    }
    let head = html! {
        @if job_running(&job) { noscript { meta http-equiv="refresh" content="3"; } }
    };
    let body = html! {
        h1 { (t("Background job")) }
        (fragment)
        p.muted.small { (tf("Started {time}.", &[("time", &fmt_time(job.created_at))])) }
        @if let Some(b) = safe_next(q.back.as_deref()) { p { a href=(b) { "← " (t("back")) } } }
    };
    Ok(layout::page_with_head(
        Some(&ctx.user),
        t("Background job"),
        Nav::None,
        head,
        body,
    ))
}

// ---------------------------------------------------------------------------
// Git packs
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct GitPackForm {
    #[serde(default)]
    url: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    subdir: String,
    #[serde(default)]
    mode: String,
    #[serde(default)]
    slug: String,
}

fn gitpack_form(ctx: &Ctx, f: &GitPackForm, error: Option<&str>) -> Markup {
    let body = html! {
        p.crumbs { a href="/sets" { "← " (t("Sets")) } }
        h1 { (t("Add a git pack")) }
        p {
            (t("Subscribe to a git repository that holds a set directory")) " (" code { "set.toml" } " " (t("plus media")) ")."
            " " (t("The server clones it, imports it as a new set and pulls again on a schedule or when the webhook fires."))
        }
        @if let Some(e) = error { (layout::alert("error", e)) }
        form.stack.narrow method="post" action="/gitpacks/new" {
            (ctx.csrf_input())
            div.field-row {
                label for="gp-url" { (t("Repository URL")) }
                input #gp-url type="url" name="url" value=(f.url) required placeholder="https://github.com/someone/cats.git"
                    spellcheck="false" autocapitalize="none";
            }
            div.two-col {
                div.field-row {
                    label for="gp-branch" { (t("Branch (optional)")) }
                    input #gp-branch type="text" name="branch" value=(f.branch) placeholder=(t("default branch")) spellcheck="false";
                }
                div.field-row {
                    label for="gp-subdir" { (t("Folder in the repository (optional)")) }
                    input #gp-subdir type="text" name="subdir" value=(f.subdir) placeholder=(t("repository root")) spellcheck="false";
                }
            }
            fieldset.field-row {
                legend { (t("Mode")) }
                label.radio {
                    input type="radio" name="mode" value="read_only" checked[f.mode != "push"];
                    " " strong { (t("Read-only")) } " — " (t("edit upstream; the web UI shows an “Edit upstream” link"))
                }
                label.radio {
                    input type="radio" name="mode" value="push" checked[f.mode == "push"];
                    " " strong { (t("Push edits back")) } " — " (t("needs push credentials on the server"))
                }
            }
            div.field-row {
                label for="gp-slug" { (t("Slug (optional)")) }
                input #gp-slug type="text" name="slug" value=(f.slug) maxlength="64" pattern="[a-z0-9][a-z0-9\\-]*" spellcheck="false";
            }
            button.primary type="submit" { (t("Add git pack")) }
        }
    };
    ctx.page(t("Add a git pack"), Nav::Sets, body)
}

pub async fn gitpack_page(State(state): State<AppState>, ctx: Ctx) -> AppResult<Markup> {
    if !gitpack::enabled(&state) {
        return Err(AppError::not_found());
    }
    Ok(gitpack_form(&ctx, &GitPackForm::default(), None))
}

fn opt(s: &str) -> Option<String> {
    Some(s.trim().to_string()).filter(|s| !s.is_empty())
}

pub async fn gitpack_create(
    State(state): State<AppState>,
    ctx: Ctx,
    Form(f): Form<GitPackForm>,
) -> AppResult<Response> {
    let req = CreateGitPack {
        url: f.url.trim().to_string(),
        branch: opt(&f.branch),
        subdir: opt(&f.subdir),
        mode: if f.mode == "push" {
            GitMode::Push
        } else {
            GitMode::ReadOnly
        },
        slug: opt(&f.slug),
    };
    let created = match gitpack::create(&state, &ctx.user, req).await {
        Ok(c) => c,
        Err(e) if e.status.is_client_error() && e.code != "not_found" => {
            return Ok((e.status, gitpack_form(&ctx, &f, Some(&t_dyn(&e.message)))).into_response());
        }
        Err(e) => return Err(e),
    };
    let set = &created.set;
    let body = html! {
        h1 { (t("Git pack added")) }
        (layout::alert("success", &tf("“{name}” is being cloned.", &[("name", &set.name)])))
        (job_fragment(&created.job, Some(&set_url(&set.id))))
        section.secret-box {
            h2 { (t("Webhook")) }
            p { (t("To pull on every push, add this webhook to the repository.")) " " strong { (t("The secret is shown only now.")) } }
            dl.kv {
                dt { "Payload URL" } dd { code.copyable { (created.webhook_url) } }
                dt { (t("Secret")) } dd { code.copyable { (created.webhook_secret) } }
            }
            p.hint {
                (t("GitHub/Gitea/Forgejo: use the secret for the HMAC signature (X-Hub-Signature-256)."))
                " " (t("Others: send it in the X-Stickpile-Webhook-Secret header."))
            }
        }
        p { a.button.primary href=(set_url(&set.id)) { (t("Open the set")) } }
    };
    Ok(ctx.page(t("Git pack added"), Nav::Sets, body).into_response())
}

pub async fn pull(State(state): State<AppState>, ctx: Ctx, Path(id): Path<String>) -> AppResult<Response> {
    let job = gitpack::pull_now(&state, &ctx.user, &id).await?;
    let back = set_url(&id);
    if ctx.htmx {
        Ok(job_fragment(&job, Some(&back)).into_response())
    } else {
        Ok(ctx.redirect(&format!(
            "/jobs/{}?back={}",
            job.id,
            form_urlencoded::byte_serialize(back.as_bytes()).collect::<String>()
        )))
    }
}

// ---------------------------------------------------------------------------
// Telegram
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct TgImportForm {
    #[serde(default)]
    pack_name: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    visibility: String,
}

fn tg_import_form(ctx: &Ctx, f: &TgImportForm, error: Option<&str>) -> Markup {
    let body = html! {
        p.crumbs { a href="/sets" { "← " (t("Sets")) } }
        h1 { (t("Import a Telegram sticker pack")) }
        p { (t("The pack becomes a new set; each sticker keeps its emoji. Animated (.tgs) stickers are skipped.")) }
        @if let Some(e) = error { (layout::alert("error", e)) }
        form.stack.narrow method="post" action="/telegram/import" {
            (ctx.csrf_input())
            div.field-row {
                label for="tg-pack" { (t("Pack name or link")) }
                input #tg-pack type="text" name="pack_name" value=(f.pack_name) required
                    placeholder="https://t.me/addstickers/SomePack" spellcheck="false" autocapitalize="none";
            }
            div.field-row {
                label for="tg-slug" { (t("Slug (optional)")) }
                input #tg-slug type="text" name="slug" value=(f.slug) maxlength="64" pattern="[a-z0-9][a-z0-9\\-]*" spellcheck="false";
            }
            div.field-row {
                label for="tg-vis" { (t("Visibility")) }
                select #tg-vis name="visibility" {
                    option value="private" selected[f.visibility != "unlisted" && f.visibility != "public"] { (t("Private")) }
                    option value="unlisted" selected[f.visibility == "unlisted"] { (t("Unlisted")) }
                    option value="public" selected[f.visibility == "public"] { (t("Public")) }
                }
            }
            button.primary type="submit" { (t("Import")) }
        }
    };
    ctx.page(t("Import a Telegram pack"), Nav::Sets, body)
}

pub async fn telegram_import_page(State(state): State<AppState>, ctx: Ctx) -> AppResult<Markup> {
    if !telegram::enabled(&state) {
        return Err(AppError::not_found());
    }
    Ok(tg_import_form(&ctx, &TgImportForm::default(), None))
}

pub async fn telegram_import(
    State(state): State<AppState>,
    ctx: Ctx,
    Form(f): Form<TgImportForm>,
) -> AppResult<Response> {
    let req = TelegramImport {
        pack_name: f.pack_name.clone(),
        slug: opt(&f.slug),
        visibility: match f.visibility.as_str() {
            "public" => Visibility::Public,
            "unlisted" => Visibility::Unlisted,
            _ => Visibility::Private,
        },
    };
    match telegram::import(&state, &ctx.user, req).await {
        Ok(job) => Ok(ctx.redirect(&format!("/jobs/{}", job.id))),
        Err(e) if e.status.is_client_error() && e.code != "not_found" => {
            Ok((e.status, tg_import_form(&ctx, &f, Some(&t_dyn(&e.message)))).into_response())
        }
        Err(e) => Err(e),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct TgExportForm {
    #[serde(default)]
    pack_name: String,
    #[serde(default)]
    title: String,
}

fn tg_export_form(ctx: &Ctx, set: &Set, f: &TgExportForm, error: Option<&str>) -> Markup {
    let has_owner = ctx.user.user.telegram_user_id.is_some();
    let body = html! {
        p.crumbs { a href=(set_url(&set.id)) { "← " (set.name) } }
        h1 { (tf("Publish “{name}” to Telegram", &[("name", &set.name)])) }
        p {
            (t("Every item becomes a sticker (stills as 512 px WebP, animations and videos as video stickers)."))
            " " (t("The first export creates the pack, later ones update it. Items that cannot become stickers are listed afterwards."))
        }
        @if !set.telegram_parts.is_empty() {
            p {
                (t("Currently published as")) " "
                @for (i, part) in set.telegram_parts.iter().enumerate() {
                    @if i > 0 { ", " }
                    a href=(telegram::pack_url(&part.name)) rel="noopener noreferrer" target="_blank" { (part.name) }
                }
                "."
            }
        } @else if let Some(p) = &set.telegram_pack {
            p { (t("Currently published as")) " " a href=(telegram::pack_url(p)) rel="noopener noreferrer" target="_blank" { (p) } "." }
        }
        @if !has_owner {
            (layout::alert("info", t("Telegram packs belong to a Telegram user. Set your Telegram user id in Settings, unless the administrator configured a default owner.")))
        }
        @if let Some(e) = error { (layout::alert("error", e)) }
        form.stack.narrow method="post" action=(format!("/sets/{}/telegram", set.id)) {
            (ctx.csrf_input())
            div.field-row {
                label for="tge-name" { (t("Pack short name (optional)")) }
                input #tge-name type="text" name="pack_name" value=(f.pack_name) maxlength="64" spellcheck="false"
                    aria-describedby="tge-name-hint";
                p.hint #tge-name-hint { (t("Letters, digits and _, e.g. my_cats; _by_<bot name> is added for you. Empty: the bound pack, or one derived from the slug.")) }
            }
            div.field-row {
                label for="tge-title" { (t("Pack title (optional)")) }
                input #tge-title type="text" name="title" value=(f.title) maxlength="64" placeholder=(set.name);
            }
            button.primary type="submit" { (t("Publish")) }
        }
    };
    ctx.page(t("Publish to Telegram"), Nav::Sets, body)
}

async fn own_set(state: &AppState, ctx: &Ctx, id: &str) -> AppResult<Set> {
    if !telegram::enabled(state) {
        return Err(AppError::not_found());
    }
    let set = sets::detail(state, &ctx.user, id).await?.set;
    if set.owner_id != ctx.id() {
        return Err(AppError::forbidden("only the owner can publish this set"));
    }
    Ok(set)
}

pub async fn telegram_export_page(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> AppResult<Markup> {
    let set = own_set(&state, &ctx, &id).await?;
    let f = TgExportForm {
        // The name the parts derive from (part 1's, also when part 1's own
        // pack was deleted and the set links to a later part).
        pack_name: telegram::base_name(&state.db, &set.id).await?.unwrap_or_default(),
        title: String::new(),
    };
    Ok(tg_export_form(&ctx, &set, &f, None))
}

pub async fn telegram_export(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Form(f): Form<TgExportForm>,
) -> AppResult<Response> {
    let set = own_set(&state, &ctx, &id).await?;
    let req = TelegramExport {
        pack_name: opt(&f.pack_name),
        title: opt(&f.title),
        repack: false,
    };
    match telegram::export(&state, &ctx.user, &id, req).await {
        Ok(job) => {
            let back = set_url(&id);
            Ok(ctx.redirect(&format!(
                "/jobs/{}?back={}",
                job.id,
                form_urlencoded::byte_serialize(back.as_bytes()).collect::<String>()
            )))
        }
        Err(e) if e.status.is_client_error() && e.code != "not_found" => {
            Ok((e.status, tg_export_form(&ctx, &set, &f, Some(&t_dyn(&e.message)))).into_response())
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Discord and Signal import
// ---------------------------------------------------------------------------

fn visibility_select(id: &str, value: &str) -> Markup {
    html! {
        div.field-row {
            label for=(id) { (t("Visibility")) }
            select #(id) name="visibility" {
                option value="private" selected[value != "unlisted" && value != "public"] { (t("Private")) }
                option value="unlisted" selected[value == "unlisted"] { (t("Unlisted")) }
                option value="public" selected[value == "public"] { (t("Public")) }
            }
        }
    }
}

fn visibility_of(v: &str) -> Visibility {
    match v {
        "public" => Visibility::Public,
        "unlisted" => Visibility::Unlisted,
        _ => Visibility::Private,
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct DiscordImportForm {
    #[serde(default)]
    guild_id: String,
    #[serde(default)]
    stickers: Option<String>,
    #[serde(default)]
    emojis: Option<String>,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    visibility: String,
    /// Sent by the form itself, so that unticked boxes mean "no".
    #[serde(default)]
    sent: Option<String>,
}

fn discord_import_form(ctx: &Ctx, f: &DiscordImportForm, error: Option<&str>) -> Markup {
    let stickers = f.sent.is_none() || f.stickers.is_some();
    let emojis = f.sent.is_some() && f.emojis.is_some();
    let body = html! {
        p.crumbs { a href="/sets" { "← " (t("Sets")) } }
        h1 { (t("Import from Discord")) }
        p {
            (t("A Discord server's stickers and/or emoji become a new set, with their names, emoji and tags. The bot must be a member of the server."))
            " " (t("Stickers and emoji the bot made there stay linked to the new set, so publishing it back to the same server keeps them."))
        }
        @if let Some(e) = error { (layout::alert("error", e)) }
        form.stack.narrow method="post" action="/discord/import" {
            (ctx.csrf_input())
            input type="hidden" name="sent" value="1";
            div.field-row {
                label for="dc-guild" { (t("Discord server id")) }
                input #dc-guild type="text" name="guild_id" value=(f.guild_id) required inputmode="numeric"
                    placeholder="100000000000000001" spellcheck="false" autocapitalize="none" aria-describedby="dc-guild-hint";
                p.hint #dc-guild-hint { (t("In Discord: Settings, Advanced, Developer Mode on; then right-click the server and Copy Server ID.")) }
            }
            fieldset.field-row {
                legend { (t("What to import")) }
                label.check { input type="checkbox" name="stickers" value="on" checked[stickers]; " " (t("Stickers")) }
                label.check { input type="checkbox" name="emojis" value="on" checked[emojis]; " " (t("Emoji")) }
            }
            div.field-row {
                label for="dc-slug" { (t("Slug (optional)")) }
                input #dc-slug type="text" name="slug" value=(f.slug) maxlength="64" pattern="[a-z0-9][a-z0-9\\-]*" spellcheck="false";
            }
            (visibility_select("dc-vis", &f.visibility))
            button.primary type="submit" { (t("Import")) }
        }
    };
    ctx.page(t("Import from Discord"), Nav::Sets, body)
}

pub async fn discord_import_page(State(state): State<AppState>, ctx: Ctx) -> AppResult<Markup> {
    if !discord::enabled(&state) {
        return Err(AppError::not_found());
    }
    Ok(discord_import_form(&ctx, &DiscordImportForm::default(), None))
}

pub async fn discord_import(
    State(state): State<AppState>,
    ctx: Ctx,
    Form(f): Form<DiscordImportForm>,
) -> AppResult<Response> {
    let req = DiscordImport {
        guild_id: f.guild_id.clone(),
        stickers: f.stickers.is_some(),
        emojis: f.emojis.is_some(),
        slug: opt(&f.slug),
        visibility: visibility_of(&f.visibility),
    };
    match discord::import(&state, &ctx.user, req).await {
        Ok(job) => Ok(ctx.redirect(&format!("/jobs/{}", job.id))),
        Err(e) if e.status.is_client_error() && e.code != "not_found" => {
            Ok((e.status, discord_import_form(&ctx, &f, Some(&t_dyn(&e.message)))).into_response())
        }
        Err(e) => Err(e),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct SignalImportForm {
    #[serde(default)]
    link: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    visibility: String,
}

fn signal_import_form(ctx: &Ctx, f: &SignalImportForm, error: Option<&str>) -> Markup {
    let body = html! {
        p.crumbs { a href="/sets" { "← " (t("Sets")) } }
        h1 { (t("Import a Signal sticker pack")) }
        p {
            (t("The pack becomes a new set; each sticker keeps its emoji, and the pack's cover becomes the set's cover. Animated stickers keep only their first frame."))
        }
        p.hint {
            (t("The link holds the key that opens the pack. The server uses it for this import only and does not keep it."))
        }
        @if let Some(e) = error { (layout::alert("error", e)) }
        form.stack.narrow method="post" action="/signal/import" {
            (ctx.csrf_input())
            div.field-row {
                label for="sg-link" { (t("Pack link")) }
                input #sg-link type="text" name="link" value=(f.link) required autocomplete="off"
                    placeholder="https://signal.art/addstickers/#pack_id=...&pack_key=..." spellcheck="false" autocapitalize="none";
            }
            div.field-row {
                label for="sg-slug" { (t("Slug (optional)")) }
                input #sg-slug type="text" name="slug" value=(f.slug) maxlength="64" pattern="[a-z0-9][a-z0-9\\-]*" spellcheck="false";
            }
            (visibility_select("sg-vis", &f.visibility))
            button.primary type="submit" { (t("Import")) }
        }
    };
    ctx.page(t("Import a Signal pack"), Nav::Sets, body)
}

pub async fn signal_import_page(State(state): State<AppState>, ctx: Ctx) -> AppResult<Markup> {
    if !signal::enabled(&state) {
        return Err(AppError::not_found());
    }
    Ok(signal_import_form(&ctx, &SignalImportForm::default(), None))
}

pub async fn signal_import(
    State(state): State<AppState>,
    ctx: Ctx,
    Form(f): Form<SignalImportForm>,
) -> AppResult<Response> {
    let req = SignalImport {
        link: f.link.clone(),
        slug: opt(&f.slug),
        visibility: visibility_of(&f.visibility),
    };
    match signal::import(&state, &ctx.user, req).await {
        Ok(job) => Ok(ctx.redirect(&format!("/jobs/{}", job.id))),
        Err(e) if e.status.is_client_error() && e.code != "not_found" => {
            // The form comes back without the link: it is a secret.
            let f = SignalImportForm {
                link: String::new(),
                ..f
            };
            Ok((e.status, signal_import_form(&ctx, &f, Some(&t_dyn(&e.message)))).into_response())
        }
        Err(e) => Err(e),
    }
}
