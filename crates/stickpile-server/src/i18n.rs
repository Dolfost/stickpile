//! The language of a request (docs/languages.md): the signed-in user's
//! choice, else the browser's `Accept-Language`, else English. The
//! middleware sets it for the whole request (handlers, the web UI's
//! rendering, error messages), and [`t`] and friends read it.

use std::fmt::Display;

use axum::extract::Request;
use axum::http::{header, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
pub use stickpile_i18n::Lang;

use crate::auth::AuthOutcome;

tokio::task_local! {
    static LANG: Lang;
}

/// The language of the request being handled (English outside one).
pub fn current() -> Lang {
    LANG.try_with(|l| *l).unwrap_or_default()
}

/// `en` in the request's language.
pub fn t(en: &'static str) -> &'static str {
    stickpile_i18n::t(current(), en)
}

/// [`t`] for one English text that needs different translations by
/// context: `tc("object", "name")` (catalog key `object::name`).
pub fn tc(ctx: &'static str, en: &'static str) -> &'static str {
    stickpile_i18n::tc(current(), ctx, en)
}

/// [`t`] with `{name}` placeholders.
pub fn tf(en: &'static str, args: &[(&str, &dyn Display)]) -> String {
    stickpile_i18n::tf(current(), en, args)
}

/// A counted phrase: `tp(n, "{n} sticker", "{n} stickers")`.
pub fn tp(n: u64, one: &'static str, many: &'static str) -> String {
    stickpile_i18n::tp(current(), n, one, many)
}

/// A text that is not a literal (an error message), translated when the
/// catalog has it.
pub fn t_dyn(en: &str) -> String {
    stickpile_i18n::t_dyn(current(), en)
}

/// The language for `req` (see the module docs). Runs after the auth
/// middleware, which leaves the [`AuthOutcome`] in the extensions.
pub fn request_lang(req: &Request) -> Lang {
    let chosen = req
        .extensions()
        .get::<AuthOutcome>()
        .and_then(|o| o.result.as_ref().ok())
        .and_then(|u| u.as_ref())
        .and_then(|u| u.user.lang());
    chosen
        .or_else(|| {
            req.headers()
                .get(header::ACCEPT_LANGUAGE)
                .and_then(|v| v.to_str().ok())
                .and_then(Lang::from_accept_language)
        })
        .unwrap_or_default()
}

/// Run the rest of the request in its language; the response says which
/// (`Content-Language`).
pub async fn middleware(req: Request, next: Next) -> Response {
    let lang = request_lang(&req);
    LANG.scope(lang, async move {
        let mut resp = next.run(req).await;
        resp.headers_mut()
            .insert(header::CONTENT_LANGUAGE, HeaderValue::from_static(lang.code()));
        resp
    })
    .await
}

/// Run `f` as if handling a request in `lang` (tests, background renders).
pub async fn with_lang<F: std::future::Future>(lang: Lang, f: F) -> F::Output {
    LANG.scope(lang, f).await
}
