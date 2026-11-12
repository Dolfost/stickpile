//! OpenAPI 3.1 document, generated from the `#[utoipa::path]` attributes of
//! the handlers and the `openapi` feature of stickpile-api, served at
//! [`stickpile_api::routes::OPENAPI`].

use std::sync::OnceLock;

use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use super::{admin, ai, auth, integrations, items, misc, sets};

struct SecuritySchemes;

impl Modify for SecuritySchemes {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "API token `spk_...` from POST /api/v1/auth/token or /api/v1/tokens",
                    ))
                    .build(),
            ),
        );
        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                crate::auth::session::COOKIE_NAME,
                "Web session cookie; unsafe methods also need the X-CSRF-Token header",
            ))),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "stickpile",
        description = "Self-hosted sticker and meme library. Errors are `ApiError` JSON. \
                       Requests from the trusted tailscale proxy are authenticated by tailnet identity. See docs/api.md.",
        license(name = "AGPL-3.0-or-later")
    ),
    paths(
        misc::health,
        openapi_json,
        auth::token,
        auth::register,
        auth::me,
        auth::update_me,
        auth::change_password,
        auth::totp_begin,
        auth::totp_confirm,
        auth::totp_disable,
        auth::list_tokens,
        auth::create_token,
        auth::revoke_token,
        sets::list,
        sets::create,
        sets::get,
        sets::update,
        sets::delete,
        sets::reorder,
        sets::upload,
        sets::import,
        sets::export,
        sets::put_cover,
        sets::delete_cover,
        sets::cover_file,
        integrations::create_gitpack,
        integrations::pull,
        integrations::git_webhook,
        integrations::telegram_export,
        integrations::telegram_import,
        items::sync,
        items::get,
        items::update,
        items::delete,
        items::file,
        items::like,
        items::unlike,
        items::better,
        items::enrich,
        ai::suggest,
        misc::search,
        misc::usage,
        misc::popular,
        misc::stats_me,
        misc::profiles,
        misc::job,
        admin::list_users,
        admin::create_user,
        admin::update_user,
        admin::delete_user,
        admin::list_invitations,
        admin::create_invitation,
        admin::delete_invitation,
        admin::audit,
        admin::storage,
        admin::jobs,
    ),
    components(schemas(stickpile_api::ApiError, stickpile_api::UploadMeta)),
    modifiers(&SecuritySchemes),
    tags(
        (name = "auth", description = "Login, registration, account, API tokens"),
        (name = "sets", description = "Sets, uploads, import/export"),
        (name = "items", description = "Items and their files"),
        (name = "ai", description = "AI suggestions for images before they are uploaded"),
        (name = "search", description = "Hybrid search"),
        (name = "stats", description = "Usage events, popular items, statistics"),
        (name = "admin", description = "Administration (role admin)"),
        (name = "integrations", description = "Git packs and Telegram import/export (background jobs)"),
        (name = "misc", description = "Health, profiles, jobs, this document"),
    )
)]
pub struct ApiDoc;

/// The generated document as JSON (built once).
pub fn spec_json() -> &'static str {
    static SPEC: OnceLock<String> = OnceLock::new();
    SPEC.get_or_init(|| ApiDoc::openapi().to_pretty_json().unwrap_or_else(|_| "{}".into()))
}

/// This OpenAPI document (unauthenticated).
#[utoipa::path(get, path = stickpile_api::routes::OPENAPI, tag = "misc",
    responses((status = 200, description = "OpenAPI 3.1 JSON", content_type = "application/json")))]
pub async fn openapi_json() -> Response {
    let mut resp = spec_json().into_response();
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    resp
}
