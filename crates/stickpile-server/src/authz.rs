//! Authorization: the visibility table of docs/plan.md, in one place.
//!
//! | Caller                         | own set | other's private | other's unlisted | other's public |
//! |--------------------------------|---------|-----------------|------------------|----------------|
//! | read set / item / file by id   | yes     | no (404)        | yes              | yes            |
//! | listed in browse               | yes     | no              | no               | yes            |
//! | search, client cache           | yes     | no              | no               | yes            |
//! | edit                           | yes unless read-only git pack | no | no (403) | no (403)   |
//! | like, report usage             | yes     | no (404)        | yes              | yes            |
//!
//! Admins get no implicit content access, only the admin API and the
//! server-wide problems report (`service::problems`: set names and
//! problems, never items of others' private sets). Content the
//! caller cannot read is reported as 404, never 403, so ids cannot be probed.

use stickpile_api::Visibility;

use crate::db::{self, SetRow};
use crate::error::{AppError, AppResult};
use crate::ids::is_id;
use crate::state::AppState;

/// SQL condition (sets aliased `s`, one bind: viewer id) for "visible in
/// search / browse / client cache": own sets and public sets.
pub const SEARCHABLE_SQL: &str = "(s.owner_id = ? OR s.visibility = 'public')";

pub fn can_read_set(viewer: &str, set: &SetRow) -> bool {
    set.owner_id == viewer || set.visibility() != Visibility::Private
}

pub fn can_list_set(viewer: &str, set: &SetRow) -> bool {
    searchable(viewer, &set.owner_id, set.visibility())
}

/// Own items and public items are searchable and synced to client caches.
pub fn searchable(viewer: &str, owner_id: &str, visibility: Visibility) -> bool {
    owner_id == viewer || visibility == Visibility::Public
}

pub fn can_edit_set(viewer: &str, set: &SetRow) -> bool {
    set.owner_id == viewer && !set.read_only()
}

/// Deleting (or unsubscribing from) a set: the owner, even for read-only git packs.
pub fn can_delete_set(viewer: &str, set: &SetRow) -> bool {
    set.owner_id == viewer
}

pub fn can_like(viewer: &str, set: &SetRow) -> bool {
    can_read_set(viewer, set)
}

pub fn can_report_usage(viewer: &str, set: &SetRow) -> bool {
    can_read_set(viewer, set)
}

/// Load a set the viewer may read; otherwise 404.
pub async fn readable_set(state: &AppState, viewer: &str, set_id: &str) -> AppResult<SetRow> {
    if !is_id(set_id) {
        return Err(AppError::not_found());
    }
    match db::set_by_id(&state.db, set_id).await? {
        Some(s) if can_read_set(viewer, &s) => Ok(s),
        _ => Err(AppError::not_found()),
    }
}

/// Load a set the viewer may edit: unreadable -> 404, readable but not
/// editable (someone else's, or a read-only git pack) -> 403.
pub async fn editable_set(state: &AppState, viewer: &str, set_id: &str) -> AppResult<SetRow> {
    let set = readable_set(state, viewer, set_id).await?;
    if can_edit_set(viewer, &set) {
        Ok(set)
    } else if set.owner_id == viewer {
        Err(AppError::forbidden(
            "this set is a read-only git pack; edit it upstream",
        ))
    } else {
        Err(AppError::forbidden("only the owner can change this set"))
    }
}

/// Load a set the viewer owns (delete): unreadable -> 404, others' -> 403.
pub async fn owned_set(state: &AppState, viewer: &str, set_id: &str) -> AppResult<SetRow> {
    let set = readable_set(state, viewer, set_id).await?;
    if can_delete_set(viewer, &set) {
        Ok(set)
    } else {
        Err(AppError::forbidden("only the owner can do this"))
    }
}

/// An item together with its set.
#[derive(Debug, Clone)]
pub struct ItemRef {
    pub item_id: String,
    pub media_id: String,
    pub set: SetRow,
}

/// Load an item the viewer may read; otherwise 404.
pub async fn readable_item(state: &AppState, viewer: &str, item_id: &str) -> AppResult<ItemRef> {
    if !is_id(item_id) {
        return Err(AppError::not_found());
    }
    let row: Option<(String, String)> = sqlx::query_as("SELECT set_id, media_id FROM items WHERE id = ?")
        .bind(item_id)
        .fetch_optional(&state.db)
        .await?;
    let Some((set_id, media_id)) = row else {
        return Err(AppError::not_found());
    };
    let set = db::set_by_id(&state.db, &set_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    if !can_read_set(viewer, &set) {
        return Err(AppError::not_found());
    }
    Ok(ItemRef {
        item_id: item_id.to_string(),
        media_id,
        set,
    })
}

/// Load an item the viewer may edit (404 / 403 as for sets).
pub async fn editable_item(state: &AppState, viewer: &str, item_id: &str) -> AppResult<ItemRef> {
    let item = readable_item(state, viewer, item_id).await?;
    if can_edit_set(viewer, &item.set) {
        Ok(item)
    } else if item.set.owner_id == viewer {
        Err(AppError::forbidden(
            "this set is a read-only git pack; edit it upstream",
        ))
    } else {
        Err(AppError::forbidden("only the owner can change this item"))
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

    #[test]
    fn visibility_table() {
        for (vis, read, list) in [
            ("private", false, false),
            ("unlisted", true, false),
            ("public", true, true),
        ] {
            let s = set("owner", vis, "local", None);
            assert!(can_read_set("owner", &s));
            assert!(can_list_set("owner", &s));
            assert!(can_edit_set("owner", &s));
            assert_eq!(can_read_set("other", &s), read, "{vis}");
            assert_eq!(can_list_set("other", &s), list, "{vis}");
            assert_eq!(can_like("other", &s), read, "{vis}");
            assert_eq!(can_report_usage("other", &s), read, "{vis}");
            assert!(!can_edit_set("other", &s));
            assert!(!can_delete_set("other", &s));
        }
    }

    #[test]
    fn read_only_git_pack() {
        let s = set("owner", "public", "git", Some("read_only"));
        assert!(!can_edit_set("owner", &s));
        assert!(can_delete_set("owner", &s));
        let s = set("owner", "public", "git", Some("push"));
        assert!(can_edit_set("owner", &s));
    }
}
