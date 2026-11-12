//! Sets: list, create, read, update, delete, reorder. Shared by the JSON API
//! and the web UI.

use serde::Deserialize;
use sqlx::{Sqlite, SqlitePool, Transaction};
use stickpile_api::routes as r;
use stickpile_api::{
    CoverImage, CreateSet, GitBinding, GitMode, MediaFormat, ReorderItems, Set, SetDetail, SetSource, SetSummary,
    UpdateSet, Variant, Visibility,
};

use crate::auth::CurrentUser;
use crate::authz;
use crate::db::{self, enum_str, parse_enum, SetRow, SET_COLUMNS};
use crate::error::{AppError, AppResult};
use crate::ids::{new_id, now};
use crate::index;
use crate::service::{cover, hydrate};
use crate::state::AppState;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SetScope {
    /// The caller's own sets (default).
    #[default]
    Mine,
    /// Public sets of every user (including the caller's).
    Public,
    /// Own sets plus other users' public sets.
    All,
}

/// `[a-z0-9-]`, 1..=64 chars, no leading/trailing dash.
pub fn validate_slug(slug: &str) -> AppResult<()> {
    let ok = (1..=64).contains(&slug.len())
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "slug must be 1..=64 characters of a-z 0-9 - (no leading/trailing dash)",
        ))
    }
}

pub fn validate_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 200 || name.chars().any(char::is_control) {
        return Err(AppError::bad_request("name must be 1..=200 printable characters"));
    }
    Ok(name.to_string())
}

pub fn validate_description(d: &str) -> AppResult<String> {
    if d.chars().count() > 5000 {
        return Err(AppError::bad_request("description must be at most 5000 characters"));
    }
    Ok(d.to_string())
}

fn validate_aspect(a: Option<(u32, u32)>) -> AppResult<()> {
    if let Some((w, h)) = a {
        if w == 0 || h == 0 || w > 10_000 || h > 10_000 {
            return Err(AppError::bad_request("aspect_ratio components must be 1..=10000"));
        }
    }
    Ok(())
}

fn validate_px(px: Option<u32>) -> AppResult<()> {
    if let Some(px) = px {
        if !(16..=16_384).contains(&px) {
            return Err(AppError::bad_request("recommended_px must be 16..=16384"));
        }
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct SetListRow {
    #[sqlx(flatten)]
    set: SetRow,
    owner_username: String,
    item_count: i64,
    /// The chosen cover while it is still in the set.
    chosen_cover_id: Option<String>,
    /// What lists show without a cover image: the chosen cover, else the
    /// first item.
    shown_cover_id: Option<String>,
    /// The cover image's media (`s.cover_media_id`), if any.
    cover_sha256: Option<String>,
    cover_format: Option<String>,
    cover_width: Option<i64>,
    cover_height: Option<i64>,
}

impl SetListRow {
    fn cover(&self) -> Cover {
        let image = match (&self.set.cover_media_id, &self.cover_sha256) {
            (Some(_), Some(sha)) => Some(cover::image_info(
                &self.set.id,
                sha,
                self.cover_format
                    .as_deref()
                    .and_then(parse_enum)
                    .unwrap_or(MediaFormat::Png),
                self.cover_width.unwrap_or(0).max(0) as u32,
                self.cover_height.unwrap_or(0).max(0) as u32,
            )),
            _ => None,
        };
        Cover {
            item: self.chosen_cover_id.clone(),
            shown_item: self.shown_cover_id.clone(),
            image,
        }
    }
}

/// What a set shows as its cover (see [`crate::service::cover`]).
#[derive(Debug, Clone, Default)]
pub struct Cover {
    /// The chosen cover item while it is still in the set.
    pub item: Option<String>,
    /// The item shown without a cover image: the chosen one, else the first.
    pub shown_item: Option<String>,
    /// The set's own cover image; it wins over the items.
    pub image: Option<CoverImage>,
}

impl Cover {
    /// URL path of the thumbnail shown for the set.
    pub fn url(&self) -> Option<String> {
        match (&self.image, &self.shown_item) {
            (Some(image), _) => Some(image.thumb_url.clone()),
            (None, Some(item)) => Some(r::path(r::ITEM_FILE, &[item, Variant::Thumb.as_str()])),
            (None, None) => None,
        }
    }
}

/// SQL for the chosen cover of set `s` while that item is still in it.
const CHOSEN_COVER_SQL: &str = "(SELECT id FROM items WHERE id = s.cover_item_id AND set_id = s.id)";

fn list_select() -> String {
    let cols = SET_COLUMNS
        .split(", ")
        .map(|c| format!("s.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT {cols}, u.username AS owner_username, \
         (SELECT COUNT(*) FROM items WHERE set_id = s.id) AS item_count, \
         {CHOSEN_COVER_SQL} AS chosen_cover_id, \
         COALESCE({CHOSEN_COVER_SQL}, (SELECT id FROM items WHERE set_id = s.id ORDER BY position, id LIMIT 1)) \
         AS shown_cover_id, \
         cm.sha256 AS cover_sha256, cm.format AS cover_format, cm.width AS cover_width, cm.height AS cover_height \
         FROM sets s JOIN users u ON u.id = s.owner_id LEFT JOIN media cm ON cm.id = s.cover_media_id"
    )
}

/// `cover`: the set's cover, its items already checked to be in the set.
pub fn to_api(row: &SetRow, owner_username: &str, item_count: u64, cover: Cover, viewer: &str) -> Set {
    let git = (row.source == "git").then(|| GitBinding {
        url: row.git_url.clone().unwrap_or_default(),
        branch: row.git_branch.clone(),
        mode: row
            .git_mode
            .as_deref()
            .and_then(parse_enum)
            .unwrap_or(GitMode::ReadOnly),
        last_commit: row.git_last_commit.clone(),
        last_pull_at: row.git_last_pull_at,
        last_error: row.git_last_error.clone(),
        web_url: row
            .git_url
            .as_deref()
            .and_then(|u| stickpile_gitpack::web_url(u, row.git_branch.as_deref(), row.git_subdir.as_deref())),
    });
    Set {
        id: row.id.clone(),
        owner_id: row.owner_id.clone(),
        owner_username: owner_username.to_string(),
        slug: row.slug.clone(),
        name: row.name.clone(),
        description: row.description.clone(),
        visibility: row.visibility(),
        aspect_ratio: row.aspect(),
        recommended_px: row.recommended_px.map(|v| v as u32),
        version: row.version.max(0) as u64,
        source: parse_enum(&row.source).unwrap_or(SetSource::Local),
        git,
        telegram_pack: row.telegram_pack.clone(),
        cover_url: cover.url(),
        cover_item_id: cover.item,
        cover_image: cover.image,
        item_count,
        editable: authz::can_edit_set(viewer, row),
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn to_summary(r: &SetListRow) -> SetSummary {
    SetSummary {
        id: r.set.id.clone(),
        slug: r.set.slug.clone(),
        name: r.set.name.clone(),
        description: r.set.description.clone(),
        owner_username: r.owner_username.clone(),
        visibility: r.set.visibility(),
        source: parse_enum(&r.set.source).unwrap_or(SetSource::Local),
        item_count: r.item_count.max(0) as u64,
        cover_item_id: r.shown_cover_id.clone(),
        cover_url: r.cover().url(),
        updated_at: r.set.updated_at,
    }
}

/// API view of a set row for `viewer`.
pub async fn load_api(db: &SqlitePool, row: &SetRow, viewer: &str) -> AppResult<Set> {
    let r = sqlx::query_as::<_, SetListRow>(&format!("{} WHERE s.id = ?", list_select()))
        .bind(&row.id)
        .fetch_one(db)
        .await?;
    Ok(to_api(
        &r.set,
        &r.owner_username,
        r.item_count.max(0) as u64,
        r.cover(),
        viewer,
    ))
}

/// The chosen cover of a set while that item is still in it.
pub async fn chosen_cover(db: &SqlitePool, set_id: &str) -> AppResult<Option<String>> {
    Ok(
        sqlx::query_scalar(&format!("SELECT {CHOSEN_COVER_SQL} FROM sets s WHERE s.id = ?"))
            .bind(set_id)
            .fetch_optional(db)
            .await?
            .flatten(),
    )
}

/// Summary of one set (item detail pages).
pub async fn summary(db: &SqlitePool, set_id: &str) -> AppResult<SetSummary> {
    let r = sqlx::query_as::<_, SetListRow>(&format!("{} WHERE s.id = ?", list_select()))
        .bind(set_id)
        .fetch_one(db)
        .await?;
    Ok(to_summary(&r))
}

/// Browse: own sets and/or public sets, most recently updated first.
pub async fn list(state: &AppState, viewer: &CurrentUser, scope: SetScope) -> AppResult<Vec<SetSummary>> {
    let (cond, bind_viewer) = match scope {
        SetScope::Mine => ("s.owner_id = ?", true),
        SetScope::Public => ("s.visibility = 'public'", false),
        SetScope::All => (authz::SEARCHABLE_SQL, true),
    };
    let sql = format!("{} WHERE {cond} ORDER BY s.updated_at DESC, s.id", list_select());
    let mut q = sqlx::query_as::<_, SetListRow>(&sql);
    if bind_viewer {
        q = q.bind(viewer.id());
    }
    let rows = q.fetch_all(&state.db).await?;
    Ok(rows
        .iter()
        .filter(|r| authz::can_list_set(viewer.id(), &r.set))
        .map(to_summary)
        .collect())
}

pub struct NewSet {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub visibility: Visibility,
    pub aspect_ratio: Option<(u32, u32)>,
    pub recommended_px: Option<u32>,
    pub version: u64,
    pub extra_toml: Option<String>,
}

/// Insert a set owned by `owner_id` (slug clash -> 409).
pub async fn insert(db: &SqlitePool, owner_id: &str, new: NewSet) -> AppResult<SetRow> {
    validate_slug(&new.slug)?;
    let name = validate_name(&new.name)?;
    let description = validate_description(&new.description)?;
    validate_aspect(new.aspect_ratio)?;
    validate_px(new.recommended_px)?;
    let id = new_id();
    let t = now();
    let res = sqlx::query(
        "INSERT INTO sets (id, owner_id, slug, name, description, visibility, aspect_w, aspect_h, recommended_px, \
         version, source, extra_toml, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'local', ?, ?, ?)",
    )
    .bind(&id)
    .bind(owner_id)
    .bind(&new.slug)
    .bind(&name)
    .bind(&description)
    .bind(enum_str(&new.visibility))
    .bind(new.aspect_ratio.map(|a| a.0 as i64))
    .bind(new.aspect_ratio.map(|a| a.1 as i64))
    .bind(new.recommended_px.map(|v| v as i64))
    .bind(new.version.clamp(1, i64::MAX as u64) as i64)
    .bind(&new.extra_toml)
    .bind(t)
    .bind(t)
    .execute(db)
    .await;
    match res {
        Ok(_) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(AppError::conflict(format!(
                "you already have a set with slug {:?}",
                new.slug
            )));
        }
        Err(e) => return Err(e.into()),
    }
    db::set_by_id(db, &id)
        .await?
        .ok_or_else(|| AppError::internal("set vanished after insert"))
}

pub async fn create(state: &AppState, viewer: &CurrentUser, req: CreateSet) -> AppResult<Set> {
    let row = insert(
        &state.db,
        viewer.id(),
        NewSet {
            slug: req.slug,
            name: req.name,
            description: req.description,
            visibility: req.visibility,
            aspect_ratio: req.aspect_ratio,
            recommended_px: req.recommended_px,
            version: 1,
            extra_toml: None,
        },
    )
    .await?;
    Ok(to_api(&row, &viewer.user.username, 0, Cover::default(), viewer.id()))
}

pub async fn detail(state: &AppState, viewer: &CurrentUser, set_id: &str) -> AppResult<SetDetail> {
    let row = authz::readable_set(state, viewer.id(), set_id).await?;
    let set = load_api(&state.db, &row, viewer.id()).await?;
    let rows = hydrate::fetch_by_set(&state.db, set_id).await?;
    let items = hydrate::items(&state.db, viewer.id(), &rows).await?;
    Ok(SetDetail { set, items })
}

/// Bump `version` and `updated_at` (any content or metadata change). For
/// push-mode git packs this also (re)schedules the debounced `GitPush`.
pub async fn bump_version(conn: &mut sqlx::SqliteConnection, set_id: &str) -> AppResult<()> {
    sqlx::query("UPDATE sets SET version = version + 1, updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(set_id)
        .execute(&mut *conn)
        .await?;
    crate::service::gitpack::schedule_push(conn, set_id).await
}

/// Bump `updated_at` of every item of a set (their summaries changed: set
/// name/slug, or visibility for other users' caches).
pub async fn touch_items(conn: &mut sqlx::SqliteConnection, set_id: &str) -> AppResult<()> {
    sqlx::query("UPDATE items SET updated_at = ? WHERE set_id = ?")
        .bind(now())
        .bind(set_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Tombstone audiences (see the `item_tombstones` table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    Owner,
    Others,
    All,
}

impl Audience {
    fn as_str(self) -> &'static str {
        match self {
            Audience::Owner => "owner",
            Audience::Others => "others",
            Audience::All => "all",
        }
    }
    /// Audience of a removed item that was in a set with `visibility`.
    pub fn for_removal(visibility: Visibility) -> Audience {
        if visibility == Visibility::Public {
            Audience::All
        } else {
            Audience::Owner
        }
    }
}

pub async fn tombstone(
    conn: &mut sqlx::SqliteConnection,
    item_ids: &[String],
    owner_id: &str,
    audience: Audience,
) -> AppResult<()> {
    let t = now();
    for id in item_ids {
        sqlx::query("INSERT INTO item_tombstones (item_id, owner_id, audience, removed_at) VALUES (?, ?, ?, ?)")
            .bind(id)
            .bind(owner_id)
            .bind(audience.as_str())
            .bind(t)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Renumber positions 0..n in the current order.
pub async fn renumber(conn: &mut sqlx::SqliteConnection, set_id: &str) -> AppResult<()> {
    let ids: Vec<(String,)> = sqlx::query_as("SELECT id FROM items WHERE set_id = ? ORDER BY position, id")
        .bind(set_id)
        .fetch_all(&mut *conn)
        .await?;
    for (pos, (id,)) in ids.iter().enumerate() {
        sqlx::query("UPDATE items SET position = ? WHERE id = ? AND position != ?")
            .bind(pos as i64)
            .bind(id)
            .bind(pos as i64)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

pub async fn begin(db: &SqlitePool) -> AppResult<Transaction<'static, Sqlite>> {
    Ok(db.begin_with("BEGIN IMMEDIATE").await?)
}

/// Apply a visibility change's cache-sync consequences inside `tx`.
pub async fn on_visibility_change(conn: &mut sqlx::SqliteConnection, set: &SetRow, new: Visibility) -> AppResult<()> {
    let old = set.visibility();
    if old == new {
        return Ok(());
    }
    let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM items WHERE set_id = ?")
        .bind(&set.id)
        .fetch_all(&mut *conn)
        .await?;
    if old == Visibility::Public {
        tombstone(conn, &ids, &set.owner_id, Audience::Others).await?;
    } else if new == Visibility::Public {
        for id in &ids {
            sqlx::query("DELETE FROM item_tombstones WHERE item_id = ? AND audience = 'others'")
                .bind(id)
                .execute(&mut *conn)
                .await?;
        }
    }
    Ok(())
}

pub async fn update(state: &AppState, viewer: &CurrentUser, set_id: &str, req: UpdateSet) -> AppResult<Set> {
    let set = authz::editable_set(state, viewer.id(), set_id).await?;
    if let Some(slug) = &req.slug {
        validate_slug(slug)?;
    }
    let name = req.name.as_deref().map(validate_name).transpose()?;
    let description = req.description.as_deref().map(validate_description).transpose()?;
    if let Some(a) = req.aspect_ratio {
        validate_aspect(a)?;
    }
    if let Some(px) = req.recommended_px {
        validate_px(px)?;
    }
    let old_cover = chosen_cover(&state.db, set_id).await?;
    let new_cover = match req.cover_item_id.clone() {
        None => old_cover.clone(),
        Some(None) => None,
        Some(Some(id)) => {
            let in_set: Option<String> = sqlx::query_scalar("SELECT id FROM items WHERE id = ? AND set_id = ?")
                .bind(&id)
                .bind(set_id)
                .fetch_optional(&state.db)
                .await?;
            Some(in_set.ok_or_else(|| AppError::bad_request("the cover must be an item of this set"))?)
        }
    };
    let mut tx = begin(&state.db).await?;
    let new_slug = req.slug.clone().unwrap_or_else(|| set.slug.clone());
    let new_name = name.unwrap_or_else(|| set.name.clone());
    let new_desc = description.unwrap_or_else(|| set.description.clone());
    let new_vis = req.visibility.unwrap_or(set.visibility());
    let new_aspect = req.aspect_ratio.unwrap_or(set.aspect());
    let new_px = req.recommended_px.unwrap_or(set.recommended_px.map(|v| v as u32));
    // Choosing an item (or the first item) replaces a cover image.
    let new_cover_media = if req.cover_item_id.is_some() {
        None
    } else {
        set.cover_media_id.clone()
    };
    let changed = new_slug != set.slug
        || new_name != set.name
        || new_desc != set.description
        || new_vis != set.visibility()
        || new_aspect != set.aspect()
        || new_px != set.recommended_px.map(|v| v as u32)
        || new_cover != old_cover
        || new_cover_media != set.cover_media_id;
    if changed {
        on_visibility_change(&mut tx, &set, new_vis).await?;
        let res = sqlx::query(
            "UPDATE sets SET slug = ?, name = ?, description = ?, visibility = ?, aspect_w = ?, aspect_h = ?, \
             recommended_px = ?, cover_item_id = ?, cover_media_id = ? WHERE id = ?",
        )
        .bind(&new_slug)
        .bind(&new_name)
        .bind(&new_desc)
        .bind(enum_str(&new_vis))
        .bind(new_aspect.map(|a| a.0 as i64))
        .bind(new_aspect.map(|a| a.1 as i64))
        .bind(new_px.map(|v| v as i64))
        .bind(&new_cover)
        .bind(&new_cover_media)
        .bind(set_id)
        .execute(&mut *tx)
        .await;
        match res {
            Ok(_) => {}
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(AppError::conflict(format!(
                    "you already have a set with slug {new_slug:?}"
                )));
            }
            Err(e) => return Err(e.into()),
        }
        bump_version(&mut tx, set_id).await?;
        if new_slug != set.slug || new_name != set.name || new_vis != set.visibility() {
            touch_items(&mut tx, set_id).await?;
        }
    }
    tx.commit().await?;
    if changed {
        index::refresh_set(state, set_id).await?;
    }
    let row = db::set_by_id(&state.db, set_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    load_api(&state.db, &row, viewer.id()).await
}

/// Delete a set and its items (the owner only). Media files, the cover
/// image's included, are removed later by the orphan-media GC.
pub async fn delete(state: &AppState, viewer: &CurrentUser, set_id: &str) -> AppResult<()> {
    let set = authz::owned_set(state, viewer.id(), set_id).await?;
    delete_row(state, &set).await?;
    db::audit(
        &state.db,
        Some(viewer.id()),
        "set.delete",
        Some(&set.id),
        viewer.ip_string().as_deref(),
        Some(serde_json::json!({"slug": set.slug, "name": set.name, "owner_id": set.owner_id})),
    )
    .await;
    Ok(())
}

/// Delete without authorization checks or audit (also used for user deletion).
pub async fn delete_row(state: &AppState, set: &SetRow) -> AppResult<()> {
    let mut tx = begin(&state.db).await?;
    let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM items WHERE set_id = ?")
        .bind(&set.id)
        .fetch_all(&mut *tx)
        .await?;
    tombstone(&mut tx, &ids, &set.owner_id, Audience::for_removal(set.visibility())).await?;
    sqlx::query("DELETE FROM sets WHERE id = ?")
        .bind(&set.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    index::remove_set(state, &set.id);
    Ok(())
}

/// Replace the item order with `req.item_ids` (must be a permutation).
pub async fn reorder(state: &AppState, viewer: &CurrentUser, set_id: &str, req: ReorderItems) -> AppResult<SetDetail> {
    authz::editable_set(state, viewer.id(), set_id).await?;
    let mut tx = begin(&state.db).await?;
    let mut current: Vec<String> = sqlx::query_scalar("SELECT id FROM items WHERE set_id = ? ORDER BY position, id")
        .bind(set_id)
        .fetch_all(&mut *tx)
        .await?;
    let mut wanted = req.item_ids.clone();
    let order_changed = current != wanted;
    current.sort();
    wanted.sort();
    wanted.dedup();
    if current != wanted || wanted.len() != req.item_ids.len() {
        return Err(AppError::bad_request(
            "item_ids must list every item of the set exactly once",
        ));
    }
    if order_changed {
        for (pos, id) in req.item_ids.iter().enumerate() {
            sqlx::query("UPDATE items SET position = ? WHERE id = ?")
                .bind(pos as i64)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        bump_version(&mut tx, set_id).await?;
    }
    tx.commit().await?;
    detail(state, viewer, set_id).await
}
