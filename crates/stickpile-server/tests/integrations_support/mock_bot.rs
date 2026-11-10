//! Compact in-process mock of the Telegram Bot API sticker methods (axum on
//! 127.0.0.1:0), enough for what `stickpile-telegram` calls: `getMe`,
//! `getStickerSet`, `getFile` + file downloads, `createNewStickerSet`,
//! `addStickerToSet`, `replaceStickerInSet`, `deleteStickerFromSet`,
//! `setStickerPositionInSet`, `setStickerEmojiList`, `setStickerKeywords`,
//! `setStickerSetTitle`. JSON or multipart requests (`attach://` parts),
//! `{ok, result}` envelopes, `file_unique_id` derived from the content, new
//! stickers appended, and the last sticker of a pack cannot be deleted.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{json, Map, Value};

pub const TOKEN: &str = "777:mock-bot-token";
pub const BOT: &str = "pile_test_bot";

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
}

#[derive(Default)]
pub struct MockState {
    pub packs: BTreeMap<String, MockPack>,
    /// file_id -> (file_path, bytes)
    files: HashMap<String, (String, Vec<u8>)>,
    /// Method names in call order.
    pub calls: Vec<String>,
    next: u64,
}

pub type Shared = Arc<Mutex<MockState>>;
type R<T> = Result<T, String>;

pub fn unique_id(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("U{h:016x}")
}

impl MockState {
    fn sticker(&mut self, bytes: Vec<u8>, format: &str, emoji_list: Vec<String>, keywords: Vec<String>) -> MockSticker {
        self.next += 1;
        let ext = match format {
            "video" => "webm",
            "animated" => "tgs",
            _ => "webp",
        };
        let file_id = format!("F{}", self.next);
        self.files.insert(
            file_id.clone(),
            (format!("stickers/f{}.{ext}", self.next), bytes.clone()),
        );
        MockSticker {
            file_id,
            fuid: unique_id(&bytes),
            format: format.into(),
            emoji_list,
            keywords,
            bytes,
        }
    }

    /// A pack created outside stickpile: (format, bytes, emoji) per sticker.
    pub fn seed(&mut self, name: &str, title: &str, owner: i64, stickers: Vec<(&str, Vec<u8>, &str)>) {
        let stickers = stickers
            .into_iter()
            .map(|(f, b, e)| self.sticker(b, f, vec![e.to_string()], vec![]))
            .collect();
        self.packs.insert(
            name.into(),
            MockPack {
                title: title.into(),
                owner,
                stickers,
            },
        );
    }

    pub fn count(&self, method: &str) -> usize {
        self.calls.iter().filter(|c| *c == method).count()
    }

    fn find(&self, file_id: &str) -> R<(String, usize)> {
        for (name, p) in &self.packs {
            if let Some(i) = p.stickers.iter().position(|s| s.file_id == file_id) {
                return Ok((name.clone(), i));
            }
        }
        Err("STICKER_ID_INVALID".into())
    }

    fn owned(&mut self, p: &Map<String, Value>) -> R<&mut MockPack> {
        let user = int(p, "user_id")?;
        let pack = self.packs.get_mut(&text(p, "name")?).ok_or("STICKERSET_INVALID")?;
        if pack.owner != user {
            return Err("USER_ID_INVALID".into());
        }
        Ok(pack)
    }

    fn input(&mut self, v: &Value, files: &HashMap<String, Vec<u8>>) -> R<MockSticker> {
        let part = v["sticker"]
            .as_str()
            .and_then(|a| a.strip_prefix("attach://"))
            .ok_or("expected attach://")?;
        let bytes = files.get(part).ok_or("file part missing")?.clone();
        let format = v["format"].as_str().unwrap_or("");
        if !["static", "video"].contains(&format) {
            return Err("invalid sticker format".into());
        }
        let emoji_list = strings(&v["emoji_list"])?;
        if emoji_list.is_empty() || emoji_list.len() > 20 {
            return Err("invalid emoji list".into());
        }
        let keywords = if v.get("keywords").is_some() {
            strings(&v["keywords"])?
        } else {
            vec![]
        };
        Ok(self.sticker(bytes, format, emoji_list, keywords))
    }

    fn dispatch(&mut self, method: &str, p: &Map<String, Value>, files: &HashMap<String, Vec<u8>>) -> R<Value> {
        match method {
            "getMe" => Ok(json!({"id": 4242, "is_bot": true, "first_name": "Pile", "username": BOT})),
            "getStickerSet" => {
                let name = text(p, "name")?;
                let pack = self.packs.get(&name).ok_or("STICKERSET_INVALID")?;
                let stickers: Vec<Value> = pack
                    .stickers
                    .iter()
                    .map(|s| {
                        json!({"file_id": s.file_id, "file_unique_id": s.fuid, "type": "regular",
                               "width": 512, "height": 512, "is_animated": s.format == "animated",
                               "is_video": s.format == "video", "emoji": s.emoji_list.first()})
                    })
                    .collect();
                Ok(json!({"name": name, "title": pack.title, "sticker_type": "regular", "stickers": stickers}))
            }
            "getFile" => {
                let id = text(p, "file_id")?;
                let (path, bytes) = self.files.get(&id).ok_or("invalid file_id")?;
                Ok(
                    json!({"file_id": id, "file_unique_id": unique_id(bytes), "file_size": bytes.len(), "file_path": path}),
                )
            }
            "createNewStickerSet" => {
                let name = text(p, "name")?;
                if !name.to_ascii_lowercase().ends_with(&format!("_by_{BOT}")) {
                    return Err("invalid sticker set name is specified".into());
                }
                if self.packs.contains_key(&name) {
                    return Err("sticker set name is already occupied".into());
                }
                let list = json_param(p, "stickers")?.as_array().cloned().unwrap_or_default();
                if list.is_empty() || list.len() > 50 {
                    return Err("invalid number of stickers".into());
                }
                let mut pack = MockPack {
                    title: text(p, "title")?,
                    owner: int(p, "user_id")?,
                    stickers: vec![],
                };
                for v in &list {
                    let s = self.input(v, files)?;
                    if !pack.stickers.iter().any(|x| x.fuid == s.fuid) {
                        pack.stickers.push(s);
                    }
                }
                self.packs.insert(name, pack);
                Ok(json!(true))
            }
            "addStickerToSet" => {
                let v = json_param(p, "sticker")?;
                self.owned(p)?;
                let s = self.input(&v, files)?;
                let pack = self.owned(p)?;
                if pack.stickers.len() >= 120 {
                    return Err("STICKERS_TOO_MUCH".into());
                }
                if !pack.stickers.iter().any(|x| x.fuid == s.fuid) {
                    pack.stickers.push(s);
                }
                Ok(json!(true))
            }
            "replaceStickerInSet" => {
                let v = json_param(p, "sticker")?;
                let old = text(p, "old_sticker")?;
                let i = self
                    .owned(p)?
                    .stickers
                    .iter()
                    .position(|s| s.file_id == old)
                    .ok_or("STICKER_ID_INVALID")?;
                let s = self.input(&v, files)?;
                self.owned(p)?.stickers[i] = s;
                Ok(json!(true))
            }
            "deleteStickerFromSet" => {
                let (name, i) = self.find(&text(p, "sticker")?)?;
                let pack = self.packs.get_mut(&name).unwrap();
                if pack.stickers.len() == 1 {
                    return Err("mock refuses to delete the last sticker of a pack".into());
                }
                pack.stickers.remove(i);
                Ok(json!(true))
            }
            "setStickerPositionInSet" => {
                let (name, i) = self.find(&text(p, "sticker")?)?;
                let pos = int(p, "position")?;
                let pack = self.packs.get_mut(&name).unwrap();
                if pos < 0 || pos as usize >= pack.stickers.len() {
                    return Err("STICKER_POSITION_INVALID".into());
                }
                let s = pack.stickers.remove(i);
                pack.stickers.insert(pos as usize, s);
                Ok(json!(true))
            }
            "setStickerEmojiList" => {
                let (name, i) = self.find(&text(p, "sticker")?)?;
                let list = strings(&json_param(p, "emoji_list")?)?;
                if list.is_empty() {
                    return Err("invalid emoji list".into());
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
                self.packs.get_mut(&name).unwrap().stickers[i].keywords = list;
                Ok(json!(true))
            }
            "setStickerSetTitle" => {
                let pack = self.packs.get_mut(&text(p, "name")?).ok_or("STICKERSET_INVALID")?;
                pack.title = text(p, "title")?;
                Ok(json!(true))
            }
            _ => Err("method not found".into()),
        }
    }
}

fn text(p: &Map<String, Value>, key: &str) -> R<String> {
    match p.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Number(n)) => Ok(n.to_string()),
        _ => Err(format!("parameter {key} missing")),
    }
}

fn int(p: &Map<String, Value>, key: &str) -> R<i64> {
    text(p, key)?.parse().map_err(|_| format!("{key} is not an integer"))
}

/// Arrays/objects arrive as JSON values (JSON body) or JSON text (multipart).
fn json_param(p: &Map<String, Value>, key: &str) -> R<Value> {
    match p.get(key) {
        Some(Value::String(s)) => serde_json::from_str(s).map_err(|_| format!("{key} is not JSON")),
        Some(v) => Ok(v.clone()),
        None => Err(format!("parameter {key} missing")),
    }
}

fn strings(v: &Value) -> R<Vec<String>> {
    v.as_array()
        .ok_or("expected an array")?
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::to_string)
                .ok_or_else(|| "expected strings".to_string())
        })
        .collect()
}

async fn parse_body(req: Request) -> R<(Map<String, Value>, HashMap<String, Vec<u8>>)> {
    let ct = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let mut params = Map::new();
    let mut files = HashMap::new();
    if ct.starts_with("multipart/form-data") {
        let mut form = Multipart::from_request(req, &()).await.map_err(|e| e.to_string())?;
        while let Some(field) = form.next_field().await.map_err(|e| e.to_string())? {
            let name = field.name().unwrap_or_default().to_string();
            if field.file_name().is_some() {
                files.insert(name, field.bytes().await.map_err(|e| e.to_string())?.to_vec());
            } else {
                params.insert(name, Value::String(field.text().await.map_err(|e| e.to_string())?));
            }
        }
    } else if ct.starts_with("application/json") {
        let body = axum::body::to_bytes(req.into_body(), 1 << 20)
            .await
            .map_err(|e| e.to_string())?;
        if let Value::Object(m) = serde_json::from_slice(&body).map_err(|e| e.to_string())? {
            params = m;
        }
    }
    Ok((params, files))
}

fn fail(description: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"ok": false, "error_code": 400, "description": format!("Bad Request: {description}")})),
    )
        .into_response()
}

async fn handle(State(state): State<Shared>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    if let Some(rest) = path.strip_prefix(&format!("/file/bot{TOKEN}/")) {
        let st = state.lock().unwrap();
        return match st.files.values().find(|(p, _)| p == rest) {
            Some((_, b)) => b.clone().into_response(),
            None => (StatusCode::NOT_FOUND, "no such file").into_response(),
        };
    }
    let Some(method) = path.strip_prefix(&format!("/bot{TOKEN}/")) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"ok": false, "error_code": 401, "description": "Unauthorized"})),
        )
            .into_response();
    };
    let method = method.to_string();
    let (params, files) = match parse_body(req).await {
        Ok(x) => x,
        Err(e) => return fail(&e),
    };
    let mut st = state.lock().unwrap();
    st.calls.push(method.clone());
    match st.dispatch(&method, &params, &files) {
        Ok(v) => Json(json!({"ok": true, "result": v})).into_response(),
        Err(e) => fail(&e),
    }
}

/// Start the mock; returns its base URL and state.
pub async fn start() -> (String, Shared) {
    let state: Shared = Arc::default();
    let app = Router::new().fallback(handle).with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), state)
}
