//! Guild-level export (sync) and import.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::api::{
    DiscordApi, DiscordError, Emoji, Guild, InputFile, NewSticker, Sticker, StickerChanges, StickerFormat,
};
use crate::files::check_file;
use crate::image::{inspect, ImageFormat};
use crate::names::{emoji_name, split_tags, sticker_description, sticker_name, sticker_tags, unique_emoji_name};

/// Guild stickers or guild emoji.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpressionKind {
    Sticker,
    Emoji,
}

impl ExpressionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ExpressionKind::Sticker => "sticker",
            ExpressionKind::Emoji => "emoji",
        }
    }
}

/// One set item, already rendered for `kind` (see [`crate::requirements`]).
#[derive(Clone)]
pub struct ExportItem {
    /// Stable stickpile key (the item id) used for the mapping.
    pub key: String,
    /// Changes when the media changes (e.g. the media sha256): the sticker or
    /// emoji is re-uploaded when it differs from the mapping's.
    pub content_key: String,
    /// The item's name: becomes the sticker name (2-30 characters) or the
    /// emoji name (2-32 of `A-Z a-z 0-9 _`) after sanitising.
    pub name: String,
    /// Sticker description (stickers only; empty is fine).
    pub description: String,
    /// The item's emoji; the first one becomes the sticker's tag.
    pub emojis: Vec<String>,
    /// The rendered file (sticker: PNG/APNG/GIF, at most 320x320 and 512
    /// KiB; emoji: PNG/GIF/JPEG/WebP, at most 256 KiB).
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for ExportItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportItem")
            .field("key", &self.key)
            .field("content_key", &self.content_key)
            .field("name", &self.name)
            .field("emojis", &self.emojis)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// item key -> (content_key, Discord sticker or emoji id): what stickpile
/// created in the guild. Persisted by the server per (set, guild, kind) and
/// passed to the next sync. Only ids in here are ever modified or deleted.
pub type GuildMapping = BTreeMap<String, (String, String)>;

/// An item that is not (or no longer) in the guild, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedItem {
    pub key: String,
    pub name: String,
    pub reason: String,
}

/// Slots of one kind after the sync.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotUsage {
    pub used: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncReport {
    pub kind: ExpressionKind,
    pub guild_id: String,
    pub guild_name: String,
    /// Server Boost level of the guild.
    pub premium_tier: u8,
    pub added: u32,
    pub removed: u32,
    /// Kept items whose name, description or tags were changed.
    pub updated: u32,
    /// Kept items that needed no change.
    pub unchanged: u32,
    pub skipped: Vec<SkippedItem>,
    /// Stickers, or static emoji.
    pub slots: SlotUsage,
    /// Animated emoji (emoji sync only).
    pub animated_slots: Option<SlotUsage>,
    /// New mapping to persist.
    pub mapping: GuildMapping,
    /// Opens the guild in Discord.
    pub url: Option<String>,
}

impl SyncReport {
    fn new(kind: ExpressionKind, guild_id: &str, mapping: GuildMapping) -> SyncReport {
        SyncReport {
            kind,
            guild_id: guild_id.to_string(),
            guild_name: String::new(),
            premium_tier: 0,
            added: 0,
            removed: 0,
            updated: 0,
            unchanged: 0,
            skipped: Vec::new(),
            slots: SlotUsage::default(),
            animated_slots: None,
            mapping,
            url: Some(format!("https://discord.com/channels/{guild_id}")),
        }
    }
}

/// A sync that stopped part way. `partial.mapping` is what stickpile knows
/// it has in the guild at that point: persist it like a successful report's
/// mapping, or the next sync cannot clean up what this one created.
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct SyncError {
    pub error: DiscordError,
    pub partial: Box<SyncReport>,
}

fn fail(error: DiscordError, report: SyncReport) -> SyncError {
    SyncError {
        error,
        partial: Box::new(report),
    }
}

/// A remote expression, as far as the sync cares.
struct Remote {
    id: String,
    name: String,
    description: String,
    tags: String,
    animated: bool,
}

impl From<&Sticker> for Remote {
    fn from(s: &Sticker) -> Remote {
        Remote {
            id: s.id.clone(),
            name: s.name.clone(),
            description: s.description.clone().unwrap_or_default(),
            tags: s.tags.clone(),
            animated: false,
        }
    }
}

impl From<&Emoji> for Remote {
    fn from(e: &Emoji) -> Remote {
        Remote {
            id: e.id.clone(),
            name: e.name.clone(),
            description: String::new(),
            tags: String::new(),
            animated: e.animated,
        }
    }
}

/// What an exported item should look like in the guild.
struct Wanted<'a> {
    item: &'a ExportItem,
    name: String,
    description: String,
    tags: String,
    format: ImageFormat,
    animated: bool,
}

/// Slot index: 0 for stickers and static emoji, 1 for animated emoji.
fn slot_of(kind: ExpressionKind, animated: bool) -> usize {
    usize::from(kind == ExpressionKind::Emoji && animated)
}

/// Discord's "maximum number reached" codes: 30008 emoji, 30018 animated
/// emoji, 30039 stickers (developer docs, topics/opcodes-and-status-codes).
fn is_slot_error(e: &DiscordError) -> bool {
    matches!(e.code(), Some(30008 | 30018 | 30039))
}

fn no_slot_reason(kind: ExpressionKind, animated: bool, limit: usize, tier: u8) -> String {
    let what = match (kind, animated) {
        (ExpressionKind::Sticker, _) => "sticker",
        (ExpressionKind::Emoji, false) => "emoji",
        (ExpressionKind::Emoji, true) => "animated emoji",
    };
    format!("no free {what} slot: the server has all {limit} in use (Server Boost level {tier})")
}

/// Makes the guild's stickers or emoji (`kind`) that stickpile created match
/// `items`.
///
/// * Only expressions whose id is in `previous` are modified or deleted.
///   Everything else in the guild (made by people, other bots, or another
///   set) is left alone but counts against the slots.
/// * An item whose mapping entry is still in the guild with the same
///   `content_key` is kept; its name, description and tags are updated when
///   they differ. A changed `content_key` deletes the old one and uploads
///   the new file (Discord cannot replace a file).
/// * Mapped expressions whose item is gone (or not exported this time) are
///   deleted; deletions run first to free slots.
/// * New items are created in `items` order while slots are free (stickers:
///   5/15/30/60 by boost level; emoji: 50/100/150/250, static and animated
///   counted separately). Items beyond that are skipped with a reason.
/// * Items whose file breaks the rules ([`crate::check_file`]), duplicates
///   of an earlier item's `content_key`, and items Discord refuses (HTTP
///   400, e.g. a file it cannot read) are skipped with a reason; the sync
///   goes on.
/// * Emoji names are made unique within the guild (`cat`, `cat_2`, ...).
///
/// Duplicate item keys are an error before any call. Any other failure
/// (network, permissions, an unknown guild) stops the sync and returns a
/// [`SyncError`] whose `partial` report holds the mapping as it stands.
pub async fn sync_guild(
    api: &DiscordApi,
    guild_id: &str,
    kind: ExpressionKind,
    items: &[ExportItem],
    previous: &GuildMapping,
) -> Result<SyncReport, SyncError> {
    let mut report = SyncReport::new(kind, guild_id, previous.clone());
    let mut keys = HashSet::new();
    for it in items {
        if !keys.insert(it.key.as_str()) {
            return Err(fail(
                DiscordError::Invalid(format!("duplicate item key {:?}", it.key)),
                report,
            ));
        }
    }

    let guild = match api.guild(guild_id).await {
        Ok(g) => g,
        Err(e) => return Err(fail(e, report)),
    };
    report.guild_name = guild.name.clone();
    report.premium_tier = guild.premium_tier;
    let remote: Vec<Remote> = match kind {
        ExpressionKind::Sticker => match api.list_stickers(guild_id).await {
            Ok(list) => list.iter().map(Remote::from).collect(),
            Err(e) => return Err(fail(e, report)),
        },
        ExpressionKind::Emoji => match api.list_emojis(guild_id).await {
            Ok(list) => list.iter().map(Remote::from).collect(),
            Err(e) => return Err(fail(e, report)),
        },
    };
    let by_id: HashMap<&str, &Remote> = remote.iter().map(|r| (r.id.as_str(), r)).collect();

    // Mapping entries whose expression was deleted by hand are forgotten.
    report.mapping.retain(|_, (_, id)| by_id.contains_key(id.as_str()));
    let managed: HashSet<String> = report.mapping.values().map(|(_, id)| id.clone()).collect();

    // What each item should become; unusable items are skipped.
    let mut wanted: Vec<Wanted<'_>> = Vec::new();
    let mut seen_content: HashMap<&str, &str> = HashMap::new();
    let mut emoji_names_taken: HashSet<String> = remote
        .iter()
        .filter(|r| !managed.contains(&r.id))
        .map(|r| r.name.to_ascii_lowercase())
        .collect();
    for (index, it) in items.iter().enumerate() {
        let skip = |reason: String| SkippedItem {
            key: it.key.clone(),
            name: it.name.clone(),
            reason,
        };
        if let Some(first) = seen_content.get(it.content_key.as_str()) {
            report
                .skipped
                .push(skip(format!("the same picture as {first:?}, exported once")));
            continue;
        }
        let info = match check_file(kind, &it.bytes) {
            Ok(info) => info,
            Err(reason) => {
                report.skipped.push(skip(reason));
                continue;
            }
        };
        seen_content.insert(it.content_key.as_str(), it.name.as_str());
        let fallback = format!("{} {}", kind.as_str(), index + 1);
        let (name, description, tags) = match kind {
            ExpressionKind::Sticker => (
                sticker_name(&it.name, &fallback),
                sticker_description(&it.description),
                sticker_tags(&it.emojis),
            ),
            ExpressionKind::Emoji => {
                let base = emoji_name(&it.name, &fallback);
                (
                    unique_emoji_name(&base, &mut emoji_names_taken),
                    String::new(),
                    String::new(),
                )
            }
        };
        wanted.push(Wanted {
            item: it,
            name,
            description,
            tags,
            format: info.format,
            animated: info.animated,
        });
    }

    // Keep, replace or add.
    let mut kept: Vec<(&Wanted<'_>, &Remote)> = Vec::new();
    let mut to_add: Vec<&Wanted<'_>> = Vec::new();
    let mut keep_ids: HashSet<&str> = HashSet::new();
    let mut kept_keys: HashSet<&str> = HashSet::new();
    for w in &wanted {
        let current = report
            .mapping
            .get(&w.item.key)
            .filter(|(content, _)| *content == w.item.content_key)
            .and_then(|(_, id)| by_id.get(id.as_str()));
        match current {
            // An emoji whose animation changed kind would sit in the wrong
            // slot list; Discord decides that from the file, so re-upload.
            // One expression serves one item, even if a damaged mapping
            // names it twice.
            Some(r)
                if (r.animated == w.animated || kind == ExpressionKind::Sticker)
                    && !keep_ids.contains(r.id.as_str()) =>
            {
                keep_ids.insert(r.id.as_str());
                kept_keys.insert(w.item.key.as_str());
                kept.push((w, r));
            }
            _ => to_add.push(w),
        }
    }

    // A mapping entry whose expression another item keeps is dropped
    // without touching the expression.
    report
        .mapping
        .retain(|key, (_, id)| kept_keys.contains(key.as_str()) || !keep_ids.contains(id.as_str()));
    // Deletions first: mapped expressions that are not kept.
    let doomed: Vec<(String, String)> = report
        .mapping
        .iter()
        .filter(|(key, _)| !kept_keys.contains(key.as_str()))
        .map(|(key, (_, id))| (key.clone(), id.clone()))
        .collect();
    let mut deleted: HashSet<String> = HashSet::new();
    for (key, id) in doomed {
        let result = match kind {
            ExpressionKind::Sticker => api.delete_sticker(guild_id, &id).await,
            ExpressionKind::Emoji => api.delete_emoji(guild_id, &id).await,
        };
        match result {
            Ok(()) => {}
            // Already gone.
            Err(DiscordError::Api { status: 404, .. }) => {}
            Err(e) => return Err(fail(e, report)),
        }
        report.mapping.remove(&key);
        report.removed += 1;
        deleted.insert(id);
    }

    // Updates of kept expressions.
    for (w, r) in &kept {
        let result = match kind {
            ExpressionKind::Sticker => {
                let changes = StickerChanges {
                    name: (r.name != w.name).then(|| w.name.clone()),
                    description: (r.description != w.description).then(|| w.description.clone()),
                    tags: (r.tags != w.tags).then(|| w.tags.clone()),
                };
                if changes.name.is_none() && changes.description.is_none() && changes.tags.is_none() {
                    None
                } else {
                    Some(api.modify_sticker(guild_id, &r.id, &changes).await.map(|_| ()))
                }
            }
            ExpressionKind::Emoji if r.name != w.name => {
                Some(api.modify_emoji(guild_id, &r.id, &w.name).await.map(|_| ()))
            }
            ExpressionKind::Emoji => None,
        };
        match result {
            None => report.unchanged += 1,
            Some(Ok(())) => report.updated += 1,
            Some(Err(e)) if e.is_rejection() => {
                report.unchanged += 1;
                report.skipped.push(SkippedItem {
                    key: w.item.key.clone(),
                    name: w.item.name.clone(),
                    reason: format!("kept, but its name or tags were not updated: {e}"),
                });
            }
            Some(Err(e)) => return Err(fail(e, report)),
        }
    }

    // Slots left after the deletions.
    let limit = match kind {
        ExpressionKind::Sticker => guild.sticker_slots(),
        ExpressionKind::Emoji => guild.emoji_slots(),
    };
    let mut used = [0usize; 2];
    for r in remote.iter().filter(|r| !deleted.contains(&r.id)) {
        used[slot_of(kind, r.animated)] += 1;
    }
    let mut full = [false; 2];

    for w in to_add {
        let slot = slot_of(kind, w.animated);
        let skip = |reason: String| SkippedItem {
            key: w.item.key.clone(),
            name: w.item.name.clone(),
            reason,
        };
        if full[slot] || used[slot] >= limit {
            report
                .skipped
                .push(skip(no_slot_reason(kind, w.animated, limit, guild.premium_tier)));
            continue;
        }
        let file = InputFile {
            bytes: w.item.bytes.clone(),
            format: w.format,
        };
        let created = match kind {
            ExpressionKind::Sticker => {
                let new = NewSticker {
                    name: w.name.clone(),
                    description: w.description.clone(),
                    tags: w.tags.clone(),
                    file,
                };
                api.create_sticker(guild_id, &new).await.map(|s| s.id)
            }
            ExpressionKind::Emoji => api.create_emoji(guild_id, &w.name, &file).await.map(|e| e.id),
        };
        match created {
            Ok(id) => {
                report
                    .mapping
                    .insert(w.item.key.clone(), (w.item.content_key.clone(), id));
                report.added += 1;
                used[slot] += 1;
            }
            Err(e) if is_slot_error(&e) => {
                full[slot] = true;
                report
                    .skipped
                    .push(skip(no_slot_reason(kind, w.animated, limit, guild.premium_tier)));
            }
            Err(e) if e.is_rejection() => report.skipped.push(skip(format!("Discord refused it: {e}"))),
            Err(e) => return Err(fail(e, report)),
        }
    }

    report.slots = SlotUsage { used: used[0], limit };
    if kind == ExpressionKind::Emoji {
        report.animated_slots = Some(SlotUsage { used: used[1], limit });
    }
    Ok(report)
}

/// What to import from a guild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportOptions {
    pub stickers: bool,
    pub emojis: bool,
    /// Files larger than this are skipped.
    pub max_file_bytes: u64,
}

/// One downloaded sticker or emoji: an item candidate for the server.
#[derive(Clone, PartialEq)]
pub struct ImportedExpression {
    pub kind: ExpressionKind,
    /// The Discord id: seeds the export mapping when `created_by_bot`.
    pub remote_id: String,
    pub name: String,
    /// Sticker description.
    pub description: Option<String>,
    /// Emoji found in a sticker's tags.
    pub emojis: Vec<String>,
    /// Other words in a sticker's tags.
    pub keywords: Vec<String>,
    pub animated: bool,
    /// Sniffed from the bytes (None: not a format stickpile reads).
    pub format: Option<ImageFormat>,
    pub bytes: Vec<u8>,
    /// Uploaded by this bot. Only these should seed the export mapping, so
    /// that a later export never touches expressions people made.
    pub created_by_bot: bool,
}

impl std::fmt::Debug for ImportedExpression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportedExpression")
            .field("kind", &self.kind)
            .field("remote_id", &self.remote_id)
            .field("name", &self.name)
            .field("animated", &self.animated)
            .field("format", &self.format)
            .field("bytes", &self.bytes.len())
            .field("created_by_bot", &self.created_by_bot)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedGuild {
    pub guild_id: String,
    pub guild_name: String,
    /// Stickers first, then emoji, each in the order Discord lists them.
    pub items: Vec<ImportedExpression>,
    /// (kind, Discord id, reason) of expressions that were not imported.
    pub skipped: Vec<(ExpressionKind, String, String)>,
}

/// Reason given for Lottie stickers.
pub const LOTTIE_SKIP_REASON: &str = "Lottie stickers are not supported";

/// Downloads a guild's stickers and/or emoji from the CDN.
///
/// Lottie stickers, emoji managed by an integration, files over
/// `max_file_bytes` and files that are not PNG, GIF, JPEG or WebP are
/// listed in `skipped`. APNG stickers arrive as APNG; stickpile's ingest
/// keeps only their first frame (docs/media.md).
pub async fn import_guild(
    api: &DiscordApi,
    guild_id: &str,
    options: &ImportOptions,
) -> Result<ImportedGuild, DiscordError> {
    let guild: Guild = api.guild(guild_id).await?;
    let me = api.current_user().await?;
    let by_bot = |user: &Option<crate::api::User>| user.as_ref().is_some_and(|u| u.id == me.id);
    let mut out = ImportedGuild {
        guild_id: guild.id.clone(),
        guild_name: guild.name.clone(),
        items: Vec::new(),
        skipped: Vec::new(),
    };
    if options.stickers {
        for s in api.list_stickers(guild_id).await? {
            if s.format() == Some(StickerFormat::Lottie) {
                out.skipped
                    .push((ExpressionKind::Sticker, s.id.clone(), LOTTIE_SKIP_REASON.to_string()));
                continue;
            }
            let url = api.sticker_url(&s)?;
            let bytes = match api.download(&url, options.max_file_bytes).await {
                Ok(b) => b,
                Err(DiscordError::TooLarge(cap)) => {
                    out.skipped.push((
                        ExpressionKind::Sticker,
                        s.id.clone(),
                        format!("file is larger than {cap} bytes"),
                    ));
                    continue;
                }
                Err(e) => return Err(e),
            };
            let info = inspect(&bytes);
            let (emojis, keywords) = split_tags(&s.tags);
            out.items.push(ImportedExpression {
                kind: ExpressionKind::Sticker,
                remote_id: s.id.clone(),
                name: s.name.clone(),
                description: s.description.clone().filter(|d| !d.is_empty()),
                emojis,
                keywords,
                animated: info.is_some_and(|i| i.animated),
                format: info.map(|i| i.format),
                bytes,
                created_by_bot: by_bot(&s.user),
            });
        }
    }
    if options.emojis {
        for e in api.list_emojis(guild_id).await? {
            if e.managed {
                out.skipped.push((
                    ExpressionKind::Emoji,
                    e.id.clone(),
                    "managed by an integration".to_string(),
                ));
                continue;
            }
            let mut result = Err(DiscordError::Invalid("no URL".into()));
            for url in api.emoji_urls(&e)? {
                result = api.download(&url, options.max_file_bytes).await;
                if !matches!(result, Err(DiscordError::Api { .. })) {
                    break;
                }
            }
            let bytes = match result {
                Ok(b) => b,
                Err(DiscordError::TooLarge(cap)) => {
                    out.skipped.push((
                        ExpressionKind::Emoji,
                        e.id.clone(),
                        format!("file is larger than {cap} bytes"),
                    ));
                    continue;
                }
                Err(err) => return Err(err),
            };
            let info = inspect(&bytes);
            out.items.push(ImportedExpression {
                kind: ExpressionKind::Emoji,
                remote_id: e.id.clone(),
                name: e.name.clone(),
                description: None,
                emojis: Vec::new(),
                keywords: Vec::new(),
                animated: e.animated,
                format: info.map(|i| i.format),
                bytes,
                created_by_bot: by_bot(&e.user),
            });
        }
    }
    let (items, unreadable): (Vec<_>, Vec<_>) = out.items.into_iter().partition(|it| it.format.is_some());
    out.items = items;
    for it in unreadable {
        out.skipped
            .push((it.kind, it.remote_id, "not a PNG, GIF, JPEG or WebP file".to_string()));
    }
    Ok(out)
}
