//! Thin typed wrapper over the Discord REST API (v10) routes stickpile needs:
//! the bot user, the guild (for its boost tier), and guild stickers and
//! emoji (list, create, modify, delete), plus CDN downloads.
//!
//! Authentication is `Authorization: Bot <token>` on API requests only; CDN
//! downloads carry no credentials. The token never appears in errors, in
//! `Debug` output or in logs, and the header is marked sensitive.
//!
//! Rate limits (developer docs, topics/rate-limits): every response may
//! carry `X-RateLimit-Remaining` and `X-RateLimit-Reset-After`; when a route
//! has no requests left, the next request on it waits for the reset. A 429
//! answer is retried up to [`MAX_RETRIES`] times after its `retry_after`
//! (body, else the `Retry-After` header), each wait capped by
//! [`DiscordApi::with_max_retry_wait`]; a global 429 holds back every route.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::multipart::{Form, Part};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::files::data_uri;
use crate::image::ImageFormat;

/// API base: version 10 is the current version (developer docs, reference).
pub const DEFAULT_API_BASE: &str = "https://discord.com/api/v10";
/// Image CDN (developer docs, reference "Image Formatting").
pub const DEFAULT_CDN_BASE: &str = "https://cdn.discordapp.com";
/// GIF stickers are served from here instead of the CDN.
pub const DEFAULT_MEDIA_BASE: &str = "https://media.discordapp.net";
/// How often a rate-limited (429) call is retried.
pub const MAX_RETRIES: u32 = 3;
/// Default cap for a single rate-limit wait.
pub const DEFAULT_MAX_RETRY_WAIT: Duration = Duration::from_secs(30);
/// JSON responses larger than this are rejected.
const MAX_RESPONSE_BYTES: u64 = 16 << 20;
/// Shown in the guild's audit log for every change stickpile makes.
const AUDIT_LOG_REASON: &str = "stickpile export";

#[derive(Debug, thiserror::Error)]
pub enum DiscordError {
    /// A non-2xx answer from the API. `code` is Discord's JSON error code
    /// (e.g. 30039 "Maximum number of stickers reached"), `message` its text
    /// with the per-field errors appended.
    #[error("Discord API error (HTTP {status}): {message}")]
    Api {
        status: u16,
        code: Option<i64>,
        message: String,
        /// Seconds to wait, for 429 answers.
        retry_after: Option<f64>,
    },
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("file is larger than {0} bytes")]
    TooLarge(u64),
    #[error("{0}")]
    Invalid(String),
}

impl DiscordError {
    pub fn status(&self) -> Option<u16> {
        match self {
            DiscordError::Api { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// Discord's JSON error code, if any.
    pub fn code(&self) -> Option<i64> {
        match self {
            DiscordError::Api { code, .. } => *code,
            _ => None,
        }
    }

    /// A 400 answer: Discord refused this one request (a bad file, a full
    /// slot list, an invalid name) and other requests may still work.
    pub fn is_rejection(&self) -> bool {
        matches!(self, DiscordError::Api { status: 400, .. })
    }
}

fn http_err(e: reqwest::Error) -> DiscordError {
    DiscordError::Http(e)
}

/// A bot token, kept out of `Debug` output.
#[derive(Clone)]
pub struct BotToken(String);

impl std::fmt::Debug for BotToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BotToken(<redacted>)")
    }
}

impl BotToken {
    /// From the token text (surrounding whitespace and a leading `Bot `
    /// are ignored).
    pub fn new(token: &str) -> Result<BotToken, DiscordError> {
        let t = token.trim();
        let t = t.strip_prefix("Bot ").unwrap_or(t).trim();
        if t.is_empty() {
            return Err(DiscordError::Invalid("the Discord bot token is empty".into()));
        }
        if !t.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(DiscordError::Invalid(
                "the Discord bot token contains characters a token cannot have".into(),
            ));
        }
        Ok(BotToken(t.to_string()))
    }

    /// Reads the token from a file (the server's `bot_token_file`). Errors
    /// name the file, never its content.
    pub fn from_file(path: &Path) -> Result<BotToken, DiscordError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| DiscordError::Invalid(format!("cannot read {}: {}", path.display(), e.kind())))?;
        BotToken::new(&text).map_err(|e| DiscordError::Invalid(format!("{}: {e}", path.display())))
    }

    fn header(&self) -> HeaderValue {
        let mut v = HeaderValue::from_str(&format!("Bot {}", self.0)).expect("checked to be visible ASCII");
        v.set_sensitive(true);
        v
    }
}

/// The bot's own user (`GET /users/@me`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct User {
    pub id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub bot: bool,
}

/// The parts of a guild object stickpile reads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Guild {
    pub id: String,
    pub name: String,
    /// Server Boost level 0-3.
    #[serde(default)]
    pub premium_tier: u8,
    #[serde(default)]
    pub features: Vec<String>,
}

impl Guild {
    /// Sticker slots: 5, 15, 30, 60 for boost levels 0-3, at least 60 with
    /// the `MORE_STICKERS` feature. The developer docs do not list the
    /// numbers (the create route says "five free sticker slots by default,
    /// and each Boost level will grant access to more slots"); these are the
    /// ones discord.py's `Guild.sticker_limit` uses and Discord's Server
    /// Boosting FAQ states.
    pub fn sticker_slots(&self) -> usize {
        let tier = match self.premium_tier {
            0 => 5,
            1 => 15,
            2 => 30,
            _ => 60,
        };
        if self.features.iter().any(|f| f == "MORE_STICKERS") {
            tier.max(60)
        } else {
            tier
        }
    }

    /// Emoji slots, counted separately for static and animated emoji: 50,
    /// 100, 150, 250 for boost levels 0-3, at least 200 with `MORE_EMOJI`
    /// (same sources as [`Guild::sticker_slots`]).
    pub fn emoji_slots(&self) -> usize {
        let tier = match self.premium_tier {
            0 => 50,
            1 => 100,
            2 => 150,
            _ => 250,
        };
        if self.features.iter().any(|f| f == "MORE_EMOJI") {
            tier.max(200)
        } else {
            tier
        }
    }
}

/// Discord sticker file formats (`format_type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StickerFormat {
    Png,
    Apng,
    Lottie,
    Gif,
}

impl StickerFormat {
    pub fn from_code(code: u8) -> Option<StickerFormat> {
        match code {
            1 => Some(StickerFormat::Png),
            2 => Some(StickerFormat::Apng),
            3 => Some(StickerFormat::Lottie),
            4 => Some(StickerFormat::Gif),
            _ => None,
        }
    }
}

/// A guild sticker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sticker {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: String,
    /// 1 PNG, 2 APNG, 3 Lottie, 4 GIF.
    pub format_type: u8,
    /// False when the guild lost the boosts the slot needs.
    #[serde(default)]
    pub available: Option<bool>,
    #[serde(default)]
    pub guild_id: Option<String>,
    /// Who uploaded it (sent when the bot may manage expressions).
    #[serde(default)]
    pub user: Option<User>,
}

impl Sticker {
    pub fn format(&self) -> Option<StickerFormat> {
        StickerFormat::from_code(self.format_type)
    }
}

/// A guild emoji.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Emoji {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub animated: bool,
    /// Managed by an integration (e.g. Twitch): never touched.
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub available: Option<bool>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub user: Option<User>,
}

/// A file to upload: the bytes and their format.
#[derive(Clone)]
pub struct InputFile {
    pub bytes: Vec<u8>,
    pub format: ImageFormat,
}

impl std::fmt::Debug for InputFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputFile")
            .field("format", &self.format)
            .field("len", &self.bytes.len())
            .finish()
    }
}

/// `POST /guilds/{guild}/stickers` form fields.
#[derive(Debug, Clone)]
pub struct NewSticker {
    /// 2-30 characters.
    pub name: String,
    /// Empty or 2-100 characters.
    pub description: String,
    /// At most 200 characters; stickpile sends one raw emoji.
    pub tags: String,
    /// PNG, APNG or GIF, at most 512 KiB, at most 320x320.
    pub file: InputFile,
}

/// `PATCH /guilds/{guild}/stickers/{id}`: only the fields set are changed.
#[derive(Debug, Clone, Default, Serialize)]
pub struct StickerChanges {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `Some("")` clears the description (sent as `null`).
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "empty_as_null")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
}

fn empty_as_null<S: serde::Serializer>(v: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
    match v.as_deref() {
        Some("") | None => s.serialize_none(),
        Some(text) => s.serialize_str(text),
    }
}

enum Body<'a> {
    None,
    Json(Value),
    Sticker(&'a NewSticker),
}

#[derive(Default)]
struct RateState {
    /// Route key -> no requests before this instant.
    routes: HashMap<String, Instant>,
    /// Global limit hit: no requests before this instant.
    global: Option<Instant>,
}

#[derive(Clone)]
pub struct DiscordApi {
    http: reqwest::Client,
    base: String,
    cdn: String,
    media: String,
    token: BotToken,
    max_retry_wait: Duration,
    limits: Arc<Mutex<RateState>>,
}

impl std::fmt::Debug for DiscordApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscordApi")
            .field("base", &self.base)
            .field("cdn", &self.cdn)
            .finish_non_exhaustive()
    }
}

/// Only ids made of digits go into a URL path.
fn snowflake(id: &str) -> Result<&str, DiscordError> {
    if !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(id)
    } else {
        Err(DiscordError::Invalid(format!("{id:?} is not a Discord id")))
    }
}

fn header_f64(headers: &HeaderMap, name: &str) -> Option<f64> {
    headers
        .get(name)?
        .to_str()
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
}

fn secs(v: f64) -> Duration {
    Duration::from_secs_f64(v.clamp(0.0, 86_400.0))
}

/// `message` plus the per-field errors of a Discord error body, e.g.
/// `Invalid Form Body (name: Must be between 2 and 30 in length.)`.
fn error_text(body: &Value) -> String {
    fn walk(v: &Value, path: &str, out: &mut Vec<String>) {
        let Some(map) = v.as_object() else { return };
        for (k, child) in map {
            if k == "_errors" {
                for e in child.as_array().into_iter().flatten() {
                    let msg = e["message"].as_str().unwrap_or("invalid");
                    out.push(if path.is_empty() {
                        msg.to_string()
                    } else {
                        format!("{path}: {msg}")
                    });
                }
            } else {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                walk(child, &p, out);
            }
        }
    }
    let message = body["message"].as_str().unwrap_or("").to_string();
    let mut details = Vec::new();
    walk(&body["errors"], "", &mut details);
    match (message.is_empty(), details.is_empty()) {
        (_, true) => message,
        (true, false) => details.join("; "),
        (false, false) => format!("{message} ({})", details.join("; ")),
    }
}

impl DiscordApi {
    /// A client for the real Discord API.
    pub fn new(token: BotToken) -> DiscordApi {
        DiscordApi::with_base(DEFAULT_API_BASE, token)
    }

    /// `base` like `https://discord.com/api/v10` (tests point it at a mock).
    /// The CDN bases default to Discord's; see [`DiscordApi::with_cdn`].
    pub fn with_base(base: &str, token: BotToken) -> DiscordApi {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .user_agent(concat!(
                "DiscordBot (https://github.com/Dolfost/stickpile, ",
                env!("CARGO_PKG_VERSION"),
                ")"
            ))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        DiscordApi {
            http,
            base: base.trim_end_matches('/').to_string(),
            cdn: DEFAULT_CDN_BASE.into(),
            media: DEFAULT_MEDIA_BASE.into(),
            token,
            max_retry_wait: DEFAULT_MAX_RETRY_WAIT,
            limits: Arc::default(),
        }
    }

    /// Where sticker and emoji images are downloaded from: the CDN
    /// (`https://cdn.discordapp.com`) and, for GIF stickers, the media proxy
    /// (`https://media.discordapp.net`).
    pub fn with_cdn(mut self, cdn: &str, media: &str) -> DiscordApi {
        self.cdn = cdn.trim_end_matches('/').to_string();
        self.media = media.trim_end_matches('/').to_string();
        self
    }

    /// Cap for a single rate-limit wait (default 30 s).
    pub fn with_max_retry_wait(mut self, wait: Duration) -> DiscordApi {
        self.max_retry_wait = wait;
        self
    }

    /// Waits until `route` (and the global limit) may be used again.
    async fn wait_for(&self, route: &str) {
        let until = {
            let st = self.limits.lock().expect("rate state");
            [st.global, st.routes.get(route).copied()].into_iter().flatten().max()
        };
        if let Some(until) = until {
            let wait = until.saturating_duration_since(Instant::now()).min(self.max_retry_wait);
            if !wait.is_zero() {
                tracing::debug!(route, wait_ms = wait.as_millis() as u64, "Discord rate limit: waiting");
                tokio::time::sleep(wait).await;
            }
        }
    }

    fn note_limits(&self, route: &str, headers: &HeaderMap) {
        let remaining = header_f64(headers, "x-ratelimit-remaining");
        let reset_after = header_f64(headers, "x-ratelimit-reset-after");
        let mut st = self.limits.lock().expect("rate state");
        match (remaining, reset_after) {
            (Some(r), Some(after)) if r < 1.0 => {
                st.routes.insert(route.to_string(), Instant::now() + secs(after));
            }
            _ => {
                st.routes.remove(route);
            }
        }
    }

    fn build(&self, method: &Method, path: &str, body: &Body<'_>) -> Result<reqwest::RequestBuilder, DiscordError> {
        let mut req = self
            .http
            .request(method.clone(), format!("{}{path}", self.base))
            .header(AUTHORIZATION, self.token.header());
        if *method != Method::GET {
            req = req.header("X-Audit-Log-Reason", AUDIT_LOG_REASON);
        }
        Ok(match body {
            Body::None => req,
            Body::Json(v) => req.json(v),
            Body::Sticker(s) => {
                let file = Part::bytes(s.file.bytes.clone())
                    .file_name(format!("sticker.{}", s.file.format.extension()))
                    .mime_str(s.file.format.mime())
                    .map_err(http_err)?;
                let form = Form::new()
                    .text("name", s.name.clone())
                    .text("description", s.description.clone())
                    .text("tags", s.tags.clone())
                    .part("file", file);
                req.multipart(form)
            }
        })
    }

    /// One API call with rate-limit handling; returns the response body
    /// (`Value::Null` for 204).
    async fn call(&self, method: Method, route: &str, path: &str, body: Body<'_>) -> Result<Value, DiscordError> {
        let mut retries = 0;
        loop {
            self.wait_for(route).await;
            let response = self.build(&method, path, &body)?.send().await.map_err(http_err)?;
            let status = response.status();
            let headers = response.headers().clone();
            self.note_limits(route, &headers);
            let bytes = read_capped(response, MAX_RESPONSE_BYTES).await.map_err(|e| match e {
                DiscordError::TooLarge(_) => DiscordError::Invalid(format!("{route}: response too large")),
                other => other,
            })?;
            if status == StatusCode::NO_CONTENT {
                return Ok(Value::Null);
            }
            let parsed: Option<Value> = serde_json::from_slice(&bytes).ok();
            if status.is_success() {
                return parsed
                    .ok_or_else(|| DiscordError::Invalid(format!("{route}: HTTP {status}, the answer is not JSON")));
            }
            let body_json = parsed.unwrap_or(Value::Null);
            if status == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = body_json["retry_after"]
                    .as_f64()
                    .or_else(|| header_f64(&headers, "retry-after"))
                    .unwrap_or(1.0);
                let global =
                    body_json["global"].as_bool().unwrap_or(false) || headers.get("x-ratelimit-global").is_some();
                {
                    let mut st = self.limits.lock().expect("rate state");
                    let until = Instant::now() + secs(retry_after);
                    if global {
                        st.global = Some(until);
                    } else {
                        st.routes.insert(route.to_string(), until);
                    }
                }
                if retries < MAX_RETRIES {
                    retries += 1;
                    tracing::warn!(route, retry = retries, retry_after, global, "Discord rate limit");
                    continue;
                }
                return Err(DiscordError::Api {
                    status: 429,
                    code: body_json["code"].as_i64(),
                    message: error_text(&body_json),
                    retry_after: Some(retry_after),
                });
            }
            let mut message = error_text(&body_json);
            if message.is_empty() {
                message = status.canonical_reason().unwrap_or("error").to_string();
            }
            return Err(DiscordError::Api {
                status: status.as_u16(),
                code: body_json["code"].as_i64(),
                message,
                retry_after: None,
            });
        }
    }

    async fn call_as<T: DeserializeOwned>(
        &self,
        method: Method,
        route: &str,
        path: &str,
        body: Body<'_>,
    ) -> Result<T, DiscordError> {
        let v = self.call(method, route, path, body).await?;
        serde_json::from_value(v).map_err(|e| DiscordError::Invalid(format!("{route}: unexpected answer ({e})")))
    }

    /// The bot's own user.
    pub async fn current_user(&self) -> Result<User, DiscordError> {
        self.call_as(Method::GET, "GET /users/@me", "/users/@me", Body::None)
            .await
    }

    pub async fn guild(&self, guild_id: &str) -> Result<Guild, DiscordError> {
        let g = snowflake(guild_id)?;
        self.call_as(
            Method::GET,
            &format!("GET /guilds/{g}"),
            &format!("/guilds/{g}"),
            Body::None,
        )
        .await
    }

    pub async fn list_stickers(&self, guild_id: &str) -> Result<Vec<Sticker>, DiscordError> {
        let g = snowflake(guild_id)?;
        self.call_as(
            Method::GET,
            &format!("GET /guilds/{g}/stickers"),
            &format!("/guilds/{g}/stickers"),
            Body::None,
        )
        .await
    }

    /// Creates a guild sticker (`multipart/form-data`).
    pub async fn create_sticker(&self, guild_id: &str, sticker: &NewSticker) -> Result<Sticker, DiscordError> {
        let g = snowflake(guild_id)?;
        self.call_as(
            Method::POST,
            &format!("POST /guilds/{g}/stickers"),
            &format!("/guilds/{g}/stickers"),
            Body::Sticker(sticker),
        )
        .await
    }

    pub async fn modify_sticker(
        &self,
        guild_id: &str,
        sticker_id: &str,
        changes: &StickerChanges,
    ) -> Result<Sticker, DiscordError> {
        let g = snowflake(guild_id)?;
        let s = snowflake(sticker_id)?;
        let body = serde_json::to_value(changes).map_err(|e| DiscordError::Invalid(e.to_string()))?;
        self.call_as(
            Method::PATCH,
            &format!("PATCH /guilds/{g}/stickers/:id"),
            &format!("/guilds/{g}/stickers/{s}"),
            Body::Json(body),
        )
        .await
    }

    pub async fn delete_sticker(&self, guild_id: &str, sticker_id: &str) -> Result<(), DiscordError> {
        let g = snowflake(guild_id)?;
        let s = snowflake(sticker_id)?;
        self.call(
            Method::DELETE,
            &format!("DELETE /guilds/{g}/stickers/:id"),
            &format!("/guilds/{g}/stickers/{s}"),
            Body::None,
        )
        .await?;
        Ok(())
    }

    pub async fn list_emojis(&self, guild_id: &str) -> Result<Vec<Emoji>, DiscordError> {
        let g = snowflake(guild_id)?;
        self.call_as(
            Method::GET,
            &format!("GET /guilds/{g}/emojis"),
            &format!("/guilds/{g}/emojis"),
            Body::None,
        )
        .await
    }

    /// Creates a guild emoji from `file` (sent as a data URI). `name` must
    /// be 2-32 characters of `A-Z a-z 0-9 _`.
    pub async fn create_emoji(&self, guild_id: &str, name: &str, file: &InputFile) -> Result<Emoji, DiscordError> {
        let g = snowflake(guild_id)?;
        let body = json!({"name": name, "image": data_uri(file.format.mime(), &file.bytes), "roles": []});
        self.call_as(
            Method::POST,
            &format!("POST /guilds/{g}/emojis"),
            &format!("/guilds/{g}/emojis"),
            Body::Json(body),
        )
        .await
    }

    pub async fn modify_emoji(&self, guild_id: &str, emoji_id: &str, name: &str) -> Result<Emoji, DiscordError> {
        let g = snowflake(guild_id)?;
        let e = snowflake(emoji_id)?;
        self.call_as(
            Method::PATCH,
            &format!("PATCH /guilds/{g}/emojis/:id"),
            &format!("/guilds/{g}/emojis/{e}"),
            Body::Json(json!({"name": name})),
        )
        .await
    }

    pub async fn delete_emoji(&self, guild_id: &str, emoji_id: &str) -> Result<(), DiscordError> {
        let g = snowflake(guild_id)?;
        let e = snowflake(emoji_id)?;
        self.call(
            Method::DELETE,
            &format!("DELETE /guilds/{g}/emojis/:id"),
            &format!("/guilds/{g}/emojis/{e}"),
            Body::None,
        )
        .await?;
        Ok(())
    }

    /// Image URL of a sticker: PNG and APNG from the CDN, GIF from the media
    /// proxy, Lottie JSON from the CDN (developer docs, reference "Image
    /// Formatting").
    pub fn sticker_url(&self, sticker: &Sticker) -> Result<String, DiscordError> {
        let id = snowflake(&sticker.id)?;
        Ok(match sticker.format() {
            Some(StickerFormat::Gif) => format!("{}/stickers/{id}.gif", self.media),
            Some(StickerFormat::Lottie) => format!("{}/stickers/{id}.json", self.cdn),
            _ => format!("{}/stickers/{id}.png", self.cdn),
        })
    }

    /// Image URLs of an emoji, best first: animated emoji as animated WebP
    /// (the docs advise WebP for animations, since WebP and AVIF uploads do
    /// not convert well to GIF), static ones as PNG with WebP as fallback.
    pub fn emoji_urls(&self, emoji: &Emoji) -> Result<Vec<String>, DiscordError> {
        let id = snowflake(&emoji.id)?;
        Ok(if emoji.animated {
            vec![format!("{}/emojis/{id}.webp?animated=true", self.cdn)]
        } else {
            vec![
                format!("{}/emojis/{id}.png", self.cdn),
                format!("{}/emojis/{id}.webp", self.cdn),
            ]
        })
    }

    /// Downloads a CDN file without credentials, failing with
    /// [`DiscordError::TooLarge`] past `max_bytes`.
    pub async fn download(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, DiscordError> {
        if !(url.starts_with(&format!("{}/", self.cdn)) || url.starts_with(&format!("{}/", self.media))) {
            return Err(DiscordError::Invalid(format!("{url} is not a Discord CDN URL")));
        }
        let response = self.http.get(url).send().await.map_err(http_err)?;
        let status = response.status();
        if !status.is_success() {
            return Err(DiscordError::Api {
                status: status.as_u16(),
                code: None,
                message: format!("download failed: HTTP {status}"),
                retry_after: None,
            });
        }
        read_capped(response, max_bytes).await
    }
}

/// Reads a response body, failing as soon as it exceeds `cap` bytes.
async fn read_capped(mut response: reqwest::Response, cap: u64) -> Result<Vec<u8>, DiscordError> {
    if response.content_length().is_some_and(|len| len > cap) {
        return Err(DiscordError::TooLarge(cap));
    }
    let mut out = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(http_err)? {
        if out.len() as u64 + chunk.len() as u64 > cap {
            return Err(DiscordError::TooLarge(cap));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        let t = BotToken::new("  Bot abc.DEF-123 \n").unwrap();
        assert_eq!(t.0, "abc.DEF-123");
        assert_eq!(format!("{t:?}"), "BotToken(<redacted>)");
        assert!(BotToken::new(" \n").is_err());
        let err = BotToken::new("abc def").unwrap_err().to_string();
        assert!(!err.contains("abc"), "{err}");
        assert!(t.header().is_sensitive());
    }

    #[test]
    fn slots() {
        let g = |tier: u8, features: &[&str]| Guild {
            id: "1".into(),
            name: "g".into(),
            premium_tier: tier,
            features: features.iter().map(|s| s.to_string()).collect(),
        };
        let stickers: Vec<usize> = (0..4).map(|t| g(t, &[]).sticker_slots()).collect();
        assert_eq!(stickers, [5, 15, 30, 60]);
        let emojis: Vec<usize> = (0..4).map(|t| g(t, &[]).emoji_slots()).collect();
        assert_eq!(emojis, [50, 100, 150, 250]);
        assert_eq!(g(0, &["MORE_STICKERS"]).sticker_slots(), 60);
        assert_eq!(g(0, &["MORE_EMOJI"]).emoji_slots(), 200);
        assert_eq!(g(3, &["MORE_EMOJI"]).emoji_slots(), 250);
    }

    #[test]
    fn error_texts() {
        let body = json!({
            "code": 50035,
            "message": "Invalid Form Body",
            "errors": {"name": {"_errors": [{"code": "BASE_TYPE_BAD_LENGTH", "message": "Must be between 2 and 30 in length."}]}}
        });
        assert_eq!(
            error_text(&body),
            "Invalid Form Body (name: Must be between 2 and 30 in length.)"
        );
        assert_eq!(
            error_text(&json!({"message": "Unknown Guild", "code": 10004})),
            "Unknown Guild"
        );
        assert_eq!(error_text(&Value::Null), "");
    }

    #[test]
    fn ids_are_checked() {
        assert!(snowflake("123456789012345678").is_ok());
        for bad in ["", "12/../3", "abc", "1?x=1", &"9".repeat(21)] {
            assert!(snowflake(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn sticker_changes_json() {
        let c = StickerChanges {
            name: Some("cat".into()),
            description: Some(String::new()),
            tags: None,
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({"name": "cat", "description": null})
        );
        assert_eq!(serde_json::to_value(StickerChanges::default()).unwrap(), json!({}));
    }
}
