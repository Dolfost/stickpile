//! Telegram Bot API, just the sticker parts (docs/telegram.md).
//!
//! * Import: `getStickerSet` + `getFile` + file download of every sticker.
//!   Static WebP/PNG and video (WebM) stickers are supported; animated
//!   `.tgs` (Lottie) stickers are a stretch goal and reported as skipped.
//! * Export: create or update a pack owned by a Telegram user so that it
//!   matches a stickpile set: add missing stickers, delete removed ones, update
//!   emoji lists/keywords, then fix positions. Pack names must end in
//!   `_by_<bot username>`.
//! * Sets larger than one pack (120 stickers) are exported as several packs,
//!   their parts ([`parts`]).
//!
//! The base URL is configurable so tests run against a mock Bot API.

pub mod api;
pub mod parts;
pub mod sync;

pub use api::{BotApi, BotUser, InputSticker, Sticker, StickerFormat, StickerSet, TelegramError, TgFile};
pub use parts::{
    export_parts, part_pack_name, part_title, plan_parts, PartReport, PartStates, PartUpdate, PartsError, PartsExport,
    PartsFailure, PartsPlan, PartsReport, PartsTotals, PlannedPart,
};
pub use sync::{
    complete_pack_name, default_pack_name, export_pack, export_pack_resumable, import_pack, is_emoji, valid_pack_name,
    ExportItem, ExportReport, ImportedPack, ImportedSticker, PackExportError, PackMapping, PackState, PackThumbnail,
    DEFAULT_EMOJI, MAX_PACK_STICKERS, TGS_SKIP_REASON,
};
