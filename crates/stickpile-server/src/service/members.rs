//! Contributors of a set (docs/collaboration.md): the owner adds users and
//! chooses what each may do ([`Permissions`]); a contributor may leave.
//!
//! A contributor sees the set like its owner does: private sets too, in
//! browse, search and their client cache. Adding one touches the set's
//! items so that cache picks them up; removing one (or leaving) records
//! targeted removals so it drops them again.

use stickpile_api::{Member, Permissions, PutMember, Visibility};

use crate::auth::CurrentUser;
use crate::authz;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::ids::now;
use crate::service::sets;
use crate::state::AppState;

#[derive(sqlx::FromRow)]
struct MemberRow {
    username: String,
    display_name: String,
    can_add: bool,
    can_edit_own: bool,
    can_edit_all: bool,
    can_remove_all: bool,
    can_edit_pack: bool,
    can_review: bool,
    created_at: i64,
}

impl MemberRow {
    fn to_api(&self) -> Member {
        Member {
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            permissions: Permissions {
                add: self.can_add,
                edit_own: self.can_edit_own,
                edit_all: self.can_edit_all,
                remove_all: self.can_remove_all,
                edit_pack: self.can_edit_pack,
                review: self.can_review,
            },
            added_at: self.created_at,
        }
    }
}

const MEMBER_SELECT: &str = "SELECT u.username, u.display_name, m.can_add, m.can_edit_own, m.can_edit_all, \
     m.can_remove_all, m.can_edit_pack, m.can_review, m.created_at \
     FROM set_members m JOIN users u ON u.id = m.user_id";

/// The contributors of a set, by username, without checking access.
pub async fn of_set(db: &sqlx::SqlitePool, set_id: &str) -> AppResult<Vec<Member>> {
    let rows: Vec<MemberRow> = sqlx::query_as(&format!(
        "{MEMBER_SELECT} WHERE m.set_id = ? ORDER BY u.username COLLATE NOCASE"
    ))
    .bind(set_id)
    .fetch_all(db)
    .await?;
    Ok(rows.iter().map(MemberRow::to_api).collect())
}

/// The contributors of a set, for its owner and its contributors (others
/// get 403; 404 if they cannot read the set).
pub async fn list(state: &AppState, viewer: &CurrentUser, set_id: &str) -> AppResult<Vec<Member>> {
    let set = authz::readable_set(state, viewer.id(), set_id).await?;
    if set.role() == stickpile_api::SetRole::Viewer {
        return Err(AppError::forbidden(
            "only the owner and contributors see who contributes",
        ));
    }
    of_set(&state.db, &set.id).await
}

/// Add `username` as a contributor, or change their permissions (owner).
pub async fn put(
    state: &AppState,
    viewer: &CurrentUser,
    set_id: &str,
    username: &str,
    req: PutMember,
) -> AppResult<Member> {
    let set = authz::readable_set(state, viewer.id(), set_id).await?;
    // Not `can_manage`: the owner of a read-only git pack may still share
    // it, and its contributors can then only read it.
    if !set.is_owner() {
        return Err(AppError::forbidden("only the owner can choose contributors"));
    }
    let user = db::user_by_username(&state.db, username.trim())
        .await?
        .filter(|u| !u.disabled)
        .ok_or_else(|| AppError::bad_request(format!("there is no user {:?}", username.trim())))?;
    if user.id == set.owner_id {
        return Err(AppError::bad_request("the owner can already do everything"));
    }
    let p = req.permissions;
    let t = now();
    let mut tx = sets::begin(&state.db).await?;
    let existed: Option<i64> = sqlx::query_scalar("SELECT 1 FROM set_members WHERE set_id = ? AND user_id = ?")
        .bind(&set.id)
        .bind(&user.id)
        .fetch_optional(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO set_members (set_id, user_id, can_add, can_edit_own, can_edit_all, can_remove_all, \
         can_edit_pack, can_review, added_by, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(set_id, user_id) DO UPDATE SET can_add = excluded.can_add, can_edit_own = excluded.can_edit_own, \
         can_edit_all = excluded.can_edit_all, can_remove_all = excluded.can_remove_all, \
         can_edit_pack = excluded.can_edit_pack, can_review = excluded.can_review, updated_at = excluded.updated_at",
    )
    .bind(&set.id)
    .bind(&user.id)
    .bind(p.add)
    .bind(p.edit_own)
    .bind(p.edit_all)
    .bind(p.remove_all)
    .bind(p.edit_pack)
    .bind(p.review)
    .bind(viewer.id())
    .bind(t)
    .bind(t)
    .execute(&mut *tx)
    .await?;
    // A new contributor's cache has to fetch the items it could not see.
    if existed.is_none() && set.visibility() != Visibility::Public {
        sets::touch_items(&mut tx, &set.id).await?;
    }
    tx.commit().await?;
    db::audit(
        &state.db,
        Some(viewer.id()),
        if existed.is_some() {
            "set.member.update"
        } else {
            "set.member.add"
        },
        Some(&set.id),
        viewer.ip_string().as_deref(),
        Some(serde_json::json!({ "user_id": user.id, "permissions": p })),
    )
    .await;
    let row: MemberRow = sqlx::query_as(&format!("{MEMBER_SELECT} WHERE m.set_id = ? AND m.user_id = ?"))
        .bind(&set.id)
        .bind(&user.id)
        .fetch_one(&state.db)
        .await?;
    Ok(row.to_api())
}

/// Remove `username` from the contributors: the owner may remove anyone, a
/// contributor themself (leaving). Idempotent for the owner.
pub async fn remove(state: &AppState, viewer: &CurrentUser, set_id: &str, username: &str) -> AppResult<()> {
    let set = authz::readable_set(state, viewer.id(), set_id).await?;
    let leaving = username.trim().eq_ignore_ascii_case(&viewer.user.username);
    if !set.is_owner() && !leaving {
        return Err(AppError::forbidden("only the owner can remove contributors"));
    }
    let Some(user) = db::user_by_username(&state.db, username.trim()).await? else {
        return Ok(());
    };
    let mut tx = sets::begin(&state.db).await?;
    let n = sqlx::query("DELETE FROM set_members WHERE set_id = ? AND user_id = ?")
        .bind(&set.id)
        .bind(&user.id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if n > 0 {
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM items WHERE set_id = ?")
            .bind(&set.id)
            .fetch_all(&mut *tx)
            .await?;
        sets::user_tombstones(&mut tx, std::slice::from_ref(&user.id), &ids).await?;
    }
    tx.commit().await?;
    if n > 0 {
        db::audit(
            &state.db,
            Some(viewer.id()),
            if leaving {
                "set.member.leave"
            } else {
                "set.member.remove"
            },
            Some(&set.id),
            viewer.ip_string().as_deref(),
            Some(serde_json::json!({ "user_id": user.id })),
        )
        .await;
    }
    Ok(())
}
