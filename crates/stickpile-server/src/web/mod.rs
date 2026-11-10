//! Server-rendered web UI (maud + htmx). Placeholder: the web UI agent
//! replaces this module.
//!
//! How pages plug into the rest of the server:
//!
//! * Authentication: take [`crate::auth::CurrentUser`] (or
//!   [`crate::auth::MaybeUser`] for pages that also render for anonymous
//!   visitors). Log in with [`crate::auth::login::password_login`] (purpose
//!   `WebSession`), then [`crate::auth::session::create`] and send its
//!   `cookie` as `Set-Cookie`; log out with [`crate::auth::session::destroy`]
//!   plus [`crate::auth::session::clear_cookie_header`].
//! * CSRF: forms carry `<input type=hidden name=csrf_token>` (urlencoded
//!   bodies are checked by the auth middleware), htmx requests send the
//!   `X-CSRF-Token` header (`hx-headers` on `<body>`); the token is
//!   [`crate::auth::CurrentUser::csrf_token`]. Login and register forms have no
//!   session yet: call [`crate::security::check_same_origin`] in the handler.
//! * Business logic: call the same [`crate::service`] functions as the JSON
//!   API (they do their own authorization).

use axum::response::Html;
use axum::routing::get;
use axum::Router;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(index))
}

async fn index() -> Html<&'static str> {
    Html(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>stickpile</title></head>\
         <body><h1>stickpile</h1><p>The web UI is not available yet. The REST API lives under \
         <a href=\"/api/v1/openapi.json\">/api/v1</a>.</p></body></html>",
    )
}
