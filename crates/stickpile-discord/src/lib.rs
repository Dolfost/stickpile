//! Discord guild stickers and emoji for stickpile (docs/discord.md).
//!
//! * Export: [`sync_guild`] makes the stickers or the emoji stickpile created
//!   in one guild match a set: add missing ones, delete removed or changed
//!   ones, update names and tags, respect the guild's slot limits. Only
//!   expressions recorded in the caller's [`GuildMapping`] are ever touched;
//!   guild expressions are shared with people.
//! * Import: [`import_guild`] downloads a guild's stickers and/or emoji from
//!   the CDN as item candidates.
//! * Files: [`requirements`] and [`check_file`] state what Discord accepts,
//!   so the server can render fitting files; [`names`] turns item names and
//!   emoji into Discord names and tags.
//!
//! The API and CDN bases are configurable so tests run against a mock.

pub mod api;
pub mod files;
pub mod image;
pub mod names;
pub mod sync;

pub use api::{
    BotToken, DiscordApi, DiscordError, Emoji, Guild, InputFile, NewSticker, Sticker, StickerChanges, StickerFormat,
    User, DEFAULT_API_BASE, DEFAULT_CDN_BASE, DEFAULT_MAX_RETRY_WAIT, DEFAULT_MEDIA_BASE, MAX_RETRIES,
};
pub use files::{check_file, requirements, Requirements, EMOJI_REQUIREMENTS, STICKER_REQUIREMENTS};
pub use image::{inspect, ImageFormat, ImageInfo};
pub use sync::{
    import_guild, sync_guild, ExportItem, ExpressionKind, GuildMapping, ImportOptions, ImportedExpression,
    ImportedGuild, SkippedItem, SlotUsage, SyncError, SyncReport, LOTTIE_SKIP_REASON,
};
