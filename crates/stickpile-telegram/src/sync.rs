//! Pack-level import and export.

use std::collections::BTreeMap;

use crate::api::{BotApi, StickerFormat, TelegramError};

#[derive(Debug, Clone)]
pub struct ImportedSticker {
    pub file_unique_id: String,
    pub bytes: Vec<u8>,
    pub format: StickerFormat,
    pub emojis: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ImportedPack {
    pub name: String,
    pub title: String,
    /// In pack order.
    pub stickers: Vec<ImportedSticker>,
    /// Stickers that could not be imported (e.g. `.tgs`), with reasons.
    pub skipped: Vec<(String, String)>,
}

pub async fn import_pack(api: &BotApi, name: &str, max_file_bytes: u64) -> Result<ImportedPack, TelegramError> {
    let _ = (api, name, max_file_bytes);
    todo!("telegram agent")
}

/// One set item, already rendered as a Telegram sticker (512 px WebP or VP9 WebM).
#[derive(Debug, Clone)]
pub struct ExportItem {
    /// Stable stickpile key (item id) used for the mapping.
    pub key: String,
    /// Changes when the media changes (e.g. media sha256): re-upload if different.
    pub content_key: String,
    pub bytes: Vec<u8>,
    pub format: StickerFormat,
    pub emojis: Vec<String>,
    pub keywords: Vec<String>,
}

/// item key -> (content_key, Telegram file_unique_id). Persisted by the server
/// between exports.
pub type PackMapping = BTreeMap<String, (String, String)>;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportReport {
    pub created_pack: bool,
    pub added: u32,
    pub removed: u32,
    pub updated: u32,
    pub reordered: bool,
    /// New mapping to persist.
    pub mapping: PackMapping,
}

/// Make pack `name` (owned by `owner_user_id`) match `items` in order.
pub async fn export_pack(
    api: &BotApi,
    owner_user_id: i64,
    name: &str,
    title: &str,
    items: &[ExportItem],
    previous: &PackMapping,
) -> Result<ExportReport, TelegramError> {
    let _ = (api, owner_user_id, name, title, items, previous);
    todo!("telegram agent")
}

/// `^[A-Za-z][A-Za-z0-9_]*_by_<bot>$`, <= 64 chars, no `__`.
pub fn valid_pack_name(name: &str, bot_username: &str) -> bool {
    let _ = (name, bot_username);
    todo!("telegram agent")
}
