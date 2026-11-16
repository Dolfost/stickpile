//! Discord import and publishing through a bot (docs/discord.md), on top of
//! `stickpile-discord`.
//!
//! * [`import`] queues a `DiscordImport` job ([`import_job`]):
//!   `stickpile_discord::import_guild` downloads a server's stickers and/or
//!   emoji (capped at `limits.max_upload_mb`); a new set named after the
//!   server is created and each one goes through the normal ingest with its
//!   name, its tag emoji and keywords. Expressions the bot created seed the
//!   set's publish mapping, so a later publish to that server keeps them,
//!   but only for the user who owns the server's publishing here (or an
//!   admin adopting it) and only those no other set maps already.
//! * Publishing is the [`DiscordPlatform`] of the registry
//!   ([`crate::service::publish`]): options `guild_id` (required the first
//!   time, then remembered) and `kind` (`stickers` or `emojis`). The
//!   `DiscordExport` job ([`export_job`]) renders every item for the kind
//!   ([`crate::service::renditions`]) and runs
//!   `stickpile_discord::sync_guild` with the mapping stored for (set,
//!   server, kind) in `set_publications.state`, also when the sync fails
//!   part way.
//!
//! One bot serves every user and may edit everything it created, so the
//! first stickpile user who publishes to a server and kind owns it
//! (`discord_guilds`, migration 0013); others are refused.

use std::collections::{BTreeMap, HashSet};

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use stickpile_api::{DiscordExport, DiscordImport, Job, JobKind, Visibility};
use stickpile_discord::{
    BotToken, DiscordApi, DiscordError, ExportItem, ExpressionKind, GuildMapping, ImageFormat, ImportOptions,
};

use crate::auth::CurrentUser;
use crate::authz;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::i18n::{t, tf};
use crate::ids;
use crate::index;
use crate::jobs::{self, JobError, JobResult, JobRow};
use crate::service::hydrate;
use crate::service::publish::{self, BoxFuture, Platform, PlatformOption, PublicationRow};
use crate::service::renditions::{self, Kind, Rendered};
use crate::service::sets::{self, NewSet};
use crate::service::upload::{self, clean_list, stem_for, ItemInput, UploadFile};
use crate::state::AppState;

/// The registry's id, and the `set_publications.platform` of Discord rows.
pub const PLATFORM: &str = "discord";

const DISABLED: &str =
    "the Discord integration is disabled on this server (discord.enabled or the bot token is missing)";
const TAKEN: &str = "another stickpile user publishes this kind (stickers or emoji) to that Discord server; \
     choose another server or the other kind";

/// The integration is configured: `discord.enabled` and a token file that
/// reads as a token.
pub fn enabled(state: &AppState) -> bool {
    state.config.discord.enabled && token(state).is_ok()
}

/// The bot token. Errors name the file, never its content.
fn token(state: &AppState) -> Result<BotToken, DiscordError> {
    let path = state
        .config
        .discord
        .bot_token_file
        .as_ref()
        .ok_or_else(|| DiscordError::Invalid("discord.bot_token_file is not set".into()))?;
    BotToken::from_file(path)
}

fn disabled() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "not_found", DISABLED)
}

fn api(state: &AppState) -> Result<DiscordApi, JobError> {
    if !state.config.discord.enabled {
        return Err(JobError::Fatal(DISABLED.into()));
    }
    let token = token(state).map_err(|e| JobError::Fatal(e.to_string()))?;
    let c = &state.config.discord;
    Ok(DiscordApi::with_base(&c.api_url, token).with_cdn(&c.cdn_url, &c.media_url))
}

/// Opens the server in Discord.
pub fn guild_url(guild_id: &str) -> String {
    format!("https://discord.com/channels/{guild_id}")
}

/// A server id as people paste it: the number, or a
/// `discord.com/channels/<id>/...` link.
fn clean_guild_id(raw: &str) -> AppResult<String> {
    let s = raw.trim().trim_end_matches('/');
    let s = match s.split_once("/channels/") {
        Some((_, rest)) => rest.split('/').next().unwrap_or(""),
        None => s,
    };
    if (15..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()) {
        Ok(s.to_string())
    } else {
        Err(AppError::bad_request(
            "guild_id must be a Discord server id (a number like 100000000000000001) or a discord.com/channels/ link",
        ))
    }
}

fn parse_kind(raw: &str) -> AppResult<ExpressionKind> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "sticker" | "stickers" => Ok(ExpressionKind::Sticker),
        "emoji" | "emojis" => Ok(ExpressionKind::Emoji),
        _ => Err(AppError::bad_request("kind must be stickers or emojis")),
    }
}

/// `stickers` / `emojis`: how options and the export page name a kind.
fn kind_option(kind: ExpressionKind) -> &'static str {
    match kind {
        ExpressionKind::Sticker => "stickers",
        ExpressionKind::Emoji => "emojis",
    }
}

fn discord_error(e: DiscordError) -> JobError {
    match &e {
        DiscordError::Http(_) => JobError::Retry(e.to_string()),
        DiscordError::Api { status, .. } if *status == 429 || *status >= 500 => JobError::Retry(e.to_string()),
        DiscordError::Api { code: Some(10004), .. } => JobError::Fatal(
            "Discord does not know this server, or the bot is not a member of it (Unknown Guild)".into(),
        ),
        _ => JobError::Fatal(e.to_string()),
    }
}

fn retry(e: impl std::fmt::Display) -> JobError {
    JobError::Retry(e.to_string())
}

// ---------------------------------------------------------------------------
// What a set keeps about Discord (`set_publications.state`)
// ---------------------------------------------------------------------------

/// `set_publications.state` of a `discord` row.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Stored {
    /// The server and kind of the last publish: the defaults of the next.
    #[serde(default)]
    guild_id: Option<String>,
    #[serde(default)]
    kind: Option<ExpressionKind>,
    #[serde(default)]
    guild_name: Option<String>,
    /// `<guild id>:<sticker|emoji>` -> what stickpile created there for
    /// this set (item id -> (content key, Discord id)).
    #[serde(default)]
    mappings: BTreeMap<String, GuildMapping>,
}

fn mapping_key(guild_id: &str, kind: ExpressionKind) -> String {
    format!("{guild_id}:{}", kind.as_str())
}

fn stored_of(row: Option<&PublicationRow>) -> Stored {
    row.and_then(PublicationRow::state_json)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

async fn load(db: &SqlitePool, set_id: &str) -> AppResult<Stored> {
    Ok(stored_of(publish::stored(db, set_id, PLATFORM).await?.as_ref()))
}

async fn save(db: &SqlitePool, set_id: &str, stored: &Stored) -> Result<(), JobError> {
    let Some(guild) = stored.guild_id.as_deref() else {
        return Ok(());
    };
    let target = stored.guild_name.clone().unwrap_or_else(|| guild.to_string());
    let state = serde_json::to_value(stored).map_err(retry)?;
    publish::bind(
        db,
        set_id,
        PLATFORM,
        Some(&target),
        Some(&guild_url(guild)),
        Some(&state),
    )
    .await?;
    Ok(())
}

/// Discord ids of (`guild_id`, `kind`) that the mappings of sets other than
/// `except` hold.
async fn mapped_elsewhere(
    db: &SqlitePool,
    except: &str,
    guild_id: &str,
    kind: ExpressionKind,
) -> Result<HashSet<String>, JobError> {
    let rows: Vec<PublicationRow> = sqlx::query_as("SELECT * FROM set_publications WHERE platform = ? AND set_id <> ?")
        .bind(PLATFORM)
        .bind(except)
        .fetch_all(db)
        .await
        .map_err(retry)?;
    let key = mapping_key(guild_id, kind);
    Ok(rows
        .iter()
        .filter_map(|r| stored_of(Some(r)).mappings.remove(&key))
        .flat_map(|m| m.into_values().map(|(_, id)| id))
        .collect())
}

// ---------------------------------------------------------------------------
// Server ownership
// ---------------------------------------------------------------------------

/// The stickpile user who publishes `kind` to `guild_id`, if anyone does.
async fn guild_owner(db: &SqlitePool, guild_id: &str, kind: ExpressionKind) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT user_id FROM discord_guilds WHERE guild_id = ? AND kind = ?")
        .bind(guild_id)
        .bind(kind.as_str())
        .fetch_optional(db)
        .await
}

/// Record `user_id` as the one who publishes `kind` to `guild_id` unless
/// someone already is, and return who is afterwards (the first claim wins).
async fn claim_guild(db: &SqlitePool, guild_id: &str, kind: ExpressionKind, user_id: &str) -> Result<String, JobError> {
    sqlx::query(
        "INSERT INTO discord_guilds (guild_id, kind, user_id, created_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT(guild_id, kind) DO NOTHING",
    )
    .bind(guild_id)
    .bind(kind.as_str())
    .bind(user_id)
    .bind(ids::now())
    .execute(db)
    .await
    .map_err(retry)?;
    guild_owner(db, guild_id, kind)
        .await
        .map_err(retry)?
        .ok_or_else(|| JobError::Retry("the claim on the Discord server vanished".into()))
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Queue the import of a server's stickers and/or emoji into a new set of
/// the caller. The slug, when not given, comes from the server's name.
pub async fn import(state: &AppState, user: &CurrentUser, req: DiscordImport) -> AppResult<Job> {
    if !enabled(state) {
        return Err(disabled());
    }
    let guild = clean_guild_id(&req.guild_id)?;
    if !req.stickers && !req.emojis {
        return Err(AppError::bad_request("choose stickers, emojis or both to import"));
    }
    let slug = sets::requested_slug(&state.db, user.id(), req.slug.as_deref()).await?;
    state.limiters.check_upload(user.id())?;
    jobs::enqueue(
        state,
        JobKind::DiscordImport,
        json!({
            "guild_id": guild,
            "stickers": req.stickers,
            "emojis": req.emojis,
            "slug": slug,
            "visibility": req.visibility,
        }),
        Some(guild.clone()),
        Some(user.id()),
    )
    .await
}

fn extension(format: Option<ImageFormat>) -> &'static str {
    match format {
        Some(ImageFormat::Gif) => "gif",
        Some(ImageFormat::Jpeg) => "jpg",
        Some(ImageFormat::Webp) => "webp",
        _ => "png",
    }
}

/// `DiscordImport` job.
pub async fn import_job(state: &AppState, job: &JobRow) -> JobResult {
    let p = job.payload();
    let guild = p["guild_id"]
        .as_str()
        .ok_or_else(|| JobError::Fatal("payload without guild_id".into()))?
        .to_string();
    let options = ImportOptions {
        stickers: p["stickers"].as_bool().unwrap_or(true),
        emojis: p["emojis"].as_bool().unwrap_or(false),
        max_file_bytes: state.config.limits.max_upload_mb.saturating_mul(1024 * 1024),
    };
    let visibility: Visibility = serde_json::from_value(p["visibility"].clone()).unwrap_or_default();
    let Some(owner) = job.owner_id.as_deref() else {
        return Err(JobError::Fatal("job without owner".into()));
    };
    let Some(user) = db::user_by_id(&state.db, owner).await.map_err(retry)? else {
        return Ok(Some(json!({"skipped": "the user no longer exists"})));
    };
    let api = api(state)?;
    // Everything is downloaded before the set exists, so a retry after a
    // network error starts clean.
    let imported = stickpile_discord::import_guild(&api, &guild, &options)
        .await
        .map_err(discord_error)?;
    let set_name =
        sets::validate_name(&imported.guild_name).unwrap_or_else(|_| format!("Discord {}", imported.guild_id));
    let slug = match p["slug"].as_str() {
        Some(s) => s.to_string(),
        None => {
            let base = stickpile_setfmt::slugify(&imported.guild_name);
            sets::free_slug(&state.db, &user.id, if base.is_empty() { "discord" } else { &base }).await?
        }
    };
    let side = if options.stickers { 320 } else { 128 };
    let row = sets::insert(
        &state.db,
        &user.id,
        NewSet {
            slug,
            name: set_name,
            description: String::new(),
            visibility,
            aspect_ratio: Some((1, 1)),
            recommended_px: Some(side),
            version: 1,
            extra_toml: None,
        },
    )
    .await
    .map_err(|e| JobError::Fatal(e.message))?;

    let mut skipped: Vec<Value> = imported
        .skipped
        .iter()
        .map(|(kind, id, reason)| json!({"kind": kind.as_str(), "remote_id": id, "reason": reason}))
        .collect();
    // (kind, item id, content key, Discord id) of what the bot made.
    let mut bot_made: Vec<(ExpressionKind, String, String, String)> = Vec::new();
    let mut count = 0u32;
    for (i, e) in imported.items.into_iter().enumerate() {
        let name = match e.kind {
            ExpressionKind::Sticker => e.name.trim().to_string(),
            ExpressionKind::Emoji => e.name.replace('_', " ").trim().to_string(),
        };
        let name = if name.is_empty() {
            format!("{} {}", e.kind.as_str(), i + 1)
        } else {
            name
        };
        let input = ItemInput {
            stem: stem_for(&name),
            name,
            emojis: clean_list(&e.emojis, "emojis").unwrap_or_default(),
            keywords: clean_list(&e.keywords, "keywords").unwrap_or_default(),
            ..Default::default()
        };
        let file = UploadFile::from_bytes(
            format!("{:03}-{}.{}", i + 1, e.kind.as_str(), extension(e.format)),
            e.bytes,
        );
        match upload::add_to_set(state, &row, &file, &input, &row.owner_id).await {
            Ok(added) if added.already_in_set => skipped.push(json!({
                "kind": e.kind.as_str(),
                "remote_id": e.remote_id,
                "reason": "the same file as an earlier sticker or emoji",
            })),
            Ok(added) => {
                count += 1;
                if e.created_by_bot {
                    bot_made.push((e.kind, added.item_id, added.media.sha256.clone(), e.remote_id));
                }
            }
            Err(err) if err.status.is_server_error() => return Err(retry(err)),
            Err(err) => skipped.push(json!({"kind": e.kind.as_str(), "remote_id": e.remote_id, "reason": err.message})),
        }
    }
    index::refresh_set(state, &row.id).await?;

    // What the bot made can be published back without copies: for the user
    // who publishes there, or for an admin adopting it; never what another
    // set already maps.
    let mut stored = Stored {
        guild_id: Some(imported.guild_id.clone()),
        kind: None,
        guild_name: Some(imported.guild_name.clone()),
        mappings: BTreeMap::new(),
    };
    let mut bound = 0usize;
    for kind in [ExpressionKind::Sticker, ExpressionKind::Emoji] {
        let mine: Vec<&(ExpressionKind, String, String, String)> =
            bot_made.iter().filter(|(k, ..)| *k == kind).collect();
        if mine.is_empty() {
            continue;
        }
        let may = match guild_owner(&state.db, &guild, kind).await.map_err(retry)? {
            Some(o) => o == user.id,
            None => user.is_admin() && claim_guild(&state.db, &guild, kind, &user.id).await? == user.id,
        };
        if !may {
            continue;
        }
        let taken = mapped_elsewhere(&state.db, &row.id, &guild, kind).await?;
        let mapping: GuildMapping = mine
            .into_iter()
            .filter(|(.., id)| !taken.contains(id))
            .map(|(_, item, key, id)| (item.clone(), (key.clone(), id.clone())))
            .collect();
        if mapping.is_empty() {
            continue;
        }
        bound += mapping.len();
        stored.kind.get_or_insert(kind);
        stored.mappings.insert(mapping_key(&guild, kind), mapping);
    }
    if bound > 0 {
        save(&state.db, &row.id, &stored).await?;
    }
    Ok(Some(json!({
        "set_id": row.id,
        "slug": row.slug,
        "guild_id": imported.guild_id,
        "guild_name": imported.guild_name,
        "url": guild_url(&imported.guild_id),
        "imported": count,
        "skipped": skipped,
        "bound": bound,
    })))
}

// ---------------------------------------------------------------------------
// Publishing
// ---------------------------------------------------------------------------

/// Check a publish of `set_id` by `user` and return the `DiscordExport` job
/// payload: the server (given, else the one used last; required the first
/// time) and the kind (given, else the last one for that server, else
/// stickers).
pub async fn prepare_export(
    state: &AppState,
    user: &CurrentUser,
    set_id: &str,
    req: DiscordExport,
) -> AppResult<Value> {
    if !enabled(state) {
        return Err(disabled());
    }
    let set = authz::owned_set(state, user.id(), set_id).await?;
    let stored = load(&state.db, &set.id).await?;
    let guild = match req.guild_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(g) => clean_guild_id(g)?,
        None => stored.guild_id.clone().ok_or_else(|| {
            AppError::bad_request("guild_id is required the first time: the id of the Discord server to publish to")
        })?,
    };
    let kind = match req.kind.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(k) => parse_kind(k)?,
        None if stored.guild_id.as_deref() == Some(guild.as_str()) => stored.kind.unwrap_or(ExpressionKind::Sticker),
        None => ExpressionKind::Sticker,
    };
    if guild_owner(&state.db, &guild, kind)
        .await?
        .is_some_and(|o| o != user.id())
    {
        return Err(AppError::bad_request(TAKEN));
    }
    Ok(json!({ "set_id": set.id, "guild_id": guild, "kind": kind.as_str() }))
}

/// `DiscordExport` job.
pub async fn export_job(state: &AppState, job: &JobRow) -> JobResult {
    let p = job.payload();
    let set_id = p["set_id"]
        .as_str()
        .ok_or_else(|| JobError::Fatal("payload without set_id".into()))?
        .to_string();
    let guild = p["guild_id"]
        .as_str()
        .ok_or_else(|| JobError::Fatal("payload without guild_id".into()))?
        .to_string();
    let kind = parse_kind(p["kind"].as_str().unwrap_or("sticker")).map_err(|e| JobError::Fatal(e.message))?;
    let lock = jobs::subject_lock(&format!("discord:{set_id}"));
    let _guard = lock.lock().await;
    let Some(set) = db::set_by_id(&state.db, &set_id).await.map_err(retry)? else {
        return Ok(Some(json!({"skipped": "the set no longer exists"})));
    };
    if db::user_by_id(&state.db, &set.owner_id).await.map_err(retry)?.is_none() {
        return Ok(Some(json!({"skipped": "the owner no longer exists"})));
    }
    let api = api(state)?;
    if claim_guild(&state.db, &guild, kind, &set.owner_id).await? != set.owner_id {
        return Err(JobError::Fatal(TAKEN.into()));
    }

    let rkind = Kind::discord(kind);
    let rows = hydrate::fetch_by_set(&state.db, &set.id).await?;
    let media_ids: Vec<String> = rows.iter().map(|r| r.media_id.clone()).collect();
    let variants = hydrate::variant_rows(&state.db, &media_ids).await?;
    let mut items = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    let mut notes: Vec<Value> = Vec::new();
    for r in &rows {
        let original = renditions::original(state, &variants, r);
        match renditions::render(state, r, original, rkind).await? {
            Rendered::Made(m) => {
                if let Some(note) = m.note {
                    notes.push(json!({"item_id": r.id, "name": r.name, "note": note}));
                }
                items.push(ExportItem {
                    key: r.id.clone(),
                    content_key: r.sha256.clone(),
                    name: r.name.clone(),
                    description: String::new(),
                    emojis: r.emojis(),
                    bytes: m.bytes,
                });
            }
            Rendered::Skip(reason) => skipped.push(json!({"item_id": r.id, "name": r.name, "reason": reason})),
        }
    }
    if items.is_empty() {
        return Err(JobError::Fatal(match kind {
            ExpressionKind::Sticker => "nothing to publish: no item of the set can become a Discord sticker".into(),
            ExpressionKind::Emoji => "nothing to publish: no item of the set can become a Discord emoji".into(),
        }));
    }

    let mut stored = load(&state.db, &set.id).await?;
    let key = mapping_key(&guild, kind);
    let previous = stored.mappings.get(&key).cloned().unwrap_or_default();
    let (report, failure) = match stickpile_discord::sync_guild(&api, &guild, kind, &items, &previous).await {
        Ok(r) => (r, None),
        Err(e) => (*e.partial, Some(e.error)),
    };
    // A failed sync still keeps what it created, or the next one could not
    // clean it up; one that changed nothing leaves the binding alone.
    if failure.is_none() || report.mapping != previous {
        stored.mappings.insert(key, report.mapping.clone());
        stored.guild_id = Some(guild.clone());
        stored.kind = Some(kind);
        if !report.guild_name.is_empty() {
            stored.guild_name = Some(report.guild_name.clone());
        }
        save(&state.db, &set.id, &stored).await?;
    }
    if let Some(e) = failure {
        return Err(discord_error(e));
    }
    let names: std::collections::HashMap<&str, &str> = rows.iter().map(|r| (r.id.as_str(), r.name.as_str())).collect();
    for s in &report.skipped {
        skipped.push(json!({
            "item_id": s.key,
            "name": names.get(s.key.as_str()).copied().unwrap_or(s.name.as_str()),
            "reason": s.reason,
        }));
    }
    let exported = items.len().saturating_sub(report.skipped.len());
    Ok(Some(json!({
        "set_id": set.id,
        "guild_id": guild,
        "guild_name": report.guild_name,
        "kind": kind.as_str(),
        "url": report.url.clone().unwrap_or_else(|| guild_url(&guild)),
        "premium_tier": report.premium_tier,
        "added": report.added,
        "removed": report.removed,
        "updated": report.updated,
        "unchanged": report.unchanged,
        "exported": exported,
        "skipped": skipped,
        "notes": notes,
        "slots": report.slots,
        "animated_slots": report.animated_slots,
    })))
}

/// Discord in the publish registry: a server's stickers or emoji.
pub struct DiscordPlatform;

impl Platform for DiscordPlatform {
    fn id(&self) -> &'static str {
        PLATFORM
    }

    fn name(&self) -> &'static str {
        "Discord"
    }

    fn job_kind(&self) -> JobKind {
        JobKind::DiscordExport
    }

    fn configured(&self, state: &AppState) -> bool {
        enabled(state)
    }

    fn options(&self) -> Vec<PlatformOption> {
        vec![
            PlatformOption {
                key: "guild_id",
                label: t("Discord server id"),
                hint: Some(t("In Discord: Settings, Advanced, Developer Mode on; then right-click the server and Copy Server ID. Empty: the server used last time.")),
                max_chars: 200,
                choices: Vec::new(),
            },
            PlatformOption {
                key: "kind",
                label: t("Publish as"),
                hint: None,
                max_chars: 16,
                choices: vec![("stickers", t("Stickers")), ("emojis", t("Emoji"))],
            },
        ]
    }

    fn prepare<'a>(
        &'a self,
        state: &'a AppState,
        user: &'a CurrentUser,
        set_id: &'a str,
        options: &'a Value,
    ) -> BoxFuture<'a, AppResult<Value>> {
        Box::pin(async move {
            let req: DiscordExport = if options.is_null() {
                DiscordExport::default()
            } else {
                serde_json::from_value(options.clone()).map_err(|e| {
                    AppError::bad_request(format!(
                        "options.discord: {e} (guild_id and kind, both strings and optional after the first publish)"
                    ))
                })?
            };
            prepare_export(state, user, set_id, req).await
        })
    }

    fn summary(&self, r: &Value) -> String {
        let n = |k: &str| r.get(k).and_then(Value::as_u64).unwrap_or(0);
        let skipped = r.get("skipped").and_then(Value::as_array).map_or(0, Vec::len);
        let args: [(&str, &dyn std::fmt::Display); 4] = [
            ("exported", &n("exported")),
            ("added", &n("added")),
            ("removed", &n("removed")),
            ("skipped", &skipped),
        ];
        match r.get("kind").and_then(Value::as_str) {
            Some("emoji") => tf(
                "{exported} emoji: {added} added, {removed} removed, {skipped} left out",
                &args,
            ),
            _ => tf(
                "{exported} stickers: {added} added, {removed} removed, {skipped} left out",
                &args,
            ),
        }
    }

    fn notice(&self, _state: &AppState, _user: &CurrentUser) -> Option<&'static str> {
        Some(t(
            "The bot must be a member of the server with the Create Expressions permission. Only what stickpile created there is changed.",
        ))
    }

    fn option_values(&self, row: Option<&PublicationRow>) -> Vec<(&'static str, String)> {
        let stored = stored_of(row);
        let mut out = Vec::new();
        if let Some(g) = stored.guild_id {
            out.push(("guild_id", g));
        }
        if let Some(k) = stored.kind {
            out.push(("kind", kind_option(k).to_string()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_ids_from_links() {
        assert_eq!(clean_guild_id(" 100000000000000001 ").unwrap(), "100000000000000001");
        assert_eq!(
            clean_guild_id("https://discord.com/channels/100000000000000001/200000000000000002").unwrap(),
            "100000000000000001"
        );
        for bad in [
            "",
            "abc",
            "12345",
            "1000000000000000011111",
            "https://discord.com/channels/@me",
        ] {
            assert!(clean_guild_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn kinds() {
        assert_eq!(parse_kind("Stickers").unwrap(), ExpressionKind::Sticker);
        assert_eq!(parse_kind("emoji").unwrap(), ExpressionKind::Emoji);
        assert!(parse_kind("gifs").is_err());
        assert_eq!(kind_option(ExpressionKind::Emoji), "emojis");
    }

    #[test]
    fn summaries_by_kind() {
        let r = json!({"kind": "emoji", "exported": 4, "added": 4, "removed": 0, "skipped": [{}]});
        assert_eq!(DiscordPlatform.summary(&r), "4 emoji: 4 added, 0 removed, 1 left out");
        let r = json!({"kind": "sticker", "exported": 2, "added": 1, "removed": 1, "skipped": []});
        assert_eq!(
            DiscordPlatform.summary(&r),
            "2 stickers: 1 added, 1 removed, 0 left out"
        );
    }
}
