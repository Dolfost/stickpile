//! Admin API: users, invitations, audit log, storage, jobs. Every mutating
//! action is audited.

use stickpile_api::{
    AdminCreateUser, AdminUpdateUser, AdminUser, AuditEntry, CreateInvitation, Invitation, InvitationCreated, Role,
    StorageUsage,
};

use crate::auth::{login::invitation_hash, password, session, tokens, CurrentUser};
use crate::db::{self, enum_str, UserRow, USER_COLUMNS};
use crate::error::{AppError, AppResult};
use crate::ids::{is_id, new_id, now, random_base32};
use crate::service::sets;
use crate::service::stats::user_storage;
use crate::service::users::{self, NewUser};
use crate::state::AppState;

async fn to_admin_user(state: &AppState, u: &UserRow) -> AppResult<AdminUser> {
    let sets_n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sets WHERE owner_id = ?")
        .bind(&u.id)
        .fetch_one(&state.db)
        .await?;
    let items_n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM items i JOIN sets s ON s.id = i.set_id WHERE s.owner_id = ?")
            .bind(&u.id)
            .fetch_one(&state.db)
            .await?;
    Ok(AdminUser {
        user: u.to_api(),
        disabled: u.disabled,
        has_password: u.password_hash.is_some(),
        last_login_at: u.last_login_at,
        sets: sets_n.max(0) as u64,
        items: items_n.max(0) as u64,
        storage_bytes: user_storage(&state.db, &u.id).await?,
    })
}

async fn user_or_404(state: &AppState, id: &str) -> AppResult<UserRow> {
    if !is_id(id) {
        return Err(AppError::not_found());
    }
    db::user_by_id(&state.db, id).await?.ok_or_else(AppError::not_found)
}

pub async fn list_users(state: &AppState) -> AppResult<Vec<AdminUser>> {
    let rows = sqlx::query_as::<_, UserRow>(&format!("SELECT {USER_COLUMNS} FROM users ORDER BY username"))
        .fetch_all(&state.db)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for u in &rows {
        out.push(to_admin_user(state, u).await?);
    }
    Ok(out)
}

pub async fn create_user(state: &AppState, admin: &CurrentUser, req: AdminCreateUser) -> AppResult<AdminUser> {
    if let Some(pw) = &req.password {
        password::validate(pw, state.config.auth.min_password_length)?;
    }
    let user = users::create(
        &state.db,
        NewUser {
            username: req.username,
            display_name: req.display_name,
            password: req.password,
            role: req.role,
            tailscale_login: req.tailscale_login,
        },
    )
    .await?;
    db::audit(
        &state.db,
        Some(admin.id()),
        "user.create",
        Some(&user.id),
        admin.ip_string().as_deref(),
        Some(
            serde_json::json!({"username": user.username, "role": user.role, "tailscale_login": user.tailscale_login}),
        ),
    )
    .await;
    to_admin_user(state, &user).await
}

pub async fn update_user(
    state: &AppState,
    admin: &CurrentUser,
    id: &str,
    req: AdminUpdateUser,
) -> AppResult<AdminUser> {
    let user = user_or_404(state, id).await?;
    let demoting = req.role == Some(Role::User) && user.is_admin();
    let disabling = req.disabled == Some(true) && !user.disabled;
    if (demoting || disabling) && user.is_admin() && !user.disabled && users::active_admin_count(&state.db).await? <= 1
    {
        return Err(AppError::conflict("cannot demote or disable the last active admin"));
    }
    let display = req
        .display_name
        .as_deref()
        .map(users::validate_display_name)
        .transpose()?;
    let mut changes = serde_json::Map::new();
    let mut user = user;
    let mut renamed_from = None;
    if let Some(name) = &req.username {
        if let Some(old) = users::rename(state, id, name).await? {
            changes.insert("username".into(), serde_json::json!({"from": old, "to": name.trim()}));
            user = user_or_404(state, id).await?;
            renamed_from = Some(old);
        }
    }
    if let Some(d) = display {
        let d = if d.is_empty() || renamed_from.as_deref() == Some(d.as_str()) {
            user.username.clone()
        } else {
            d
        };
        sqlx::query("UPDATE users SET display_name = ? WHERE id = ?")
            .bind(&d)
            .bind(id)
            .execute(&state.db)
            .await?;
        changes.insert("display_name".into(), d.into());
    }
    if let Some(role) = req.role {
        sqlx::query("UPDATE users SET role = ? WHERE id = ?")
            .bind(enum_str(&role))
            .bind(id)
            .execute(&state.db)
            .await?;
        changes.insert("role".into(), enum_str(&role).into());
    }
    if let Some(disabled) = req.disabled {
        sqlx::query("UPDATE users SET disabled = ? WHERE id = ?")
            .bind(disabled)
            .bind(id)
            .execute(&state.db)
            .await?;
        if disabled {
            session::destroy_all(&state.db, id, None).await?;
        }
        changes.insert("disabled".into(), disabled.into());
    }
    if let Some(pw) = req.password {
        password::validate(&pw, state.config.auth.min_password_length)?;
        let hash = password::hash(pw).await?;
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(hash)
            .bind(id)
            .execute(&state.db)
            .await?;
        session::destroy_all(&state.db, id, None).await?;
        // A reset means the old credentials may be compromised: API tokens go
        // too (a self-service password change keeps them).
        let revoked = tokens::revoke_all(&state.db, id).await?;
        changes.insert("password".into(), "reset".into());
        changes.insert("tokens_revoked".into(), revoked.into());
    }
    if let Some(login) = &req.tailscale_login {
        let login = login.as_deref().map(users::validate_tailscale_login).transpose()?;
        let res = sqlx::query("UPDATE users SET tailscale_login = ? WHERE id = ?")
            .bind(&login)
            .bind(id)
            .execute(&state.db)
            .await;
        match res {
            Ok(_) => {}
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(AppError::conflict("that tailscale login is bound to another user"));
            }
            Err(e) => return Err(e.into()),
        }
        changes.insert(
            "tailscale_login".into(),
            login.map(Into::into).unwrap_or(serde_json::Value::Null),
        );
    }
    if req.reset_totp {
        sqlx::query(
            "UPDATE users SET totp_secret = NULL, totp_pending_secret = NULL, totp_last_step = NULL WHERE id = ?",
        )
        .bind(id)
        .execute(&state.db)
        .await?;
        changes.insert("reset_totp".into(), true.into());
    }
    db::audit(
        &state.db,
        Some(admin.id()),
        "user.update",
        Some(id),
        admin.ip_string().as_deref(),
        Some(serde_json::Value::Object(changes)),
    )
    .await;
    let user = user_or_404(state, id).await?;
    to_admin_user(state, &user).await
}

/// Delete a user and everything they own (sets, items, tokens, sessions).
pub async fn delete_user(state: &AppState, admin: &CurrentUser, id: &str) -> AppResult<()> {
    let user = user_or_404(state, id).await?;
    if user.id == admin.id() {
        return Err(AppError::conflict("you cannot delete your own account"));
    }
    if user.is_admin() && !user.disabled && users::active_admin_count(&state.db).await? <= 1 {
        return Err(AppError::conflict("cannot delete the last active admin"));
    }
    let owned: Vec<String> = sqlx::query_scalar("SELECT id FROM sets WHERE owner_id = ?")
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    let sets_deleted = owned.len();
    for set_id in owned {
        if let Some(set) = db::set_by_id(&state.db, &set_id).await? {
            sets::delete_row(state, &set).await?;
        }
    }
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    db::audit(
        &state.db,
        Some(admin.id()),
        "user.delete",
        Some(id),
        admin.ip_string().as_deref(),
        Some(serde_json::json!({"username": user.username, "sets_deleted": sets_deleted})),
    )
    .await;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct InvitationRow {
    id: String,
    role: String,
    created_by: String,
    created_at: i64,
    expires_at: Option<i64>,
    used_by: Option<String>,
    used_at: Option<i64>,
}

impl InvitationRow {
    fn to_api(&self) -> Invitation {
        Invitation {
            id: self.id.clone(),
            role: if self.role == "admin" { Role::Admin } else { Role::User },
            created_by: self.created_by.clone(),
            created_at: self.created_at,
            expires_at: self.expires_at,
            used_by: self.used_by.clone(),
            used_at: self.used_at,
        }
    }
}

const INVITATION_COLUMNS: &str = "id, role, created_by, created_at, expires_at, used_by, used_at";

pub async fn list_invitations(state: &AppState) -> AppResult<Vec<Invitation>> {
    let rows = sqlx::query_as::<_, InvitationRow>(&format!(
        "SELECT {INVITATION_COLUMNS} FROM invitations ORDER BY created_at DESC, id"
    ))
    .fetch_all(&state.db)
    .await?;
    Ok(rows.iter().map(InvitationRow::to_api).collect())
}

/// Default invitation lifetime: 7 days.
pub const INVITATION_TTL: i64 = 7 * 86_400;

/// Create an invitation; the code is only in the returned URL.
pub async fn create_invitation(
    state: &AppState,
    admin: &CurrentUser,
    req: CreateInvitation,
) -> AppResult<InvitationCreated> {
    let ttl = req.expires_in.unwrap_or(INVITATION_TTL);
    if !(60..=365 * 86_400).contains(&ttl) {
        return Err(AppError::bad_request(
            "expires_in must be between 60 seconds and 365 days",
        ));
    }
    let code = random_base32(20);
    let id = new_id();
    let t = now();
    sqlx::query(
        "INSERT INTO invitations (id, code_hash, role, created_by, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(invitation_hash(&code))
    .bind(enum_str(&req.role))
    .bind(admin.id())
    .bind(t)
    .bind(t + ttl)
    .execute(&state.db)
    .await?;
    db::audit(
        &state.db,
        Some(admin.id()),
        "invitation.create",
        Some(&id),
        admin.ip_string().as_deref(),
        Some(serde_json::json!({"role": enum_str(&req.role), "expires_at": t + ttl})),
    )
    .await;
    let row = sqlx::query_as::<_, InvitationRow>(&format!("SELECT {INVITATION_COLUMNS} FROM invitations WHERE id = ?"))
        .bind(&id)
        .fetch_one(&state.db)
        .await?;
    let base = state.config.server.public_url.trim_end_matches('/');
    Ok(InvitationCreated {
        invitation: row.to_api(),
        url: format!("{base}/register?code={code}"),
    })
}

pub async fn delete_invitation(state: &AppState, admin: &CurrentUser, id: &str) -> AppResult<()> {
    let n = sqlx::query("DELETE FROM invitations WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::not_found());
    }
    db::audit(
        &state.db,
        Some(admin.id()),
        "invitation.delete",
        Some(id),
        admin.ip_string().as_deref(),
        None,
    )
    .await;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct AuditRow {
    id: i64,
    at: i64,
    actor_id: Option<String>,
    actor_username: Option<String>,
    action: String,
    target: Option<String>,
    ip: Option<String>,
    details: Option<String>,
}

/// Newest first; `before` is an entry id for paging.
pub async fn audit_page(state: &AppState, before: Option<i64>, limit: Option<i64>) -> AppResult<Vec<AuditEntry>> {
    let limit = limit.unwrap_or(100).clamp(1, 1000);
    let rows = sqlx::query_as::<_, AuditRow>(
        "SELECT a.id, a.at, a.actor_id, u.username AS actor_username, a.action, a.target, a.ip, a.details \
         FROM audit_log a LEFT JOIN users u ON u.id = a.actor_id WHERE a.id < ? ORDER BY a.id DESC LIMIT ?",
    )
    .bind(before.unwrap_or(i64::MAX))
    .bind(limit)
    .fetch_all(&state.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| AuditEntry {
            id: r.id,
            at: r.at,
            actor_id: r.actor_id,
            actor_username: r.actor_username,
            action: r.action,
            target: r.target,
            ip: r.ip,
            details: r.details.and_then(|d| serde_json::from_str(&d).ok()),
        })
        .collect())
}

pub async fn storage(state: &AppState) -> AppResult<StorageUsage> {
    let media_dir = state.media_dir();
    let media_bytes = tokio::task::spawn_blocking(move || crate::files::dir_size_blocking(&media_dir)).await?;
    let db_path = state.config.db_path();
    let database_bytes = ["", "-wal", "-shm"]
        .iter()
        .filter_map(|suffix| std::fs::metadata(format!("{}{suffix}", db_path.display())).ok())
        .map(|m| m.len())
        .sum();
    let media_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media")
        .fetch_one(&state.db)
        .await?;
    let item_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
        .fetch_one(&state.db)
        .await?;
    let per_user: Vec<(String, i64)> = sqlx::query_as(
        "SELECT u.username, COALESCE(SUM(v.total), 0) FROM users u \
         LEFT JOIN (SELECT DISTINCT s.owner_id AS owner_id, i.media_id AS media_id FROM items i JOIN sets s ON s.id = i.set_id) x \
           ON x.owner_id = u.id \
         LEFT JOIN (SELECT media_id, SUM(size_bytes) AS total FROM media_variants GROUP BY media_id) v ON v.media_id = x.media_id \
         GROUP BY u.id ORDER BY 2 DESC, u.username",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(StorageUsage {
        media_bytes,
        database_bytes,
        media_count: media_count.max(0) as u64,
        item_count: item_count.max(0) as u64,
        per_user: per_user.into_iter().map(|(u, b)| (u, b.max(0) as u64)).collect(),
    })
}
