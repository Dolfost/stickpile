//! In-process mock of Signal's chat service (the sticker upload form) and
//! CDN (pack files, S3 POST uploads), on two 127.0.0.1:0 listeners.
//!
//! Modelled on Signal-Server's `StickerController` (form for 1-201 files,
//! keys `stickers/<pack id>/manifest.proto` and `stickers/<pack id>/full/<n>`,
//! size limits 10 KiB and 301 KiB, Basic auth `<aci>.<device>:<password>`)
//! and on what Signal-Desktop sends to the CDN (the policy fields in a fixed
//! order, the file last). The CDN answers 403 for missing files, as Signal's
//! does according to signalstickers-client. Nothing here was observed on
//! Signal's servers.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{json, Value};

pub const ACI: &str = "8f3a1c2e-1b2c-4d5e-8f90-123456789abc";
pub const DEVICE: u32 = 2;
pub const PASSWORD: &str = "SECRET-device-password";

/// The fields of one issued upload policy.
#[derive(Clone, Debug)]
pub struct Policy {
    pub key: String,
    pub credential: String,
    pub policy: String,
    pub signature: String,
    pub max_bytes: usize,
}

#[derive(Default)]
pub struct MockState {
    /// CDN objects by key (`stickers/<id>/...`).
    pub objects: HashMap<String, Vec<u8>>,
    /// Policies handed out, by object key.
    pub policies: HashMap<String, Policy>,
    /// (Basic auth user, file count) of every form request that passed auth.
    pub forms: Vec<(String, usize)>,
    /// Form requests answered with 401.
    pub unauthorized: u32,
    /// How many more form requests to answer with 413 and Retry-After.
    pub rate_limited: u32,
    /// Field names of every CDN upload, in order.
    pub upload_fields: Vec<Vec<String>>,
    /// CDN uploads refused, with the reason.
    pub refused: Vec<String>,
    /// A CDN request carried an Authorization header.
    pub cdn_saw_auth: bool,
    /// CDN GET paths.
    pub downloads: Vec<String>,
    next: u64,
}

pub type Shared = Arc<Mutex<MockState>>;

impl MockState {
    fn pack_id(&mut self) -> String {
        self.next += 1;
        format!("{:032x}", 0xfeed_0000_0000u64 + self.next)
    }

    fn policy(&mut self, key: String, max_bytes: usize) -> Value {
        self.next += 1;
        let p = Policy {
            credential: format!("AKIAMOCK/20261116/us-east-1/s3/aws4_request#{}", self.next),
            policy: format!("policy-for-{key}"),
            signature: format!("{:064x}", self.next),
            key: key.clone(),
            max_bytes,
        };
        let v = json!({
            "key": p.key, "credential": p.credential, "acl": "private",
            "algorithm": "AWS4-HMAC-SHA256", "date": "20261116T120000Z",
            "policy": p.policy, "signature": p.signature,
        });
        self.policies.insert(key, p);
        v
    }
}

fn basic_user(headers: &axum::http::HeaderMap) -> Option<(String, String)> {
    use base64_lite::decode;
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = String::from_utf8(decode(encoded)?).ok()?;
    let (user, pass) = decoded.split_once(':')?;
    Some((user.to_string(), pass.to_string()))
}

/// Minimal base64 decoding for the Basic auth header.
mod base64_lite {
    pub fn decode(s: &str) -> Option<Vec<u8>> {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let mut acc = 0u32;
        let mut bits = 0;
        for c in s.trim().bytes().filter(|c| *c != b'=') {
            let v = alphabet.iter().position(|a| *a == c)? as u32;
            acc = acc << 6 | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        Some(out)
    }
}

async fn service(State(state): State<Shared>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let Some(count) = path.strip_prefix("/v1/sticker/pack/form/") else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    if req.method() != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let mut st = state.lock().unwrap();
    let expected_user = format!("{ACI}.{DEVICE}");
    match basic_user(req.headers()) {
        Some((user, pass)) if user == expected_user && pass == PASSWORD => {}
        _ => {
            st.unauthorized += 1;
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    if st.rate_limited > 0 {
        st.rate_limited -= 1;
        let mut r = StatusCode::PAYLOAD_TOO_LARGE.into_response();
        r.headers_mut().insert("retry-after", HeaderValue::from_static("4320"));
        return r;
    }
    let Ok(count) = count.parse::<usize>() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !(1..=201).contains(&count) {
        return (StatusCode::BAD_REQUEST, "count must be 1 to 201").into_response();
    }
    st.forms.push((expected_user, count));
    let pack_id = st.pack_id();
    let manifest = st.policy(format!("stickers/{pack_id}/manifest.proto"), 10 * 1024);
    let mut manifest = manifest;
    manifest["id"] = json!(-1);
    let stickers: Vec<Value> = (0..count)
        .map(|i| {
            let mut v = st.policy(format!("stickers/{pack_id}/full/{i}"), 301 * 1024);
            v["id"] = json!(i);
            v
        })
        .collect();
    Json(json!({"packId": pack_id, "manifest": manifest, "stickers": stickers})).into_response()
}

const FIELD_ORDER: [&str; 9] = [
    "key",
    "x-amz-credential",
    "acl",
    "x-amz-algorithm",
    "x-amz-date",
    "policy",
    "x-amz-signature",
    "Content-Type",
    "file",
];

async fn cdn(State(state): State<Shared>, req: Request) -> Response {
    if req.headers().contains_key(header::AUTHORIZATION) {
        state.lock().unwrap().cdn_saw_auth = true;
    }
    let path = req.uri().path().to_string();
    if req.method() == Method::GET {
        let mut st = state.lock().unwrap();
        st.downloads.push(path.clone());
        let key = path.trim_start_matches('/');
        return match st.objects.get(key) {
            Some(bytes) => bytes.clone().into_response(),
            // Signal's CDN answers 403 for files that do not exist.
            None => (StatusCode::FORBIDDEN, "AccessDenied").into_response(),
        };
    }
    if req.method() != Method::POST || path != "/" {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let mut form = match Multipart::from_request(req, &()).await {
        Ok(f) => f,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    let mut fields: Vec<(String, Vec<u8>)> = Vec::new();
    while let Ok(Some(field)) = form.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        let bytes = field.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
        fields.push((name, bytes));
    }
    let mut st = state.lock().unwrap();
    let names: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
    st.upload_fields.push(names.clone());
    let refuse = |st: &mut MockState, why: String| {
        st.refused.push(why.clone());
        (StatusCode::FORBIDDEN, why).into_response()
    };
    if names != FIELD_ORDER {
        return refuse(&mut st, format!("fields out of order: {names:?}"));
    }
    let text = |i: usize| String::from_utf8_lossy(&fields[i].1).to_string();
    let key = text(0);
    let Some(policy) = st.policies.get(&key).cloned() else {
        return refuse(&mut st, format!("no policy for {key}"));
    };
    if text(1) != policy.credential || text(5) != policy.policy || text(6) != policy.signature {
        return refuse(&mut st, format!("policy fields do not match for {key}"));
    }
    if text(2) != "private" || text(3) != "AWS4-HMAC-SHA256" || text(7) != "application/octet-stream" {
        return refuse(&mut st, "unexpected acl, algorithm or content type".into());
    }
    let file = fields[8].1.clone();
    if file.is_empty() || file.len() > policy.max_bytes {
        return refuse(&mut st, format!("{key}: {} bytes is outside the policy", file.len()));
    }
    st.objects.insert(key, file);
    StatusCode::NO_CONTENT.into_response()
}

/// Starts the mock; returns (service URL, CDN URL, state).
pub async fn start() -> (String, String, Shared) {
    let state: Shared = Arc::default();
    let service_app = Router::new().fallback(service).with_state(state.clone());
    let cdn_app = Router::new().fallback(cdn).with_state(state.clone());
    let service_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cdn_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let service_addr = service_listener.local_addr().unwrap();
    let cdn_addr = cdn_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(service_listener, service_app).await.unwrap() });
    tokio::spawn(async move { axum::serve(cdn_listener, cdn_app).await.unwrap() });
    (format!("http://{service_addr}"), format!("http://{cdn_addr}"), state)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    out.extend(kind);
    out.extend(data);
    out.extend([0, 0, 0, 0]);
}

/// A PNG (or, with `frames`, an APNG looping forever with 100 ms frames)
/// header the crate's checks accept; `seed` makes the content unique.
pub fn png(side: u32, frames: Option<u32>, seed: u8) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = side.to_be_bytes().to_vec();
    ihdr.extend(side.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    if let Some(n) = frames {
        let mut actl = n.to_be_bytes().to_vec();
        actl.extend(0u32.to_be_bytes());
        chunk(&mut out, b"acTL", &actl);
        for i in 0..n {
            let mut fctl = i.to_be_bytes().to_vec();
            fctl.extend(side.to_be_bytes());
            fctl.extend(side.to_be_bytes());
            fctl.extend([0; 8]);
            fctl.extend(100u16.to_be_bytes());
            fctl.extend(1000u16.to_be_bytes());
            fctl.extend([0, 0]);
            chunk(&mut out, b"fcTL", &fctl);
        }
    }
    chunk(&mut out, b"IDAT", &[seed; 8]);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// A lossless WebP header of `side` x `side`; `seed` makes it unique.
pub fn webp(side: u32, seed: u8) -> Vec<u8> {
    let mut out = b"RIFF\0\0\0\0WEBPVP8L\x09\0\0\0\x2f".to_vec();
    let bits: u32 = (side - 1) | ((side - 1) << 14);
    out.extend(bits.to_le_bytes());
    out.extend([seed, seed, seed, seed, 0]);
    out
}
