//! Authorization: the visibility table of docs/plan.md and the
//! contributor permissions of docs/collaboration.md, in one place.
//!
//! | Caller                         | own set | contributed | other's private | other's unlisted | other's public |
//! |--------------------------------|---------|-------------|-----------------|------------------|----------------|
//! | read set / item / file by id   | yes     | yes         | no (404)        | yes              | yes            |
//! | listed in browse               | yes     | yes         | no              | no               | yes            |
//! | search, client cache           | yes     | yes         | no              | no               | yes            |
//! | change                         | yes unless read-only git pack | per [`Permissions`] | no | no (403) | no (403) |
//! | like, report usage             | yes     | yes         | no (404)        | yes              | yes            |
//!
//! A contributor's [`Permissions`] say which changes they may make: add
//! stickers, edit or remove the ones they added, edit or remove everyone's,
//! edit the set (name, description, cover, order), review proposals. The
//! set's settings (visibility, slug, git, Telegram), its contributors and
//! deleting it stay with the owner. Nobody changes a read-only git pack.
//!
//! Admins get no implicit content access, only the admin API and the
//! server-wide problems report (`service::problems`: set names and
//! problems, never items of others' private sets). Content the
//! caller cannot read is reported as 404, never 403, so ids cannot be probed.

use std::collections::HashSet;

use stickpile_api::{Permissions, SetRole, Visibility};

use crate::db::{self, SetRow};
use crate::error::{AppError, AppResult};
use crate::ids::is_id;
use crate::state::AppState;

/// SQL condition (sets aliased `s`, one bind: viewer id) for "visible in
/// search / browse / client cache": own sets, sets the viewer contributes
/// to, and public sets.
pub const SEARCHABLE_SQL: &str = "(s.visibility = 'public' OR EXISTS (SELECT 1 FROM (SELECT ? AS v) viewer \
     WHERE viewer.v = s.owner_id OR EXISTS (SELECT 1 FROM set_members sm WHERE sm.set_id = s.id AND sm.user_id = viewer.v)))";

/// The viewer's standing in one set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Owner,
    Contributor(Permissions),
    Other,
}

impl Standing {
    pub fn role(self) -> SetRole {
        match self {
            Standing::Owner => SetRole::Owner,
            Standing::Contributor(_) => SetRole::Contributor,
            Standing::Other => SetRole::Viewer,
        }
    }
}

/// A set together with the viewer's standing in it.
#[derive(Debug, Clone)]
pub struct SetAccess {
    pub viewer: String,
    pub set: SetRow,
    pub standing: Standing,
}

impl SetAccess {
    pub fn is_owner(&self) -> bool {
        self.standing == Standing::Owner
    }

    pub fn role(&self) -> SetRole {
        self.standing.role()
    }

    pub fn can_read(&self) -> bool {
        self.standing != Standing::Other || self.set.visibility() != Visibility::Private
    }

    /// What the viewer may change: everything for the owner, the granted
    /// permissions for a contributor; nothing in a read-only git pack.
    pub fn permissions(&self) -> Permissions {
        if self.set.read_only() {
            return Permissions::NONE;
        }
        match self.standing {
            Standing::Owner => Permissions::ALL,
            Standing::Contributor(p) => p,
            Standing::Other => Permissions::NONE,
        }
    }

    /// The set's settings, contributors and Telegram export (owner, not a
    /// read-only git pack).
    pub fn can_manage(&self) -> bool {
        self.is_owner() && !self.set.read_only()
    }

    pub fn can_add(&self) -> bool {
        self.permissions().add
    }

    pub fn can_edit_pack(&self) -> bool {
        self.permissions().edit_pack
    }

    pub fn can_review(&self) -> bool {
        self.permissions().review
    }

    /// Edit an item added by `added_by`.
    pub fn can_edit_item(&self, added_by: Option<&str>) -> bool {
        let p = self.permissions();
        p.edit_all || (p.edit_own && added_by == Some(self.viewer.as_str()))
    }

    /// Remove an item added by `added_by` from the set.
    pub fn can_remove_item(&self, added_by: Option<&str>) -> bool {
        let p = self.permissions();
        p.remove_all || (p.edit_own && added_by == Some(self.viewer.as_str()))
    }

    /// "only the owner can ..." or "this set is a read-only git pack ...".
    pub(crate) fn refuse(&self, what: &str) -> AppError {
        if self.set.read_only() && self.standing != Standing::Other {
            AppError::forbidden("this set is a read-only git pack; edit it upstream")
        } else {
            AppError::forbidden(format!("you may not {what} in this set"))
        }
    }
}

/// The set's fields (`access.slug`, `access.visibility()`), so code that
/// only needs the row reads like it did before the access check.
impl std::ops::Deref for SetAccess {
    type Target = SetRow;
    fn deref(&self) -> &SetRow {
        &self.set
    }
}

/// The contributor permissions of `user_id` in `set_id`, if they are one.
pub async fn member_permissions(db: &sqlx::SqlitePool, set_id: &str, user_id: &str) -> AppResult<Option<Permissions>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        can_add: bool,
        can_edit_own: bool,
        can_edit_all: bool,
        can_remove_all: bool,
        can_edit_pack: bool,
        can_review: bool,
    }
    let row: Option<Row> = sqlx::query_as(
        "SELECT can_add, can_edit_own, can_edit_all, can_remove_all, can_edit_pack, can_review \
         FROM set_members WHERE set_id = ? AND user_id = ?",
    )
    .bind(set_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| Permissions {
        add: r.can_add,
        edit_own: r.can_edit_own,
        edit_all: r.can_edit_all,
        remove_all: r.can_remove_all,
        edit_pack: r.can_edit_pack,
        review: r.can_review,
    }))
}

/// Ids of the sets `user_id` contributes to.
pub async fn member_set_ids(db: &sqlx::SqlitePool, user_id: &str) -> AppResult<HashSet<String>> {
    Ok(sqlx::query_scalar("SELECT set_id FROM set_members WHERE user_id = ?")
        .bind(user_id)
        .fetch_all(db)
        .await?
        .into_iter()
        .collect())
}

/// The viewer's standing in `set` (one query for non-owners).
pub async fn access(db: &sqlx::SqlitePool, viewer: &str, set: SetRow) -> AppResult<SetAccess> {
    let standing = if set.owner_id == viewer {
        Standing::Owner
    } else {
        match member_permissions(db, &set.id, viewer).await? {
            Some(p) => Standing::Contributor(p),
            None => Standing::Other,
        }
    };
    Ok(SetAccess {
        viewer: viewer.to_string(),
        set,
        standing,
    })
}

/// Read access without the database: `member` is whether the viewer
/// contributes to the set.
pub fn can_read_set(viewer: &str, set: &SetRow, member: bool) -> bool {
    set.owner_id == viewer || member || set.visibility() != Visibility::Private
}

/// Own, contributed and public items are searchable and synced to client caches.
pub fn searchable(viewer: &str, owner_id: &str, visibility: Visibility, member: bool) -> bool {
    owner_id == viewer || member || visibility == Visibility::Public
}

/// Load a set the viewer may read; otherwise 404.
pub async fn readable_set(state: &AppState, viewer: &str, set_id: &str) -> AppResult<SetAccess> {
    if !is_id(set_id) {
        return Err(AppError::not_found());
    }
    let Some(set) = db::set_by_id(&state.db, set_id).await? else {
        return Err(AppError::not_found());
    };
    let a = access(&state.db, viewer, set).await?;
    if a.can_read() {
        Ok(a)
    } else {
        Err(AppError::not_found())
    }
}

/// What a change to a set needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Settings, contributors, Telegram export (the owner, not read-only).
    Manage,
    /// Delete or unsubscribe (the owner, even of a read-only git pack).
    Own,
    Add,
    EditPack,
    Review,
}

/// Load a set the viewer may change as `need` says: unreadable -> 404,
/// readable but not allowed -> 403.
pub async fn set_for(state: &AppState, viewer: &str, set_id: &str, need: Need) -> AppResult<SetAccess> {
    let a = readable_set(state, viewer, set_id).await?;
    let (ok, what) = match need {
        Need::Manage => (a.can_manage(), "change the settings"),
        Need::Own => (a.is_owner(), "do this"),
        Need::Add => (a.can_add(), "add stickers"),
        Need::EditPack => (a.can_edit_pack(), "edit the pack"),
        Need::Review => (a.can_review(), "review proposals"),
    };
    if ok {
        return Ok(a);
    }
    if matches!(need, Need::Manage | Need::Own) && !a.is_owner() {
        return Err(AppError::forbidden("only the owner can do this"));
    }
    Err(a.refuse(what))
}

/// Load a set only its owner may change (settings), 404 / 403 as for [`set_for`].
pub async fn editable_set(state: &AppState, viewer: &str, set_id: &str) -> AppResult<SetAccess> {
    set_for(state, viewer, set_id, Need::Manage).await
}

/// Load a set the viewer owns (delete): unreadable -> 404, others' -> 403.
pub async fn owned_set(state: &AppState, viewer: &str, set_id: &str) -> AppResult<SetAccess> {
    set_for(state, viewer, set_id, Need::Own).await
}

/// An item together with its set and the viewer's standing in it.
#[derive(Debug, Clone)]
pub struct ItemRef {
    pub item_id: String,
    pub media_id: String,
    pub added_by: Option<String>,
    pub access: SetAccess,
}

impl ItemRef {
    pub fn set(&self) -> &SetRow {
        &self.access.set
    }
    pub fn can_edit(&self) -> bool {
        self.access.can_edit_item(self.added_by.as_deref())
    }
    pub fn can_remove(&self) -> bool {
        self.access.can_remove_item(self.added_by.as_deref())
    }
}

/// Load an item the viewer may read; otherwise 404.
pub async fn readable_item(state: &AppState, viewer: &str, item_id: &str) -> AppResult<ItemRef> {
    if !is_id(item_id) {
        return Err(AppError::not_found());
    }
    let row: Option<(String, String, Option<String>)> =
        sqlx::query_as("SELECT set_id, media_id, added_by FROM items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((set_id, media_id, added_by)) = row else {
        return Err(AppError::not_found());
    };
    let set = db::set_by_id(&state.db, &set_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let access = access(&state.db, viewer, set).await?;
    if !access.can_read() {
        return Err(AppError::not_found());
    }
    Ok(ItemRef {
        item_id: item_id.to_string(),
        media_id,
        added_by,
        access,
    })
}

/// Load an item the viewer may edit (404 / 403 as for sets).
pub async fn editable_item(state: &AppState, viewer: &str, item_id: &str) -> AppResult<ItemRef> {
    let item = readable_item(state, viewer, item_id).await?;
    if item.can_edit() {
        Ok(item)
    } else {
        Err(item.access.refuse("edit this sticker"))
    }
}

/// Load an item the viewer may remove from its set (404 / 403 as for sets).
pub async fn removable_item(state: &AppState, viewer: &str, item_id: &str) -> AppResult<ItemRef> {
    let item = readable_item(state, viewer, item_id).await?;
    if item.can_remove() {
        Ok(item)
    } else {
        Err(item.access.refuse("remove this sticker"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(owner: &str, vis: &str, source: &str, git_mode: Option<&str>) -> SetRow {
        SetRow {
            id: "s".into(),
            owner_id: owner.into(),
            slug: "s".into(),
            name: "s".into(),
            description: String::new(),
            visibility: vis.into(),
            aspect_w: None,
            aspect_h: None,
            recommended_px: None,
            version: 1,
            source: source.into(),
            git_url: None,
            git_branch: None,
            git_subdir: None,
            git_mode: git_mode.map(str::to_string),
            git_last_commit: None,
            git_last_pull_at: None,
            git_last_error: None,
            telegram_pack: None,
            cover_item_id: None,
            cover_media_id: None,
            extra_toml: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn acc(viewer: &str, set: SetRow, standing: Standing) -> SetAccess {
        SetAccess {
            viewer: viewer.into(),
            set,
            standing,
        }
    }

    #[test]
    fn visibility_table() {
        for (vis, read) in [("private", false), ("unlisted", true), ("public", true)] {
            let s = set("owner", vis, "local", None);
            assert!(can_read_set("owner", &s, false));
            assert!(can_read_set("member", &s, true), "{vis}");
            assert_eq!(can_read_set("other", &s, false), read, "{vis}");
            assert!(searchable("member", "owner", s.visibility(), true));
            assert_eq!(searchable("other", "owner", s.visibility(), false), vis == "public");
            let owner = acc("owner", s.clone(), Standing::Owner);
            assert!(owner.can_read() && owner.can_manage() && owner.can_add());
            assert_eq!(owner.permissions(), Permissions::ALL);
            let other = acc("other", s.clone(), Standing::Other);
            assert_eq!(other.can_read(), read, "{vis}");
            assert_eq!(other.permissions(), Permissions::NONE);
            assert!(!other.can_manage() && !other.can_edit_item(Some("other")));
            let member = acc("member", s, Standing::Contributor(Permissions::CONTRIBUTOR_DEFAULT));
            assert!(member.can_read());
            assert!(!member.can_manage());
        }
    }

    #[test]
    fn contributor_permissions() {
        let s = set("owner", "private", "local", None);
        let own = Permissions {
            edit_own: true,
            ..Permissions::NONE
        };
        let m = acc("m", s.clone(), Standing::Contributor(own));
        assert!(m.can_edit_item(Some("m")) && m.can_remove_item(Some("m")));
        assert!(!m.can_edit_item(Some("owner")) && !m.can_remove_item(Some("owner")));
        assert!(!m.can_edit_item(None), "an item of a removed account is nobody's own");
        assert!(!m.can_add() && !m.can_edit_pack() && !m.can_review());

        let all = Permissions {
            edit_all: true,
            ..Permissions::NONE
        };
        let m = acc("m", s.clone(), Standing::Contributor(all));
        assert!(m.can_edit_item(Some("owner")) && m.can_edit_item(None));
        assert!(!m.can_remove_item(Some("owner")), "editing is not removing");
        let rm = Permissions {
            remove_all: true,
            ..Permissions::NONE
        };
        let m = acc("m", s.clone(), Standing::Contributor(rm));
        assert!(m.can_remove_item(Some("owner")) && !m.can_edit_item(Some("owner")));
        let m = acc("m", s, Standing::Contributor(Permissions::ALL));
        assert!(m.can_add() && m.can_edit_pack() && m.can_review());
        assert!(!m.can_manage() && !m.is_owner(), "settings stay the owner's");
    }

    #[test]
    fn read_only_git_pack() {
        let s = set("owner", "public", "git", Some("read_only"));
        let owner = acc("owner", s.clone(), Standing::Owner);
        assert!(!owner.can_manage() && !owner.can_add());
        assert!(owner.is_owner(), "the owner may still delete it");
        let m = acc("m", s, Standing::Contributor(Permissions::ALL));
        assert_eq!(m.permissions(), Permissions::NONE);
        let s = set("owner", "public", "git", Some("push"));
        assert!(acc("owner", s, Standing::Owner).can_manage());
    }
}
