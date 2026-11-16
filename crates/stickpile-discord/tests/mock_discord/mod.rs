//! In-process mock of the Discord REST API routes stickpile uses, and of the
//! CDN (axum on 127.0.0.1:0).
//!
//! Behaves like Discord where stickpile depends on it (developer docs):
//! `Authorization: Bot <token>`, JSON error bodies `{code, message, errors}`,
//! 429 answers with `retry_after` in the body and `Retry-After`, the
//! `X-RateLimit-*` headers, multipart sticker uploads, data-URI emoji, slot
//! limits by boost level (stickers 5/15/30/60, emoji 50/100/150/250 static
//! and the same animated) with error codes 30039/30008/30018, 204 on delete,
//! and an unauthenticated CDN. All of it is this mock's reading of the docs,
//! not observed Discord behaviour.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use base64::Engine;
use serde_json::{json, Value};

pub const TOKEN: &str = "MTk4NjIyNDgzNDcx.SECRET-token_value";
pub const BOT_ID: &str = "900000000000000001";
pub const HUMAN_ID: &str = "900000000000000002";
pub const GUILD: &str = "100000000000000001";

#[derive(Clone, Debug)]
pub struct MockSticker {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: String,
    pub format_type: u8,
    pub bytes: Vec<u8>,
    pub user_id: String,
}

#[derive(Clone, Debug)]
pub struct MockEmoji {
    pub id: String,
    pub name: String,
    pub animated: bool,
    pub managed: bool,
    pub bytes: Vec<u8>,
    pub user_id: String,
}

#[derive(Default)]
pub struct MockState {
    pub premium_tier: u8,
    pub features: Vec<String>,
    pub stickers: Vec<MockSticker>,
    pub emojis: Vec<MockEmoji>,
    /// Route keys ("POST /stickers", ...) in call order, rate-limited
    /// attempts included.
    pub calls: Vec<String>,
    pub call_times: Vec<(String, Instant)>,
    /// Route key -> how many more calls to answer with 429.
    pub rate_limit: HashMap<String, u32>,
    pub retry_after: f64,
    pub global: bool,
    /// Route key -> `X-RateLimit-Reset-After` sent with `Remaining: 0` on
    /// successful answers.
    pub exhaust: HashMap<String, f64>,
    /// Route key -> (HTTP status, how many calls) to fail.
    pub fail: HashMap<String, (u16, u32)>,
    /// Route key -> (HTTP status, successful calls first): then one call
    /// fails.
    pub fail_after: HashMap<String, (u16, u32)>,
    /// Every Authorization header the API saw.
    pub auth_headers: Vec<String>,
    /// Mutating calls without `X-Audit-Log-Reason`.
    pub missing_audit_reason: u32,
    /// CDN paths requested.
    pub cdn_calls: Vec<String>,
    /// A CDN request carried an Authorization header.
    pub cdn_saw_auth: bool,
    next_id: u64,
}

pub type Shared = Arc<Mutex<MockState>>;

impl MockState {
    fn id(&mut self) -> String {
        self.next_id += 1;
        format!("{}", 200_000_000_000_000_000u64 + self.next_id)
    }

    pub fn count(&self, route: &str) -> usize {
        self.calls.iter().filter(|c| *c == route).count()
    }

    /// A sticker made by someone in the guild (not by the bot).
    pub fn seed_sticker(&mut self, name: &str, tags: &str, format_type: u8, bytes: Vec<u8>, user: &str) -> String {
        let id = self.id();
        self.stickers.push(MockSticker {
            id: id.clone(),
            name: name.into(),
            description: None,
            tags: tags.into(),
            format_type,
            bytes,
            user_id: user.into(),
        });
        id
    }

    pub fn seed_emoji(&mut self, name: &str, animated: bool, bytes: Vec<u8>, user: &str) -> String {
        let id = self.id();
        self.emojis.push(MockEmoji {
            id: id.clone(),
            name: name.into(),
            animated,
            managed: false,
            bytes,
            user_id: user.into(),
        });
        id
    }

    pub fn sticker(&self, id: &str) -> Option<&MockSticker> {
        self.stickers.iter().find(|s| s.id == id)
    }

    pub fn emoji(&self, id: &str) -> Option<&MockEmoji> {
        self.emojis.iter().find(|e| e.id == id)
    }

    fn sticker_slots(&self) -> usize {
        let tier = [5, 15, 30, 60][usize::from(self.premium_tier.min(3))];
        if self.features.iter().any(|f| f == "MORE_STICKERS") {
            tier.max(60)
        } else {
            tier
        }
    }

    fn emoji_slots(&self) -> usize {
        [50, 100, 150, 250][usize::from(self.premium_tier.min(3))]
    }
}

type MockResult = Result<(StatusCode, Value), (StatusCode, Value)>;

fn user(id: &str) -> Value {
    json!({"id": id, "username": if id == BOT_ID { "stickpile" } else { "someone" }, "bot": id == BOT_ID})
}

fn sticker_json(s: &MockSticker) -> Value {
    json!({
        "id": s.id, "name": s.name, "description": s.description, "tags": s.tags,
        "type": 2, "format_type": s.format_type, "available": true, "guild_id": GUILD,
        "user": user(&s.user_id),
    })
}

fn emoji_json(e: &MockEmoji) -> Value {
    json!({
        "id": e.id, "name": e.name, "roles": [], "require_colons": true, "managed": e.managed,
        "animated": e.animated, "available": true, "user": user(&e.user_id),
    })
}

fn form_error(field: &str, message: &str) -> (StatusCode, Value) {
    (
        StatusCode::BAD_REQUEST,
        json!({
            "code": 50035, "message": "Invalid Form Body",
            "errors": {field: {"_errors": [{"code": "BASE_TYPE_BAD_LENGTH", "message": message}]}}
        }),
    )
}

fn coded(status: StatusCode, code: i64, message: &str) -> (StatusCode, Value) {
    (status, json!({"code": code, "message": message}))
}

fn len_ok(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.chars().count())
}

fn sticker_format(bytes: &[u8]) -> Option<u8> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(if bytes.windows(4).any(|w| w == b"acTL") { 2 } else { 1 })
    } else if bytes.starts_with(b"GIF8") {
        Some(4)
    } else {
        None
    }
}

/// Request parts the dispatcher needs.
struct Req {
    method: Method,
    segments: Vec<String>,
    json: Value,
    form: HashMap<String, (Option<String>, Vec<u8>)>,
}

impl MockState {
    fn dispatch(&mut self, r: &Req) -> MockResult {
        let seg: Vec<&str> = r.segments.iter().map(String::as_str).collect();
        match (&r.method, seg.as_slice()) {
            (&Method::GET, ["users", "@me"]) => Ok((StatusCode::OK, user(BOT_ID))),
            (_, ["guilds", g, ..]) if *g != GUILD => Err(coded(StatusCode::NOT_FOUND, 10004, "Unknown Guild")),
            (&Method::GET, ["guilds", _]) => Ok((
                StatusCode::OK,
                json!({"id": GUILD, "name": "Cat Lounge", "premium_tier": self.premium_tier,
                       "features": self.features, "emojis": [], "roles": []}),
            )),
            (&Method::GET, ["guilds", _, "stickers"]) => Ok((
                StatusCode::OK,
                Value::Array(self.stickers.iter().map(sticker_json).collect()),
            )),
            (&Method::POST, ["guilds", _, "stickers"]) => {
                let text = |k: &str| {
                    r.form
                        .get(k)
                        .map(|(_, v)| String::from_utf8_lossy(v).to_string())
                        .unwrap_or_default()
                };
                let (name, description, tags) = (text("name"), text("description"), text("tags"));
                if !len_ok(&name, 2, 30) {
                    return Err(form_error("name", "Must be between 2 and 30 in length."));
                }
                if !description.is_empty() && !len_ok(&description, 2, 100) {
                    return Err(form_error("description", "Must be between 2 and 100 in length."));
                }
                if !len_ok(&tags, 1, 200) {
                    return Err(form_error("tags", "Must be between 1 and 200 in length."));
                }
                let Some((Some(_filename), bytes)) = r.form.get("file") else {
                    return Err(form_error("file", "This field is required"));
                };
                if bytes.len() > 512 * 1024 {
                    return Err(coded(
                        StatusCode::BAD_REQUEST,
                        50045,
                        "File uploaded exceeds the maximum size",
                    ));
                }
                let Some(format_type) = sticker_format(bytes) else {
                    return Err(coded(StatusCode::BAD_REQUEST, 50046, "Invalid Asset"));
                };
                if self.stickers.len() >= self.sticker_slots() {
                    return Err(coded(
                        StatusCode::BAD_REQUEST,
                        30039,
                        "Maximum number of stickers reached",
                    ));
                }
                let id = self.id();
                let s = MockSticker {
                    id,
                    name,
                    description: (!description.is_empty()).then_some(description),
                    tags,
                    format_type,
                    bytes: bytes.clone(),
                    user_id: BOT_ID.into(),
                };
                let out = sticker_json(&s);
                self.stickers.push(s);
                Ok((StatusCode::OK, out))
            }
            (&Method::PATCH, ["guilds", _, "stickers", id]) => {
                let Some(i) = self.stickers.iter().position(|s| s.id == *id) else {
                    return Err(coded(StatusCode::NOT_FOUND, 10060, "Unknown sticker"));
                };
                if let Some(name) = r.json["name"].as_str() {
                    if !len_ok(name, 2, 30) {
                        return Err(form_error("name", "Must be between 2 and 30 in length."));
                    }
                    self.stickers[i].name = name.into();
                }
                if r.json.get("description").is_some() {
                    self.stickers[i].description = r.json["description"].as_str().map(str::to_string);
                }
                if let Some(tags) = r.json["tags"].as_str() {
                    self.stickers[i].tags = tags.into();
                }
                Ok((StatusCode::OK, sticker_json(&self.stickers[i])))
            }
            (&Method::DELETE, ["guilds", _, "stickers", id]) => {
                let Some(i) = self.stickers.iter().position(|s| s.id == *id) else {
                    return Err(coded(StatusCode::NOT_FOUND, 10060, "Unknown sticker"));
                };
                self.stickers.remove(i);
                Ok((StatusCode::NO_CONTENT, Value::Null))
            }
            (&Method::GET, ["guilds", _, "emojis"]) => Ok((
                StatusCode::OK,
                Value::Array(self.emojis.iter().map(emoji_json).collect()),
            )),
            (&Method::POST, ["guilds", _, "emojis"]) => {
                let name = r.json["name"].as_str().unwrap_or_default().to_string();
                if !len_ok(&name, 2, 32) || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err(form_error("name", "Must be between 2 and 32 in length."));
                }
                let image = r.json["image"].as_str().unwrap_or_default();
                let Some((mime, data)) = image.strip_prefix("data:").and_then(|rest| rest.split_once(";base64,"))
                else {
                    return Err(form_error("image", "Invalid image data"));
                };
                if !["image/png", "image/gif", "image/jpeg", "image/webp"].contains(&mime) {
                    return Err(form_error("image", "Invalid image data"));
                }
                let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
                    return Err(form_error("image", "Invalid image data"));
                };
                if bytes.len() > 256 * 1024 {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        json!({"message": "Asset exceeds maximum size: 262144"}),
                    ));
                }
                // The mock's stand-in for "animated": a looping GIF.
                let animated = mime == "image/gif" && bytes.windows(11).any(|w| w == b"NETSCAPE2.0");
                let used = self.emojis.iter().filter(|e| e.animated == animated).count();
                if used >= self.emoji_slots() {
                    return Err(if animated {
                        coded(
                            StatusCode::BAD_REQUEST,
                            30018,
                            "Maximum number of animated emojis reached",
                        )
                    } else {
                        coded(StatusCode::BAD_REQUEST, 30008, "Maximum number of emojis reached")
                    });
                }
                let id = self.id();
                let e = MockEmoji {
                    id,
                    name,
                    animated,
                    managed: false,
                    bytes,
                    user_id: BOT_ID.into(),
                };
                let out = emoji_json(&e);
                self.emojis.push(e);
                Ok((StatusCode::CREATED, out))
            }
            (&Method::PATCH, ["guilds", _, "emojis", id]) => {
                let Some(i) = self.emojis.iter().position(|e| e.id == *id) else {
                    return Err(coded(StatusCode::NOT_FOUND, 10014, "Unknown Emoji"));
                };
                if let Some(name) = r.json["name"].as_str() {
                    if !len_ok(name, 2, 32) {
                        return Err(form_error("name", "Must be between 2 and 32 in length."));
                    }
                    self.emojis[i].name = name.into();
                }
                Ok((StatusCode::OK, emoji_json(&self.emojis[i])))
            }
            (&Method::DELETE, ["guilds", _, "emojis", id]) => {
                let Some(i) = self.emojis.iter().position(|e| e.id == *id) else {
                    return Err(coded(StatusCode::NOT_FOUND, 10014, "Unknown Emoji"));
                };
                self.emojis.remove(i);
                Ok((StatusCode::NO_CONTENT, Value::Null))
            }
            _ => Err(coded(StatusCode::NOT_FOUND, 0, "404: Not Found")),
        }
    }
}

/// "POST /stickers", "GET /guilds", "PATCH /emojis", "GET /users/@me", ...
fn route_key(method: &Method, segments: &[String]) -> String {
    let what = match segments {
        [a, b] if a == "users" => format!("/{a}/{b}"),
        [_, _] => "/guilds".to_string(),
        [_, _, kind, ..] => format!("/{kind}"),
        _ => "/?".to_string(),
    };
    format!("{method} {what}")
}

fn respond(status: StatusCode, body: Value, extra: &[(&'static str, String)]) -> Response {
    let mut response = if status == StatusCode::NO_CONTENT {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (status, Json(body)).into_response()
    };
    for (k, v) in extra {
        response
            .headers_mut()
            .insert(*k, HeaderValue::from_str(v).expect("header value"));
    }
    response
}

async fn cdn(state: Shared, path: &str, headers: &HeaderMap) -> Response {
    let mut st = state.lock().unwrap();
    st.cdn_calls.push(path.to_string());
    if headers.contains_key(header::AUTHORIZATION) {
        st.cdn_saw_auth = true;
    }
    let found = if let Some(file) = path.strip_prefix("/cdn/stickers/") {
        let (id, ext) = file.split_once('.').unwrap_or((file, ""));
        st.sticker(id)
            .filter(|s| (ext == "png" && s.format_type <= 2) || (ext == "json" && s.format_type == 3))
            .map(|s| s.bytes.clone())
    } else if let Some(file) = path.strip_prefix("/media/stickers/") {
        let id = file.strip_suffix(".gif").unwrap_or("");
        st.sticker(id).filter(|s| s.format_type == 4).map(|s| s.bytes.clone())
    } else if let Some(file) = path.strip_prefix("/cdn/emojis/") {
        let (id, ext) = file.split_once('.').unwrap_or((file, ""));
        // Static emoji are served as PNG only when stored as PNG, like an
        // upload that does not convert.
        st.emoji(id)
            .filter(|e| ext == "webp" || (ext == "png" && e.bytes.starts_with(b"\x89PNG")))
            .map(|e| e.bytes.clone())
    } else {
        None
    };
    match found {
        Some(bytes) => bytes.into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn handle(State(state): State<Shared>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let headers = req.headers().clone();
    if path.starts_with("/cdn/") || path.starts_with("/media/") {
        return cdn(state, &path, &headers).await;
    }
    let Some(rest) = path.strip_prefix("/api/v10/") else {
        return respond(
            StatusCode::NOT_FOUND,
            json!({"message": "404: Not Found", "code": 0}),
            &[],
        );
    };
    let segments: Vec<String> = rest.split('/').map(str::to_string).collect();
    let method = req.method().clone();
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let route = route_key(&method, &segments);
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let mut parsed = Req {
        method: method.clone(),
        segments,
        json: Value::Null,
        form: HashMap::new(),
    };
    if content_type.starts_with("multipart/form-data") {
        let mut form = match Multipart::from_request(req, &()).await {
            Ok(f) => f,
            Err(e) => return respond(StatusCode::BAD_REQUEST, json!({"message": e.to_string()}), &[]),
        };
        while let Ok(Some(field)) = form.next_field().await {
            let name = field.name().unwrap_or_default().to_string();
            let filename = field.file_name().map(str::to_string);
            let bytes = field.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            parsed.form.insert(name, (filename, bytes));
        }
    } else if content_type.starts_with("application/json") {
        let body = axum::body::to_bytes(req.into_body(), 4 << 20).await.unwrap_or_default();
        parsed.json = serde_json::from_slice(&body).unwrap_or(Value::Null);
    }

    let mut st = state.lock().unwrap();
    st.auth_headers.push(auth.clone());
    st.calls.push(route.clone());
    st.call_times.push((route.clone(), Instant::now()));
    if auth != format!("Bot {TOKEN}") {
        return respond(
            StatusCode::UNAUTHORIZED,
            json!({"message": "401: Unauthorized", "code": 0}),
            &[],
        );
    }
    if method != Method::GET && !headers.contains_key("x-audit-log-reason") {
        st.missing_audit_reason += 1;
    }
    let retry_after = st.retry_after;
    let global = st.global;
    if let Some(n) = st.rate_limit.get_mut(&route) {
        if *n > 0 {
            *n -= 1;
            let mut extra = vec![
                ("retry-after", format!("{}", retry_after.ceil() as u64)),
                ("x-ratelimit-scope", if global { "global" } else { "user" }.to_string()),
            ];
            if global {
                extra.push(("x-ratelimit-global", "true".to_string()));
            }
            return respond(
                StatusCode::TOO_MANY_REQUESTS,
                json!({"message": "You are being rate limited.", "retry_after": retry_after, "global": global}),
                &extra,
            );
        }
    }
    if let Some((status, n)) = st.fail.get_mut(&route) {
        if *n > 0 {
            *n -= 1;
            let status = StatusCode::from_u16(*status).unwrap();
            return respond(status, json!({"message": "injected failure", "code": 0}), &[]);
        }
    }
    if let Some((status, n)) = st.fail_after.get_mut(&route) {
        if *n == 0 {
            let status = StatusCode::from_u16(*status).unwrap();
            st.fail_after.remove(&route);
            return respond(status, json!({"message": "injected failure", "code": 0}), &[]);
        }
        *n -= 1;
    }
    let exhaust = st.exhaust.get(&route).copied();
    match st.dispatch(&parsed) {
        Ok((status, body)) => {
            let mut extra = vec![("x-ratelimit-bucket", "abcd1234".to_string())];
            match exhaust {
                Some(after) => {
                    extra.push(("x-ratelimit-limit", "1".into()));
                    extra.push(("x-ratelimit-remaining", "0".into()));
                    extra.push(("x-ratelimit-reset-after", format!("{after}")));
                }
                None => {
                    extra.push(("x-ratelimit-limit", "5".into()));
                    extra.push(("x-ratelimit-remaining", "4".into()));
                    extra.push(("x-ratelimit-reset-after", "1.0".into()));
                }
            }
            respond(status, body, &extra)
        }
        Err((status, body)) => respond(status, body, &[]),
    }
}

/// Starts the mock; returns (API base, CDN base, media base, state).
pub async fn start() -> (String, String, String, Shared) {
    let state: Shared = Arc::new(Mutex::new(MockState {
        retry_after: 1.0,
        ..Default::default()
    }));
    let app = Router::new().fallback(handle).with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (
        format!("http://{addr}/api/v10"),
        format!("http://{addr}/cdn"),
        format!("http://{addr}/media"),
        state,
    )
}

/// A PNG header the crate's checks accept (pixel data is not decoded);
/// `seed` makes the content unique.
pub fn png(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let chunk = |out: &mut Vec<u8>, kind: &[u8], data: &[u8]| {
        out.extend((data.len() as u32).to_be_bytes());
        out.extend(kind);
        out.extend(data);
        out.extend([0, 0, 0, 0]);
    };
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &[seed, seed, seed]);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// A GIF with `frames` frames (looping when animated); `seed` makes it unique.
pub fn gif(w: u16, h: u16, frames: u32, seed: u8) -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend(w.to_le_bytes());
    out.extend(h.to_le_bytes());
    out.extend([0x80, 0, 0, 0, 0, 0, 255, 255, seed]);
    if frames > 1 {
        out.extend([0x21, 0xFF, 11]);
        out.extend(b"NETSCAPE2.0");
        out.extend([3, 1, 0, 0, 0]);
    }
    for _ in 0..frames {
        out.extend([0x21, 0xF9, 4, 0, 10, 0, 0, 0, 0x2C, 0, 0, 0, 0]);
        out.extend(w.to_le_bytes());
        out.extend(h.to_le_bytes());
        out.extend([0, 2, 2, 0x4C, 0x01, 0]);
    }
    out.push(0x3B);
    out
}
