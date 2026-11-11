//! Telegram Bot API, just the sticker parts (docs/telegram.md).
//!
//! * Import: `getStickerSet` + `getFile` + file download of every sticker.
//!   Static WebP/PNG and video (WebM) stickers are supported; animated
//!   `.tgs` (Lottie) stickers are a stretch goal and reported as skipped.
//! * Export: create or update a pack owned by a Telegram user so that it
//!   matches a stickpile set: add missing stickers, delete removed ones, update
//!   emoji lists/keywords, then fix positions. Pack names must end in
//!   `_by_<bot username>`.
//!
//! The base URL is configurable so tests run against a mock Bot API.

pub mod api;
pub mod sync;

pub use api::{BotApi, BotUser, InputSticker, Sticker, StickerFormat, StickerSet, TelegramError, TgFile};
pub use sync::{
    complete_pack_name, default_pack_name, export_pack, import_pack, valid_pack_name, ExportItem, ExportReport,
    ImportedPack, ImportedSticker, PackMapping, DEFAULT_EMOJI, MAX_PACK_STICKERS, TGS_SKIP_REASON,
};
