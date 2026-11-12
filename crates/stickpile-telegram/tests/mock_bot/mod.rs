//! In-process mock of the Bot API sticker methods (axum on 127.0.0.1:0).
//!
//! Behaves like Telegram where stickpile depends on it: `{ok, result}` /
//! `{ok: false, error_code, description, parameters}` envelopes, JSON or
//! multipart requests with `attach://` parts, `file_unique_id` derived from
//! the file content (so identical uploads are deduplicated within a set),
//! new stickers appended at the end, and file downloads under
//! `/file/bot<token>/<file_path>`. It additionally refuses to delete the
//! last sticker of a pack, to catch exports that empty a pack mid-way.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{json, Map, Value};

pub const TOKEN: &str = "123456:SECRET-token_value";
pub const BOT: &str = "stickpile_test_bot";
pub const BOT_ID: i64 = 4242;
pub const OWNER: i64 = 1001;

#[derive(Clone, Debug)]
pub struct MockSticker {
    pub file_id: String,
    pub fuid: String,
    pub format: String,
    pub emoji_list: Vec<String>,
    pub keywords: Vec<String>,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct MockPack {
    pub title: String,
    pub owner: i64,
    pub stickers: Vec<MockSticker>,
    /// Set with setStickerSetThumbnail (None: the first sticker is shown).
    pub thumbnail: Option<Vec<u8>>,
}

#[derive(Default)]
pub struct MockState {
    pub packs: BTreeMap<String, MockPack>,
    /// file_id -> (file_path, bytes)
    pub files: HashMap<String, (String, Vec<u8>)>,
    /// Method names in call order (including rate-limited attempts).
    pub calls: Vec<String>,
    /// method -> how many more calls to answer with 429.
    pub rate_limit: HashMap<String, u32>,
    pub retry_after: u64,
    /// Answer every file download with 404.
    pub fail_downloads: bool,
    next: u64,
}

pub type Shared = Arc<Mutex<MockState>>;

type MockResult<T> = Result<T, (i64, String)>;

fn bad<T>(what: &str) -> MockResult<T> {
    Err((400, format!("Bad Request: {what}")))
}

/// Stand-in for Telegram's content-derived `file_unique_id` (FNV-1a).
pub fn unique_id(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("U{h:016x}")
}

impl MockState {
    pub fn new_sticker(
        &mut self,
        bytes: Vec<u8>,
        format: &str,
        emoji_list: Vec<String>,
        keywords: Vec<String>,
    ) -> MockSticker {
        self.next += 1;
        let ext = match format {
            "video" => "webm",
            "animated" => "tgs",
            _ => "webp",
        };
        let file_id = format!("FILE{}", self.next);
        self.files.insert(
            file_id.clone(),
            (format!("stickers/file_{}.{ext}", self.next), bytes.clone()),
        );
        MockSticker {
            file_id,
            fuid: unique_id(&bytes),
            format: format.to_string(),
            emoji_list,
            keywords,
            bytes,
        }
    }

    /// Creates a pack directly: (format, bytes, emoji) per sticker.
    pub fn seed_pack(&mut self, name: &str, title: &str, owner: i64, stickers: Vec<(&str, Vec<u8>, &str)>) {
        let stickers = stickers
            .into_iter()
            .map(|(format, bytes, emoji)| self.new_sticker(bytes, format, vec![emoji.to_string()], vec![]))
            .collect();
        self.packs.insert(
            name.to_string(),
            MockPack {
                title: title.to_string(),
                owner,
                stickers,
                thumbnail: None,
            },
        );
    }

    pub fn count(&self, method: &str) -> usize {
        self.calls.iter().filter(|c| *c == method).count()
    }

    pub fn pack(&self, name: &str) -> &MockPack {
        &self.packs[name]
    }

    fn find(&self, file_id: &str) -> MockResult<(String, usize)> {
        for (name, pack) in &self.packs {
            if let Some(i) = pack.stickers.iter().position(|s| s.file_id == file_id) {
                return Ok((name.clone(), i));
            }
        }
        bad("STICKER_ID_INVALID")
    }

    fn owned_pack(&mut self, params: &Map<String, Value>) -> MockResult<&mut MockPack> {
        let user = int(params, "user_id")?;
        let name = text(params, "name")?;
        let pack = self
            .packs
            .get_mut(&name)
            .ok_or((400, "Bad Request: STICKERSET_INVALID".to_string()))?;
        if pack.owner != user {
            return bad("USER_ID_INVALID");
        }
        Ok(pack)
    }

    fn input_sticker(&mut self, v: &Value, files: &HashMap<String, Vec<u8>>) -> MockResult<MockSticker> {
        let attach = v["sticker"]
            .as_str()
            .ok_or((400, "Bad Request: sticker missing".to_string()))?;
        let Some(part) = attach.strip_prefix("attach://") else {
            return bad("expected attach://");
        };
        let Some(bytes) = files.get(part) else {
            return bad("file part missing");
        };
        let format = v["format"].as_str().unwrap_or("");
        if !["static", "animated", "video"].contains(&format) {
            return bad("invalid sticker format");
        }
        let emoji_list = strings(&v["emoji_list"])?;
        if !emoji_ok(&emoji_list) {
            return bad("STICKER_EMOJI_INVALID");
        }
        let keywords = if v.get("keywords").is_some() {
            strings(&v["keywords"])?
        } else {
            vec![]
        };
        if keywords.len() > 20 {
            return bad("too many keywords");
        }
        Ok(self.new_sticker(bytes.clone(), format, emoji_list, keywords))
    }

    fn dispatch(
        &mut self,
        method: &str,
        p: &Map<String, Value>,
        files: &HashMap<String, Vec<u8>>,
    ) -> MockResult<Value> {
        match method {
            "getMe" => Ok(json!({"id": BOT_ID, "is_bot": true, "first_name": "Stickpile", "username": BOT})),
            "getStickerSet" => {
                let name = text(p, "name")?;
                let pack = self
                    .packs
                    .get(&name)
                    .ok_or((400, "Bad Request: STICKERSET_INVALID".to_string()))?;
                Ok(pack_json(&name, pack))
            }
            "getFile" => {
                let id = text(p, "file_id")?;
                let Some((path, bytes)) = self.files.get(&id) else {
                    return bad("invalid file_id");
                };
                Ok(
                    json!({"file_id": id, "file_unique_id": unique_id(bytes), "file_size": bytes.len(), "file_path": path}),
                )
            }
            "createNewStickerSet" => {
                let user = int(p, "user_id")?;
                let name = text(p, "name")?;
                let title = text(p, "title")?;
                if !name.to_ascii_lowercase().ends_with(&format!("_by_{BOT}")) {
                    return bad("invalid sticker set name is specified");
                }
                if self.packs.contains_key(&name) {
                    return bad("sticker set name is already occupied");
                }
                let list = json_param(p, "stickers")?;
                let list = list.as_array().cloned().unwrap_or_default();
                if list.is_empty() || list.len() > 50 {
                    return bad("invalid number of stickers");
                }
                let mut pack = MockPack {
                    title,
                    owner: user,
                    stickers: vec![],
                    thumbnail: None,
                };
                for v in &list {
                    let s = self.input_sticker(v, files)?;
                    if !pack.stickers.iter().any(|x| x.fuid == s.fuid) {
                        pack.stickers.push(s);
                    }
                }
                self.packs.insert(name, pack);
                Ok(json!(true))
            }
            "addStickerToSet" => {
                let v = json_param(p, "sticker")?;
                if self.owned_pack(p)?.stickers.len() >= 120 {
                    return bad("STICKERS_TOO_MUCH");
                }
                let s = self.input_sticker(&v, files)?;
                let pack = self.owned_pack(p)?;
                if !pack.stickers.iter().any(|x| x.fuid == s.fuid) {
                    pack.stickers.push(s);
                }
                Ok(json!(true))
            }
            "replaceStickerInSet" => {
                let v = json_param(p, "sticker")?;
                let old = text(p, "old_sticker")?;
                let index = self.owned_pack(p)?.stickers.iter().position(|s| s.file_id == old);
                let Some(index) = index else {
                    return bad("STICKER_ID_INVALID");
                };
                let s = self.input_sticker(&v, files)?;
                let pack = self.owned_pack(p)?;
                if !pack.stickers.iter().any(|x| x.fuid == s.fuid) {
                    pack.stickers[index] = s;
                }
                Ok(json!(true))
            }
            "deleteStickerFromSet" => {
                let (name, i) = self.find(&text(p, "sticker")?)?;
                let pack = self.packs.get_mut(&name).unwrap();
                if pack.stickers.len() == 1 {
                    return bad("mock refuses to delete the last sticker of a pack");
                }
                pack.stickers.remove(i);
                Ok(json!(true))
            }
            "setStickerPositionInSet" => {
                let (name, i) = self.find(&text(p, "sticker")?)?;
                let position = int(p, "position")?;
                let pack = self.packs.get_mut(&name).unwrap();
                if position < 0 || position as usize >= pack.stickers.len() {
                    return bad("STICKER_POSITION_INVALID");
                }
                let s = pack.stickers.remove(i);
                pack.stickers.insert(position as usize, s);
                Ok(json!(true))
            }
            "setStickerEmojiList" => {
                let (name, i) = self.find(&text(p, "sticker")?)?;
                let list = strings(&json_param(p, "emoji_list")?)?;
                if !emoji_ok(&list) {
                    return bad("STICKER_EMOJI_INVALID");
                }
                self.packs.get_mut(&name).unwrap().stickers[i].emoji_list = list;
                Ok(json!(true))
            }
            "setStickerKeywords" => {
                let (name, i) = self.find(&text(p, "sticker")?)?;
                let list = if p.contains_key("keywords") {
                    strings(&json_param(p, "keywords")?)?
                } else {
                    vec![]
                };
                if list.len() > 20 || list.iter().map(|k| k.chars().count()).sum::<usize>() > 64 {
                    return bad("invalid keywords");
                }
                self.packs.get_mut(&name).unwrap().stickers[i].keywords = list;
                Ok(json!(true))
            }
            "setStickerSetThumbnail" => {
                if text(p, "format")? != "static" {
                    return bad("only static thumbnails here");
                }
                let thumbnail = match files.get("thumbnail") {
                    Some(b) if valid_thumbnail(b) => Some(b.clone()),
                    Some(_) => return bad("STICKERSET_THUMBNAIL_INVALID"),
                    None => None,
                };
                self.owned_pack(p)?.thumbnail = thumbnail;
                Ok(json!(true))
            }
            "setStickerSetTitle" => {
                let name = text(p, "name")?;
                let title = text(p, "title")?;
                let Some(pack) = self.packs.get_mut(&name) else {
                    return bad("STICKERSET_INVALID");
                };
                pack.title = title;
                Ok(json!(true))
            }
            _ => Err((404, "Not Found: method not found".to_string())),
        }
    }
}

fn pack_json(name: &str, pack: &MockPack) -> Value {
    let stickers: Vec<Value> = pack
        .stickers
        .iter()
        .map(|s| {
            json!({
                "file_id": s.file_id,
                "file_unique_id": s.fuid,
                "type": "regular",
                "width": 512,
                "height": 512,
                "is_animated": s.format == "animated",
                "is_video": s.format == "video",
                "emoji": s.emoji_list.first(),
                "set_name": name,
                "file_size": s.bytes.len(),
            })
        })
        .collect();
    json!({"name": name, "title": pack.title, "sticker_type": "regular", "stickers": stickers})
}

fn text(p: &Map<String, Value>, key: &str) -> MockResult<String> {
    match p.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Number(n)) => Ok(n.to_string()),
        _ => bad(&format!("parameter {key} missing")),
    }
}

fn int(p: &Map<String, Value>, key: &str) -> MockResult<i64> {
    match p.get(key) {
        Some(Value::Number(n)) => n.as_i64().ok_or((400, format!("Bad Request: {key} not an integer"))),
        Some(Value::String(s)) => s
            .parse()
            .map_err(|_| (400, format!("Bad Request: {key} not an integer"))),
        _ => bad(&format!("parameter {key} missing")),
    }
}

/// Arrays/objects arrive as JSON values (JSON body) or JSON text (multipart).
fn json_param(p: &Map<String, Value>, key: &str) -> MockResult<Value> {
    match p.get(key) {
        Some(Value::String(s)) => serde_json::from_str(s).map_err(|_| (400, format!("Bad Request: {key} is not JSON"))),
        Some(v) => Ok(v.clone()),
        None => bad(&format!("parameter {key} missing")),
    }
}

fn strings(v: &Value) -> MockResult<Vec<String>> {
    let Some(list) = v.as_array() else {
        return bad("expected an array");
    };
    list.iter()
        .map(|x| {
            x.as_str()
                .map(str::to_string)
                .ok_or((400, "Bad Request: expected strings".into()))
        })
        .collect()
}

/// Telegram's rule for a static set thumbnail: PNG or WebP, exactly 100×100,
/// at most 128 KB (the PNG size is read from its IHDR).
fn valid_thumbnail(bytes: &[u8]) -> bool {
    if bytes.len() > 128 * 1024 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.len() < 24 {
        return false;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    (w, h) == (100, 100)
}

/// Telegram refuses words as sticker emoji (STICKER_EMOJI_INVALID).
fn emoji_ok(list: &[String]) -> bool {
    !list.is_empty() && list.len() <= 20 && list.iter().all(|e| !e.chars().any(|c| c.is_ascii_alphabetic()))
}

fn ok(v: Value) -> Response {
    Json(json!({"ok": true, "result": v})).into_response()
}

fn error(code: i64, description: &str) -> Response {
    let status = StatusCode::from_u16(code as u16).unwrap_or(StatusCode::BAD_REQUEST);
    (
        status,
        Json(json!({"ok": false, "error_code": code, "description": description})),
    )
        .into_response()
}

async fn parse_body(req: Request) -> Result<(Map<String, Value>, HashMap<String, Vec<u8>>), String> {
    let content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let mut params = Map::new();
    let mut files = HashMap::new();
    if content_type.starts_with("multipart/form-data") {
        let mut form = Multipart::from_request(req, &()).await.map_err(|e| e.to_string())?;
        while let Some(field) = form.next_field().await.map_err(|e| e.to_string())? {
            let name = field.name().unwrap_or_default().to_string();
            if field.file_name().is_some() {
                files.insert(name, field.bytes().await.map_err(|e| e.to_string())?.to_vec());
            } else {
                params.insert(name, Value::String(field.text().await.map_err(|e| e.to_string())?));
            }
        }
    } else if content_type.starts_with("application/json") {
        let body = axum::body::to_bytes(req.into_body(), 1 << 20)
            .await
            .map_err(|e| e.to_string())?;
        match serde_json::from_slice(&body).map_err(|e| e.to_string())? {
            Value::Object(map) => params = map,
            _ => return Err("JSON body must be an object".into()),
        }
    }
    Ok((params, files))
}

async fn handle(State(state): State<Shared>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    if let Some(rest) = path.strip_prefix("/file/bot") {
        let Some(file_path) = rest.strip_prefix(TOKEN).and_then(|r| r.strip_prefix('/')) else {
            return (StatusCode::UNAUTHORIZED, "bad token").into_response();
        };
        let st = state.lock().unwrap();
        if st.fail_downloads {
            return (StatusCode::NOT_FOUND, "gone").into_response();
        }
        return match st.files.values().find(|(p, _)| p == file_path) {
            Some((_, bytes)) => bytes.clone().into_response(),
            None => (StatusCode::NOT_FOUND, "no such file").into_response(),
        };
    }
    let Some((token, method)) = path.strip_prefix("/bot").and_then(|r| r.split_once('/')) else {
        return error(404, "Not Found");
    };
    if token != TOKEN {
        return error(401, "Unauthorized");
    }
    let method = method.to_string();
    let (params, files) = match parse_body(req).await {
        Ok(parsed) => parsed,
        Err(e) => return error(400, &format!("Bad Request: {e}")),
    };
    let mut st = state.lock().unwrap();
    st.calls.push(method.clone());
    let retry_after = st.retry_after;
    if let Some(n) = st.rate_limit.get_mut(&method) {
        if *n > 0 {
            *n -= 1;
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "ok": false,
                    "error_code": 429,
                    "description": format!("Too Many Requests: retry after {retry_after}"),
                    "parameters": {"retry_after": retry_after},
                })),
            )
                .into_response();
        }
    }
    match st.dispatch(&method, &params, &files) {
        Ok(v) => ok(v),
        Err((code, description)) => error(code, &description),
    }
}

/// Starts the mock; returns its base URL and shared state.
pub async fn start() -> (String, Shared) {
    let state: Shared = Arc::new(Mutex::new(MockState {
        retry_after: 1,
        ..Default::default()
    }));
    let app = Router::new().fallback(handle).with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), state)
}
