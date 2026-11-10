//! Sets: browse (mine / public), create, the set page with its item editor,
//! settings, delete, reordering.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Form;
use maud::{html, Markup};
use serde::Deserialize;
use stickpile_api::routes as r;
use stickpile_api::{CreateSet, Item, ReorderItems, Set, SetDetail, SetSource, UpdateSet, Visibility};

use super::components::{fmt_time, grid, opt_time, plural, set_card, set_url, source_badge, visibility_badge};
use super::layout::{self, Nav};
use super::{integrations, items, upload, Ctx};
use crate::error::{AppError, AppResult};
use crate::service::sets::{self, SetScope};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Browse
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    tab: Option<String>,
}

pub async fn list(State(state): State<AppState>, ctx: Ctx, Query(q): Query<ListQuery>) -> AppResult<Markup> {
    let public = q.tab.as_deref() == Some("public");
    let scope = if public { SetScope::Public } else { SetScope::Mine };
    let list = sets::list(&state, &ctx.user, scope).await?;
    let body = html! {
        div.section-head {
            h1 { "Sets" }
            a.button.primary href="/sets/new" { "New set" }
            a.button href="/upload#import" { "Import a set directory" }
            (integrations::set_list_links(&state))
        }
        nav.tabs aria-label="Which sets" {
            @if public {
                a href="/sets" { "Mine" }
                a href="/sets?tab=public" aria-current="page" { "Public" }
            } @else {
                a href="/sets" aria-current="page" { "Mine" }
                a href="/sets?tab=public" { "Public" }
            }
        }
        p.muted.small {
            @if public {
                "Public sets of every user on this server. Unlisted sets are only reachable by their link."
            } @else {
                "Your own sets, most recently changed first."
            }
        }
        @if list.is_empty() {
            p.muted { @if public { "No public sets yet." } @else { "You have no sets yet." } }
        } @else {
            (grid(html! { @for s in &list { (set_card(s)) } }))
        }
    };
    Ok(ctx.page("Sets", Nav::Sets, body))
}

// ---------------------------------------------------------------------------
// Create / settings forms
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct SetForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    visibility: String,
    #[serde(default)]
    aspect: String,
    #[serde(default)]
    recommended_px: String,
}

impl SetForm {
    fn from_set(s: &Set) -> SetForm {
        SetForm {
            name: s.name.clone(),
            slug: s.slug.clone(),
            description: s.description.clone(),
            visibility: vis_str(s.visibility).into(),
            aspect: s.aspect_ratio.map(|(w, h)| format!("{w}:{h}")).unwrap_or_default(),
            recommended_px: s.recommended_px.map(|p| p.to_string()).unwrap_or_default(),
        }
    }
}

fn vis_str(v: Visibility) -> &'static str {
    match v {
        Visibility::Private => "private",
        Visibility::Unlisted => "unlisted",
        Visibility::Public => "public",
    }
}

fn parse_visibility(s: &str) -> AppResult<Visibility> {
    match s {
        "" | "private" => Ok(Visibility::Private),
        "unlisted" => Ok(Visibility::Unlisted),
        "public" => Ok(Visibility::Public),
        _ => Err(AppError::bad_request("visibility must be private, unlisted or public")),
    }
}

/// `W:H` (also `W/H`, `WxH`); empty = none.
pub fn parse_aspect(s: &str) -> AppResult<Option<(u32, u32)>> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let bad = || AppError::bad_request("aspect ratio must look like 1:1 or 16:9");
    let (w, h) = s.split_once([':', '/', 'x', 'X']).ok_or_else(bad)?;
    let w: u32 = w.trim().parse().map_err(|_| bad())?;
    let h: u32 = h.trim().parse().map_err(|_| bad())?;
    Ok(Some((w, h)))
}

fn parse_px(s: &str) -> AppResult<Option<u32>> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    s.parse()
        .map(Some)
        .map_err(|_| AppError::bad_request("recommended size must be a number of pixels"))
}

fn set_fields(ctx: &Ctx, f: &SetForm, new: bool) -> Markup {
    let _ = ctx;
    html! {
        div.field-row {
            label for="set-name" { "Name" }
            input #set-name type="text" name="name" value=(f.name) required maxlength="200";
        }
        div.field-row {
            label for="set-slug" { "Slug" }
            input #set-slug type="text" name="slug" value=(f.slug) maxlength="64" pattern="[a-z0-9][a-z0-9\\-]*"
                required[!new] aria-describedby="set-slug-hint" spellcheck="false" autocapitalize="none";
            p.hint #set-slug-hint {
                "Lowercase letters, digits and dashes; used in " code { "set:" } " searches and as the export folder name."
                @if new { " Leave empty to derive it from the name." }
            }
        }
        div.field-row {
            label for="set-desc" { "Description" }
            textarea #set-desc name="description" rows="3" maxlength="5000" { (f.description) }
        }
        fieldset.field-row {
            legend { "Visibility" }
            @for (value, label, hint) in [
                ("private", "Private", "only you"),
                ("unlisted", "Unlisted", "anyone signed in who has the link; not listed or searchable"),
                ("public", "Public", "listed and searchable by every user of this server"),
            ] {
                label.radio {
                    input type="radio" name="visibility" value=(value)
                        checked[f.visibility == value || (f.visibility.is_empty() && value == "private")];
                    " " strong { (label) } " — " (hint)
                }
            }
        }
        div.two-col {
            div.field-row {
                label for="set-aspect" { "Recommended aspect ratio" }
                input #set-aspect type="text" name="aspect" value=(f.aspect) placeholder="1:1" maxlength="11"
                    aria-describedby="set-aspect-hint";
                p.hint #set-aspect-hint { "Width:height, e.g. 1:1. Empty for none." }
            }
            div.field-row {
                label for="set-px" { "Recommended size (px)" }
                input #set-px type="number" name="recommended_px" value=(f.recommended_px) min="16" max="16384"
                    placeholder="512" aria-describedby="set-px-hint";
                p.hint #set-px-hint { "Longest side; 512 for Telegram stickers. Drives the quality score." }
            }
        }
    }
}

fn new_form(ctx: &Ctx, f: &SetForm, error: Option<&str>) -> Markup {
    let body = html! {
        h1 { "New set" }
        @if let Some(e) = error { (layout::alert("error", e)) }
        form.stack.narrow method="post" action="/sets" {
            (ctx.csrf_input())
            (set_fields(ctx, f, true))
            button.primary type="submit" { "Create set" }
        }
        p.muted { "Have a set directory or an export tar? " a href="/upload#import" { "Import it instead" } "." }
    };
    ctx.page("New set", Nav::Sets, body)
}

pub async fn new_page(ctx: Ctx) -> Markup {
    new_form(&ctx, &SetForm::default(), None)
}

fn form_error(e: &AppError) -> String {
    let mut m = e.message.clone();
    if let Some(first) = m.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    m
}

pub async fn create(State(state): State<AppState>, ctx: Ctx, Form(f): Form<SetForm>) -> AppResult<Response> {
    let attempt = async {
        let slug = if f.slug.trim().is_empty() {
            stickpile_setfmt::slugify(&f.name)
        } else {
            f.slug.trim().to_string()
        };
        let req = CreateSet {
            slug,
            name: f.name.clone(),
            description: f.description.clone(),
            visibility: parse_visibility(&f.visibility)?,
            aspect_ratio: parse_aspect(&f.aspect)?,
            recommended_px: parse_px(&f.recommended_px)?,
        };
        sets::create(&state, &ctx.user, req).await
    };
    match attempt.await {
        Ok(set) => Ok(ctx.redirect(&set_url(&set.id))),
        Err(e) if e.status.is_client_error() => {
            Ok((e.status, new_form(&ctx, &f, Some(&form_error(&e)))).into_response())
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Set page
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Position,
    Likes,
    Copies,
    Quality,
    Recent,
}

impl Sort {
    const ALL: [(Sort, &'static str, &'static str); 5] = [
        (Sort::Position, "position", "Set order"),
        (Sort::Likes, "likes", "Most liked"),
        (Sort::Copies, "copies", "Most copied"),
        (Sort::Quality, "quality", "Best quality"),
        (Sort::Recent, "recent", "Recently changed"),
    ];
    fn parse(s: Option<&str>) -> Sort {
        Sort::ALL
            .iter()
            .find(|(_, k, _)| Some(*k) == s)
            .map(|(s, _, _)| *s)
            .unwrap_or(Sort::Position)
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct SetQuery {
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    min_likes: Option<String>,
}

/// Filter ("hide below N likes") and sort a set's items.
pub fn arrange(items: &[Item], sort: Sort, min_likes: u64) -> Vec<&Item> {
    let mut v: Vec<&Item> = items.iter().filter(|i| i.likes >= min_likes).collect();
    match sort {
        Sort::Position => {}
        Sort::Likes => v.sort_by(|a, b| b.likes.cmp(&a.likes).then(a.position.cmp(&b.position))),
        Sort::Copies => v.sort_by(|a, b| b.copies.cmp(&a.copies).then(a.position.cmp(&b.position))),
        Sort::Quality => v.sort_by(|a, b| {
            b.media
                .quality
                .map(i16::from)
                .unwrap_or(-1)
                .cmp(&a.media.quality.map(i16::from).unwrap_or(-1))
                .then(a.position.cmp(&b.position))
        }),
        Sort::Recent => v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.position.cmp(&b.position))),
    }
    v
}

fn git_box(set: &Set) -> Markup {
    html! {
        @if let Some(g) = &set.git {
            aside.git-box {
                h2.small-head { "Git pack" }
                dl.kv {
                    dt { "Repository" } dd { code { (g.url) } }
                    @if let Some(b) = &g.branch { dt { "Branch" } dd { code { (b) } } }
                    dt { "Mode" } dd { (match g.mode { stickpile_api::GitMode::ReadOnly => "read-only (edit upstream)", stickpile_api::GitMode::Push => "edits are pushed back" }) }
                    dt { "Last pull" } dd { (opt_time(g.last_pull_at)) }
                    @if let Some(c) = &g.last_commit { dt { "Commit" } dd { code { (c.chars().take(12).collect::<String>()) } } }
                    @if let Some(e) = &g.last_error { dt { "Last error" } dd.error-text { (e) } }
                }
                @if let Some(web) = &g.web_url {
                    p { a.button href=(web) rel="noopener noreferrer" target="_blank" { "Edit upstream" } }
                }
            }
        }
    }
}

pub async fn page(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Query(q): Query<SetQuery>,
) -> AppResult<Markup> {
    let detail = sets::detail(&state, &ctx.user, &id).await?;
    let sort = Sort::parse(q.sort.as_deref());
    let min_likes: u64 = q.min_likes.as_deref().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let set = &detail.set;
    let is_owner = set.owner_id == ctx.id();
    let body = html! {
        header.set-head {
            div.section-head {
                h1 { (set.name) }
                (visibility_badge(set.visibility)) " " (source_badge(set.source))
            }
            @if !set.description.is_empty() { p.description { (set.description) } }
            p.meta {
                "by " (set.owner_username) " · " (plural(set.item_count, "item", "items"))
                " · version " (set.version) " · updated " (fmt_time(set.updated_at))
                " · " code { (set.slug) }
                @if let Some(tg) = &set.telegram_pack {
                    " · Telegram: " a href=(format!("https://t.me/addstickers/{tg}")) rel="noopener noreferrer" target="_blank" { (tg) }
                }
            }
            nav.actions aria-label="Set actions" {
                @if is_owner { a.button href=(format!("/sets/{}/settings", set.id)) { "Settings" } }
                a.button href=(r::path(r::SET_EXPORT, &[&set.id])) download { "Export (.tar)" }
                a.button href=(format!("/popular?scope=set&set={}", set.id)) { "Popular in this set" }
                @if is_owner { (integrations::set_actions(&state, &ctx, set)) }
            }
            (git_box(set))
            @if set.source == SetSource::Git && !set.editable {
                p.muted { "This set mirrors a git repository and is read-only here. Edit it upstream; changes arrive with the next pull." }
            }
        }
        @if set.editable {
            section.uploader aria-labelledby="upload-head" {
                h2 #upload-head { "Add items" }
                (upload::upload_form(&ctx, Some(&set.id), &[]))
                details.import-into {
                    summary { "Replace from a set directory" }
                    (upload::import_form(&ctx, Some(&set.id)))
                }
            }
        }
        section aria-labelledby="items-head" {
            div.section-head {
                h2 #items-head { "Items" }
                form.filters method="get" action=(set_url(&set.id)) {
                    label for="f-min-likes" { "Hide below" }
                    input #f-min-likes type="number" name="min_likes" min="0" value=(min_likes) class="narrow-num";
                    span { "likes" }
                    label for="f-sort" { "Sort" }
                    select #f-sort name="sort" {
                        @for (s, key, label) in Sort::ALL {
                            option value=(key) selected[s == sort] { (label) }
                        }
                    }
                    button type="submit" { "Apply" }
                }
            }
            (editor_grid(&ctx, &detail, sort, min_likes, false))
        }
    };
    Ok(ctx.page(&set.name, Nav::Sets, body))
}

/// The `#set-items` grid (swapped by reorder requests; `oob` marks it for
/// an htmx out-of-band swap after uploads and imports).
pub fn editor_grid(ctx: &Ctx, detail: &SetDetail, sort: Sort, min_likes: u64, oob: bool) -> Markup {
    let shown = arrange(&detail.items, sort, min_likes);
    let movable = detail.set.editable && sort == Sort::Position && min_likes == 0;
    let last = detail.items.len().saturating_sub(1);
    let hidden = detail.items.len() - shown.len();
    html! {
        div #set-items hx-swap-oob=[oob.then_some("true")] {
            @if detail.items.is_empty() {
                p.muted { "This set is empty." @if detail.set.editable { " Upload files above." } }
            } @else {
                @if hidden > 0 {
                    p.muted.small { (plural(hidden as u64, "item is", "items are")) " hidden by the likes filter." }
                }
                div.grid.editor-grid {
                    // When movable, nothing is filtered or re-sorted, so the
                    // index is the item's place in the set.
                    @for (idx, item) in shown.iter().enumerate() {
                        (items::editor_card(ctx, item, detail.set.editable, movable.then_some((idx == 0, idx >= last))))
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Reorder
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MoveForm {
    #[serde(default)]
    dir: String,
}

pub async fn move_item(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, item)): Path<(String, String)>,
    Form(f): Form<MoveForm>,
) -> AppResult<Response> {
    let detail = sets::detail(&state, &ctx.user, &id).await?;
    if !detail.set.editable {
        return Err(AppError::forbidden("you cannot change this set"));
    }
    let mut ids: Vec<String> = detail.items.iter().map(|i| i.id.clone()).collect();
    let Some(pos) = ids.iter().position(|i| *i == item) else {
        return Err(AppError::not_found());
    };
    let to = match f.dir.as_str() {
        "up" => pos.saturating_sub(1),
        "down" => (pos + 1).min(ids.len() - 1),
        "top" => 0,
        "bottom" => ids.len() - 1,
        _ => return Err(AppError::bad_request("dir must be up, down, top or bottom")),
    };
    let moved = ids.remove(pos);
    ids.insert(to, moved);
    let detail = sets::reorder(&state, &ctx.user, &id, ReorderItems { item_ids: ids }).await?;
    Ok(ctx.done(
        editor_grid(&ctx, &detail, Sort::Position, 0, false),
        None,
        &format!("{}#item-{item}", set_url(&id)),
    ))
}

// ---------------------------------------------------------------------------
// Settings and delete
// ---------------------------------------------------------------------------

fn settings_form(ctx: &Ctx, set: &Set, f: &SetForm, error: Option<&str>, saved: bool) -> Markup {
    let body = html! {
        p.crumbs { a href=(set_url(&set.id)) { "← " (set.name) } }
        h1 { "Set settings" }
        @if saved { (layout::alert("success", "Saved.")) }
        @if let Some(e) = error { (layout::alert("error", e)) }
        @if set.editable {
            form.stack.narrow method="post" action=(format!("/sets/{}/settings", set.id)) {
                (ctx.csrf_input())
                (set_fields(ctx, f, false))
                button.primary type="submit" { "Save" }
            }
        } @else {
            p { "This set is a read-only git pack: its metadata comes from the repository's set.toml." }
        }
        section.danger {
            h2 { "Delete" }
            p { "Deleting the set removes it and all its items for everyone. This cannot be undone." }
            a.button.danger href=(format!("/sets/{}/delete", set.id)) { "Delete this set…" }
        }
    };
    ctx.page("Set settings", Nav::Sets, body)
}

async fn owned(state: &AppState, ctx: &Ctx, id: &str) -> AppResult<Set> {
    let set = sets::detail(state, &ctx.user, id).await?.set;
    if set.owner_id != ctx.id() {
        return Err(AppError::forbidden("only the owner can change this set"));
    }
    Ok(set)
}

#[derive(Debug, Default, Deserialize)]
pub struct SavedQuery {
    #[serde(default)]
    saved: Option<String>,
}

pub async fn settings_page(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Query(q): Query<SavedQuery>,
) -> AppResult<Markup> {
    let set = owned(&state, &ctx, &id).await?;
    Ok(settings_form(
        &ctx,
        &set,
        &SetForm::from_set(&set),
        None,
        q.saved.is_some(),
    ))
}

pub async fn settings_submit(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Form(f): Form<SetForm>,
) -> AppResult<Response> {
    let set = owned(&state, &ctx, &id).await?;
    let attempt = async {
        let req = UpdateSet {
            slug: Some(f.slug.trim().to_string()),
            name: Some(f.name.clone()),
            description: Some(f.description.clone()),
            visibility: Some(parse_visibility(&f.visibility)?),
            aspect_ratio: Some(parse_aspect(&f.aspect)?),
            recommended_px: Some(parse_px(&f.recommended_px)?),
        };
        sets::update(&state, &ctx.user, &id, req).await
    };
    match attempt.await {
        Ok(_) => Ok(ctx.redirect(&format!("/sets/{id}/settings?saved=1"))),
        Err(e) if e.status.is_client_error() => Ok((
            if e.status == StatusCode::FORBIDDEN {
                StatusCode::FORBIDDEN
            } else {
                e.status
            },
            settings_form(&ctx, &set, &f, Some(&form_error(&e)), false),
        )
            .into_response()),
        Err(e) => Err(e),
    }
}

pub async fn delete_page(State(state): State<AppState>, ctx: Ctx, Path(id): Path<String>) -> AppResult<Markup> {
    let set = owned(&state, &ctx, &id).await?;
    let body = html! {
        p.crumbs { a href=(set_url(&set.id)) { "← " (set.name) } }
        h1 { "Delete “" (set.name) "”?" }
        p { "This removes the set and its " (plural(set.item_count, "item", "items")) ". Exported copies are not affected." }
        form.inline-row method="post" action=(format!("/sets/{}/delete", set.id)) {
            (ctx.csrf_input())
            button.danger type="submit" { "Delete set" }
            a.button href=(set_url(&set.id)) { "Cancel" }
        }
    };
    Ok(ctx.page("Delete set", Nav::Sets, body))
}

pub async fn delete_submit(State(state): State<AppState>, ctx: Ctx, Path(id): Path<String>) -> AppResult<Response> {
    sets::delete(&state, &ctx.user, &id).await?;
    Ok(ctx.redirect("/sets"))
}

#[cfg(test)]
mod tests {
    use super::parse_aspect;

    #[test]
    fn aspect_forms() {
        assert_eq!(parse_aspect("").unwrap(), None);
        assert_eq!(parse_aspect("1:1").unwrap(), Some((1, 1)));
        assert_eq!(parse_aspect(" 16 / 9 ").unwrap(), Some((16, 9)));
        assert_eq!(parse_aspect("4x3").unwrap(), Some((4, 3)));
        assert!(parse_aspect("wide").is_err());
    }
}
