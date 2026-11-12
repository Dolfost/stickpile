//! Static assets compiled into the binary, served under `/static/`.
//!
//! Pages link them through [`url`], which appends a content hash
//! (`/static/app.css?v=<hash>`), so the long immutable cache lifetime is safe
//! across upgrades.

use std::sync::LazyLock;

use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use sha2::{Digest, Sha256};

use crate::state::AppState;

pub const HTMX: &[u8] = include_bytes!("../assets/htmx.min.js");
pub const APP_CSS: &[u8] = include_bytes!("../assets/app.css");
pub const APP_JS: &[u8] = include_bytes!("../assets/app.js");
pub const UPLOAD_JS: &[u8] = include_bytes!("../assets/upload.js");
pub const EMOJI_JS: &[u8] = include_bytes!("../assets/emoji.js");
/// Display preferences (theme, view options); loaded without `defer`.
pub const PREFS_JS: &[u8] = include_bytes!("../assets/prefs.js");
/// The emoji picker's data, from Unicode emoji and CLDR files
/// (`scripts/gen-emoji-data.py`, Unicode License v3).
pub const EMOJI_JSON: &[u8] = include_bytes!("../assets/emoji.json");
pub const FAVICON: &[u8] = include_bytes!("../assets/favicon.svg");

const JS: &str = "text/javascript; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const SVG: &str = "image/svg+xml";
const JSON: &str = "application/json";

/// (name, bytes, content type) of every asset.
const ALL: [(&str, &[u8], &str); 8] = [
    ("htmx.min.js", HTMX, JS),
    ("app.css", APP_CSS, CSS),
    ("app.js", APP_JS, JS),
    ("upload.js", UPLOAD_JS, JS),
    ("emoji.js", EMOJI_JS, JS),
    ("emoji.json", EMOJI_JSON, JSON),
    ("prefs.js", PREFS_JS, JS),
    ("favicon.svg", FAVICON, SVG),
];

static VERSION: LazyLock<String> = LazyLock::new(|| {
    let mut h = Sha256::new();
    for (name, bytes, _) in ALL {
        h.update(name.as_bytes());
        h.update(bytes);
    }
    hex::encode(&h.finalize()[..6])
});

/// Cache-busting URL of an asset, e.g. `/static/app.css?v=1a2b3c4d5e6f`.
pub fn url(name: &str) -> String {
    format!("/static/{name}?v={}", *VERSION)
}

fn asset(bytes: &'static [u8], content_type: &'static str) -> Response {
    let mut resp = bytes.into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    resp
}

/// `/static/*` routes.
pub fn routes() -> Router<AppState> {
    let mut r = Router::new();
    for (name, bytes, content_type) in ALL {
        r = r.route(
            &format!("/static/{name}"),
            get(move || async move { asset(bytes, content_type) }),
        );
    }
    r
}
