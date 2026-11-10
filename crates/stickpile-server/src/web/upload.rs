//! Bulk upload (file input, drag-and-drop via app.js) and set-directory
//! import (folder input or tar), with results: errors per file and
//! near-duplicate warnings ("You already have this in set X").
//!
//! All forms are `multipart/form-data` with the CSRF token as the first
//! field (see [`super::MultipartCaller`]); htmx submits them with the
//! `X-CSRF-Token` header and swaps the results in under the form.

use axum::extract::{Path, Query, Request, State};
use axum::response::{IntoResponse, Response};
use maud::{html, Markup};
use serde::Deserialize;
use stickpile_api::{ImportReport, NearDuplicate, SetSummary, UploadResult};

use super::components::{item_url, plural, set_url, thumb};
use super::layout::{self, Nav};
use super::{integrations, Ctx, MultipartCaller};
use crate::authz;
use crate::error::{AppError, AppResult};
use crate::service::upload::{self, field_text, field_to_file, multipart_error, UploadParts};
use crate::service::{sets, transfer, ui};
use crate::state::AppState;

const ACCEPT: &str =
    "image/png,image/jpeg,image/webp,image/gif,video/mp4,video/webm,.png,.jpg,.jpeg,.webp,.gif,.mp4,.webm";

/// The upload form. `set_id` fixes the target set (set page); otherwise the
/// form has a set selector over `sets`.
pub fn upload_form(ctx: &Ctx, set_id: Option<&str>, sets: &[SetSummary]) -> Markup {
    upload_form_selected(ctx, set_id, sets, None)
}

fn upload_form_selected(ctx: &Ctx, set_id: Option<&str>, sets: &[SetSummary], selected: Option<&str>) -> Markup {
    let action = match set_id {
        Some(id) => format!("/sets/{id}/upload"),
        None => "/upload".to_string(),
    };
    let key = set_id.unwrap_or("any");
    let results_id = format!("upload-results-{key}");
    html! {
        form.upload-form method="post" enctype="multipart/form-data" action=(action)
            hx-post=(action) hx-encoding="multipart/form-data" hx-target=(format!("#{results_id}")) hx-swap="innerHTML"
            hx-disabled-elt="find button[type=submit]" {
            (ctx.csrf_input())
            @if set_id.is_none() {
                div.field-row {
                    label for="upload-set" { "Add to set" }
                    select #upload-set name="set_id" required {
                        @for s in sets {
                            option value=(s.id) selected[selected == Some(s.id.as_str())] { (s.name) }
                        }
                    }
                }
            }
            div.dropzone data-dropzone="" {
                label for=(format!("upload-files-{key}")) {
                    strong { "Choose files" } " or drop them here"
                }
                input id=(format!("upload-files-{key}")) type="file" name="file" multiple required accept=(ACCEPT);
                p.hint { "PNG, JPEG, WebP, GIF, MP4 or WebM. The type is detected from the content; metadata is stripped." }
            }
            button.primary type="submit" { "Upload" }
            span.busy aria-hidden="true" { "Uploading…" }
        }
        div.upload-results id=(results_id) aria-live="polite" {}
    }
}

/// Folder and tar import forms. `into` imports into (replaces the contents
/// of) an existing set; otherwise a new set is created.
pub fn import_form(ctx: &Ctx, into: Option<&str>) -> Markup {
    let action = match into {
        Some(id) => format!("/sets/{id}/import"),
        None => "/import".to_string(),
    };
    let key = into.unwrap_or("new");
    let results_id = format!("import-results-{key}");
    let form = |kind: &str, input: Markup, button: &str| {
        html! {
            form.import-form method="post" enctype="multipart/form-data" action=(action)
                hx-post=(action) hx-encoding="multipart/form-data" hx-target=(format!("#{results_id}")) hx-swap="innerHTML"
                hx-disabled-elt="find button[type=submit]" data-import=(kind) {
                (ctx.csrf_input())
                (input)
                button type="submit" { (button) }
                span.busy aria-hidden="true" { "Importing…" }
            }
        }
    };
    html! {
        @if into.is_some() {
            p.warning {
                "The set becomes equal to the directory: matching files get the directory's metadata, new files are added, "
                strong { "items missing from the directory are removed" } ", and the order follows the file names."
            }
        } @else {
            p.hint { "A set directory holds a " code { "set.toml" } " and the media files (" code { "001-cat.png" } ", …). Exports of other stickpile servers work too." }
        }
        div.import-forms {
            (form("folder", html! {
                div.field-row {
                    label for=(format!("import-dir-{key}")) { "Set folder" }
                    input id=(format!("import-dir-{key}")) type="file" name="files" webkitdirectory multiple required;
                }
            }, "Import folder"))
            (form("archive", html! {
                div.field-row {
                    label for=(format!("import-tar-{key}")) { "Or an export archive (.tar)" }
                    input id=(format!("import-tar-{key}")) type="file" name="archive" accept=".tar,application/x-tar" required;
                }
            }, "Import archive"))
        }
        div.import-results id=(results_id) aria-live="polite" {}
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct UploadPageQuery {
    #[serde(default)]
    set: Option<String>,
}

pub async fn page(State(state): State<AppState>, ctx: Ctx, Query(q): Query<UploadPageQuery>) -> AppResult<Markup> {
    let sets = ui::editable_sets(&state, &ctx.user).await?;
    let body = html! {
        h1 { "Upload" }
        section aria-labelledby="up-files" {
            h2 #up-files { "Add files to a set" }
            @if sets.is_empty() {
                p { "You need a set first. " a.button href="/sets/new" { "Create a set" } }
            } @else {
                (upload_form_selected(&ctx, None, &sets, q.set.as_deref()))
            }
        }
        section #import aria-labelledby="up-import" {
            h2 #up-import { "Import a set directory as a new set" }
            (import_form(&ctx, None))
        }
        (integrations::upload_page_links(&state))
    };
    Ok(ctx.page("Upload", Nav::Upload, body))
}

/// Read an upload body: optional `set_id` text field (before the files),
/// file parts (empty file inputs are skipped).
async fn read_upload(
    state: &AppState,
    caller: &MultipartCaller,
    req: Request,
    path_set: Option<&str>,
) -> AppResult<(String, UploadParts)> {
    let mut mp = caller.multipart(state, req).await?;
    let max_file = state.config.limits.max_upload_mb * 1024 * 1024;
    let mut set_id = path_set.map(str::to_string);
    let mut checked = false;
    let mut files = Vec::new();
    while let Some(mut field) = mp.next_field().await.map_err(multipart_error)? {
        let filename = field.file_name().map(str::to_string);
        match (field.name(), filename) {
            (Some("set_id"), None) if path_set.is_none() => {
                set_id = Some(field_text(&mut field, 64).await?.trim().to_string());
            }
            (_, Some(name)) if !name.is_empty() => {
                let sid = set_id
                    .as_deref()
                    .ok_or_else(|| AppError::bad_request("choose a set before the files"))?;
                if !checked {
                    // Authorize before writing any temp file.
                    authz::editable_set(state, caller.ctx.id(), sid).await?;
                    checked = true;
                }
                files.push(field_to_file(state, &mut field, max_file).await?);
            }
            _ => {}
        }
    }
    let set_id = set_id.ok_or_else(|| AppError::bad_request("choose a set"))?;
    if files.is_empty() {
        return Err(AppError::bad_request("choose at least one file"));
    }
    Ok((
        set_id,
        UploadParts {
            files,
            meta: Vec::new(),
        },
    ))
}

async fn do_upload(
    state: &AppState,
    caller: MultipartCaller,
    req: Request,
    path_set: Option<&str>,
) -> AppResult<Response> {
    let (set_id, parts) = read_upload(state, &caller, req, path_set).await?;
    let ctx = &caller.ctx;
    let single_name = (parts.files.len() == 1).then(|| upload::base_name(&parts.files[0].filename).to_string());
    let results = match upload::upload(state, &ctx.user, &set_id, parts).await {
        Ok(r) => r,
        // A single failed file comes back as an error; show it like the others.
        Err(e) if matches!(e.code, "unsupported_media" | "too_large" | "bad_request" | "conflict") => {
            vec![UploadResult {
                file: single_name.unwrap_or_default(),
                item: None,
                error: Some(e.message),
                already_in_set: false,
                near_duplicates: Vec::new(),
            }]
        }
        Err(e) => return Err(e),
    };
    let fragment = upload_results(&results, &set_id);
    if ctx.htmx {
        // On the set page, refresh its item grid too.
        let grid = match path_set {
            Some(id) => refreshed_grid(state, ctx, id).await?,
            None => html! {},
        };
        Ok(html! { (fragment) (grid) }.into_response())
    } else {
        let body = html! {
            p.crumbs { a href=(set_url(&set_id)) { "← back to the set" } }
            h1 { "Upload results" }
            (fragment)
        };
        Ok(ctx.page("Upload results", Nav::Upload, body).into_response())
    }
}

/// The set page's item grid as an out-of-band swap.
async fn refreshed_grid(state: &AppState, ctx: &Ctx, set_id: &str) -> AppResult<Markup> {
    let detail = sets::detail(state, &ctx.user, set_id).await?;
    Ok(super::sets::editor_grid(
        ctx,
        &detail,
        super::sets::Sort::Position,
        0,
        true,
    ))
}

pub async fn upload_any(State(state): State<AppState>, caller: MultipartCaller, req: Request) -> AppResult<Response> {
    do_upload(&state, caller, req, None).await
}

pub async fn upload_to_set(
    State(state): State<AppState>,
    caller: MultipartCaller,
    Path(id): Path<String>,
    req: Request,
) -> AppResult<Response> {
    do_upload(&state, caller, req, Some(&id)).await
}

fn near_dup_line(n: &NearDuplicate) -> Markup {
    html! {
        li.near-dup {
            "You already have this in set "
            a href=(set_url(&n.existing.set_id)) { (n.existing.set_name) }
            ": "
            a href=(item_url(&n.existing.id)) { (n.existing.name) }
            @if n.same_content { " (the same file)" } @else { " (looks the same, distance " (n.distance) ")" }
        }
    }
}

pub fn upload_results(results: &[UploadResult], set_id: &str) -> Markup {
    let added = results.iter().filter(|r| r.item.is_some() && !r.already_in_set).count() as u64;
    let present = results.iter().filter(|r| r.already_in_set).count() as u64;
    let failed = results.iter().filter(|r| r.error.is_some()).count() as u64;
    html! {
        div.results {
            p.summary {
                strong { (plural(added, "file added", "files added")) }
                @if present > 0 { ", " (present) " already in the set" }
                @if failed > 0 { ", " span.error-text { (failed) " failed" } }
                " · " a href=(set_url(set_id)) { "open the set" }
            }
            ul.result-list {
                @for r in results {
                    li class=(if r.error.is_some() { "err" } else if r.already_in_set { "dup" } else { "ok" }) {
                        @if let Some(item) = &r.item {
                            a.mini-thumb href=(item_url(&item.id)) { (thumb(&item.id, &item.name, item.media.width, item.media.height)) }
                        }
                        div {
                            strong { (r.file) }
                            @if let Some(e) = &r.error {
                                " — " span.error-text { (e) }
                            } @else if r.already_in_set {
                                " — already in this set"
                            } @else if let Some(item) = &r.item {
                                " — added as " a href=(item_url(&item.id)) { (item.name) }
                            }
                            @if !r.near_duplicates.is_empty() {
                                ul.near-dups { @for n in &r.near_duplicates { (near_dup_line(n)) } }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

async fn do_import(state: &AppState, caller: MultipartCaller, req: Request, into: Option<&str>) -> AppResult<Response> {
    if let Some(id) = into {
        authz::editable_set(state, caller.ctx.id(), id).await?;
    }
    let mp = caller.multipart(state, req).await?;
    let ctx = &caller.ctx;
    let report = transfer::import_multipart(state, &ctx.user, into, mp).await?;
    let fragment = import_report(&report);
    if ctx.htmx {
        let grid = match into {
            Some(id) => refreshed_grid(state, ctx, id).await?,
            None => html! {},
        };
        Ok(html! { (fragment) (grid) }.into_response())
    } else {
        let body = html! {
            h1 { "Import results" }
            (fragment)
        };
        Ok(ctx.page("Import results", Nav::Upload, body).into_response())
    }
}

pub async fn import_new(State(state): State<AppState>, caller: MultipartCaller, req: Request) -> AppResult<Response> {
    do_import(&state, caller, req, None).await
}

pub async fn import_into(
    State(state): State<AppState>,
    caller: MultipartCaller,
    Path(id): Path<String>,
    req: Request,
) -> AppResult<Response> {
    do_import(&state, caller, req, Some(&id)).await
}

pub fn import_report(r: &ImportReport) -> Markup {
    html! {
        div.results {
            @if let Some(set) = &r.set {
                (layout::alert("success", &format!("Imported into “{}”.", set.name)))
                p { a.button.primary href=(set_url(&set.id)) { "Open the set" } }
            }
            dl.kv {
                dt { "Added" } dd { (r.created) }
                dt { "Updated" } dd { (r.updated) }
                dt { "Unchanged" } dd { (r.unchanged) }
                dt { "Removed" } dd { (r.removed) }
            }
            @if !r.errors.is_empty() {
                h3 { "Problems" }
                ul.result-list {
                    @for e in &r.errors {
                        li.err {
                            @if !e.file.is_empty() { strong { (e.file) } " — " }
                            span.error-text { (e.message) }
                        }
                    }
                }
            }
            @if !r.near_duplicates.is_empty() {
                h3 { "Already in your library" }
                ul.near-dups { @for n in &r.near_duplicates { (near_dup_line(n)) } }
            }
        }
    }
}
