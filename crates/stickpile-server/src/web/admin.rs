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
use crate::i18n::{t, t_dyn, tf};
use crate::ids::now;
use crate::service::admin as svc;
use crate::service::ui;
use crate::service::users::USERNAME_PATTERN;
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
        (Tab::Overview, "/admin", t("Overview")),
        (Tab::Users, "/admin/users", t("Users")),
        (Tab::Invitations, "/admin/invitations", t("Invitations")),
        (Tab::Audit, "/admin/audit", t("Audit log")),
    ];
    html! {
        nav.tabs aria-label=(t("Admin sections")) {
            @for (tab, href, label) in items {
                @if tab == active { a href=(href) aria-current="page" { (label) } } @else { a href=(href) { (label) } }
            }
        }
    }
}

fn page(ctx: &AdminCtx, title: &str, tab: Tab, body: Markup) -> Markup {
    ctx.page(
        title,
        Nav::Admin,
        html! {
            h1 { (t("Admin")) }
            (tabs(tab))
            (body)
        },
    )
}

fn registration_label(r: Registration) -> &'static str {
    match r {
        Registration::Closed => t("closed (admins create accounts)"),
        Registration::Invite => t("by invitation"),
        Registration::Open => t("open to anyone who can reach the server"),
    }
}

/// A role in the request's language.
fn role_label(r: Role) -> &'static str {
    if r == Role::Admin {
        t("admin")
    } else {
        t("user")
    }
}

/// A job status as stored (`queued`...) in the request's language.
fn job_status_label(s: &str) -> &str {
    match s {
        "queued" => t("queued"),
        "running" => t("running"),
        "done" => t("done"),
        "failed" => t("failed"),
        other => other,
    }
}

/// A job kind as stored (`git_pull`...) in the request's language.
fn job_kind_label(k: &str) -> String {
    match k {
        "variants" => t("variants").to_string(),
        "enrich" => t("enrich").to_string(),
        "git_pull" => t("git pull").to_string(),
        "git_push" => t("git push").to_string(),
        "telegram_import" => t("telegram import").to_string(),
        "telegram_export" => t("telegram export").to_string(),
        other => other.replace('_', " "),
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
        AiStatus::Off => t("off"),
        AiStatus::Loading => t("loading"),
        AiStatus::Ready => t("ready"),
        AiStatus::Failed => t("failed to load (see the server log)"),
    };
    let pending: u64 = counts
        .iter()
        .filter(|c| c.status == "queued" || c.status == "running")
        .map(|c| c.count)
        .sum();
    let body = html! {
        dl.tiles {
            div.tile { dt { (t("Media on disk")) } dd { (fmt_bytes(storage.media_bytes)) } }
            div.tile { dt { (t("Database")) } dd { (fmt_bytes(storage.database_bytes)) } }
            div.tile { dt { (t("Media files")) } dd { (storage.media_count) } }
            div.tile { dt { (t("Items")) } dd { (storage.item_count) } }
            div.tile { dt { (t("Jobs waiting or running")) } dd { (pending) } }
        }
        dl.kv {
            dt { (t("Registration")) } dd { (registration_label(state.config.auth.registration)) " " span.muted { (t("(set in the configuration)")) } }
            dt { (t("AI enrichment")) } dd { (ai) }
            dt { (t("Public URL")) } dd { code { (state.config.server.public_url) } }
        }
        div.two-col {
            section aria-labelledby="a-storage" {
                h2 #a-storage { (t("Storage per user")) }
                p.hint { (t("Media referenced by each user's items; shared files count for every user.")) }
                table.data {
                    thead { tr { th scope="col" { (t("User")) } th scope="col" { (t("Bytes")) } } }
                    tbody {
                        @for (u, b) in &storage.per_user {
                            tr { th scope="row" { (u) } td { (fmt_bytes(*b)) } }
                        }
                    }
                }
            }
            section aria-labelledby="a-jobs" {
                h2 #a-jobs { (t("Job queue")) }
                @if counts.is_empty() {
                    p.muted { (t("No jobs.")) }
                } @else {
                    table.data {
                        thead { tr { th scope="col" { (t("Kind")) } th scope="col" { (t("Status")) } th scope="col" { (t("Jobs")) } } }
                        tbody {
                            @for c in &counts {
                                tr class=(format!("job-{}", c.status)) {
                                    th scope="row" { (job_kind_label(&c.kind)) }
                                    td { (job_status_label(&c.status)) }
                                    td { (c.count) }
                                }
                            }
                        }
                    }
                }
                p.hint { (t("Finished jobs are kept for 30 days.")) }
            }
        }
        section aria-labelledby="a-fail" {
            h2 #a-fail { (t("Recent failures")) }
            @if failures.is_empty() {
                p.muted { (t("None.")) }
            } @else {
                table.data {
                    thead { tr { th scope="col" { (t("When")) } th scope="col" { (t("Kind")) } th scope="col" { (t("Subject")) } th scope="col" { (t("Attempts")) } th scope="col" { (t("Error")) } } }
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
    Ok(page(&ctx, t("Admin"), Tab::Overview, body))
}

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

fn user_done(done: Option<&str>) -> Option<&'static str> {
    Some(match done? {
        "created" => t("User created."),
        "updated" => t("User updated."),
        "deleted" => t("User deleted."),
        _ => return None,
    })
}

fn user_row(ctx: &AdminCtx, u: &AdminUser, min_pw: usize) -> Markup {
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
            td { (role_label(u.user.role)) }
            td {
                @if u.disabled { span.badge.vis-private { (t("disabled")) } } @else { (t("active")) }
                @if u.user.totp_enabled { " " span.badge { "TOTP" } }
                @if !u.has_password { " " span.badge title=(t("No password: tailnet sign-in only")) { (t("no password")) } }
            }
            td { @if let Some(l) = &u.user.tailscale_login { code { (l) } } }
            td { (opt_time(u.last_login_at)) }
            td { (u.sets) " / " (u.items) }
            td { (fmt_bytes(u.storage_bytes)) }
            td {
                details.manage {
                    summary { (t("Manage")) span.sr-only { " " (u.user.username) } }
                    div.manage-box {
                        (form("username", html! {
                            label for=(format!("un-{id}")) { (t("Username")) }
                            input id=(format!("un-{id}")) type="text" name="username" value=(u.user.username) required
                                minlength="2" maxlength="32" pattern=(USERNAME_PATTERN) spellcheck="false" autocapitalize="none";
                            button.small type="submit" { (t("Rename")) }
                        }))
                        (form("role", html! {
                            label for=(format!("role-{id}")) { (t("Role")) }
                            select id=(format!("role-{id}")) name="role" {
                                option value="user" selected[u.user.role == Role::User] { (t("user")) }
                                option value="admin" selected[u.user.role == Role::Admin] { (t("admin")) }
                            }
                            button.small type="submit" { (t("Set role")) }
                        }))
                        @if !is_self {
                            (form(if u.disabled { "enable" } else { "disable" }, html! {
                                button.small type="submit" { @if u.disabled { (t("Enable account")) } @else { (t("Disable account")) } }
                            }))
                        }
                        (form("password", html! {
                            label for=(format!("pw-{id}")) { (t("New password")) }
                            input id=(format!("pw-{id}")) type="password" name="password" minlength=(min_pw) required autocomplete="new-password";
                            button.small type="submit" { (t("Reset password")) }
                        }))
                        @if u.user.totp_enabled {
                            (form("reset_totp", html! { button.small type="submit" { (t("Reset TOTP (lost device)")) } }))
                        }
                        (form("tailscale", html! {
                            label for=(format!("ts-{id}")) { (t("Tailnet login")) }
                            input id=(format!("ts-{id}")) type="text" name="tailscale_login"
                                value=(u.user.tailscale_login.as_deref().unwrap_or("")) placeholder="alice@github" spellcheck="false";
                            button.small type="submit" { (t("Bind")) }
                        }))
                        @if !is_self {
                            a.button.small.danger href=(format!("/admin/users/{id}/delete")) { (t("Delete…")) }
                        }
                    }
                }
            }
        }
    }
}

fn users_page(ctx: &AdminCtx, users: &[AdminUser], min_pw: usize, notice: Option<&str>, error: Option<&str>) -> Markup {
    let body = html! {
        @if let Some(n) = notice { (layout::alert("success", n)) }
        @if let Some(e) = error { (layout::alert("error", e)) }
        div.table-wrap {
            table.data.users {
                thead { tr {
                    th scope="col" { (t("User")) } th scope="col" { (t("Role")) } th scope="col" { (t("Status")) } th scope="col" { "Tailnet" }
                    th scope="col" { (t("Last sign-in")) } th scope="col" { (t("Sets / items")) } th scope="col" { (t("Storage")) }
                    th scope="col" { span.sr-only { (t("Actions")) } }
                } }
                tbody { @for u in users { (user_row(ctx, u, min_pw)) } }
            }
        }
        section aria-labelledby="a-new-user" {
            h2 #a-new-user { (t("Create a user")) }
            form.stack.narrow method="post" action="/admin/users" {
                (ctx.csrf_input())
                div.two-col {
                    div.field-row {
                        label for="nu-name" { (t("Username")) }
                        input #nu-name type="text" name="username" required minlength="2" maxlength="32" spellcheck="false" autocapitalize="none";
                    }
                    div.field-row {
                        label for="nu-display" { (t("Display name (optional)")) }
                        input #nu-display type="text" name="display_name" maxlength="100";
                    }
                }
                div.two-col {
                    div.field-row {
                        label for="nu-pw" { (t("Password (optional)")) }
                        input #nu-pw type="password" name="password" minlength=(min_pw) autocomplete="new-password" aria-describedby="nu-pw-hint";
                        p.hint #nu-pw-hint { (t("Leave empty for tailnet-only accounts.")) }
                    }
                    div.field-row {
                        label for="nu-ts" { (t("Tailnet login (optional)")) }
                        input #nu-ts type="text" name="tailscale_login" placeholder="alice@github" spellcheck="false";
                    }
                }
                div.field-row {
                    label for="nu-role" { (t("Role")) }
                    select #nu-role name="role" { option value="user" { (t("user")) } option value="admin" { (t("admin")) } }
                }
                button.primary type="submit" { (t("Create user")) }
            }
        }
    };
    page(ctx, t("Users"), Tab::Users, body)
}

pub async fn users(State(state): State<AppState>, ctx: AdminCtx, Query(q): Query<DoneQuery>) -> AppResult<Markup> {
    let list = svc::list_users(&state).await?;
    Ok(users_page(
        &ctx,
        &list,
        state.config.auth.min_password_length,
        user_done(q.done.as_deref()),
        None,
    ))
}

async fn users_error(state: &AppState, ctx: &AdminCtx, e: AppError) -> AppResult<Response> {
    if e.status.is_server_error() {
        return Err(e);
    }
    let list = svc::list_users(state).await?;
    Ok((
        e.status,
        users_page(
            ctx,
            &list,
            state.config.auth.min_password_length,
            None,
            Some(&t_dyn(&e.message)),
        ),
    )
        .into_response())
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
    username: String,
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
        "username" => req.username = Some(f.username.clone()),
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
        h2 { (tf("Delete {username}?", &[("username", &u.user.username)])) }
        p {
            (tf(
                "This deletes the account and everything it owns: {sets} sets with {items} items, its tokens and sessions. It cannot be undone.",
                &[("sets", &u.sets), ("items", &u.items)],
            ))
        }
        form.inline-row method="post" action=(format!("/admin/users/{id}/delete")) {
            (ctx.csrf_input())
            button.danger type="submit" { (t("Delete user")) }
            a.button href="/admin/users" { (t("Cancel")) }
        }
    };
    Ok(page(&ctx, t("Delete user"), Tab::Users, body))
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
            .unwrap_or_else(|| t("deleted user").into())
    };
    let now_ts = now();
    let policy = state.config.auth.registration;
    let body = html! {
        @if let Some(n) = notice { (layout::alert("success", n)) }
        p {
            (t("Registration is")) " " strong { (registration_label(policy)) } "."
            @if policy != Registration::Invite { " " (t("Invitation links only work while registration is set to “invite”.")) }
        }
        @if let Some(url) = created {
            section.secret-box {
                h2 { (t("Invitation created")) }
                p { (t("Send this link to the person.")) " " strong { (t("It is shown only now.")) } }
                p { code.secret.copyable { (url) } }
            }
        }
        form.inline-row method="post" action="/admin/invitations" {
            (ctx.csrf_input())
            label for="inv-role" { (t("Role")) }
            select #inv-role name="role" { option value="user" { (t("user")) } option value="admin" { (t("admin")) } }
            label for="inv-exp" { (t("Valid for")) }
            select #inv-exp name="expires_days" {
                option value="1" { (t("1 day")) }
                option value="7" selected { (t("7 days")) }
                option value="30" { (t("30 days")) }
                option value="365" { (t("1 year")) }
            }
            button.primary type="submit" { (t("Create invitation")) }
        }
        @if list.is_empty() {
            p.muted { (t("No invitations.")) }
        } @else {
            table.data {
                thead { tr { th scope="col" { (t("Created")) } th scope="col" { (t("By")) } th scope="col" { (t("Role")) } th scope="col" { (t("Status")) } th scope="col" { span.sr-only { (t("Actions")) } } } }
                tbody {
                    @for inv in &list {
                        tr {
                            td { (time_el(inv.created_at)) }
                            td { (name(&inv.created_by)) }
                            td { (role_label(inv.role)) }
                            td {
                                @if let Some(u) = &inv.used_by {
                                    (tf("used by {user}", &[("user", &name(u))])) @if let Some(at) = inv.used_at { ", " (time_el(at)) }
                                } @else if inv.expires_at.is_some_and(|e| e <= now_ts) {
                                    span.muted { (t("expired")) }
                                } @else {
                                    (t("open"))
                                    @if let Some(e) = inv.expires_at { ", " (t("expires")) " " (time_el(e)) }
                                }
                            }
                            td {
                                form.inline method="post" action=(format!("/admin/invitations/{}/delete", inv.id)) {
                                    (ctx.csrf_input())
                                    button.small.danger type="submit" { (t("Revoke")) }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(page(ctx, t("Invitations"), Tab::Invitations, body))
}

pub async fn invitations(
    State(state): State<AppState>,
    ctx: AdminCtx,
    Query(q): Query<DoneQuery>,
) -> AppResult<Markup> {
    let notice = (q.done.as_deref() == Some("revoked")).then_some(t("Invitation revoked."));
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
                thead { tr { th scope="col" { (t("When")) } th scope="col" { (t("Who")) } th scope="col" { (t("Action")) } th scope="col" { (t("Target")) } th scope="col" { "IP" } th scope="col" { (t("Details")) } } }
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
        @if entries.is_empty() { p.muted { (t("No entries.")) } }
        nav.pager aria-label=(t("Audit log pages")) {
            @if q.before.is_some() { a.button href="/admin/audit" { (t("Newest")) } }
            @if let Some(id) = older { a.button href=(format!("/admin/audit?before={id}")) { (t("Older entries")) } }
        }
    };
    Ok(page(&ctx, t("Audit log"), Tab::Audit, body))
}
