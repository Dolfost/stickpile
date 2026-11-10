//! Thin typed wrapper over the Bot API methods stickpile needs.

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum TelegramError {
    /// `ok: false` from the Bot API.
    #[error("Bot API error {code}: {description}")]
    Api { code: i64, description: String, retry_after: Option<u64> },
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BotUser {
    pub id: i64,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StickerFormat {
    Static,
    Animated,
    Video,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sticker {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub is_animated: bool,
    #[serde(default)]
    pub is_video: bool,
    pub emoji: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StickerSet {
    pub name: String,
    pub title: String,
    pub sticker_type: String,
    pub stickers: Vec<Sticker>,
}

/// A sticker to upload: raw bytes plus metadata.
#[derive(Debug, Clone)]
pub struct InputSticker {
    pub bytes: Vec<u8>,
    /// Filename sent in the multipart part, e.g. `sticker.webp`.
    pub filename: String,
    pub format: StickerFormat,
    /// 1..=20 emoji.
    pub emoji_list: Vec<String>,
    /// 0..=20 search keywords.
    pub keywords: Vec<String>,
}

#[derive(Clone)]
pub struct BotApi {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl std::fmt::Debug for BotApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BotApi").field("base", &self.base).finish_non_exhaustive()
    }
}

impl BotApi {
    /// `base` like `https://api.telegram.org` (no trailing slash needed).
    pub fn new(base: &str, token: &str) -> BotApi {
        BotApi { http: reqwest::Client::new(), base: base.trim_end_matches('/').to_string(), token: token.to_string() }
    }

    pub async fn get_me(&self) -> Result<BotUser, TelegramError> {
        todo!("telegram agent")
    }
    pub async fn get_sticker_set(&self, name: &str) -> Result<StickerSet, TelegramError> {
        let _ = name;
        todo!("telegram agent")
    }
    /// `getFile` then download from `<base>/file/bot<token>/<file_path>`, capped at `max_bytes`.
    pub async fn download_file(&self, file_id: &str, max_bytes: u64) -> Result<Vec<u8>, TelegramError> {
        let _ = (file_id, max_bytes);
        todo!("telegram agent")
    }
    pub async fn create_new_sticker_set(&self, user_id: i64, name: &str, title: &str, stickers: &[InputSticker]) -> Result<(), TelegramError> {
        let _ = (user_id, name, title, stickers);
        todo!("telegram agent")
    }
    pub async fn add_sticker_to_set(&self, user_id: i64, name: &str, sticker: &InputSticker) -> Result<(), TelegramError> {
        let _ = (user_id, name, sticker);
        todo!("telegram agent")
    }
    pub async fn delete_sticker_from_set(&self, file_id: &str) -> Result<(), TelegramError> {
        let _ = file_id;
        todo!("telegram agent")
    }
    pub async fn set_sticker_position_in_set(&self, file_id: &str, position: u32) -> Result<(), TelegramError> {
        let _ = (file_id, position);
        todo!("telegram agent")
    }
    pub async fn set_sticker_emoji_list(&self, file_id: &str, emoji_list: &[String]) -> Result<(), TelegramError> {
        let _ = (file_id, emoji_list);
        todo!("telegram agent")
    }
    pub async fn set_sticker_keywords(&self, file_id: &str, keywords: &[String]) -> Result<(), TelegramError> {
        let _ = (file_id, keywords);
        todo!("telegram agent")
    }
    pub async fn set_sticker_set_title(&self, name: &str, title: &str) -> Result<(), TelegramError> {
        let _ = (name, title);
        todo!("telegram agent")
    }
}
