//! Collaboration pages (docs/collaboration.md): the Collaboration tab with
//! incoming and outgoing proposals, one proposal, a set's contributors on
//! its page, and the forms to propose stickers and changes.

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Form;
use maud::{html, Markup};
use serde::Deserialize;
use stickpile_api::{
    GitMode, Item, Member, NewProposal, Permissions, Proposal, ProposalBox, ProposalKind, ProposalStatus,
    ProposedChanges, Set, SetRole, SetSummary, Variant,
};

use super::components::{file_url, fmt_time, item_url, plural, set_url, thumb};
use super::layout::{self, Nav};
use super::{Ctx, MultipartCaller};
use crate::error::{AppError, AppResult};
use crate::service::sets::{self, SetScope};
use crate::service::{items as item_svc, members, proposals};
use crate::state::AppState;

pub fn proposal_url(id: &str) -> String {
    format!("/proposals/{id}")
}

/// The set takes proposals from the viewer: not theirs, not a read-only git pack.
pub fn can_propose(set: &Set) -> bool {
    set.role != SetRole::Owner && !set.git.as_ref().is_some_and(|g| g.mode == GitMode::ReadOnly)
}

/// The permissions with their labels, in display order.
pub const PERMISSIONS: [(&str, &str, &str); 6] = [
    ("add", "Add stickers", "upload new stickers to the set"),
    (
        "edit_own",
        "Edit their stickers",
        "edit and remove the stickers they added",
    ),
    ("edit_all", "Edit all stickers", "edit everyone's stickers, yours too"),
    ("remove_all", "Remove any sticker", "remove everyone's stickers"),
    ("edit_pack", "Edit the pack", "name, description, cover and order"),
    ("review", "Review proposals", "accept or decline what others propose"),
];

fn perm(p: &Permissions, key: &str) -> bool {
    match key {
        "add" => p.add,
        "edit_own" => p.edit_own,
        "edit_all" => p.edit_all,
        "remove_all" => p.remove_all,
        "edit_pack" => p.edit_pack,
        "review" => p.review,
        _ => false,
    }
}

/// "Add stickers, Edit their stickers" or "View only".
pub fn permissions_summary(p: &Permissions) -> String {
    let on: Vec<&str> = PERMISSIONS
        .iter()
        .filter(|(k, ..)| perm(p, k))
        .map(|(_, label, _)| *label)
        .collect();
    if on.is_empty() {
        "View only".to_string()
    } else {
        on.join(", ")
    }
}

// ---------------------------------------------------------------------------
// The Collaboration tab
// ---------------------------------------------------------------------------

/// The nav badge: open proposals waiting for the viewer (fetched by htmx).
pub async fn badge(State(state): State<AppState>, ctx: Ctx) -> AppResult<Markup> {
    let n = proposals::incoming_open(&state, &ctx.user).await?;
    Ok(html! {
        @if n > 0 {
            span.nav-badge title=(plural(n, "proposal waits for you", "proposals wait for you")) { (n) }
        }
    })
}

pub async fn tab(State(state): State<AppState>, ctx: Ctx) -> AppResult<Markup> {
    let incoming = proposals::list(&state, &ctx.user, ProposalBox::Incoming, None, 200).await?;
    let outgoing = proposals::list(&state, &ctx.user, ProposalBox::Outgoing, None, 200).await?;
    let contributing = sets::list(&state, &ctx.user, SetScope::Contributing).await?;
    let open =
        |v: &[Proposal]| -> Vec<Proposal> { v.iter().filter(|p| p.status == ProposalStatus::Open).cloned().collect() };
    let (inc_open, out_open) = (open(&incoming), open(&outgoing));
    let mut decided: Vec<&Proposal> = incoming
        .iter()
        .chain(outgoing.iter().filter(|p| !incoming.iter().any(|i| i.id == p.id)))
        .filter(|p| p.status != ProposalStatus::Open)
        .collect();
    decided.sort_by_key(|p| std::cmp::Reverse(p.decided_at));
    decided.truncate(30);
    let body = html! {
        h1 { "Collaboration" }
        p.muted {
            "Proposals for your packs, the ones you sent to others, and the packs you contribute to. "
            "To propose something, open someone's pack or sticker and choose " em { "Propose" } "."
        }
        section aria-labelledby="incoming-head" {
            h2 #incoming-head { "Incoming" @if !inc_open.is_empty() { " " span.count { "(" (inc_open.len()) ")" } } }
            @if inc_open.is_empty() {
                p.muted { "Nothing waits for you." }
            } @else {
                div.proposals { @for p in &inc_open { (proposal_card(&ctx, p, true)) } }
            }
        }
        section aria-labelledby="outgoing-head" {
            h2 #outgoing-head { "Your proposals" @if !out_open.is_empty() { " " span.count { "(" (out_open.len()) ")" } } }
            @if out_open.is_empty() {
                p.muted { "You have no open proposals." }
            } @else {
                div.proposals { @for p in &out_open { (proposal_card(&ctx, p, true)) } }
            }
        }
        section aria-labelledby="contrib-head" {
            h2 #contrib-head { "Packs you contribute to" }
            @if contributing.is_empty() {
                p.muted { "None yet. A pack's owner can add you on its page." }
            } @else {
                ul.contrib-list { @for s in &contributing { li { (set_link(s)) } } }
            }
        }
        section aria-labelledby="history-head" {
            h2 #history-head { "Decided" }
            @if decided.is_empty() {
                p.muted { "No decided proposals yet." }
            } @else {
                div.proposals { @for p in &decided { (proposal_card(&ctx, p, true)) } }
            }
        }
    };
    Ok(ctx.page("Collaboration", Nav::Collaboration, body))
}

fn set_link(s: &SetSummary) -> Markup {
    html! {
        a href=(set_url(&s.id)) { (s.name) }
        " " span.muted { "by " (s.owner_username) " · " (plural(s.item_count, "sticker", "stickers")) }
    }
}

fn kind_label(p: &Proposal) -> &'static str {
    match p.kind {
        ProposalKind::AddItems => "New stickers",
        ProposalKind::EditItem => "Sticker changes",
        ProposalKind::EditSet if p.cover_image_url.is_some() || p.changes.is_empty() => "New cover image",
        ProposalKind::EditSet => "Pack changes",
    }
}

fn status_badge(s: ProposalStatus) -> Markup {
    let (class, label) = match s {
        ProposalStatus::Open => ("badge vis-unlisted", "Open"),
        ProposalStatus::Accepted => ("badge vis-public", "Accepted"),
        ProposalStatus::Declined => ("badge", "Declined"),
        ProposalStatus::Withdrawn => ("badge", "Withdrawn"),
    };
    html! { span class=(class) { (label) } }
}

fn list_text(v: &[String]) -> String {
    if v.is_empty() {
        "(none)".to_string()
    } else {
        v.join(", ")
    }
}

/// (field, before, after) rows of an edit proposal.
fn diff_rows(c: &ProposedChanges, b: &ProposedChanges) -> Vec<(&'static str, String, String)> {
    let text = |s: &Option<String>| s.clone().unwrap_or_default();
    let mut rows = Vec::new();
    if let Some(n) = &c.name {
        rows.push(("Name", text(&b.name), n.clone()));
    }
    if let Some(d) = &c.description {
        rows.push(("Description", text(&b.description), d.clone()));
    }
    for (label, after, before) in [
        ("Keywords", &c.keywords, &b.keywords),
        ("Emojis", &c.emojis, &b.emojis),
        ("Tags", &c.tags, &b.tags),
    ] {
        if let Some(a) = after {
            rows.push((label, list_text(before.as_deref().unwrap_or(&[])), list_text(a)));
        }
    }
    if let Some(v) = &c.vibe {
        let shown = |v: &Option<String>| v.clone().unwrap_or_else(|| "(none)".into());
        rows.push(("Vibe", shown(&b.vibe.clone().flatten()), shown(v)));
    }
    rows
}

fn cover_thumb(item_id: &Option<String>) -> Markup {
    html! {
        @match item_id {
            Some(id) => { a.mini-thumb href=(item_url(id)) { img src=(file_url(id, Variant::Thumb)) alt="" width="64" height="64" loading="lazy"; } }
            None => { span.muted { "the first sticker" } }
        }
    }
}

/// One proposal: what it asks for, its state and the buttons the viewer may press.
pub fn proposal_card(ctx: &Ctx, p: &Proposal, link: bool) -> Markup {
    let here = proposal_url(&p.id);
    html! {
        article.proposal id=(format!("proposal-{}", p.id)) {
            header.proposal-head {
                strong {
                    @if link { a href=(here) { (kind_label(p)) } } @else { (kind_label(p)) }
                }
                " for " a href=(set_url(&p.set.id)) { (p.set.name) }
                " " span.muted { "(" (p.set.owner_username) ")" }
                " " (status_badge(p.status))
                p.meta.muted {
                    "from " (p.proposer_username) " · " (fmt_time(p.created_at))
                    @if let (Some(r), Some(at)) = (&p.reviewer_username, p.decided_at) {
                        " · " (match p.status { ProposalStatus::Accepted => "accepted", _ => "declined" })
                        " by " (r) " · " (fmt_time(at))
                    }
                    @if p.status == ProposalStatus::Withdrawn {
                        @if let Some(at) = p.decided_at { " · withdrawn " (fmt_time(at)) }
                    }
                }
            }
            @if !p.message.is_empty() { blockquote.proposal-message { (p.message) } }
            @match p.kind {
                ProposalKind::AddItems => {
                    ul.proposed-items {
                        @for it in &p.items {
                            li {
                                @if let Some(item) = &it.item_id {
                                    a.mini-thumb href=(item_url(item)) { img src=(file_url(item, Variant::Thumb)) alt=(it.name) width="96" height="96" loading="lazy"; }
                                } @else if let Some(url) = &it.thumb_url {
                                    span.mini-thumb { img src=(url) alt=(it.name) width="96" height="96" loading="lazy"; }
                                } @else {
                                    span.mini-thumb.gone { "no file" }
                                }
                                div {
                                    strong { (it.name) }
                                    @if !it.emojis.is_empty() { " " (it.emojis.concat()) }
                                    @if !it.tags.is_empty() { br; span.muted.small { (it.tags.join(", ")) } }
                                }
                            }
                        }
                    }
                }
                ProposalKind::EditItem => {
                    div.inline-row {
                        @if let Some(item) = &p.item {
                            a.mini-thumb href=(item_url(&item.id)) { img src=(file_url(&item.id, Variant::Thumb)) alt=(item.name) width="64" height="64" loading="lazy"; }
                            a href=(item_url(&item.id)) { (item.name) }
                        } @else {
                            span.muted { "The sticker was removed." }
                        }
                    }
                    (diff_table(&p.changes, &p.before, false))
                }
                ProposalKind::EditSet => {
                    @if let Some(url) = &p.cover_image_url {
                        p { "As the cover image:" }
                        span.mini-thumb { img src=(url) alt="the proposed cover" width="96" height="96" loading="lazy"; }
                    }
                    (diff_table(&p.changes, &p.before, true))
                }
            }
            @if !p.review_note.is_empty() {
                p.review-note { strong { "Note: " } (p.review_note) }
            }
            @if p.can_decide || p.can_withdraw {
                div.proposal-actions {
                    @if p.can_decide {
                        form.inline-row method="post" action=(format!("{here}/accept")) {
                            (ctx.csrf_input())
                            input type="hidden" name="next" value=(ctx.here);
                            label.sr-only for=(format!("note-{}", p.id)) { "Note for " (p.proposer_username) }
                            input id=(format!("note-{}", p.id)) type="text" name="note" maxlength="1000" placeholder="A note (optional)";
                            button.primary type="submit" { "Accept" }
                            button type="submit" formaction=(format!("{here}/decline")) { "Decline" }
                        }
                    }
                    @if p.can_withdraw {
                        form.inline method="post" action=(format!("{here}/withdraw")) {
                            (ctx.csrf_input())
                            input type="hidden" name="next" value=(ctx.here);
                            button type="submit" { "Withdraw" }
                        }
                    }
                }
            }
        }
    }
}

fn diff_table(c: &ProposedChanges, b: &ProposedChanges, set: bool) -> Markup {
    let rows = diff_rows(c, b);
    html! {
        @if !rows.is_empty() || (set && c.cover_item_id.is_some()) {
            table.diff {
                thead { tr { th { "Field" } th { "Now" } th { "Proposed" } } }
                tbody {
                    @for (field, before, after) in &rows {
                        tr { th scope="row" { (field) } td.before { (before) } td.after { (after) } }
                    }
                    @if set && c.cover_item_id.is_some() {
                        tr { th scope="row" { "Cover" } td.before { (cover_thumb(&b.cover_item_id)) } td.after { (cover_thumb(&c.cover_item_id)) } }
                    }
                }
            }
        }
    }
}

pub async fn page(State(state): State<AppState>, ctx: Ctx, Path(id): Path<String>) -> AppResult<Markup> {
    let p = proposals::get(&state, &ctx.user, &id).await?;
    let body = html! {
        p.crumbs { a href="/collaboration" { "← Collaboration" } }
        h1 { (kind_label(&p)) }
        (proposal_card(&ctx, &p, false))
    };
    Ok(ctx.page(kind_label(&p), Nav::Collaboration, body))
}

#[derive(Debug, Default, Deserialize)]
pub struct DecideForm {
    #[serde(default)]
    note: String,
    #[serde(default)]
    next: Option<String>,
}

pub async fn accept(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Form(f): Form<DecideForm>,
) -> AppResult<Response> {
    proposals::accept(&state, &ctx.user, &id, &f.note).await?;
    Ok(ctx.done(html! {}, f.next.as_deref(), &proposal_url(&id)))
}

pub async fn decline(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Form(f): Form<DecideForm>,
) -> AppResult<Response> {
    proposals::decline(&state, &ctx.user, &id, &f.note).await?;
    Ok(ctx.done(html! {}, f.next.as_deref(), &proposal_url(&id)))
}

pub async fn withdraw(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Form(f): Form<DecideForm>,
) -> AppResult<Response> {
    proposals::withdraw(&state, &ctx.user, &id).await?;
    Ok(ctx.done(html! {}, f.next.as_deref(), &proposal_url(&id)))
}

// ---------------------------------------------------------------------------
// Contributors (a section of the set page)
// ---------------------------------------------------------------------------

fn permission_checks(prefix: &str, p: &Permissions) -> Markup {
    html! {
        @for (key, label, hint) in PERMISSIONS {
            label.check title=(hint) {
                input type="checkbox" name=(key) id=(format!("{prefix}-{key}")) checked[perm(p, key)];
                " " (label)
            }
        }
    }
}

/// Contributors of `set` for its owner (manage) and contributors (who, and Leave).
pub fn contributors_section(ctx: &Ctx, set: &Set, members: &[Member]) -> Markup {
    let base = format!("/sets/{}/contributors", set.id);
    let owner = set.role == SetRole::Owner;
    html! {
        section.contributors aria-labelledby="contrib-head" {
            h2 #contrib-head { "Contributors" }
            @if owner {
                p.muted {
                    "People you add here see this set (also when it is private) and may change it as you allow. "
                    "Settings, visibility, contributors and deleting stay yours."
                }
            }
            @if members.is_empty() {
                p.muted { "No contributors yet." }
            } @else {
                ul.member-list {
                    @for m in members {
                        li.member {
                            strong { (m.display_name) } " " span.muted { "@" (m.username) }
                            @if owner {
                                form.inline-row.member-perms method="post" action=(format!("{base}/{}", m.username)) {
                                    (ctx.csrf_input())
                                    (permission_checks(&format!("m-{}", m.username), &m.permissions))
                                    button type="submit" { "Save" }
                                    button.danger type="submit" formaction=(format!("{base}/{}/remove", m.username)) { "Remove" }
                                }
                            } @else {
                                " · " span.small { (permissions_summary(&m.permissions)) }
                                @if m.username.eq_ignore_ascii_case(&ctx.user.user.username) {
                                    form.inline method="post" action=(format!("{base}/{}/remove", m.username)) {
                                        (ctx.csrf_input())
                                        button type="submit" { "Leave" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            @if owner {
                form.stack.add-member method="post" action=(base) {
                    (ctx.csrf_input())
                    div.field-row {
                        label for="new-member" { "Add a contributor" }
                        input #new-member type="text" name="username" required maxlength="64" placeholder="username"
                            autocomplete="off" spellcheck="false" autocapitalize="none";
                    }
                    fieldset.checks {
                        legend { "They may" }
                        (permission_checks("new", &Permissions::CONTRIBUTOR_DEFAULT))
                    }
                    button.primary type="submit" { "Add" }
                }
            }
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct MemberForm {
    #[serde(default)]
    username: String,
    #[serde(default)]
    add: Option<String>,
    #[serde(default)]
    edit_own: Option<String>,
    #[serde(default)]
    edit_all: Option<String>,
    #[serde(default)]
    remove_all: Option<String>,
    #[serde(default)]
    edit_pack: Option<String>,
    #[serde(default)]
    review: Option<String>,
}

impl MemberForm {
    fn permissions(&self) -> Permissions {
        Permissions {
            add: self.add.is_some(),
            edit_own: self.edit_own.is_some(),
            edit_all: self.edit_all.is_some(),
            remove_all: self.remove_all.is_some(),
            edit_pack: self.edit_pack.is_some(),
            review: self.review.is_some(),
        }
    }
}

pub async fn add_member(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Form(f): Form<MemberForm>,
) -> AppResult<Response> {
    let req = stickpile_api::PutMember {
        permissions: f.permissions(),
    };
    members::put(&state, &ctx.user, &id, &f.username, req).await?;
    Ok(ctx.redirect(&format!("{}#contributors", set_url(&id))))
}

pub async fn update_member(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, username)): Path<(String, String)>,
    Form(f): Form<MemberForm>,
) -> AppResult<Response> {
    let req = stickpile_api::PutMember {
        permissions: f.permissions(),
    };
    members::put(&state, &ctx.user, &id, &username, req).await?;
    Ok(ctx.redirect(&format!("{}#contributors", set_url(&id))))
}

pub async fn remove_member(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, username)): Path<(String, String)>,
) -> AppResult<Response> {
    let leaving = username.eq_ignore_ascii_case(&ctx.user.user.username);
    members::remove(&state, &ctx.user, &id, &username).await?;
    if leaving {
        Ok(ctx.redirect("/collaboration"))
    } else {
        Ok(ctx.redirect(&format!("{}#contributors", set_url(&id))))
    }
}

// ---------------------------------------------------------------------------
// Proposing
// ---------------------------------------------------------------------------

fn propose_form(ctx: &Ctx, set: &Set, items: &[Item], f: &SetChangeForm, error: Option<&str>) -> Markup {
    let base = format!("/sets/{}/propose", set.id);
    html! {
        p.crumbs { a href=(set_url(&set.id)) { "← " (set.name) } " by " (set.owner_username) }
        h1 { "Propose to “" (set.name) "”" }
        p.muted {
            (set.owner_username) " decides; you see the answer under " a href="/collaboration" { "Collaboration" } ". "
            "To change one sticker, open it and choose " em { "Suggest changes" } "."
        }
        @if let Some(e) = error { (layout::alert("error", e)) }
        section aria-labelledby="p-items" {
            h2 #p-items { "New stickers" }
            form.stack.narrow method="post" action=(format!("{base}/items")) enctype="multipart/form-data" {
                (ctx.csrf_input())
                div.field-row {
                    label for="p-files" { "Files" }
                    input #p-files type="file" name="file" multiple required
                        accept="image/png,image/jpeg,image/webp,image/gif,video/mp4,video/webm";
                }
                div.field-row {
                    label for="p-items-msg" { "Message (optional)" }
                    textarea #p-items-msg name="message" rows="2" maxlength="1000" {}
                }
                button.primary type="submit" { "Propose stickers" }
            }
        }
        section aria-labelledby="p-set" {
            h2 #p-set { "Name, description, cover" }
            form.stack.narrow method="post" action=(format!("{base}/changes")) {
                (ctx.csrf_input())
                div.field-row {
                    label for="p-name" { "Name" }
                    input #p-name type="text" name="name" value=(f.name) maxlength="200";
                }
                div.field-row {
                    label for="p-desc" { "Description" }
                    textarea #p-desc name="description" rows="3" maxlength="5000" { (f.description) }
                }
                @if !items.is_empty() {
                    div.field-row {
                        label for="p-cover" { "Cover" }
                        select #p-cover name="cover" {
                            option value="" selected[f.cover.is_empty()] { "(keep the cover)" }
                            @for it in items {
                                option value=(it.id) selected[f.cover == it.id] { (it.name) }
                            }
                        }
                    }
                }
                div.field-row {
                    label for="p-set-msg" { "Message (optional)" }
                    textarea #p-set-msg name="message" rows="2" maxlength="1000" { (f.message) }
                }
                button.primary type="submit" { "Propose changes" }
            }
        }
        section aria-labelledby="p-cover-img" {
            h2 #p-cover-img { "A cover image" }
            form.stack.narrow method="post" action=(format!("{base}/cover")) enctype="multipart/form-data" {
                (ctx.csrf_input())
                div.field-row {
                    label for="p-cover-file" { "Image (PNG, JPEG or WebP)" }
                    input #p-cover-file type="file" name="file" required accept="image/png,image/jpeg,image/webp";
                }
                div.field-row {
                    label for="p-cover-msg" { "Message (optional)" }
                    textarea #p-cover-msg name="message" rows="2" maxlength="1000" {}
                }
                button.primary type="submit" { "Propose cover image" }
            }
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct SetChangeForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    cover: String,
    #[serde(default)]
    message: String,
}

async fn proposable(state: &AppState, ctx: &Ctx, id: &str) -> AppResult<stickpile_api::SetDetail> {
    let d = sets::detail(state, &ctx.user, id).await?;
    if !can_propose(&d.set) {
        return Err(AppError::bad_request("you cannot propose changes to this set"));
    }
    Ok(d)
}

pub async fn propose_page(State(state): State<AppState>, ctx: Ctx, Path(id): Path<String>) -> AppResult<Markup> {
    let d = proposable(&state, &ctx, &id).await?;
    let f = SetChangeForm {
        name: d.set.name.clone(),
        description: d.set.description.clone(),
        ..Default::default()
    };
    Ok(ctx.page("Propose", Nav::Sets, propose_form(&ctx, &d.set, &d.items, &f, None)))
}

/// A client error: the propose page again with the message.
async fn propose_error(state: &AppState, ctx: &Ctx, id: &str, f: &SetChangeForm, e: AppError) -> AppResult<Response> {
    if !e.status.is_client_error() || e.status == StatusCode::NOT_FOUND {
        return Err(e);
    }
    let d = proposable(state, ctx, id).await?;
    let page = ctx.page(
        "Propose",
        Nav::Sets,
        propose_form(ctx, &d.set, &d.items, f, Some(&e.message)),
    );
    Ok((e.status, page).into_response())
}

pub async fn propose_changes(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Form(f): Form<SetChangeForm>,
) -> AppResult<Response> {
    let d = proposable(&state, &ctx, &id).await?;
    // Unchanged fields are left out, so "nothing to change" means it.
    let changes = ProposedChanges {
        name: Some(f.name.trim().to_string()).filter(|n| *n != d.set.name && !n.is_empty()),
        description: Some(f.description.trim().to_string()).filter(|x| *x != d.set.description),
        cover_item_id: Some(f.cover.trim().to_string()).filter(|c| !c.is_empty()),
        ..Default::default()
    };
    let req = NewProposal {
        message: f.message.clone(),
        changes,
    };
    match proposals::propose_set_changes(&state, &ctx.user, &id, req).await {
        Ok(p) => Ok(ctx.redirect(&proposal_url(&p.id))),
        Err(e) => propose_error(&state, &ctx, &id, &f, e).await,
    }
}

pub async fn propose_items(
    State(state): State<AppState>,
    caller: MultipartCaller,
    Path(id): Path<String>,
    req: Request,
) -> AppResult<Response> {
    let ctx = &caller.ctx;
    let outcome = async {
        proposals::check_target(&state, &ctx.user, &id).await?;
        let mp = caller.multipart(&state, req).await?;
        let (parts, message) = proposals::read_multipart(&state, mp).await?;
        proposals::propose_items(&state, &ctx.user, &id, parts, &message).await
    }
    .await;
    match outcome {
        Ok(p) => Ok(ctx.redirect(&proposal_url(&p.id))),
        Err(e) => propose_error(&state, ctx, &id, &SetChangeForm::default(), e).await,
    }
}

pub async fn propose_cover(
    State(state): State<AppState>,
    caller: MultipartCaller,
    Path(id): Path<String>,
    req: Request,
) -> AppResult<Response> {
    let ctx = &caller.ctx;
    let outcome = async {
        proposals::check_target(&state, &ctx.user, &id).await?;
        let mp = caller.multipart(&state, req).await?;
        let (parts, message) = proposals::read_multipart(&state, mp).await?;
        let [file] = <[_; 1]>::try_from(parts.files).map_err(|_| AppError::bad_request("choose one image"))?;
        proposals::propose_cover_image(&state, &ctx.user, &id, &file, &message).await
    }
    .await;
    match outcome {
        Ok(p) => Ok(ctx.redirect(&proposal_url(&p.id))),
        Err(e) => propose_error(&state, ctx, &id, &SetChangeForm::default(), e).await,
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct SuggestForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    keywords: String,
    #[serde(default)]
    emojis: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    vibe: String,
    #[serde(default)]
    message: String,
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

fn suggest_form(ctx: &Ctx, item: &Item, f: &SuggestForm, error: Option<&str>) -> Markup {
    html! {
        p.crumbs { a href=(item_url(&item.id)) { "← " (item.name) } }
        h1 { "Suggest changes to “" (item.name) "”" }
        p.muted { "The pack's owner decides; you see the answer under " a href="/collaboration" { "Collaboration" } "." }
        @if let Some(e) = error { (layout::alert("error", e)) }
        div.preview-small { (thumb(&item.id, &item.name, item.media.width, item.media.height)) }
        form.stack.narrow method="post" action=(format!("/items/{}/suggest", item.id)) {
            (ctx.csrf_input())
            div.field-row {
                label for="s-name" { "Name" }
                input #s-name type="text" name="name" value=(f.name) maxlength="200";
            }
            div.field-row {
                label for="s-keywords" { "Keywords (comma separated)" }
                input #s-keywords type="text" name="keywords" value=(f.keywords);
            }
            div.field-row {
                label for="s-emojis" { "Emojis (comma separated)" }
                input #s-emojis type="text" name="emojis" value=(f.emojis);
            }
            div.field-row {
                label for="s-tags" { "Tags (comma separated)" }
                input #s-tags type="text" name="tags" value=(f.tags);
            }
            div.field-row {
                label for="s-vibe" { "Vibe" }
                input #s-vibe type="text" name="vibe" value=(f.vibe) maxlength="64";
            }
            div.field-row {
                label for="s-msg" { "Message (optional)" }
                textarea #s-msg name="message" rows="2" maxlength="1000" { (f.message) }
            }
            button.primary type="submit" { "Send suggestion" }
        }
    }
}

impl SuggestForm {
    fn from_item(item: &Item) -> SuggestForm {
        SuggestForm {
            name: item.name.clone(),
            keywords: item.keywords.join(", "),
            emojis: item.emojis.join(", "),
            tags: item.tags.join(", "),
            vibe: item.vibe.clone().unwrap_or_default(),
            message: String::new(),
        }
    }
}

pub async fn suggest_page(State(state): State<AppState>, ctx: Ctx, Path(id): Path<String>) -> AppResult<Markup> {
    let item = item_svc::detail(&state, &ctx.user, &id).await?.item;
    Ok(ctx.page(
        "Suggest changes",
        Nav::Sets,
        suggest_form(&ctx, &item, &SuggestForm::from_item(&item), None),
    ))
}

pub async fn suggest_submit(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Form(f): Form<SuggestForm>,
) -> AppResult<Response> {
    let item = item_svc::detail(&state, &ctx.user, &id).await?.item;
    let vibe = Some(f.vibe.trim().to_string()).filter(|v| !v.is_empty());
    let changes = ProposedChanges {
        name: Some(f.name.trim().to_string()),
        keywords: Some(split_list(&f.keywords)),
        emojis: Some(split_list(&f.emojis)),
        tags: Some(split_list(&f.tags)),
        vibe: (vibe != item.vibe).then_some(vibe),
        ..Default::default()
    };
    let req = NewProposal {
        message: f.message.clone(),
        changes,
    };
    match proposals::propose_item_changes(&state, &ctx.user, &id, req).await {
        Ok(p) => Ok(ctx.redirect(&proposal_url(&p.id))),
        Err(e) if e.status.is_client_error() && e.status != StatusCode::NOT_FOUND => {
            let page = ctx.page(
                "Suggest changes",
                Nav::Sets,
                suggest_form(&ctx, &item, &f, Some(&e.message)),
            );
            Ok((e.status, page).into_response())
        }
        Err(e) => Err(e),
    }
}
