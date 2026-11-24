//! Telegram import and export through the Bot API (docs/telegram.md).
//!
//! * [`import`] queues a `TelegramImport` job ([`import_job`]):
//!   `stickpile_telegram::import_pack` downloads every sticker (capped at
//!   `limits.max_upload_mb`), a new set (`source = telegram`, named after the
//!   pack title) is created and each sticker goes through the normal ingest
//!   ([`upload::add_to_set`]) with its emoji as user metadata. Animated
//!   `.tgs` stickers and files that fail ingest are listed in the job result.
//!   If the bot itself owns the pack, the set is bound to it and the export
//!   mapping seeded, so a later export keeps the existing stickers.
//! * [`export`] queues a `TelegramExport` job ([`export_job`]): every item
//!   becomes its `tg_sticker` (stills) or `tg_video_sticker` (animations and
//!   videos) variant. Variants the `Variants` job has not produced yet are
//!   rendered in the job. Items that cannot become stickers are skipped and
//!   listed. A set with more stickers than one pack holds is published as
//!   several packs, its parts (`stickpile_telegram::parts`); each part's
//!   pack name, mapping and thumbnail key are stored in `telegram_parts`
//!   (migration 0017), `sets.telegram_pack` names the first, and
//!   `sets.telegram_base` is the name the parts' names derive from. The
//!   parts of an export under a new name are pending in
//!   `telegram_pending_parts` (migration 0018) until it has finished.
//! * [`plan`] says what an export would do to the parts, without changing
//!   anything (the count a repack's confirmation shows).
//!
//! The pack owner is the exporting user's `telegram_user_id` (`PATCH
//! /api/v1/me`), else `telegram.default_owner_user_id`.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use axum::http::StatusCode;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use stickpile_api::{
    Job, JobKind, MediaKind, PublishedPart, TelegramExport, TelegramImport, TelegramPlan, Variant, Visibility,
};
use stickpile_telegram::{
    BotApi, ExportItem, PackMapping, PackState, PackThumbnail, PartStates, PartUpdate, PartsError, PartsFailure,
    PartsPlan, StickerFormat, TelegramError, MAX_PACK_STICKERS,
};

use crate::auth::CurrentUser;
use crate::authz;
use crate::config::read_secret;
use crate::db::{self, VariantRow};
use crate::error::{AppError, AppResult};
use crate::files;
use crate::ids;
use crate::index;
use crate::jobs::{self, JobError, JobResult, JobRow};
use crate::service::sets::{self, NewSet};
use crate::service::upload::{self, clean_list, stem_for, ItemInput, UploadFile};
use crate::service::{cover, hydrate};
use crate::state::AppState;

const DISABLED: &str =
    "the Telegram integration is disabled on this server (telegram.enabled or the bot token is missing)";
const NO_OWNER: &str = "no Telegram user id to own the pack: set yours in your profile (telegram_user_id, \
     PATCH /api/v1/me) or ask the admin to set telegram.default_owner_user_id";

/// The integration is configured: `telegram.enabled` and a readable,
/// non-empty bot token file.
pub fn enabled(state: &AppState) -> bool {
    state.config.telegram.enabled && token(state).is_some()
}

fn token(state: &AppState) -> Option<String> {
    let path = state.config.telegram.bot_token_file.as_ref()?;
    read_secret(path)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn disabled() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "not_found", DISABLED)
}

fn bot(state: &AppState) -> Result<BotApi, JobError> {
    if !state.config.telegram.enabled {
        return Err(JobError::Fatal(DISABLED.into()));
    }
    let token = token(state).ok_or_else(|| JobError::Fatal("cannot read telegram.bot_token_file".into()))?;
    Ok(BotApi::new(&state.config.telegram.api_url, &token))
}

/// `https://t.me/addstickers/<name>`.
pub fn pack_url(name: &str) -> String {
    format!("https://t.me/addstickers/{name}")
}

/// A pack short name as users paste it: bare, or a `t.me/addstickers/` link.
fn clean_pack_name(raw: &str) -> AppResult<String> {
    let s = raw.trim().trim_end_matches('/');
    let s = s.rsplit_once("/addstickers/").map(|(_, n)| n).unwrap_or(s);
    let ok = (1..=64).contains(&s.len())
        && s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(s.to_string())
    } else {
        Err(AppError::bad_request(
            "pack_name must be a sticker pack short name (1-64 of A-Z a-z 0-9 _, starting with a letter) or a t.me/addstickers/ link",
        ))
    }
}

/// Telegram user id that owns packs exported by `user`.
fn owner_id(state: &AppState, user: &db::UserRow) -> Option<i64> {
    user.telegram_user_id
        .or(state.config.telegram.default_owner_user_id)
        .filter(|id| *id > 0)
}

fn tg_error(e: TelegramError) -> JobError {
    match &e {
        TelegramError::Http(_) => JobError::Retry(e.to_string()),
        TelegramError::Api { code, .. } if *code == 429 || *code >= 500 => JobError::Retry(e.to_string()),
        _ if e.is_missing_sticker_set() => JobError::Fatal("sticker pack not found".into()),
        _ => JobError::Fatal(e.to_string()),
    }
}

fn retry(e: impl std::fmt::Display) -> JobError {
    JobError::Retry(e.to_string())
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Queue the import of a pack into a new set of the caller. The slug
/// defaults to the slugified pack name and must be free (409).
pub async fn import(state: &AppState, user: &CurrentUser, req: TelegramImport) -> AppResult<Job> {
    if !enabled(state) {
        return Err(disabled());
    }
    let name = clean_pack_name(&req.pack_name)?;
    let slug = match req.slug.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(s) => s,
        None => stickpile_setfmt::slugify(&name.replace('_', "-")),
    };
    sets::validate_slug(&slug)?;
    let taken: Option<i64> = sqlx::query_scalar("SELECT 1 FROM sets WHERE owner_id = ? AND slug = ?")
        .bind(user.id())
        .bind(&slug)
        .fetch_optional(&state.db)
        .await?;
    if taken.is_some() {
        return Err(AppError::conflict(format!("you already have a set with slug {slug:?}")));
    }
    state.limiters.check_upload(user.id())?;
    jobs::enqueue(
        state,
        JobKind::TelegramImport,
        json!({ "pack_name": name, "slug": slug, "visibility": req.visibility }),
        Some(name.clone()),
        Some(user.id()),
    )
    .await
}

/// `TelegramImport` job.
pub async fn import_job(state: &AppState, job: &JobRow) -> JobResult {
    let p = job.payload();
    let name = p["pack_name"]
        .as_str()
        .ok_or_else(|| JobError::Fatal("payload without pack_name".into()))?
        .to_string();
    let slug = p["slug"]
        .as_str()
        .ok_or_else(|| JobError::Fatal("payload without slug".into()))?
        .to_string();
    let visibility: Visibility = serde_json::from_value(p["visibility"].clone()).unwrap_or_default();
    let Some(owner) = job.owner_id.as_deref() else {
        return Err(JobError::Fatal("job without owner".into()));
    };
    let Some(user) = db::user_by_id(&state.db, owner).await.map_err(retry)? else {
        return Ok(Some(json!({"skipped": "the user no longer exists"})));
    };
    let api = bot(state)?;
    let max = state.config.limits.max_upload_mb.saturating_mul(1024 * 1024);
    // Everything is downloaded before the set exists, so a retry after a
    // network error starts clean.
    let pack = stickpile_telegram::import_pack(&api, &name, max)
        .await
        .map_err(tg_error)?;
    let bot_username = api.get_me().await.ok().and_then(|b| b.username);

    let title = pack.title.trim();
    let set_name = sets::validate_name(title).unwrap_or_else(|_| pack.name.clone());
    let row = sets::insert(
        &state.db,
        &user.id,
        NewSet {
            slug,
            name: set_name,
            description: String::new(),
            visibility,
            aspect_ratio: Some((1, 1)),
            recommended_px: Some(512),
            version: 1,
            extra_toml: None,
        },
    )
    .await
    .map_err(|e| JobError::Fatal(e.message))?;
    sqlx::query("UPDATE sets SET source = 'telegram' WHERE id = ?")
        .bind(&row.id)
        .execute(&state.db)
        .await
        .map_err(retry)?;

    let mut skipped: Vec<Value> = pack
        .skipped
        .iter()
        .map(|(fuid, reason)| json!({"file_unique_id": fuid, "reason": reason}))
        .collect();
    let mut mapping = PackMapping::new();
    let mut imported = 0u32;
    for (i, sticker) in pack.stickers.into_iter().enumerate() {
        let ext = match sticker.format {
            StickerFormat::Video => "webm",
            _ => "webp",
        };
        let item_name = format!("sticker {}", i + 1);
        let input = ItemInput {
            stem: stem_for(&item_name),
            name: item_name,
            emojis: clean_list(&sticker.emojis, "emojis").unwrap_or_default(),
            ..Default::default()
        };
        let file = UploadFile::from_bytes(format!("{:03}-sticker.{ext}", i + 1), sticker.bytes);
        match upload::add_to_set(state, &row, &file, &input, &row.owner_id).await {
            Ok(added) if added.already_in_set => skipped.push(json!({
                "file_unique_id": sticker.file_unique_id,
                "reason": "the same file as an earlier sticker of the pack",
            })),
            Ok(added) => {
                imported += 1;
                mapping.insert(added.item_id, (added.media.sha256.clone(), sticker.file_unique_id));
            }
            Err(e) if e.status.is_server_error() => return Err(retry(e)),
            Err(e) => skipped.push(json!({"file_unique_id": sticker.file_unique_id, "reason": e.message})),
        }
    }
    index::refresh_set(state, &row.id).await?;
    // A pack the bot created can be exported back: bind it and keep the
    // stickers on the first export. Only for its owner here, or for an admin
    // adopting a pack made outside stickpile; anyone else gets a copy.
    let own_bot_pack = bot_username
        .as_deref()
        .is_some_and(|b| stickpile_telegram::valid_pack_name(&pack.name, b));
    let allowed = own_bot_pack
        && match pack_owner(state, &pack.name).await? {
            Some(owner) => owner == user.id,
            None => user.is_admin() && claim_pack(state, &pack.name, &user.id).await? == user.id,
        };
    let bound = allowed && bind_imported(state, &row.id, &pack.name, &pack.title, &mapping).await?;
    Ok(Some(json!({
        "set_id": row.id,
        "slug": row.slug,
        "pack_name": pack.name,
        "title": pack.title,
        "url": pack_url(&pack.name),
        "imported": imported,
        "skipped": skipped,
        "bound": bound,
    })))
}

// ---------------------------------------------------------------------------
// Pack ownership
// ---------------------------------------------------------------------------

/// The bot can edit every pack it ever created, whoever asked for it, so
/// stickpile decides who may: the user who created the pack through this
/// server, or the admin who adopted one made elsewhere (`telegram_packs`).
const PACK_TAKEN: &str = "belongs to another stickpile user; choose another pack name";

/// Owner (stickpile user id) of pack `name`, if it was created or adopted
/// here. Names compare case-insensitively, as on Telegram.
async fn pack_owner(state: &AppState, name: &str) -> Result<Option<String>, JobError> {
    sqlx::query_scalar("SELECT user_id FROM telegram_packs WHERE name = ?")
        .bind(name)
        .fetch_optional(&state.db)
        .await
        .map_err(retry)
}

/// Record `user_id` as the owner of pack `name` unless someone already is,
/// and return the owner afterwards: of two exports racing for a new name,
/// the first claim wins.
async fn claim_pack(state: &AppState, name: &str, user_id: &str) -> Result<String, JobError> {
    sqlx::query("INSERT INTO telegram_packs (name, user_id, created_at) VALUES (?, ?, ?) ON CONFLICT(name) DO NOTHING")
        .bind(name)
        .bind(user_id)
        .bind(ids::now())
        .execute(&state.db)
        .await
        .map_err(retry)?;
    pack_owner(state, name)
        .await?
        .ok_or_else(|| JobError::Retry(format!("the claim on pack {name} vanished")))
}

/// May `user` export the parts `names` (number -> pack name) of set
/// `set_id`? Every name is checked before any is claimed, and nothing on
/// Telegram changes here. A name fails the export when it belongs to
/// another stickpile user, is bound to another of the user's sets (as one
/// of its parts, or as the name its parts derive from) other than `from`,
/// whose binding the export takes over, or exists on Telegram without
/// being published through this server, which only an admin may adopt.
/// The new names are then claimed for the user.
async fn authorize_parts(
    state: &AppState,
    api: &BotApi,
    user: &db::UserRow,
    set_id: &str,
    from: Option<&str>,
    names: &BTreeMap<u32, String>,
) -> Result<(), JobError> {
    let mut new = Vec::new();
    for (&number, name) in names {
        let what = if number == 1 {
            format!("the pack {name}")
        } else {
            format!("the pack {name} (part {number} of this set)")
        };
        let owned = match pack_owner(state, name).await? {
            Some(owner) if owner == user.id => true,
            Some(_) => return Err(JobError::Fatal(format!("{what} {PACK_TAKEN}"))),
            None => false,
        };
        if let Some((slug, part)) = bound_elsewhere(state, name, set_id, from).await? {
            return Err(JobError::Fatal(match part {
                Some(p) => format!("{what} is part {p} of your set {slug}; choose another pack name"),
                None => format!("{what} is the name of your set {slug}'s packs; choose another pack name"),
            }));
        }
        if owned {
            continue;
        }
        match api.get_sticker_set(name).await {
            Err(e) if e.is_missing_sticker_set() => {}
            Err(e) => return Err(tg_error(e)),
            Ok(_) if user.is_admin() => {}
            Ok(_) if number == 1 => {
                return Err(JobError::Fatal(format!(
                    "a pack named {name} already exists but was not published through this server; \
                     choose another pack name (an administrator can take it over)"
                )))
            }
            Ok(_) => {
                return Err(JobError::Fatal(format!(
                    "a pack named {name}, the name of part {number} of this set, already exists but was not \
                     published through this server; choose another pack name (an administrator can take it over)"
                )))
            }
        }
        new.push(name);
    }
    for name in new {
        if claim_pack(state, name, &user.id).await? != user.id {
            return Err(JobError::Fatal(format!("the pack {name} {PACK_TAKEN}")));
        }
    }
    Ok(())
}

/// The other set (not `set_id`, not `from`) bound to pack `name`: its slug
/// and the part the pack is (None: its parts' names derive from `name`,
/// whose own pack it no longer has).
async fn bound_elsewhere(
    state: &AppState,
    name: &str,
    set_id: &str,
    from: Option<&str>,
) -> Result<Option<(String, Option<u32>)>, JobError> {
    let from = from.unwrap_or("");
    let part: Option<(String, i64)> = sqlx::query_as(
        "SELECT s.slug, p.part FROM telegram_parts p JOIN sets s ON s.id = p.set_id \
         WHERE p.pack_name = ? AND p.set_id <> ? AND p.set_id <> ?",
    )
    .bind(name)
    .bind(set_id)
    .bind(from)
    .fetch_optional(&state.db)
    .await
    .map_err(retry)?;
    if let Some((slug, part)) = part {
        return Ok(Some((slug, u32::try_from(part).ok())));
    }
    let base: Option<String> =
        sqlx::query_scalar("SELECT slug FROM sets WHERE telegram_base = ? COLLATE NOCASE AND id <> ? AND id <> ?")
            .bind(name)
            .bind(set_id)
            .bind(from)
            .fetch_optional(&state.db)
            .await
            .map_err(retry)?;
    Ok(base.map(|slug| (slug, None)))
}

// ---------------------------------------------------------------------------
// Parts: the packs a set is published as
// ---------------------------------------------------------------------------

/// A row of `telegram_parts` (migration 0017): one pack of a set published
/// to Telegram. A set of at most 120 stickers has one, part 1.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PartRow {
    pub set_id: String,
    pub part: i64,
    pub pack_name: String,
    pub title: String,
    /// JSON [`PackMapping`]; None: claimed for an export that stored no
    /// state yet.
    pub mapping: Option<String>,
    pub thumbnail: Option<String>,
    pub stickers: i64,
}

impl PartRow {
    pub fn number(&self) -> u32 {
        u32::try_from(self.part).unwrap_or(u32::MAX)
    }

    /// What the last export left on the pack. A part without a stored state
    /// is passed to the next export empty, so that it is filled or deleted.
    fn state(&self) -> PackState {
        PackState {
            mapping: self
                .mapping
                .as_deref()
                .and_then(|m| serde_json::from_str(m).ok())
                .unwrap_or_default(),
            thumbnail: self.thumbnail.clone(),
        }
    }
}

const PART_COLUMNS: &str = "set_id, part, pack_name, title, mapping, thumbnail, stickers";

/// The parts of set `set_id`, by number.
pub async fn parts(db: &SqlitePool, set_id: &str) -> AppResult<Vec<PartRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {PART_COLUMNS} FROM telegram_parts WHERE set_id = ? ORDER BY part"
    ))
    .bind(set_id)
    .fetch_all(db)
    .await?)
}

/// Every pack a set is published as when it is published as several
/// (`Publication::parts`, `Set::telegram_parts`): the parts that hold
/// stickers, by number. Empty for one pack.
pub async fn published_parts(db: &SqlitePool, set_id: &str) -> AppResult<Vec<PublishedPart>> {
    let rows: Vec<PartRow> = sqlx::query_as(&format!(
        "SELECT {PART_COLUMNS} FROM telegram_parts WHERE set_id = ? AND stickers > 0 ORDER BY part"
    ))
    .bind(set_id)
    .fetch_all(db)
    .await?;
    if rows.len() < 2 {
        return Ok(Vec::new());
    }
    Ok(rows
        .iter()
        .map(|r| PublishedPart {
            number: r.number(),
            name: r.pack_name.clone(),
            url: Some(pack_url(&r.pack_name)),
            stickers: u32::try_from(r.stickers).ok(),
        })
        .collect())
}

/// The name every part's name of set `set_id` derives from
/// (`sets.telegram_base`): part 1's, kept when part 1's own pack is
/// deleted.
pub async fn base_name(db: &SqlitePool, set_id: &str) -> AppResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT telegram_base FROM sets WHERE id = ?")
        .bind(set_id)
        .fetch_optional(db)
        .await?
        .flatten())
}

/// Point `sets.telegram_pack` at the lowest part of the set that holds
/// stickers: part 1, or the lowest part left when part 1's pack was
/// deleted, so that older clients have a working link. While no part holds
/// stickers yet (a first export under way, a binding whose mapping is not
/// known), the link stays as long as it names one of the set's parts, and
/// is None otherwise.
async fn sync_link(conn: &mut sqlx::SqliteConnection, set_id: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE sets SET telegram_pack = COALESCE( \
         (SELECT pack_name FROM telegram_parts WHERE set_id = ?1 AND stickers > 0 ORDER BY part LIMIT 1), \
         (SELECT pack_name FROM telegram_parts WHERE set_id = ?1 AND pack_name = sets.telegram_pack)) \
         WHERE id = ?1",
    )
    .bind(set_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Bind the new set `set_id` to the pack `name` it was imported from, as its
/// part 1, with the mapping seeded from the import. A pack follows exactly
/// one set: another set bound to it as its only pack loses that binding. A
/// pack that is a part of a set published as several stays with that set:
/// nothing is bound, and false is returned.
async fn bind_imported(
    state: &AppState,
    set_id: &str,
    name: &str,
    title: &str,
    mapping: &PackMapping,
) -> Result<bool, JobError> {
    let json = serde_json::to_string(mapping).map_err(retry)?;
    let now = ids::now();
    let mut tx = state.db.begin().await.map_err(retry)?;
    let holder: Option<(String, i64)> = sqlx::query_as("SELECT set_id, part FROM telegram_parts WHERE pack_name = ?")
        .bind(name)
        .fetch_optional(&mut *tx)
        .await
        .map_err(retry)?;
    if let Some((other, part)) = holder {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM telegram_parts WHERE set_id = ?")
            .bind(&other)
            .fetch_one(&mut *tx)
            .await
            .map_err(retry)?;
        if part != 1 || count > 1 {
            return Ok(false);
        }
        sqlx::query("DELETE FROM telegram_parts WHERE set_id = ?")
            .bind(&other)
            .execute(&mut *tx)
            .await
            .map_err(retry)?;
        sqlx::query("UPDATE sets SET telegram_base = NULL, telegram_pack = NULL WHERE id = ?")
            .bind(&other)
            .execute(&mut *tx)
            .await
            .map_err(retry)?;
    }
    let based: Option<String> =
        sqlx::query_scalar("SELECT id FROM sets WHERE telegram_base = ? COLLATE NOCASE AND id <> ?")
            .bind(name)
            .bind(set_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(retry)?;
    if based.is_some() {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO telegram_parts (set_id, part, pack_name, title, mapping, thumbnail, stickers, created_at, \
         updated_at) VALUES (?, 1, ?, ?, ?, NULL, ?, ?, ?)",
    )
    .bind(set_id)
    .bind(name)
    .bind(title)
    .bind(&json)
    .bind(mapping.len() as i64)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(retry)?;
    sqlx::query("UPDATE sets SET telegram_base = ?1, telegram_pack = ?1 WHERE id = ?2")
        .bind(name)
        .bind(set_id)
        .execute(&mut *tx)
        .await
        .map_err(retry)?;
    tx.commit().await.map_err(retry)?;
    Ok(true)
}

/// Where the parts of an export come from.
enum Source {
    /// The set's own parts (none yet at its first export): the export
    /// keeps them in step.
    Own,
    /// A new name: the export starts from nothing, as an export under a new
    /// name always did. The set keeps its parts until the export has
    /// finished; then they are left on Telegram as they are, unbound.
    Rename,
    /// The name of the user's set `from`: once the export has finished, the
    /// binding moves to this set, every part with it, as a pack follows
    /// exactly one set. Until then `from` keeps it, and the export starts
    /// from its parts as known to exist, without a mapping.
    Takeover { from: String },
}

impl Source {
    /// The export's parts are pending (`telegram_pending_parts`) until it
    /// has finished, and the set's binding stays as it was.
    fn pending(&self) -> bool {
        !matches!(self, Source::Own)
    }

    fn from(&self) -> Option<&str> {
        match self {
            Source::Takeover { from } => Some(from),
            _ => None,
        }
    }
}

/// What an export of a set works on ([`target`]).
struct Target {
    /// Part 1's name, which every part's name derives from.
    base: String,
    source: Source,
    /// The parts' states for the planner.
    previous: PartStates,
}

/// What an export of `set` under `asked` (a pack name as given; None: the
/// set's own) works on, as the export and its plan see it.
///
/// Part 1's name is the one asked for, else the set's own (kept when part
/// 1's pack was deleted), else one from the slug; asking for one of the
/// set's own packs by name keeps the parts as they are. The previous states
/// are the set's own parts, none for a new name, or, for the name of
/// another of the user's sets, that set's parts as empty states; and the
/// parts registered for an export under the same name that has not
/// finished (pending): this set's with their states, another of the user's
/// sets' as empty states, so that each is filled or deleted, none left
/// behind.
async fn target(db: &SqlitePool, set: &db::SetRow, asked: Option<&str>, bot: &str) -> Result<Target, JobError> {
    let own = parts(db, &set.id).await?;
    let stored = base_name(db, &set.id).await?;
    let same = |name: &str| {
        stored.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(name))
            || own.iter().any(|r| r.pack_name.eq_ignore_ascii_case(name))
    };
    let base = match asked {
        Some(n) => match stickpile_telegram::complete_pack_name(n, bot) {
            n if same(&n) => stored.clone().unwrap_or(n),
            n => n,
        },
        None => match &stored {
            Some(n) => n.clone(),
            None => stickpile_telegram::default_pack_name(&set.slug, bot)
                .ok_or_else(|| JobError::Fatal(format!("cannot derive a pack name from {:?}", set.slug)))?,
        },
    };
    if !stickpile_telegram::valid_pack_name(&base, bot) {
        return Err(JobError::Fatal(format!(
            "pack name {base:?} is not valid for @{bot}: Telegram requires 1-64 characters of A-Z a-z 0-9 _, \
             starting with a letter, without \"__\", ending in _by_{bot} (added for you when the name has no \
             _by_…bot suffix), and a bot can only edit its own packs"
        )));
    }
    let source = if same(&base) {
        Source::Own
    } else {
        let other: Option<String> = sqlx::query_scalar(
            "SELECT id FROM sets WHERE telegram_base = ? COLLATE NOCASE AND id <> ? AND owner_id = ?",
        )
        .bind(&base)
        .bind(&set.id)
        .bind(&set.owner_id)
        .fetch_optional(db)
        .await
        .map_err(retry)?;
        match other {
            Some(from) => Source::Takeover { from },
            // The set's first export: there is no binding to keep.
            None if own.is_empty() && stored.is_none() => Source::Own,
            None => Source::Rename,
        }
    };
    let mut previous = match &source {
        Source::Own => own_states(&own),
        Source::Rename => PartStates::new(),
        Source::Takeover { from } => parts(db, from)
            .await?
            .iter()
            .map(|r| (r.number(), PackState::default()))
            .collect(),
    };
    let pending: Vec<PartRow> = sqlx::query_as(&format!(
        "SELECT {PART_COLUMNS} FROM telegram_pending_parts WHERE base = ? \
         AND set_id IN (SELECT id FROM sets WHERE owner_id = ?) ORDER BY part"
    ))
    .bind(&base)
    .bind(&set.owner_id)
    .fetch_all(db)
    .await
    .map_err(retry)?;
    for r in &pending {
        if r.set_id == set.id && (source.pending() || !previous.contains_key(&r.number())) {
            previous.insert(r.number(), r.state());
        } else {
            previous.entry(r.number()).or_default();
        }
    }
    Ok(Target { base, source, previous })
}

/// Before the export, once every name passed the checks: register every
/// part of `names` without a row, so that a part whose export stops before
/// it stores a state is still known to the next export.
///
/// The set's own parts are rows of `telegram_parts`, and `base` is stored
/// as the name they derive from. The parts of a pending export
/// ([`Source::pending`]) are rows of `telegram_pending_parts`, and the
/// set's binding stays as it is; a pending export of the set under another
/// name is given up (its packs stay on Telegram, unbound and still
/// claimed). The pending parts of the user's other sets with these names
/// are this export's now: they are among its parts, to be filled or
/// deleted.
async fn register_parts(
    state: &AppState,
    set_id: &str,
    base: &str,
    source: &Source,
    names: &BTreeMap<u32, String>,
) -> Result<(), JobError> {
    let now = ids::now();
    let mut tx = state.db.begin().await.map_err(retry)?;
    for name in names.values() {
        sqlx::query("DELETE FROM telegram_pending_parts WHERE pack_name = ? AND set_id <> ?")
            .bind(name)
            .bind(set_id)
            .execute(&mut *tx)
            .await
            .map_err(retry)?;
    }
    if source.pending() {
        sqlx::query("DELETE FROM telegram_pending_parts WHERE set_id = ? AND base <> ?")
            .bind(set_id)
            .bind(base)
            .execute(&mut *tx)
            .await
            .map_err(retry)?;
        for (&number, name) in names {
            sqlx::query(
                "INSERT INTO telegram_pending_parts (set_id, base, part, pack_name, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(set_id, part) DO UPDATE SET base = excluded.base, pack_name = excluded.pack_name",
            )
            .bind(set_id)
            .bind(base)
            .bind(i64::from(number))
            .bind(name)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(retry)?;
        }
        return tx.commit().await.map_err(retry);
    }
    // The set's own pending parts under this name (a takeover whose other
    // set no longer has the name) are its own parts now, with their states.
    sqlx::query(
        "INSERT INTO telegram_parts (set_id, part, pack_name, title, mapping, thumbnail, stickers, created_at, \
         updated_at) SELECT set_id, part, pack_name, title, mapping, thumbnail, stickers, created_at, ?1 \
         FROM telegram_pending_parts WHERE set_id = ?2 AND base = ?3 ON CONFLICT DO NOTHING",
    )
    .bind(now)
    .bind(set_id)
    .bind(base)
    .execute(&mut *tx)
    .await
    .map_err(retry)?;
    sqlx::query("DELETE FROM telegram_pending_parts WHERE set_id = ? AND base = ?")
        .bind(set_id)
        .bind(base)
        .execute(&mut *tx)
        .await
        .map_err(retry)?;
    sqlx::query("UPDATE sets SET telegram_base = ? WHERE id = ?")
        .bind(base)
        .bind(set_id)
        .execute(&mut *tx)
        .await
        .map_err(retry)?;
    for (&number, name) in names {
        sqlx::query(
            "INSERT INTO telegram_parts (set_id, part, pack_name, created_at, updated_at) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(set_id, part) DO UPDATE SET pack_name = excluded.pack_name",
        )
        .bind(set_id)
        .bind(i64::from(number))
        .bind(name)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(retry)?;
    }
    sync_link(&mut tx, set_id).await.map_err(retry)?;
    tx.commit().await.map_err(retry)
}

/// Store what the export did to one part, as soon as it is done with it: its
/// state (mapping, thumbnail key, title, sticker count), or, when its pack
/// was deleted, forget the part and, if the pack existed (`retire`), mark
/// the name as deleted, so that it is never given to a part again. A set
/// deleted meanwhile is left alone.
///
/// `pending`: the base of a pending export ([`Source::pending`]), whose
/// parts are stored with it in `telegram_pending_parts`, the set's binding
/// untouched. A pack such an export deletes is gone for the set it is
/// bound to as well (the set whose name a takeover publishes under), so
/// that set forgets the part too.
async fn store_part(
    db: &SqlitePool,
    set_id: &str,
    pending: Option<&str>,
    title: &str,
    u: PartUpdate,
) -> Result<(), JobError> {
    let now = ids::now();
    let mut tx = db.begin().await.map_err(retry)?;
    let table = match pending {
        Some(_) => "telegram_pending_parts",
        None => "telegram_parts",
    };
    // The sets whose link may change.
    let mut relink: Vec<String> = Vec::new();
    if pending.is_none() {
        relink.push(set_id.to_string());
    }
    match &u.state {
        Some(st) => {
            let mapping = serde_json::to_string(&st.mapping).map_err(retry)?;
            let title = match stickpile_telegram::part_title(title, u.number) {
                t if t.is_empty() => u.name.clone(),
                t => t,
            };
            let (base_column, base_value) = match pending {
                Some(_) => (", base", ", ?9"),
                None => ("", ""),
            };
            let sql = format!(
                "INSERT INTO {table} \
                 (set_id, part, pack_name, title, mapping, thumbnail, stickers, created_at, updated_at{base_column}) \
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8{base_value} WHERE EXISTS (SELECT 1 FROM sets WHERE id = ?1) \
                 ON CONFLICT(set_id, part) DO UPDATE SET pack_name = excluded.pack_name, title = excluded.title, \
                 mapping = excluded.mapping, thumbnail = excluded.thumbnail, stickers = excluded.stickers, \
                 updated_at = excluded.updated_at"
            );
            let query = sqlx::query(&sql)
                .bind(set_id)
                .bind(i64::from(u.number))
                .bind(&u.name)
                .bind(title)
                .bind(mapping)
                .bind(st.thumbnail.as_deref())
                .bind(st.mapping.len() as i64)
                .bind(now);
            let query = match pending {
                Some(base) => query.bind(base),
                None => query,
            };
            query.execute(&mut *tx).await.map_err(retry)?;
        }
        None => {
            sqlx::query(&format!("DELETE FROM {table} WHERE set_id = ? AND part = ?"))
                .bind(set_id)
                .bind(i64::from(u.number))
                .execute(&mut *tx)
                .await
                .map_err(retry)?;
            if pending.is_some() {
                let bound: Vec<String> =
                    sqlx::query_scalar("DELETE FROM telegram_parts WHERE pack_name = ? RETURNING set_id")
                        .bind(&u.name)
                        .fetch_all(&mut *tx)
                        .await
                        .map_err(retry)?;
                relink.extend(bound);
            }
            if u.retire {
                sqlx::query("UPDATE telegram_packs SET deleted_at = ? WHERE name = ?")
                    .bind(now)
                    .bind(&u.name)
                    .execute(&mut *tx)
                    .await
                    .map_err(retry)?;
            }
        }
    }
    for id in &relink {
        sync_link(&mut tx, id).await.map_err(retry)?;
    }
    tx.commit().await.map_err(retry)
}

/// After a pending export has finished, in one transaction: its parts
/// become the set's, in place of the ones it had (those stay on Telegram as
/// they are, unbound), and `base` the name they derive from. A set whose
/// name it was (a takeover) loses its binding: its parts are this set's
/// now, or deleted. A set deleted meanwhile is left alone.
async fn finish_parts(db: &SqlitePool, set_id: &str, base: &str) -> Result<(), JobError> {
    let now = ids::now();
    let mut tx = db.begin().await.map_err(retry)?;
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM sets WHERE id = ?")
        .bind(set_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(retry)?;
    if exists.is_none() {
        return Ok(());
    }
    let holders: Vec<String> =
        sqlx::query_scalar("SELECT id FROM sets WHERE telegram_base = ? COLLATE NOCASE AND id <> ?")
            .bind(base)
            .bind(set_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(retry)?;
    for holder in &holders {
        sqlx::query("DELETE FROM telegram_parts WHERE set_id = ?")
            .bind(holder)
            .execute(&mut *tx)
            .await
            .map_err(retry)?;
        sqlx::query("UPDATE sets SET telegram_base = NULL, telegram_pack = NULL WHERE id = ?")
            .bind(holder)
            .execute(&mut *tx)
            .await
            .map_err(retry)?;
    }
    sqlx::query("DELETE FROM telegram_parts WHERE set_id = ?")
        .bind(set_id)
        .execute(&mut *tx)
        .await
        .map_err(retry)?;
    sqlx::query(
        "INSERT INTO telegram_parts (set_id, part, pack_name, title, mapping, thumbnail, stickers, created_at, \
         updated_at) SELECT set_id, part, pack_name, title, mapping, thumbnail, stickers, created_at, ?1 \
         FROM telegram_pending_parts WHERE set_id = ?2 AND base = ?3",
    )
    .bind(now)
    .bind(set_id)
    .bind(base)
    .execute(&mut *tx)
    .await
    .map_err(retry)?;
    sqlx::query("DELETE FROM telegram_pending_parts WHERE set_id = ?")
        .bind(set_id)
        .execute(&mut *tx)
        .await
        .map_err(retry)?;
    sqlx::query("UPDATE sets SET telegram_base = ? WHERE id = ?")
        .bind(base)
        .bind(set_id)
        .execute(&mut *tx)
        .await
        .map_err(retry)?;
    sync_link(&mut tx, set_id).await.map_err(retry)?;
    tx.commit().await.map_err(retry)
}

/// The part numbers of the parts named after `base` that must not be given
/// to a part again: those whose pack stickpile deleted (`deleted_at` on the
/// user's claims, so also after a rename or a takeover), up to every number
/// an export of `items` stickers with `previous` could give a new part.
async fn retired_numbers(
    db: &SqlitePool,
    user_id: &str,
    base: &str,
    bot: &str,
    previous: &PartStates,
    items: usize,
) -> AppResult<BTreeSet<u32>> {
    let deleted: HashSet<String> =
        sqlx::query_scalar::<_, String>("SELECT name FROM telegram_packs WHERE user_id = ? AND deleted_at IS NOT NULL")
            .bind(user_id)
            .fetch_all(db)
            .await?
            .into_iter()
            .map(|n| n.to_ascii_lowercase())
            .collect();
    let mut retired = BTreeSet::new();
    if deleted.is_empty() {
        return Ok(retired);
    }
    let highest = previous.keys().max().copied().unwrap_or(0) as usize;
    let limit = u32::try_from(highest + deleted.len() + items + 1).unwrap_or(u32::MAX);
    for number in 1..=limit {
        let Ok(name) = stickpile_telegram::part_pack_name(base, number, bot) else {
            break;
        };
        if deleted.contains(&name.to_ascii_lowercase()) {
            retired.insert(number);
        }
    }
    Ok(retired)
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// Queue the export of one of the caller's sets as a sticker pack the bot
/// owns (created on the first export, updated afterwards).
pub async fn export(state: &AppState, user: &CurrentUser, set_id: &str, req: TelegramExport) -> AppResult<Job> {
    let payload = prepare_export(state, user, set_id, req).await?;
    let set_id = payload["set_id"].as_str().unwrap_or(set_id).to_string();
    jobs::enqueue(state, JobKind::TelegramExport, payload, Some(set_id), Some(user.id())).await
}

/// Check an export of `set_id` by `user` and return the `TelegramExport`
/// job payload, without queuing it ([`export`], and the publish registry,
/// which checks every platform before it queues any).
pub async fn prepare_export(
    state: &AppState,
    user: &CurrentUser,
    set_id: &str,
    req: TelegramExport,
) -> AppResult<Value> {
    if !enabled(state) {
        return Err(disabled());
    }
    let set = authz::owned_set(state, user.id(), set_id).await?;
    let pack_name = match req.pack_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => Some(clean_pack_name(p)?),
        None => None,
    };
    let title = req.title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    if title
        .as_ref()
        .is_some_and(|t| t.chars().count() > 64 || t.chars().any(char::is_control))
    {
        return Err(AppError::bad_request("title must be at most 64 printable characters"));
    }
    let row = db::user_by_id(&state.db, user.id())
        .await?
        .ok_or_else(|| AppError::unauthorized("account no longer exists"))?;
    if owner_id(state, &row).is_none() {
        return Err(AppError::bad_request(NO_OWNER));
    }
    Ok(json!({ "set_id": set.id, "pack_name": pack_name, "title": title, "repack": req.repack }))
}

fn find_variant(rows: Option<&Vec<VariantRow>>, want: Variant) -> Option<VariantRow> {
    rows?.iter().find(|v| v.variant() == Some(want)).cloned()
}

/// The pack thumbnail for the set's cover (None: neither a cover image nor a
/// chosen item, Telegram shows the first sticker), made 100×100. A cover
/// image is rendered from its stored original (always a still), a chosen
/// item from its grid thumbnail, which every kind of item has (a still frame
/// for animations and videos). The key says what was sent: `image:<sha256>`
/// or `<item id>:<sha256>`, so a new image or item uploads it again.
async fn cover_thumbnail(
    state: &AppState,
    set: &db::SetRow,
    rows: &[hydrate::ItemFull],
    variants: &mut std::collections::HashMap<String, Vec<VariantRow>>,
) -> Result<Option<PackThumbnail>, JobError> {
    if let Some(media_id) = &set.cover_media_id {
        let missing = || JobError::Fatal("the set's cover image is missing".into());
        let media = db::media_by_id(&state.db, media_id)
            .await
            .map_err(retry)?
            .ok_or_else(missing)?;
        let original = cover::variant_row(&state.db, media_id, Variant::Original)
            .await?
            .ok_or_else(missing)?;
        return Ok(Some(PackThumbnail {
            key: format!("image:{}", media.sha256),
            png: render_thumbnail(state, &original.path).await?,
        }));
    }
    let Some(cover) = sets::chosen_cover(&state.db, &set.id).await? else {
        return Ok(None);
    };
    let Some(r) = rows.iter().find(|r| r.id == cover) else {
        return Ok(None);
    };
    let mut thumb = find_variant(variants.get(&r.media_id), Variant::Thumb);
    if thumb.is_none() {
        jobs::media::generate_variants(state, &r.media_id)
            .await
            .map_err(|e| JobError::Fatal(format!("rendering the cover: {e}")))?;
        let fresh = hydrate::variant_rows(&state.db, std::slice::from_ref(&r.media_id)).await?;
        variants.extend(fresh);
        thumb = find_variant(variants.get(&r.media_id), Variant::Thumb);
    }
    let Some(v) = thumb else {
        return Err(JobError::Fatal(
            "the cover has no thumbnail to make the pack's from".into(),
        ));
    };
    Ok(Some(PackThumbnail {
        key: format!("{}:{}", r.id, r.sha256),
        png: render_thumbnail(state, &v.path).await?,
    }))
}

/// A still media file (`path` under the media dir) as a pack thumbnail PNG.
async fn render_thumbnail(state: &AppState, path: &str) -> Result<Vec<u8>, JobError> {
    let abs = files::resolve(&state.media_dir(), path).ok_or_else(|| JobError::Fatal("bad path".into()))?;
    let bytes = files::read(abs).await?;
    let limits = state.media_limits();
    tokio::task::spawn_blocking(move || {
        let frame = stickpile_media::decode_frame(&bytes, &limits)?;
        stickpile_media::pack_thumbnail(&frame)
    })
    .await
    .map_err(retry)?
    .map_err(|e| JobError::Fatal(format!("rendering the cover: {e}")))
}

/// How [`export_items`] treats the set's items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemsFor {
    /// An export: render the stickers the `Variants` job has not made yet,
    /// read every file and fit stills into Telegram's 512 KB.
    Export,
    /// A plan: change nothing and read no file. An item whose sticker is
    /// not rendered yet counts as exportable while its renditions are still
    /// to be made (a still, or an animation or video with ffmpeg on the
    /// server); every still is taken to fit.
    Plan,
}

/// The set's items as Telegram stickers, in set order, and the items left
/// out (`{item_id, name, reason}`).
async fn export_items(
    state: &AppState,
    rows: &[hydrate::ItemFull],
    variants: &mut std::collections::HashMap<String, Vec<VariantRow>>,
    mode: ItemsFor,
) -> Result<(Vec<ExportItem>, Vec<Value>), JobError> {
    let mut generated: HashSet<String> = HashSet::new();
    let mut items = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    for r in rows {
        let (want, format) = match r.kind() {
            MediaKind::Still => (Variant::TgSticker, StickerFormat::Static),
            MediaKind::Animation | MediaKind::Video => (Variant::TgVideoSticker, StickerFormat::Video),
        };
        let item = |bytes| ExportItem {
            key: r.id.clone(),
            content_key: r.sha256.clone(),
            bytes,
            format,
            emojis: r.emojis(),
            keywords: r.keywords(),
        };
        let mut variant = find_variant(variants.get(&r.media_id), want);
        if mode == ItemsFor::Plan {
            let exportable = match variant {
                Some(_) => true,
                None => {
                    let pending = !db::media_by_id(&state.db, &r.media_id)
                        .await
                        .map_err(retry)?
                        .is_some_and(|m| m.variants_done);
                    pending && (format == StickerFormat::Static || state.ffmpeg_available)
                }
            };
            if exportable {
                items.push(item(Vec::new()));
            }
            continue;
        }
        let mut why: Option<String> = None;
        if variant.is_none() && generated.insert(r.media_id.clone()) {
            // The Variants job has not run (or failed): render it now.
            if let Err(e) = jobs::media::generate_variants(state, &r.media_id).await {
                why = Some(e.to_string());
            }
            let fresh = hydrate::variant_rows(&state.db, std::slice::from_ref(&r.media_id)).await?;
            variants.extend(fresh);
            variant = find_variant(variants.get(&r.media_id), want);
        }
        let Some(v) = variant else {
            let reason = why.unwrap_or_else(|| match want {
                Variant::TgSticker => "no Telegram sticker rendition could be made".to_string(),
                _ if !state.ffmpeg_available => "animated stickers need ffmpeg on the server".to_string(),
                _ => "cannot be converted to a Telegram video sticker (at most 3 s, 512 px, 256 KB)".to_string(),
            });
            skipped.push(json!({"item_id": r.id, "name": r.name, "reason": reason}));
            continue;
        };
        let abs = files::resolve(&state.media_dir(), &v.path).ok_or_else(|| JobError::Fatal("bad path".into()))?;
        let mut bytes = files::read(abs).await?;
        if format == StickerFormat::Static {
            // The stored sticker is lossless for chats; a pack takes at most
            // 512 KB, so a larger one is re-encoded lossy for the pack only.
            let fitted = tokio::task::spawn_blocking(move || stickpile_media::tg_pack_sticker(&bytes))
                .await
                .map_err(|e| JobError::Retry(e.to_string()))?;
            match fitted {
                Ok(Some(b)) => bytes = b,
                Ok(None) => {
                    skipped.push(json!({"item_id": r.id, "name": r.name,
                        "reason": "does not fit Telegram's 512 KB sticker limit"}));
                    continue;
                }
                Err(e) => {
                    skipped.push(json!({"item_id": r.id, "name": r.name, "reason": e.to_string()}));
                    continue;
                }
            }
        }
        items.push(item(bytes));
    }
    Ok((items, skipped))
}

/// The set's parts as previous states for the planner: what each part's
/// last export left on it (an empty state for a part registered without
/// one).
fn own_states(rows: &[PartRow]) -> PartStates {
    rows.iter().map(|r| (r.number(), r.state())).collect()
}

/// The pack name of every part `plan` touches.
fn plan_names(plan: &PartsPlan, base: &str, bot: &str) -> Result<BTreeMap<u32, String>, TelegramError> {
    plan.numbers()
        .map(|n| stickpile_telegram::part_pack_name(base, n, bot).map(|name| (n, name)))
        .collect()
}

/// `TelegramExport` job: the set as one pack, or as several, its parts,
/// when it has more stickers than one pack holds (docs/telegram.md, "Sets
/// larger than one pack").
///
/// Before any change the parts are planned ([`target`]), and every part's
/// name is checked and claimed ([`authorize_parts`]) and registered
/// ([`register_parts`]). Then `export_parts` exports them, and each part's
/// state is stored as soon as that part is done ([`store_part`]), as far
/// as it got when the export stops: a retried job continues from there,
/// with the same names, so it neither creates a second pack for a part nor
/// leaves one behind. An export under a new name (a rename or a takeover)
/// keeps its parts pending and the set's binding as it was until it has
/// finished ([`finish_parts`]).
pub async fn export_job(state: &AppState, job: &JobRow) -> JobResult {
    let p = job.payload();
    let set_id = p["set_id"]
        .as_str()
        .ok_or_else(|| JobError::Fatal("payload without set_id".into()))?
        .to_string();
    let repack = p["repack"].as_bool().unwrap_or(false);
    let lock = jobs::subject_lock(&format!("telegram:{set_id}"));
    let _guard = lock.lock().await;
    let Some(set) = db::set_by_id(&state.db, &set_id).await.map_err(retry)? else {
        return Ok(Some(json!({"skipped": "the set no longer exists"})));
    };
    let Some(user) = db::user_by_id(&state.db, &set.owner_id).await.map_err(retry)? else {
        return Ok(Some(json!({"skipped": "the owner no longer exists"})));
    };
    let owner = owner_id(state, &user).ok_or_else(|| JobError::Fatal(NO_OWNER.into()))?;
    let api = bot(state)?;
    let me = api.get_me().await.map_err(tg_error)?;
    let bot_name = me
        .username
        .ok_or_else(|| JobError::Fatal("the bot has no username (getMe)".into()))?;

    // Part 1's name and where the parts come from: the set's own, none (a
    // new name), or the user's other set published under this name, whose
    // parts move here once the export has finished.
    let Target { base, source, previous } = target(&state.db, &set, p["pack_name"].as_str(), &bot_name).await?;
    let title = p["title"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| set.name.clone());

    let rows = hydrate::fetch_by_set(&state.db, &set.id).await?;
    let media_ids: Vec<String> = rows.iter().map(|r| r.media_id.clone()).collect();
    let mut variants = hydrate::variant_rows(&state.db, &media_ids).await?;
    let (items, skipped) = export_items(state, &rows, &mut variants, ItemsFor::Export).await?;
    if items.is_empty() {
        return Err(JobError::Fatal(
            "nothing to export: no item of the set can become a Telegram sticker".into(),
        ));
    }
    let thumbnail = cover_thumbnail(state, &set, &rows, &mut variants).await?;

    let retired = retired_numbers(&state.db, &user.id, &base, &bot_name, &previous, items.len()).await?;
    let plan = stickpile_telegram::plan_parts_skipping(&items, &previous, &retired, MAX_PACK_STICKERS, repack)
        .map_err(tg_error)?;
    let names = plan_names(&plan, &base, &bot_name).map_err(tg_error)?;
    authorize_parts(state, &api, &user, &set.id, source.from(), &names).await?;
    register_parts(state, &set.id, &base, &source, &names).await?;

    let export = stickpile_telegram::PartsExport {
        owner_user_id: owner,
        bot_username: &bot_name,
        part1_name: &base,
        title: &title,
        items: &items,
        previous: &previous,
        thumbnail: thumbnail.as_ref(),
    };
    let pending = source.pending().then(|| base.clone());
    let persist = |u: PartUpdate| {
        let (db, set_id, title, pending) = (state.db.clone(), set.id.clone(), title.clone(), pending.clone());
        async move { store_part(&db, &set_id, pending.as_deref(), &title, u).await }
    };
    let several = names.len() > 1;
    let report = stickpile_telegram::export_parts(&api, &export, &plan, persist)
        .await
        .map_err(|e| parts_error(e, several))?;
    if source.pending() {
        finish_parts(&state.db, &set.id, &base).await?;
    }
    // The set's link: part 1, or the lowest part left.
    let main = plan
        .parts
        .first()
        .and_then(|p| report.part(p.number))
        .cloned()
        .unwrap_or_default();
    Ok(Some(json!({
        "set_id": set.id,
        "pack_name": main.name,
        "url": main.url,
        "title": main.title,
        "created_pack": main.created_pack,
        "added": main.added,
        "removed": main.removed,
        "updated": main.updated,
        "reordered": main.reordered,
        "thumbnail": main.thumbnail,
        "thumbnail_changed": main.thumbnail_changed,
        "exported": items.len(),
        "skipped": skipped,
        "parts": report.parts,
        "moved": report.moved,
    })))
}

/// A stopped export of parts as a job error: retried when Telegram's
/// answer says so (as a single pack's), and with the part it stopped at
/// when the set has several.
fn parts_error(e: PartsError<JobError>, several: bool) -> JobError {
    let at = e
        .report
        .parts
        .last()
        .filter(|p| several && p.error.is_some())
        .map(|p| format!("part {} ({}): ", p.number, p.name))
        .unwrap_or_default();
    match e.failure {
        PartsFailure::Telegram(te) => match tg_error(te) {
            JobError::Retry(m) => JobError::Retry(format!("{at}{m}")),
            JobError::Fatal(m) => JobError::Fatal(format!("{at}{m}")),
        },
        PartsFailure::Persist(je) => je,
    }
}

/// What an export of the caller's set `set_id` would do to its Telegram
/// packs, without changing anything: the parts after it with how many
/// stickers each would hold, the parts it would delete, and how many
/// published stickers would change pack (only a repack moves any). Owner
/// only. The plan is for the set's own packs (no new name), from its items
/// as they are rendered now ([`ItemsFor::Plan`]).
pub async fn plan(state: &AppState, user: &CurrentUser, set_id: &str, repack: bool) -> AppResult<TelegramPlan> {
    if !enabled(state) {
        return Err(disabled());
    }
    let set = authz::owned_set(state, user.id(), set_id).await?;
    let token = token(state).ok_or_else(disabled)?;
    let api = BotApi::new(&state.config.telegram.api_url, &token);
    let bot_name = api
        .get_me()
        .await
        .ok()
        .and_then(|me| me.username)
        .ok_or_else(|| AppError::unavailable("Telegram did not answer; try again later."))?;
    let Target { base, previous, .. } = target(&state.db, &set, None, &bot_name).await.map_err(|e| match e {
        JobError::Fatal(m) => AppError::bad_request(m),
        JobError::Retry(m) => AppError::internal(m),
    })?;
    let rows = hydrate::fetch_by_set(&state.db, &set.id).await?;
    let media_ids: Vec<String> = rows.iter().map(|r| r.media_id.clone()).collect();
    let mut variants = hydrate::variant_rows(&state.db, &media_ids).await?;
    let (items, _) = export_items(state, &rows, &mut variants, ItemsFor::Plan)
        .await
        .map_err(AppError::internal)?;
    if items.is_empty() {
        return Err(AppError::bad_request(
            "nothing to export: no item of the set can become a Telegram sticker",
        ));
    }
    let retired = retired_numbers(&state.db, &set.owner_id, &base, &bot_name, &previous, items.len()).await?;
    let plan = stickpile_telegram::plan_parts_skipping(&items, &previous, &retired, MAX_PACK_STICKERS, repack)
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    let names = plan_names(&plan, &base, &bot_name).map_err(|e| AppError::bad_request(e.to_string()))?;
    let part = |number: u32, stickers: Option<u32>| PublishedPart {
        number,
        name: names[&number].clone(),
        url: Some(pack_url(&names[&number])),
        stickers,
    };
    Ok(TelegramPlan {
        repack,
        parts: plan
            .parts
            .iter()
            .map(|p| part(p.number, u32::try_from(p.items.len()).ok()))
            .collect(),
        delete: plan.delete.iter().map(|&n| part(n, None)).collect(),
        moved: u32::try_from(plan.moved).unwrap_or(u32::MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_names_from_links() {
        assert_eq!(clean_pack_name("Animals").unwrap(), "Animals");
        assert_eq!(
            clean_pack_name(" https://t.me/addstickers/Cats_by_bot/ ").unwrap(),
            "Cats_by_bot"
        );
        assert_eq!(clean_pack_name("t.me/addstickers/Dogs").unwrap(), "Dogs");
        for bad in ["", "1abc", "a b", "a/b", "ä", &"a".repeat(65)] {
            assert!(clean_pack_name(bad).is_err(), "{bad}");
        }
    }
}
