//! Proposals (docs/collaboration.md): a user asks the owner of a set they
//! can read to add stickers, to change a sticker (name, keywords, emojis,
//! tags, vibe) or to change the set (name, description, cover). The owner
//! and contributors with the `review` permission accept or decline it; the
//! proposer may withdraw it while it is open.
//!
//! Proposed stickers and cover images are stored as media at once (the
//! reviewers see thumbnails) but belong to no set until the proposal is
//! accepted. The orphan-media GC keeps the media of open proposals and of
//! proposals decided in the last [`KEEP_DECIDED_SECS`] (an accepted one is
//! being applied, a declined one may still be looked at); after that it is
//! unused media like any other.
//! Accepting applies the change with the reviewer's `review` right alone:
//! a reviewer need not be allowed to make the change themself.

use std::collections::HashMap;

use stickpile_api::routes as r;
use stickpile_api::{
    FieldSource, MediaFormat, MediaKind, NewProposal, Proposal, ProposalBox, ProposalKind, ProposalStatus,
    ProposedChanges, ProposedItem, UpdateItem, UpdateSet, UploadMeta,
};

use crate::auth::CurrentUser;
use crate::authz::{self, SetAccess};
use crate::db::{self, enum_str, json_list, parse_enum, to_json_list, MediaRow, SetRow, VariantRow};
use crate::error::{AppError, AppResult};
use crate::ids::{is_id, new_id, now};
use crate::index;
use crate::service::upload::{self, clean_list, clean_name, clean_vibe, ItemInput, UploadFile, UploadParts};
use crate::service::{cover, hydrate, items, sets};
use crate::state::AppState;

/// Longest proposer message and reviewer note, in characters.
pub const MAX_NOTE_CHARS: usize = 1000;
/// Open proposals one user may have for one set.
pub const MAX_OPEN_PER_SET: i64 = 20;
/// Stickers in one proposal.
pub const MAX_ITEMS: usize = 50;
/// How long the media of a decided proposal is kept (see the module docs).
pub const KEEP_DECIDED_SECS: i64 = 24 * 3600;

/// SQL condition (media aliased `m`, one bind: the oldest `decided_at` that
/// still counts) for "a proposal needs this media".
pub const PROPOSAL_MEDIA_SQL: &str = "(EXISTS (SELECT 1 FROM proposal_items pi JOIN proposals p ON p.id = pi.proposal_id \
     WHERE pi.media_id = m.id AND (p.status = 'open' OR p.decided_at >= ?1)) \
     OR EXISTS (SELECT 1 FROM proposals p WHERE p.cover_media_id = m.id AND (p.status = 'open' OR p.decided_at >= ?1)))";

#[derive(Debug, Clone, sqlx::FromRow)]
struct ProposalRow {
    id: String,
    set_id: String,
    item_id: Option<String>,
    proposer_id: String,
    kind: String,
    status: String,
    message: String,
    changes: String,
    before: String,
    cover_media_id: Option<String>,
    reviewer_id: Option<String>,
    review_note: String,
    created_at: i64,
    decided_at: Option<i64>,
}

impl ProposalRow {
    fn kind(&self) -> ProposalKind {
        parse_enum(&self.kind).unwrap_or(ProposalKind::EditSet)
    }
    fn status(&self) -> ProposalStatus {
        parse_enum(&self.status).unwrap_or(ProposalStatus::Declined)
    }
    fn changes(&self) -> ProposedChanges {
        serde_json::from_str(&self.changes).unwrap_or_default()
    }
}

const PROPOSAL_COLUMNS: &str = "p.id, p.set_id, p.item_id, p.proposer_id, p.kind, p.status, p.message, p.changes, \
     p.before, p.cover_media_id, p.reviewer_id, p.review_note, p.created_at, p.decided_at";

#[derive(Debug, Clone, sqlx::FromRow)]
struct ItemRow {
    position: i64,
    media_id: Option<String>,
    kind: String,
    format: String,
    width: i64,
    height: i64,
    filename: String,
    name: String,
    keywords: String,
    emojis: String,
    vibe: Option<String>,
    tags: String,
    item_id: Option<String>,
}

fn clean_note(s: &str, what: &str) -> AppResult<String> {
    let s = s.trim();
    if s.chars().count() > MAX_NOTE_CHARS {
        return Err(AppError::bad_request(format!(
            "the {what} is longer than {MAX_NOTE_CHARS} characters"
        )));
    }
    Ok(s.to_string())
}

// ---------------------------------------------------------------------------
// Proposing
// ---------------------------------------------------------------------------

/// A set the viewer may propose changes to: readable and not their own.
async fn target_set(state: &AppState, viewer: &CurrentUser, set_id: &str) -> AppResult<SetAccess> {
    let set = authz::readable_set(state, viewer.id(), set_id).await?;
    if set.is_owner() {
        return Err(AppError::bad_request("this is your own set: change it directly"));
    }
    if set.read_only() {
        return Err(AppError::bad_request(
            "this set is a read-only git pack; propose the change upstream",
        ));
    }
    let open: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM proposals WHERE set_id = ? AND proposer_id = ? AND status = 'open'")
            .bind(&set.id)
            .bind(viewer.id())
            .fetch_one(&state.db)
            .await?;
    if open >= MAX_OPEN_PER_SET {
        return Err(AppError::conflict(format!(
            "you already have {MAX_OPEN_PER_SET} open proposals for this set; wait for an answer"
        )));
    }
    Ok(set)
}

/// Refuse (404 / 400 / 409) before a proposal's files are read, as
/// [`propose_items`] and [`propose_cover_image`] would.
pub async fn check_target(state: &AppState, viewer: &CurrentUser, set_id: &str) -> AppResult<()> {
    target_set(state, viewer, set_id).await.map(|_| ())
}

/// The new proposal row (in a transaction the caller commits).
#[allow(clippy::too_many_arguments)]
async fn insert(
    conn: &mut sqlx::SqliteConnection,
    set_id: &str,
    item_id: Option<&str>,
    proposer: &str,
    kind: ProposalKind,
    message: &str,
    changes: &ProposedChanges,
    before: &ProposedChanges,
    cover_media_id: Option<&str>,
) -> AppResult<String> {
    let id = new_id();
    sqlx::query(
        "INSERT INTO proposals (id, set_id, item_id, proposer_id, kind, message, changes, before, cover_media_id, \
         created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(set_id)
    .bind(item_id)
    .bind(proposer)
    .bind(enum_str(&kind))
    .bind(message)
    .bind(serde_json::to_string(changes).map_err(AppError::internal)?)
    .bind(serde_json::to_string(before).map_err(AppError::internal)?)
    .bind(cover_media_id)
    .bind(now())
    .execute(&mut *conn)
    .await?;
    Ok(id)
}

/// Propose new stickers for someone else's set. Files that cannot be
/// stored are skipped; if none can, the error of the first is returned.
pub async fn propose_items(
    state: &AppState,
    viewer: &CurrentUser,
    set_id: &str,
    parts: UploadParts,
    message: &str,
) -> AppResult<Proposal> {
    let set = target_set(state, viewer, set_id).await?;
    let message = clean_note(message, "message")?;
    if parts.files.len() > MAX_ITEMS {
        return Err(AppError::bad_request(format!(
            "at most {MAX_ITEMS} stickers per proposal"
        )));
    }
    state.limiters.check_upload(viewer.id())?;
    let recommended = set
        .recommended_px
        .map(|v| v as u32)
        .unwrap_or(state.config.media.default_recommended_px);
    // Held until the proposal names the media, so the GC cannot take it.
    let guard = state.media_lock.read().await;
    let mut stored: Vec<(MediaRow, ItemInput)> = Vec::new();
    let mut first_error = None;
    for (i, file) in parts.files.iter().enumerate() {
        let outcome = async {
            let input = ItemInput::from_upload(&file.filename, parts.meta.get(i))?;
            let (media, _) = upload::media_for_file(state, file, recommended).await?;
            Ok::<_, AppError>((media, input))
        }
        .await;
        match outcome {
            Ok(pair) => stored.push(pair),
            Err(e) => {
                tracing::debug!("proposed file {:?} skipped: {e}", file.filename);
                first_error.get_or_insert(e);
            }
        }
    }
    if stored.is_empty() {
        return Err(first_error.unwrap_or_else(|| AppError::bad_request("no files to propose")));
    }
    let mut tx = sets::begin(&state.db).await?;
    let id = insert(
        &mut tx,
        &set.id,
        None,
        viewer.id(),
        ProposalKind::AddItems,
        &message,
        &ProposedChanges::default(),
        &ProposedChanges::default(),
        None,
    )
    .await?;
    for (pos, (media, input)) in stored.iter().enumerate() {
        sqlx::query(
            "INSERT INTO proposal_items (proposal_id, position, media_id, kind, format, width, height, filename, \
             name, keywords, emojis, vibe, tags) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(pos as i64)
        .bind(&media.id)
        .bind(&media.kind)
        .bind(&media.format)
        .bind(media.width)
        .bind(media.height)
        .bind(&input.stem)
        .bind(&input.name)
        .bind(to_json_list(&input.keywords))
        .bind(to_json_list(&input.emojis))
        .bind(&input.vibe)
        .bind(to_json_list(&input.tags))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    drop(guard);
    get(state, viewer, &id).await
}

/// Propose changes to someone else's sticker: name, keywords, emojis, tags
/// and vibe. Fields equal to the current values are dropped.
pub async fn propose_item_changes(
    state: &AppState,
    viewer: &CurrentUser,
    item_id: &str,
    req: NewProposal,
) -> AppResult<Proposal> {
    let r = authz::readable_item(state, viewer.id(), item_id).await?;
    if r.can_edit() {
        return Err(AppError::bad_request("you may edit this sticker directly"));
    }
    target_set(state, viewer, &r.access.set.id).await?;
    let message = clean_note(&req.message, "message")?;
    let c = req.changes;
    if c.description.is_some() || c.cover_item_id.is_some() {
        return Err(AppError::bad_request(
            "a sticker has no description or cover; propose those for its set",
        ));
    }
    let cur = hydrate::fetch_one(&state.db, item_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let mut changes = ProposedChanges::default();
    let mut before = ProposedChanges::default();
    if let Some(n) = c.name.as_deref().map(clean_name).transpose()? {
        if n.is_empty() {
            return Err(AppError::bad_request("name must not be empty"));
        }
        if n != cur.name {
            before.name = Some(cur.name.clone());
            changes.name = Some(n);
        }
    }
    for (given, current, what, slot_c, slot_b) in [
        (
            &c.keywords,
            cur.keywords(),
            "keywords",
            &mut changes.keywords,
            &mut before.keywords,
        ),
        (
            &c.emojis,
            cur.emojis(),
            "emojis",
            &mut changes.emojis,
            &mut before.emojis,
        ),
        (&c.tags, cur.tags(), "tags", &mut changes.tags, &mut before.tags),
    ] {
        if let Some(v) = given {
            let v = clean_list(v, what)?;
            if v != current {
                *slot_b = Some(current);
                *slot_c = Some(v);
            }
        }
    }
    if let Some(v) = &c.vibe {
        let v = clean_vibe(v.as_deref())?;
        if v != cur.vibe {
            before.vibe = Some(cur.vibe.clone());
            changes.vibe = Some(v);
        }
    }
    if changes.is_empty() {
        return Err(AppError::bad_request("nothing to change: the values are the same"));
    }
    let mut conn = state.db.acquire().await?;
    let id = insert(
        &mut conn,
        &r.access.set.id,
        Some(item_id),
        viewer.id(),
        ProposalKind::EditItem,
        &message,
        &changes,
        &before,
        None,
    )
    .await?;
    drop(conn);
    get(state, viewer, &id).await
}

/// Propose changes to someone else's set: name, description, or one of its
/// stickers as the cover.
pub async fn propose_set_changes(
    state: &AppState,
    viewer: &CurrentUser,
    set_id: &str,
    req: NewProposal,
) -> AppResult<Proposal> {
    let set = target_set(state, viewer, set_id).await?;
    if set.can_edit_pack() {
        return Err(AppError::bad_request("you may edit this set directly"));
    }
    let message = clean_note(&req.message, "message")?;
    let c = req.changes;
    if c.keywords.is_some() || c.emojis.is_some() || c.tags.is_some() || c.vibe.is_some() {
        return Err(AppError::bad_request(
            "keywords, emojis, tags and vibe belong to stickers; propose them for one",
        ));
    }
    let mut changes = ProposedChanges::default();
    let mut before = ProposedChanges::default();
    if let Some(n) = c.name.as_deref().map(sets::validate_name).transpose()? {
        if n != set.name {
            before.name = Some(set.name.clone());
            changes.name = Some(n);
        }
    }
    if let Some(d) = c.description.as_deref().map(sets::validate_description).transpose()? {
        if d != set.description {
            before.description = Some(set.description.clone());
            changes.description = Some(d);
        }
    }
    if let Some(item) = c.cover_item_id.filter(|i| !i.is_empty()) {
        let in_set: Option<String> = sqlx::query_scalar("SELECT id FROM items WHERE id = ? AND set_id = ?")
            .bind(&item)
            .bind(&set.id)
            .fetch_optional(&state.db)
            .await?;
        if in_set.is_none() {
            return Err(AppError::bad_request("the cover must be a sticker of this set"));
        }
        let current = sets::chosen_cover(&state.db, &set.id).await?;
        if current.as_deref() != Some(item.as_str()) || set.cover_media_id.is_some() {
            before.cover_item_id = current;
            changes.cover_item_id = Some(item);
        }
    }
    if changes.is_empty() {
        return Err(AppError::bad_request("nothing to change: the values are the same"));
    }
    let mut conn = state.db.acquire().await?;
    let id = insert(
        &mut conn,
        &set.id,
        None,
        viewer.id(),
        ProposalKind::EditSet,
        &message,
        &changes,
        &before,
        None,
    )
    .await?;
    drop(conn);
    get(state, viewer, &id).await
}

/// Propose an image (a still) as the cover of someone else's set.
pub async fn propose_cover_image(
    state: &AppState,
    viewer: &CurrentUser,
    set_id: &str,
    file: &UploadFile,
    message: &str,
) -> AppResult<Proposal> {
    let set = target_set(state, viewer, set_id).await?;
    if set.can_edit_pack() {
        return Err(AppError::bad_request("you may change the cover directly"));
    }
    let message = clean_note(message, "message")?;
    state.limiters.check_upload(viewer.id())?;
    cover::sniff_still(file).await?;
    let recommended = set
        .recommended_px
        .map(|v| v as u32)
        .unwrap_or(state.config.media.default_recommended_px);
    let guard = state.media_lock.read().await;
    let (media, _) = upload::media_for_file_where(state, file, recommended, cover::still_only).await?;
    let mut tx = sets::begin(&state.db).await?;
    let id = insert(
        &mut tx,
        &set.id,
        None,
        viewer.id(),
        ProposalKind::EditSet,
        &message,
        &ProposedChanges::default(),
        &ProposedChanges::default(),
        Some(&media.id),
    )
    .await?;
    tx.commit().await?;
    drop(guard);
    get(state, viewer, &id).await
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Who may see a proposal: its proposer, the set's owner and reviewers.
struct Seen {
    row: ProposalRow,
    set: SetAccess,
}

impl Seen {
    fn is_proposer(&self) -> bool {
        self.row.proposer_id == self.set.viewer
    }
    /// Open, the viewer reviews the set, and it is someone else's: a
    /// reviewer's own proposals need another reviewer (or the owner).
    fn can_decide(&self) -> bool {
        self.row.status() == ProposalStatus::Open && self.set.can_review() && !self.is_proposer()
    }
    fn can_withdraw(&self) -> bool {
        self.row.status() == ProposalStatus::Open && self.is_proposer()
    }
}

async fn load(state: &AppState, viewer: &CurrentUser, id: &str) -> AppResult<Seen> {
    if !is_id(id) {
        return Err(AppError::not_found());
    }
    let row: ProposalRow = sqlx::query_as(&format!("SELECT {PROPOSAL_COLUMNS} FROM proposals p WHERE p.id = ?"))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(AppError::not_found)?;
    let set = db::set_by_id(&state.db, &row.set_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let set = authz::access(&state.db, viewer.id(), set).await?;
    // A reviewer who lost the right (or a read-only pack) still sees it as
    // the owner does; anyone else only their own.
    let reviewer = set.is_owner() || matches!(set.standing, authz::Standing::Contributor(p) if p.review);
    if row.proposer_id != viewer.id() && !reviewer {
        return Err(AppError::not_found());
    }
    Ok(Seen { row, set })
}

pub async fn get(state: &AppState, viewer: &CurrentUser, id: &str) -> AppResult<Proposal> {
    let seen = load(state, viewer, id).await?;
    let mut out = to_api(state, std::slice::from_ref(&seen)).await?;
    Ok(out.pop().expect("one in, one out"))
}

/// The viewer's incoming (sets they own or review) or outgoing (their own)
/// proposals, newest first; `status` filters.
pub async fn list(
    state: &AppState,
    viewer: &CurrentUser,
    which: ProposalBox,
    status: Option<ProposalStatus>,
    limit: usize,
) -> AppResult<Vec<Proposal>> {
    let cond = match which {
        ProposalBox::Outgoing => "p.proposer_id = ?",
        ProposalBox::Incoming => {
            "p.proposer_id != ? AND p.set_id IN (SELECT id FROM sets WHERE owner_id = ? \
             UNION SELECT set_id FROM set_members WHERE user_id = ? AND can_review = 1)"
        }
    };
    let mut sql = format!("SELECT {PROPOSAL_COLUMNS} FROM proposals p WHERE {cond}");
    if status.is_some() {
        sql.push_str(" AND p.status = ?");
    }
    sql.push_str(" ORDER BY p.created_at DESC, p.id DESC LIMIT ?");
    let mut q = sqlx::query_as::<_, ProposalRow>(&sql).bind(viewer.id());
    if which == ProposalBox::Incoming {
        q = q.bind(viewer.id()).bind(viewer.id());
    }
    if let Some(s) = status {
        q = q.bind(enum_str(&s));
    }
    let rows = q.bind(limit.clamp(1, 500) as i64).fetch_all(&state.db).await?;
    let mut seen = Vec::with_capacity(rows.len());
    let mut sets_cache: HashMap<String, SetAccess> = HashMap::new();
    for row in rows {
        let set = match sets_cache.get(&row.set_id) {
            Some(s) => s.clone(),
            None => {
                let Some(set) = db::set_by_id(&state.db, &row.set_id).await? else {
                    continue;
                };
                let a = authz::access(&state.db, viewer.id(), set).await?;
                sets_cache.insert(row.set_id.clone(), a.clone());
                a
            }
        };
        seen.push(Seen { row, set });
    }
    to_api(state, &seen).await
}

/// Open proposals waiting for the viewer's decision (the nav badge).
pub async fn incoming_open(state: &AppState, viewer: &CurrentUser) -> AppResult<u64> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM proposals p JOIN sets s ON s.id = p.set_id WHERE p.status = 'open' \
         AND (s.source != 'git' OR s.git_mode = 'push') AND p.proposer_id != ? \
         AND p.set_id IN (SELECT id FROM sets WHERE owner_id = ? \
           UNION SELECT set_id FROM set_members WHERE user_id = ? AND can_review = 1)",
    )
    .bind(viewer.id())
    .bind(viewer.id())
    .bind(viewer.id())
    .fetch_one(&state.db)
    .await?;
    Ok(n.max(0) as u64)
}

async fn to_api(state: &AppState, seen: &[Seen]) -> AppResult<Vec<Proposal>> {
    let mut usernames: HashMap<String, String> = HashMap::new();
    let mut item_ids = Vec::new();
    for s in seen {
        for id in std::iter::once(&s.row.proposer_id).chain(s.row.reviewer_id.as_ref()) {
            if !usernames.contains_key(id) {
                let name = db::user_by_id(&state.db, id)
                    .await?
                    .map(|u| u.username)
                    .unwrap_or_default();
                usernames.insert(id.clone(), name);
            }
        }
        item_ids.extend(s.row.item_id.clone());
    }
    let summaries = hydrate::summaries_by_id(&state.db, &item_ids).await?;
    let mut set_summaries: HashMap<String, stickpile_api::SetSummary> = HashMap::new();
    let mut out = Vec::with_capacity(seen.len());
    for s in seen {
        let row = &s.row;
        let set = match set_summaries.get(&row.set_id) {
            Some(x) => x.clone(),
            None => {
                let x = sets::summary(&state.db, &row.set_id).await?;
                set_summaries.insert(row.set_id.clone(), x.clone());
                x
            }
        };
        let open = row.status() == ProposalStatus::Open;
        let items: Vec<ItemRow> = if row.kind() == ProposalKind::AddItems {
            sqlx::query_as(
                "SELECT position, media_id, kind, format, width, height, filename, name, keywords, emojis, vibe, \
                 tags, item_id FROM proposal_items WHERE proposal_id = ? ORDER BY position",
            )
            .bind(&row.id)
            .fetch_all(&state.db)
            .await?
        } else {
            Vec::new()
        };
        let pos_str: Vec<String> = items.iter().map(|i| i.position.to_string()).collect();
        let items = items
            .iter()
            .zip(&pos_str)
            .map(|(i, pos)| ProposedItem {
                name: i.name.clone(),
                keywords: json_list(&i.keywords),
                emojis: json_list(&i.emojis),
                tags: json_list(&i.tags),
                vibe: i.vibe.clone(),
                kind: parse_enum(&i.kind).unwrap_or(MediaKind::Still),
                format: parse_enum(&i.format).unwrap_or(MediaFormat::Png),
                width: i.width.max(0) as u32,
                height: i.height.max(0) as u32,
                thumb_url: (open && i.media_id.is_some()).then(|| r::path(r::PROPOSAL_THUMB, &[&row.id, pos])),
                item_id: i.item_id.clone(),
            })
            .collect();
        out.push(Proposal {
            id: row.id.clone(),
            kind: row.kind(),
            status: row.status(),
            set,
            // A sticker that moved to another set is gone for this
            // proposal (and may be in a set the viewer cannot read).
            item: row
                .item_id
                .as_ref()
                .and_then(|i| summaries.get(i))
                .filter(|i| i.set_id == row.set_id)
                .cloned(),
            proposer_username: usernames.get(&row.proposer_id).cloned().unwrap_or_default(),
            message: row.message.clone(),
            items,
            changes: row.changes(),
            before: serde_json::from_str(&row.before).unwrap_or_default(),
            cover_image_url: (open && row.cover_media_id.is_some())
                .then(|| r::path(r::PROPOSAL_THUMB, &[&row.id, "cover"])),
            reviewer_username: row.reviewer_id.as_ref().and_then(|r| usernames.get(r)).cloned(),
            review_note: row.review_note.clone(),
            created_at: row.created_at,
            decided_at: row.decided_at,
            can_decide: s.can_decide(),
            can_withdraw: s.can_withdraw(),
        });
    }
    Ok(out)
}

/// The thumbnail of a proposed sticker (`n`: its position) or of a
/// proposed cover image (`n` = `cover`), while the proposal is open.
pub async fn thumb(state: &AppState, viewer: &CurrentUser, id: &str, n: &str) -> AppResult<VariantRow> {
    let seen = load(state, viewer, id).await?;
    if seen.row.status() != ProposalStatus::Open {
        return Err(AppError::not_found());
    }
    let media_id = if n == "cover" {
        seen.row.cover_media_id.clone()
    } else {
        let pos: i64 = n.parse().map_err(|_| AppError::not_found())?;
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT media_id FROM proposal_items WHERE proposal_id = ? AND position = ?",
        )
        .bind(id)
        .bind(pos)
        .fetch_optional(&state.db)
        .await?
        .flatten()
    };
    let media_id = media_id.ok_or_else(AppError::not_found)?;
    cover::variant_row(&state.db, &media_id, stickpile_api::Variant::Thumb)
        .await?
        .ok_or_else(AppError::not_found)
}

// ---------------------------------------------------------------------------
// Deciding
// ---------------------------------------------------------------------------

/// Move an open proposal to `status`; false if it was not open any more.
async fn close(
    state: &AppState,
    id: &str,
    status: ProposalStatus,
    reviewer: Option<&str>,
    note: &str,
) -> AppResult<bool> {
    Ok(sqlx::query(
        "UPDATE proposals SET status = ?, reviewer_id = ?, review_note = ?, decided_at = ? \
         WHERE id = ? AND status = 'open'",
    )
    .bind(enum_str(&status))
    .bind(reviewer)
    .bind(note)
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?
    .rows_affected()
        > 0)
}

fn not_open() -> AppError {
    AppError::conflict("this proposal is not open any more")
}

/// Accept: make the change, then mark it accepted (owner or reviewer).
pub async fn accept(state: &AppState, viewer: &CurrentUser, id: &str, note: &str) -> AppResult<Proposal> {
    let seen = load(state, viewer, id).await?;
    let note = clean_note(note, "note")?;
    if seen.row.status() != ProposalStatus::Open {
        return Err(not_open());
    }
    if seen.is_proposer() {
        return Err(AppError::forbidden(
            "you cannot accept your own proposal; the owner or another reviewer decides",
        ));
    }
    if !seen.set.can_review() {
        return Err(if seen.set.read_only() {
            AppError::forbidden("this set is a read-only git pack; it cannot take changes")
        } else {
            AppError::forbidden("you may not review proposals for this set")
        });
    }
    // Claim it first, so two reviewers cannot both apply it.
    if !close(state, id, ProposalStatus::Accepted, Some(viewer.id()), &note).await? {
        return Err(not_open());
    }
    let applied = apply(state, &seen).await;
    if let Err(e) = applied {
        sqlx::query(
            "UPDATE proposals SET status = 'open', reviewer_id = NULL, review_note = '', decided_at = NULL \
             WHERE id = ?",
        )
        .bind(id)
        .execute(&state.db)
        .await?;
        return Err(e);
    }
    db::audit(
        &state.db,
        Some(viewer.id()),
        "proposal.accept",
        Some(id),
        viewer.ip_string().as_deref(),
        Some(serde_json::json!({ "set_id": seen.row.set_id, "kind": seen.row.kind })),
    )
    .await;
    get(state, viewer, id).await
}

async fn apply(state: &AppState, seen: &Seen) -> AppResult<()> {
    let row = &seen.row;
    let set: &SetRow = &seen.set.set;
    match row.kind() {
        ProposalKind::AddItems => {
            let items: Vec<ItemRow> = sqlx::query_as(
                "SELECT position, media_id, kind, format, width, height, filename, name, keywords, emojis, vibe, \
                 tags, item_id FROM proposal_items WHERE proposal_id = ? ORDER BY position",
            )
            .bind(&row.id)
            .fetch_all(&state.db)
            .await?;
            let mut added = Vec::new();
            let outcome = add_proposed(state, set, row, &items, &mut added).await;
            if let Err(e) = outcome {
                // All or nothing: take back the stickers this attempt added.
                remove_added(state, set, &added).await?;
                return Err(e);
            }
            index::refresh_items(state, &added).await?;
            Ok(())
        }
        ProposalKind::EditItem => apply_item_changes(state, seen).await,
        ProposalKind::EditSet => apply_set_changes(state, seen).await,
    }
}

/// Add the stickers of an add_items proposal; `added` collects the new items.
async fn add_proposed(
    state: &AppState,
    set: &SetRow,
    row: &ProposalRow,
    items: &[ItemRow],
    added: &mut Vec<String>,
) -> AppResult<()> {
    for it in items {
        let Some(media_id) = &it.media_id else {
            continue;
        };
        let guard = state.media_lock.read().await;
        let Some(media) = db::media_by_id(&state.db, media_id).await? else {
            continue;
        };
        let input = ItemInput {
            name: it.name.clone(),
            name_source: FieldSource::User,
            keywords: json_list(&it.keywords),
            keywords_source: FieldSource::User,
            emojis: json_list(&it.emojis),
            emojis_source: FieldSource::User,
            vibe: it.vibe.clone(),
            vibe_source: FieldSource::User,
            tags: json_list(&it.tags),
            stem: it.filename.clone(),
            ..Default::default()
        };
        let out = upload::add_media_to_set(state, set, media, false, &input, &row.proposer_id, guard).await?;
        sqlx::query("UPDATE proposal_items SET item_id = ? WHERE proposal_id = ? AND position = ?")
            .bind(&out.item_id)
            .bind(&row.id)
            .bind(it.position)
            .execute(&state.db)
            .await?;
        if !out.already_in_set {
            added.push(out.item_id);
        }
    }
    Ok(())
}

/// Remove items an accept added before it failed.
async fn remove_added(state: &AppState, set: &SetRow, added: &[String]) -> AppResult<()> {
    if added.is_empty() {
        return Ok(());
    }
    let mut tx = sets::begin(&state.db).await?;
    sets::tombstone(&mut tx, added, set, sets::Audience::for_removal(set.visibility())).await?;
    for id in added {
        sqlx::query("DELETE FROM items WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE proposal_items SET item_id = NULL WHERE item_id IN (SELECT value FROM json_each(?))")
        .bind(serde_json::to_string(added).map_err(AppError::internal)?)
        .execute(&mut *tx)
        .await?;
    sets::renumber(&mut tx, &set.id).await?;
    sets::bump_version(&mut tx, &set.id).await?;
    tx.commit().await?;
    index::remove_items(state, added);
    Ok(())
}

async fn apply_item_changes(state: &AppState, seen: &Seen) -> AppResult<()> {
    let row = &seen.row;
    let set: &SetRow = &seen.set.set;
    let item_id = row
        .item_id
        .as_deref()
        .ok_or_else(|| AppError::conflict("the sticker was removed"))?;
    let r = authz::readable_item(state, &seen.set.viewer, item_id).await?;
    if r.access.set.id != set.id {
        return Err(AppError::conflict("the sticker moved to another set"));
    }
    let c = row.changes();
    let req = UpdateItem {
        name: c.name,
        keywords: c.keywords,
        emojis: c.emojis,
        vibe: c.vibe,
        tags: c.tags,
        ..Default::default()
    };
    items::apply_update(state, &seen.set.viewer, &r, req, None).await?;
    Ok(())
}

async fn apply_set_changes(state: &AppState, seen: &Seen) -> AppResult<()> {
    let row = &seen.row;
    let set: &SetRow = &seen.set.set;
    if let Some(media_id) = &row.cover_media_id {
        // A cover image proposal changes nothing else.
        let _guard = state.media_lock.read().await;
        let mut tx = sets::begin(&state.db).await?;
        let changed = sqlx::query("UPDATE sets SET cover_media_id = ? WHERE id = ? AND cover_media_id IS NOT ?")
            .bind(media_id)
            .bind(&set.id)
            .bind(media_id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            > 0;
        if changed {
            sets::bump_version(&mut tx, &set.id).await?;
        }
        tx.commit().await?;
        return Ok(());
    }
    let c = row.changes();
    if c.is_empty() {
        // A cover image proposal whose image was cleaned up.
        return Err(AppError::conflict("the proposed cover image is gone"));
    }
    if c.cover_item_id.is_some() {
        let in_set: Option<String> = sqlx::query_scalar("SELECT id FROM items WHERE id = ? AND set_id = ?")
            .bind(c.cover_item_id.as_deref())
            .bind(&set.id)
            .fetch_optional(&state.db)
            .await?;
        if in_set.is_none() {
            return Err(AppError::conflict("the proposed cover is no longer in the set"));
        }
    }
    let req = UpdateSet {
        name: c.name,
        description: c.description,
        cover_item_id: c.cover_item_id.map(Some),
        ..Default::default()
    };
    sets::apply_update(state, set, req).await
}

/// Decline (owner or reviewer).
pub async fn decline(state: &AppState, viewer: &CurrentUser, id: &str, note: &str) -> AppResult<Proposal> {
    let seen = load(state, viewer, id).await?;
    let note = clean_note(note, "note")?;
    if seen.row.status() != ProposalStatus::Open {
        return Err(not_open());
    }
    if seen.is_proposer() {
        return Err(AppError::forbidden("withdraw your own proposal instead"));
    }
    // A read-only pack takes no changes, but its owner may still say no.
    if !seen.set.can_review() && !seen.set.is_owner() {
        return Err(AppError::forbidden("you may not review proposals for this set"));
    }
    if !close(state, id, ProposalStatus::Declined, Some(viewer.id()), &note).await? {
        return Err(not_open());
    }
    get(state, viewer, id).await
}

/// Withdraw (its proposer, while open).
pub async fn withdraw(state: &AppState, viewer: &CurrentUser, id: &str) -> AppResult<Proposal> {
    let seen = load(state, viewer, id).await?;
    if !seen.is_proposer() {
        return Err(AppError::forbidden("only its proposer can withdraw a proposal"));
    }
    if !close(state, id, ProposalStatus::Withdrawn, None, "").await? {
        return Err(not_open());
    }
    get(state, viewer, id).await
}

/// `UploadMeta` for one proposed file from simple form fields (web UI).
pub fn form_meta(name: &str, keywords: &str, emojis: &str, tags: &str) -> UploadMeta {
    let split = |s: &str| -> Option<Vec<String>> {
        let v: Vec<String> = s
            .split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect();
        (!v.is_empty()).then_some(v)
    };
    UploadMeta {
        name: Some(name.trim().to_string()).filter(|n| !n.is_empty()),
        keywords: split(keywords),
        emojis: split(emojis),
        tags: split(tags),
        ..Default::default()
    }
}

/// A multipart proposal body: file parts, an optional `meta` JSON part (as
/// for uploads) and an optional `message` text part.
pub async fn read_multipart(state: &AppState, mut mp: axum::extract::Multipart) -> AppResult<(UploadParts, String)> {
    let max_file = state.config.limits.max_upload_mb * 1024 * 1024;
    let mut files = Vec::new();
    let mut meta = Vec::new();
    let mut message = String::new();
    while let Some(mut field) = mp.next_field().await.map_err(upload::multipart_error)? {
        match field.name() {
            Some("meta") => meta = upload::parse_meta(&upload::field_text(&mut field, 1024 * 1024).await?)?,
            Some("message") => message = upload::field_text(&mut field, MAX_NOTE_CHARS * 4 + 16).await?,
            _ if field.file_name().is_some() => files.push(upload::field_to_file(state, &mut field, max_file).await?),
            _ => {}
        }
    }
    if files.is_empty() {
        return Err(AppError::bad_request("choose at least one file"));
    }
    Ok((UploadParts { files, meta }, message))
}
