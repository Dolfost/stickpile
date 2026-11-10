//! Static assets compiled into the binary, served under `/static/`.

use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::state::AppState;

pub const HTMX: &[u8] = include_bytes!("../assets/htmx.min.js");

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

/// `/static/*` routes. The web UI agent adds its CSS/JS here.
pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/static/htmx.min.js",
        get(|| async { asset(HTMX, "text/javascript; charset=utf-8") }),
    )
}
