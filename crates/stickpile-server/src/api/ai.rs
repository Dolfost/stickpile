//! AI suggestions for an image before it is uploaded (the upload tray).

use axum::extract::{Multipart, State};
use axum::Json;
use stickpile_api::routes as r;
use stickpile_api::{AiSuggestions, ApiError};

use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::service::ai::{self as svc, SUGGEST_MAX_BYTES};
use crate::service::upload::multipart_error;
use crate::state::AppState;

/// Keywords, emojis and a vibe for one still image, chosen like enrichment
/// chooses them; nothing is stored. Multipart with one file part (PNG, JPEG,
/// WebP or GIF, at most 2 MiB; animations give their first frame). CLIP
/// looks at 224 × 224 pixels, so send a small rendering. Counts against
/// `limits.ai_suggest_per_minute`.
#[utoipa::path(post, path = r::AI_SUGGEST, tag = "ai",
    request_body(content_type = "multipart/form-data", description = "one `file` part: a still image, at most 2 MiB"),
    responses((status = 200, body = AiSuggestions), (status = 400, body = ApiError), (status = 401, body = ApiError),
              (status = 403, body = ApiError), (status = 413, body = ApiError), (status = 415, body = ApiError),
              (status = 429, body = ApiError), (status = 503, body = ApiError)),
    security(("bearer" = []), ("session" = [])))]
pub async fn suggest(
    State(state): State<AppState>,
    user: CurrentUser,
    mut mp: Multipart,
) -> AppResult<Json<AiSuggestions>> {
    svc::require_ready(&state)?;
    state.limiters.check_ai_suggest(user.id())?;
    let mut image = None;
    while let Some(mut field) = mp.next_field().await.map_err(multipart_error)? {
        if field.file_name().is_none() {
            continue;
        }
        let mut buf = Vec::new();
        while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
            if (buf.len() + chunk.len()) as u64 > SUGGEST_MAX_BYTES {
                return Err(AppError::too_large(format!(
                    "the image for suggestions must be at most {} MiB; send a small rendering",
                    SUGGEST_MAX_BYTES / (1024 * 1024)
                )));
            }
            buf.extend_from_slice(&chunk);
        }
        image = Some(buf);
        break;
    }
    let image = image.ok_or_else(|| AppError::bad_request("no file part in the request"))?;
    Ok(Json(svc::suggest(&state, image).await?))
}
