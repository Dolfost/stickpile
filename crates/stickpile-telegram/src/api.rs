//! Thin typed wrapper over the Bot API methods stickpile needs.
//!
//! Requests without files are sent as JSON, uploads as `multipart/form-data`
//! (array/object parameters JSON-encoded, files attached as parts referenced
//! by `attach://<part>`). Responses are `{ok, result}` or
//! `{ok: false, error_code, description, parameters: {retry_after}}`; a 429
//! is retried up to [`MAX_RETRIES`] times after sleeping `retry_after`
//! (capped by [`BotApi::with_max_retry_wait`]).
//!
//! The bot token is part of every URL. It never appears in errors (reqwest
//! errors are stripped of their URL), in `Debug` output or in logs.

use std::time::Duration;

use reqwest::multipart::{Form, Part};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// How often a rate-limited (429) call is retried.
pub const MAX_RETRIES: u32 = 3;
/// Default cap for a single `retry_after` sleep.
pub const DEFAULT_MAX_RETRY_WAIT: Duration = Duration::from_secs(30);
/// Bot API JSON responses larger than this are rejected.
const MAX_RESPONSE_BYTES: u64 = 16 << 20;

#[derive(Debug, thiserror::Error)]
pub enum TelegramError {
    /// `ok: false` from the Bot API.
    #[error("Bot API error {code}: {description}")]
    Api {
        code: i64,
        description: String,
        retry_after: Option<u64>,
    },
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Invalid(String),
}

impl TelegramError {
    /// True for the "no such sticker set" answer of `getStickerSet`.
    pub fn is_missing_sticker_set(&self) -> bool {
        match self {
            TelegramError::Api {
                code: 400, description, ..
            } => {
                let d = description.to_ascii_uppercase();
                d.contains("STICKERSET_INVALID") || d.contains("STICKERSET_NOT_FOUND")
            }
            _ => false,
        }
    }
}

/// reqwest errors carry the request URL, which contains the token.
fn http_err(e: reqwest::Error) -> TelegramError {
    TelegramError::Http(e.without_url())
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

impl StickerFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            StickerFormat::Static => "static",
            StickerFormat::Animated => "animated",
            StickerFormat::Video => "video",
        }
    }
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

/// Bot API `File` (result of `getFile`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TgFile {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_size: Option<u64>,
    /// Absent for files the Bot API will not serve (over 20 MB).
    pub file_path: Option<String>,
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

impl InputSticker {
    fn mime(&self) -> &'static str {
        match self.format {
            StickerFormat::Video => "video/webm",
            StickerFormat::Animated => "application/x-tgsticker",
            StickerFormat::Static if self.filename.to_ascii_lowercase().ends_with(".png") => "image/png",
            StickerFormat::Static => "image/webp",
        }
    }

    /// The InputSticker JSON object, with the file attached as part `part`.
    fn to_json(&self, part: &str) -> Value {
        let mut v = json!({
            "sticker": format!("attach://{part}"),
            "format": self.format.as_str(),
            "emoji_list": self.emoji_list,
        });
        if !self.keywords.is_empty() {
            v["keywords"] = json!(self.keywords);
        }
        v
    }
}

#[derive(Deserialize)]
struct ApiResponse<T> {
    ok: bool,
    result: Option<T>,
    error_code: Option<i64>,
    description: Option<String>,
    parameters: Option<ResponseParameters>,
}

#[derive(Deserialize)]
struct ResponseParameters {
    retry_after: Option<u64>,
}

/// A file part of a multipart request.
struct Upload<'a> {
    field: String,
    sticker: &'a InputSticker,
}

/// Internal download failure: "too large" is reported separately so import
/// can skip a sticker instead of failing the pack.
pub(crate) enum DownloadError {
    TooLarge,
    Other(TelegramError),
}

impl From<TelegramError> for DownloadError {
    fn from(e: TelegramError) -> Self {
        DownloadError::Other(e)
    }
}

#[derive(Clone)]
pub struct BotApi {
    http: reqwest::Client,
    base: String,
    token: String,
    max_retry_wait: Duration,
}

impl std::fmt::Debug for BotApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BotApi")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl BotApi {
    /// `base` like `https://api.telegram.org` (no trailing slash needed).
    pub fn new(base: &str, token: &str) -> BotApi {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .user_agent(concat!("stickpile/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        BotApi {
            http,
            base: base.trim_end_matches('/').to_string(),
            token: token.trim().to_string(),
            max_retry_wait: DEFAULT_MAX_RETRY_WAIT,
        }
    }

    /// Cap for a single 429 `retry_after` sleep (default 30 s).
    pub fn with_max_retry_wait(mut self, wait: Duration) -> BotApi {
        self.max_retry_wait = wait;
        self
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: &[(&str, Value)],
        files: &[Upload<'_>],
    ) -> Result<T, TelegramError> {
        let url = format!("{}/bot{}/{}", self.base, self.token, method);
        let mut retries = 0;
        loop {
            let request = self.http.post(&url);
            let request = if files.is_empty() {
                let body: serde_json::Map<String, Value> =
                    params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
                request.json(&body)
            } else {
                let mut form = Form::new();
                for (k, v) in params {
                    let text = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    form = form.text(k.to_string(), text);
                }
                for f in files {
                    let part = Part::bytes(f.sticker.bytes.clone())
                        .file_name(f.sticker.filename.clone())
                        .mime_str(f.sticker.mime())
                        .map_err(http_err)?;
                    form = form.part(f.field.clone(), part);
                }
                request.multipart(form)
            };
            let response = request.send().await.map_err(http_err)?;
            let status = response.status();
            let body = read_capped(response, MAX_RESPONSE_BYTES).await.map_err(|e| match e {
                DownloadError::TooLarge => TelegramError::Invalid(format!("{method}: response too large")),
                DownloadError::Other(e) => e,
            })?;
            let parsed: ApiResponse<T> = serde_json::from_slice(&body).map_err(|e| {
                TelegramError::Invalid(format!("{method}: HTTP {status}, not a Bot API response ({e})"))
            })?;
            if parsed.ok {
                return parsed
                    .result
                    .ok_or_else(|| TelegramError::Invalid(format!("{method}: response has no result")));
            }
            let code = parsed.error_code.unwrap_or(i64::from(status.as_u16()));
            let retry_after = parsed.parameters.and_then(|p| p.retry_after);
            if code == 429 && retries < MAX_RETRIES {
                retries += 1;
                let wait = Duration::from_secs(retry_after.unwrap_or(1)).min(self.max_retry_wait);
                tracing::warn!(
                    method,
                    retry = retries,
                    wait_ms = wait.as_millis() as u64,
                    "Telegram rate limit"
                );
                tokio::time::sleep(wait).await;
                continue;
            }
            return Err(TelegramError::Api {
                code,
                description: parsed.description.unwrap_or_default(),
                retry_after,
            });
        }
    }

    async fn call_true(
        &self,
        method: &str,
        params: &[(&str, Value)],
        files: &[Upload<'_>],
    ) -> Result<(), TelegramError> {
        let _: Value = self.call(method, params, files).await?;
        Ok(())
    }

    pub async fn get_me(&self) -> Result<BotUser, TelegramError> {
        self.call("getMe", &[], &[]).await
    }

    pub async fn get_sticker_set(&self, name: &str) -> Result<StickerSet, TelegramError> {
        self.call("getStickerSet", &[("name", json!(name))], &[]).await
    }

    pub async fn get_file(&self, file_id: &str) -> Result<TgFile, TelegramError> {
        self.call("getFile", &[("file_id", json!(file_id))], &[]).await
    }

    /// `getFile` then download from `<base>/file/bot<token>/<file_path>`, capped at `max_bytes`.
    pub async fn download_file(&self, file_id: &str, max_bytes: u64) -> Result<Vec<u8>, TelegramError> {
        match self.download_capped(file_id, max_bytes).await {
            Ok(bytes) => Ok(bytes),
            Err(DownloadError::TooLarge) => {
                Err(TelegramError::Invalid(format!("file is larger than {max_bytes} bytes")))
            }
            Err(DownloadError::Other(e)) => Err(e),
        }
    }

    pub(crate) async fn download_capped(&self, file_id: &str, max_bytes: u64) -> Result<Vec<u8>, DownloadError> {
        let file = self.get_file(file_id).await?;
        if file.file_size.is_some_and(|size| size > max_bytes) {
            return Err(DownloadError::TooLarge);
        }
        let path = file.file_path.ok_or_else(|| {
            TelegramError::Invalid("Telegram returned no file_path (bots cannot download files over 20 MB)".into())
        })?;
        if path.is_empty()
            || path.starts_with('/')
            || path.split('/').any(|seg| seg == "..")
            || path.chars().any(|c| c.is_control() || matches!(c, '?' | '#' | '\\'))
        {
            return Err(TelegramError::Invalid("Telegram returned an unexpected file_path".into()).into());
        }
        let url = format!("{}/file/bot{}/{}", self.base, self.token, path);
        let response = self.http.get(&url).send().await.map_err(http_err)?;
        if !response.status().is_success() {
            return Err(TelegramError::Invalid(format!("file download failed: HTTP {}", response.status())).into());
        }
        read_capped(response, max_bytes).await
    }

    pub async fn create_new_sticker_set(
        &self,
        user_id: i64,
        name: &str,
        title: &str,
        stickers: &[InputSticker],
    ) -> Result<(), TelegramError> {
        if stickers.is_empty() || stickers.len() > 50 {
            return Err(TelegramError::Invalid(format!(
                "createNewStickerSet takes 1 to 50 stickers, got {}",
                stickers.len()
            )));
        }
        let uploads: Vec<Upload<'_>> = stickers
            .iter()
            .enumerate()
            .map(|(i, s)| Upload {
                field: format!("sticker{i}"),
                sticker: s,
            })
            .collect();
        let list: Vec<Value> = uploads.iter().map(|u| u.sticker.to_json(&u.field)).collect();
        let params = [
            ("user_id", json!(user_id)),
            ("name", json!(name)),
            ("title", json!(title)),
            ("stickers", Value::Array(list)),
        ];
        self.call_true("createNewStickerSet", &params, &uploads).await
    }

    pub async fn add_sticker_to_set(
        &self,
        user_id: i64,
        name: &str,
        sticker: &InputSticker,
    ) -> Result<(), TelegramError> {
        let upload = Upload {
            field: "sticker0".into(),
            sticker,
        };
        let params = [
            ("user_id", json!(user_id)),
            ("name", json!(name)),
            ("sticker", sticker.to_json(&upload.field)),
        ];
        self.call_true("addStickerToSet", &params, &[upload]).await
    }

    /// Replace `old_file_id` in place (same position) with a new sticker.
    pub async fn replace_sticker_in_set(
        &self,
        user_id: i64,
        name: &str,
        old_file_id: &str,
        sticker: &InputSticker,
    ) -> Result<(), TelegramError> {
        let upload = Upload {
            field: "sticker0".into(),
            sticker,
        };
        let params = [
            ("user_id", json!(user_id)),
            ("name", json!(name)),
            ("old_sticker", json!(old_file_id)),
            ("sticker", sticker.to_json(&upload.field)),
        ];
        self.call_true("replaceStickerInSet", &params, &[upload]).await
    }

    pub async fn delete_sticker_from_set(&self, file_id: &str) -> Result<(), TelegramError> {
        self.call_true("deleteStickerFromSet", &[("sticker", json!(file_id))], &[])
            .await
    }

    pub async fn set_sticker_position_in_set(&self, file_id: &str, position: u32) -> Result<(), TelegramError> {
        self.call_true(
            "setStickerPositionInSet",
            &[("sticker", json!(file_id)), ("position", json!(position))],
            &[],
        )
        .await
    }

    pub async fn set_sticker_emoji_list(&self, file_id: &str, emoji_list: &[String]) -> Result<(), TelegramError> {
        self.call_true(
            "setStickerEmojiList",
            &[("sticker", json!(file_id)), ("emoji_list", json!(emoji_list))],
            &[],
        )
        .await
    }

    pub async fn set_sticker_keywords(&self, file_id: &str, keywords: &[String]) -> Result<(), TelegramError> {
        self.call_true(
            "setStickerKeywords",
            &[("sticker", json!(file_id)), ("keywords", json!(keywords))],
            &[],
        )
        .await
    }

    pub async fn set_sticker_set_title(&self, name: &str, title: &str) -> Result<(), TelegramError> {
        self.call_true(
            "setStickerSetTitle",
            &[("name", json!(name)), ("title", json!(title))],
            &[],
        )
        .await
    }
}

/// Reads a response body, failing as soon as it exceeds `cap` bytes.
async fn read_capped(mut response: reqwest::Response, cap: u64) -> Result<Vec<u8>, DownloadError> {
    if response.content_length().is_some_and(|len| len > cap) {
        return Err(DownloadError::TooLarge);
    }
    let mut out = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(http_err)? {
        if out.len() as u64 + chunk.len() as u64 > cap {
            return Err(DownloadError::TooLarge);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}
