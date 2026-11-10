//! Admin panel (role `admin`, others get 403): overview (storage, job queue,
//! registration policy), users, invitations, audit log. Every action goes
//! through [`crate::service::admin`], which audits it.

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Form;
use maud::{html, Markup};
use serde::Deserialize;
use stickpile_api::{AdminCreateUser, AdminUpdateUser, AdminUser, CreateInvitation, Role};

use super::components::{fmt_bytes, opt_time, time_el};
use super::layout::{self, Nav};
use super::AdminCtx;
use crate::config::Registration;
use crate::error::{AppError, AppResult};
use crate::ids::now;
use crate::service::admin as svc;
use crate::service::ui;
use crate::state::{AiStatus, AppState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Users,
    Invitations,
    Audit,
}

fn tabs(active: Tab) -> Markup {
    let items = [
        (Tab::Overview, "/admin", "Overview"),
        (Tab::Users, "/admin/users", "Users"),
        (Tab::Invitations, "/admin/invitations", "Invitations"),
        (Tab::Audit, "/admin/audit", "Audit log"),
    ];
    html! {
        nav.tabs aria-label="Admin sections" {
            @for (t, href, label) in items {
                @if t == active { a href=(href) aria-current="page" { (label) } } @else { a href=(href) { (label) } }
            }
        }
    }
}

fn page(ctx: &AdminCtx, title: &str, tab: Tab, body: Markup) -> Markup {
    ctx.page(
        title,
        Nav::Admin,
        html! {
            h1 { "Admin" }
            (tabs(tab))
            (body)
        },
    )
}

fn registration_label(r: Registration) -> &'static str {
    match r {
        Registration::Closed => "closed (admins create accounts)",
        Registration::Invite => "by invitation",
        Registration::Open => "open to anyone who can reach the server",
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct DoneQuery {
    #[serde(default)]
    done: Option<String>,
}

// ---------------------------------------------------------------------------
// Overview
// ---------------------------------------------------------------------------

pub async fn overview(State(state): State<AppState>, ctx: AdminCtx) -> AppResult<Markup> {
    let storage = svc::storage(&state).await?;
    let counts = ui::job_counts(&state).await?;
    let failures = ui::recent_failures(&state, 10).await?;
    let ai = match state.ai_status() {
        AiStatus::Off => "off",
        AiStatus::Loading => "loading",
        AiStatus::Ready => "ready",
        AiStatus::Failed => "failed to load (see the server log)",
    };
    let pending: u64 = counts
        .iter()
        .filter(|c| c.status == "queued" || c.status == "running")
        .map(|c| c.count)
        .sum();
    let body = html! {
        dl.tiles {
            div.tile { dt { "Media on disk" } dd { (fmt_bytes(storage.media_bytes)) } }
            div.tile { dt { "Database" } dd { (fmt_bytes(storage.database_bytes)) } }
            div.tile { dt { "Media files" } dd { (storage.media_count) } }
            div.tile { dt { "Items" } dd { (storage.item_count) } }
            div.tile { dt { "Jobs waiting or running" } dd { (pending) } }
        }
        dl.kv {
            dt { "Registration" } dd { (registration_label(state.config.auth.registration)) " " span.muted { "(set in the configuration)" } }
            dt { "AI enrichment" } dd { (ai) }
            dt { "Public URL" } dd { code { (state.config.server.public_url) } }
        }
        div.two-col {
            section aria-labelledby="a-storage" {
                h2 #a-storage { "Storage per user" }
                p.hint { "Media referenced by each user's items; shared files count for every user." }
                table.data {
                    thead { tr { th scope="col" { "User" } th scope="col" { "Bytes" } } }
                    tbody {
                        @for (u, b) in &storage.per_user {
                            tr { th scope="row" { (u) } td { (fmt_bytes(*b)) } }
                        }
                    }
                }
            }
            section aria-labelledby="a-jobs" {
                h2 #a-jobs { "Job queue" }
                @if counts.is_empty() {
                    p.muted { "No jobs." }
                } @else {
                    table.data {
                        thead { tr { th scope="col" { "Kind" } th scope="col" { "Status" } th scope="col" { "Jobs" } } }
                        tbody {
                            @for c in &counts {
                                tr class=(format!("job-{}", c.status)) {
                                    th scope="row" { (c.kind.replace('_', " ")) }
                                    td { (c.status) }
                                    td { (c.count) }
                                }
                            }
                        }
                    }
                }
                p.hint { "Finished jobs are kept for 30 days." }
            }
        }
        section aria-labelledby="a-fail" {
            h2 #a-fail { "Recent failures" }
            @if failures.is_empty() {
                p.muted { "None." }
            } @else {
                table.data {
                    thead { tr { th scope="col" { "When" } th scope="col" { "Kind" } th scope="col" { "Subject" } th scope="col" { "Attempts" } th scope="col" { "Error" } } }
                    tbody {
                        @for j in &failures {
                            tr {
                                td { (opt_time(j.finished_at.or(Some(j.created_at)))) }
                                td { (format!("{:?}", j.kind)) }
                                td { code { (j.subject.as_deref().unwrap_or("")) } }
                                td { (j.attempts) }
                                td.error-text { (j.error.as_deref().unwrap_or("")) }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(page(&ctx, "Admin", Tab::Overview, body))
}

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

fn user_done(done: Option<&str>) -> Option<&'static str> {
    Some(match done? {
        "created" => "User created.",
        "updated" => "User updated.",
        "deleted" => "User deleted.",
        _ => return None,
    })
}

fn user_row(ctx: &AdminCtx, u: &AdminUser) -> Markup {
    let id = &u.user.id;
    let action = format!("/admin/users/{id}");
    let is_self = id == ctx.id();
    let form = |act: &str, inner: Markup| {
        html! {
            form.inline-row method="post" action=(action) {
                (ctx.csrf_input())
                input type="hidden" name="action" value=(act);
                (inner)
            }
        }
    };
    html! {
        tr.disabled[u.disabled] {
            th scope="row" {
                (u.user.username)
                @if u.user.display_name != u.user.username { br; span.muted.small { (u.user.display_name) } }
            }
            td { (if u.user.role == Role::Admin { "admin" } else { "user" }) }
            td {
                @if u.disabled { span.badge.vis-private { "disabled" } } @else { "active" }
                @if u.user.totp_enabled { " " span.badge { "TOTP" } }
                @if !u.has_password { " " span.badge title="No password: tailnet sign-in only" { "no password" } }
            }
            td { @if let Some(l) = &u.user.tailscale_login { code { (l) } } }
            td { (opt_time(u.last_login_at)) }
            td { (u.sets) " / " (u.items) }
            td { (fmt_bytes(u.storage_bytes)) }
            td {
                details.manage {
                    summary { "Manage" span.sr-only { " " (u.user.username) } }
                    div.manage-box {
                        (form("role", html! {
                            label for=(format!("role-{id}")) { "Role" }
                            select id=(format!("role-{id}")) name="role" {
                                option value="user" selected[u.user.role == Role::User] { "user" }
                                option value="admin" selected[u.user.role == Role::Admin] { "admin" }
                            }
                            button.small type="submit" { "Set role" }
                        }))
                        @if !is_self {
                            (form(if u.disabled { "enable" } else { "disable" }, html! {
                                button.small type="submit" { @if u.disabled { "Enable account" } @else { "Disable account" } }
                            }))
                        }
                        (form("password", html! {
                            label for=(format!("pw-{id}")) { "New password" }
                            input id=(format!("pw-{id}")) type="password" name="password" minlength="8" required autocomplete="new-password";
                            button.small type="submit" { "Reset password" }
                        }))
                        @if u.user.totp_enabled {
                            (form("reset_totp", html! { button.small type="submit" { "Reset TOTP (lost device)" } }))
                        }
                        (form("tailscale", html! {
                            label for=(format!("ts-{id}")) { "Tailnet login" }
                            input id=(format!("ts-{id}")) type="text" name="tailscale_login"
                                value=(u.user.tailscale_login.as_deref().unwrap_or("")) placeholder="alice@github" spellcheck="false";
                            button.small type="submit" { "Bind" }
                        }))
                        @if !is_self {
                            a.button.small.danger href=(format!("/admin/users/{id}/delete")) { "Delete…" }
                        }
                    }
                }
            }
        }
    }
}

fn users_page(ctx: &AdminCtx, users: &[AdminUser], notice: Option<&str>, error: Option<&str>) -> Markup {
    let body = html! {
        @if let Some(n) = notice { (layout::alert("success", n)) }
        @if let Some(e) = error { (layout::alert("error", e)) }
        div.table-wrap {
            table.data.users {
                thead { tr {
                    th scope="col" { "User" } th scope="col" { "Role" } th scope="col" { "Status" } th scope="col" { "Tailnet" }
                    th scope="col" { "Last sign-in" } th scope="col" { "Sets / items" } th scope="col" { "Storage" }
                    th scope="col" { span.sr-only { "Actions" } }
                } }
                tbody { @for u in users { (user_row(ctx, u)) } }
            }
        }
        section aria-labelledby="a-new-user" {
            h2 #a-new-user { "Create a user" }
            form.stack.narrow method="post" action="/admin/users" {
                (ctx.csrf_input())
                div.two-col {
                    div.field-row {
                        label for="nu-name" { "Username" }
                        input #nu-name type="text" name="username" required minlength="2" maxlength="32" spellcheck="false" autocapitalize="none";
                    }
                    div.field-row {
                        label for="nu-display" { "Display name (optional)" }
                        input #nu-display type="text" name="display_name" maxlength="100";
                    }
                }
                div.two-col {
                    div.field-row {
                        label for="nu-pw" { "Password (optional)" }
                        input #nu-pw type="password" name="password" minlength="8" autocomplete="new-password" aria-describedby="nu-pw-hint";
                        p.hint #nu-pw-hint { "Leave empty for tailnet-only accounts." }
                    }
                    div.field-row {
                        label for="nu-ts" { "Tailnet login (optional)" }
                        input #nu-ts type="text" name="tailscale_login" placeholder="alice@github" spellcheck="false";
                    }
                }
                div.field-row {
                    label for="nu-role" { "Role" }
                    select #nu-role name="role" { option value="user" { "user" } option value="admin" { "admin" } }
                }
                button.primary type="submit" { "Create user" }
            }
        }
    };
    page(ctx, "Users", Tab::Users, body)
}

pub async fn users(State(state): State<AppState>, ctx: AdminCtx, Query(q): Query<DoneQuery>) -> AppResult<Markup> {
    let list = svc::list_users(&state).await?;
    Ok(users_page(&ctx, &list, user_done(q.done.as_deref()), None))
}

async fn users_error(state: &AppState, ctx: &AdminCtx, e: AppError) -> AppResult<Response> {
    if e.status.is_server_error() {
        return Err(e);
    }
    let list = svc::list_users(state).await?;
    Ok((e.status, users_page(ctx, &list, None, Some(&e.message))).into_response())
}

fn opt(s: &str) -> Option<String> {
    Some(s.trim().to_string()).filter(|s| !s.is_empty())
}

fn parse_role(s: &str) -> Role {
    if s == "admin" {
        Role::Admin
    } else {
        Role::User
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct CreateUserForm {
    #[serde(default)]
    username: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    tailscale_login: String,
    #[serde(default)]
    role: String,
}

pub async fn create_user(
    State(state): State<AppState>,
    ctx: AdminCtx,
    Form(f): Form<CreateUserForm>,
) -> AppResult<Response> {
    let req = AdminCreateUser {
        username: f.username.trim().to_string(),
        display_name: opt(&f.display_name),
        password: Some(f.password.clone()).filter(|p| !p.is_empty()),
        role: parse_role(&f.role),
        tailscale_login: opt(&f.tailscale_login),
    };
    match svc::create_user(&state, &ctx.user, req).await {
        Ok(_) => Ok(ctx.redirect("/admin/users?done=created")),
        Err(e) => users_error(&state, &ctx, e).await,
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateUserForm {
    #[serde(default)]
    action: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    tailscale_login: String,
}

pub async fn update_user(
    State(state): State<AppState>,
    ctx: AdminCtx,
    Path(id): Path<String>,
    Form(f): Form<UpdateUserForm>,
) -> AppResult<Response> {
    let mut req = AdminUpdateUser::default();
    match f.action.as_str() {
        "role" => req.role = Some(parse_role(&f.role)),
        "disable" => req.disabled = Some(true),
        "enable" => req.disabled = Some(false),
        "password" => req.password = Some(f.password.clone()),
        "reset_totp" => req.reset_totp = true,
        "tailscale" => req.tailscale_login = Some(opt(&f.tailscale_login)),
        _ => return Err(AppError::bad_request("unknown action")),
    }
    match svc::update_user(&state, &ctx.user, &id, req).await {
        Ok(_) => Ok(ctx.redirect("/admin/users?done=updated")),
        Err(e) if e.code == "not_found" => Err(e),
        Err(e) => users_error(&state, &ctx, e).await,
    }
}

pub async fn delete_user_page(
    State(state): State<AppState>,
    ctx: AdminCtx,
    Path(id): Path<String>,
) -> AppResult<Markup> {
    let list = svc::list_users(&state).await?;
    let u = list.iter().find(|u| u.user.id == id).ok_or_else(AppError::not_found)?;
    let body = html! {
        h2 { "Delete " (u.user.username) "?" }
        p { "This deletes the account and everything it owns: " (u.sets) " sets with " (u.items) " items, its tokens and sessions. It cannot be undone." }
        form.inline-row method="post" action=(format!("/admin/users/{id}/delete")) {
            (ctx.csrf_input())
            button.danger type="submit" { "Delete user" }
            a.button href="/admin/users" { "Cancel" }
        }
    };
    Ok(page(&ctx, "Delete user", Tab::Users, body))
}

pub async fn delete_user(State(state): State<AppState>, ctx: AdminCtx, Path(id): Path<String>) -> AppResult<Response> {
    match svc::delete_user(&state, &ctx.user, &id).await {
        Ok(()) => Ok(ctx.redirect("/admin/users?done=deleted")),
        Err(e) if e.code == "not_found" => Err(e),
        Err(e) => users_error(&state, &ctx, e).await,
    }
}

// ---------------------------------------------------------------------------
// Invitations
// ---------------------------------------------------------------------------

async fn invitations_page(
    state: &AppState,
    ctx: &AdminCtx,
    created: Option<&str>,
    notice: Option<&str>,
) -> AppResult<Markup> {
    let list = svc::list_invitations(state).await?;
    let users = svc::list_users(state).await?;
    let name = |id: &str| {
        users
            .iter()
            .find(|u| u.user.id == id)
            .map(|u| u.user.username.clone())
            .unwrap_or_else(|| "deleted user".into())
    };
    let t = now();
    let policy = state.config.auth.registration;
    let body = html! {
        @if let Some(n) = notice { (layout::alert("success", n)) }
        p {
            "Registration is " strong { (registration_label(policy)) } "."
            @if policy != Registration::Invite { " Invitation links only work while registration is set to “invite”." }
        }
        @if let Some(url) = created {
            section.secret-box {
                h2 { "Invitation created" }
                p { "Send this link to the person. " strong { "It is shown only now." } }
                p { code.secret.copyable { (url) } }
            }
        }
        form.inline-row method="post" action="/admin/invitations" {
            (ctx.csrf_input())
            label for="inv-role" { "Role" }
            select #inv-role name="role" { option value="user" { "user" } option value="admin" { "admin" } }
            label for="inv-exp" { "Valid for" }
            select #inv-exp name="expires_days" {
                option value="1" { "1 day" }
                option value="7" selected { "7 days" }
                option value="30" { "30 days" }
                option value="365" { "1 year" }
            }
            button.primary type="submit" { "Create invitation" }
        }
        @if list.is_empty() {
            p.muted { "No invitations." }
        } @else {
            table.data {
                thead { tr { th scope="col" { "Created" } th scope="col" { "By" } th scope="col" { "Role" } th scope="col" { "Status" } th scope="col" { span.sr-only { "Actions" } } } }
                tbody {
                    @for inv in &list {
                        tr {
                            td { (time_el(inv.created_at)) }
                            td { (name(&inv.created_by)) }
                            td { (if inv.role == Role::Admin { "admin" } else { "user" }) }
                            td {
                                @if let Some(u) = &inv.used_by {
                                    "used by " (name(u)) @if let Some(at) = inv.used_at { ", " (time_el(at)) }
                                } @else if inv.expires_at.is_some_and(|e| e <= t) {
                                    span.muted { "expired" }
                                } @else {
                                    "open"
                                    @if let Some(e) = inv.expires_at { ", expires " (time_el(e)) }
                                }
                            }
                            td {
                                form.inline method="post" action=(format!("/admin/invitations/{}/delete", inv.id)) {
                                    (ctx.csrf_input())
                                    button.small.danger type="submit" { "Revoke" }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(page(ctx, "Invitations", Tab::Invitations, body))
}

pub async fn invitations(
    State(state): State<AppState>,
    ctx: AdminCtx,
    Query(q): Query<DoneQuery>,
) -> AppResult<Markup> {
    let notice = (q.done.as_deref() == Some("revoked")).then_some("Invitation revoked.");
    invitations_page(&state, &ctx, None, notice).await
}

#[derive(Debug, Default, Deserialize)]
pub struct InvitationForm {
    #[serde(default)]
    role: String,
    #[serde(default)]
    expires_days: String,
}

pub async fn create_invitation(
    State(state): State<AppState>,
    ctx: AdminCtx,
    Form(f): Form<InvitationForm>,
) -> AppResult<Markup> {
    let days: i64 = f.expires_days.trim().parse().unwrap_or(7);
    let req = CreateInvitation {
        role: parse_role(&f.role),
        expires_in: Some(days.clamp(1, 365) * 86_400),
    };
    let created = svc::create_invitation(&state, &ctx.user, req).await?;
    invitations_page(&state, &ctx, Some(&created.url), None).await
}

pub async fn delete_invitation(
    State(state): State<AppState>,
    ctx: AdminCtx,
    Path(id): Path<String>,
) -> AppResult<Response> {
    svc::delete_invitation(&state, &ctx.user, &id).await?;
    Ok(ctx.redirect("/admin/invitations?done=revoked"))
}

// ---------------------------------------------------------------------------
// Audit log
// ---------------------------------------------------------------------------

const AUDIT_PAGE: i64 = 50;

#[derive(Debug, Default, Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    before: Option<i64>,
}

pub async fn audit(State(state): State<AppState>, ctx: AdminCtx, Query(q): Query<AuditQuery>) -> AppResult<Markup> {
    let entries = svc::audit_page(&state, q.before, Some(AUDIT_PAGE)).await?;
    let older = (entries.len() as i64 == AUDIT_PAGE)
        .then(|| entries.last().map(|e| e.id))
        .flatten();
    let body = html! {
        div.table-wrap {
            table.data.audit {
                thead { tr { th scope="col" { "When" } th scope="col" { "Who" } th scope="col" { "Action" } th scope="col" { "Target" } th scope="col" { "IP" } th scope="col" { "Details" } } }
                tbody {
                    @for e in &entries {
                        tr {
                            td { (time_el(e.at)) }
                            td { (e.actor_username.as_deref().or(e.actor_id.as_deref()).unwrap_or("—")) }
                            td { code { (e.action) } }
                            td { code.small { (e.target.as_deref().unwrap_or("")) } }
                            td { (e.ip.as_deref().unwrap_or("")) }
                            td { @if let Some(d) = &e.details { code.small.details { (d.to_string()) } } }
                        }
                    }
                }
            }
        }
        @if entries.is_empty() { p.muted { "No entries." } }
        nav.pager aria-label="Audit log pages" {
            @if q.before.is_some() { a.button href="/admin/audit" { "Newest" } }
            @if let Some(id) = older { a.button href=(format!("/admin/audit?before={id}")) { "Older entries" } }
        }
    };
    Ok(page(&ctx, "Audit log", Tab::Audit, body))
}
