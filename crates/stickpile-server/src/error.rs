//! One error type for every handler. Rendered as [`stickpile_api::ApiError`]
//! JSON with a matching status; internal details are logged, never returned.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use stickpile_api::ApiError;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Clone)]
pub struct AppError {
    pub status: StatusCode,
    /// Stable machine code (see [`ApiError::code`]).
    pub code: &'static str,
    pub message: String,
    /// Seconds (`Retry-After`), for 429 and `ai_loading` responses.
    pub retry_after: Option<u64>,
}

impl AppError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> AppError {
        AppError {
            status,
            code,
            message: message.into(),
            retry_after: None,
        }
    }
    pub fn unauthorized(message: impl Into<String>) -> AppError {
        AppError::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }
    /// Password accepted but the account requires a TOTP code.
    pub fn totp_required() -> AppError {
        AppError::new(StatusCode::UNAUTHORIZED, "totp_required", "a TOTP code is required")
    }
    pub fn forbidden(message: impl Into<String>) -> AppError {
        AppError::new(StatusCode::FORBIDDEN, "forbidden", message)
    }
    pub fn not_found() -> AppError {
        AppError::new(StatusCode::NOT_FOUND, "not_found", "not found")
    }
    pub fn bad_request(message: impl Into<String>) -> AppError {
        AppError::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }
    pub fn conflict(message: impl Into<String>) -> AppError {
        AppError::new(StatusCode::CONFLICT, "conflict", message)
    }
    pub fn too_large(message: impl Into<String>) -> AppError {
        AppError::new(StatusCode::PAYLOAD_TOO_LARGE, "too_large", message)
    }
    pub fn unsupported_media(message: impl Into<String>) -> AppError {
        AppError::new(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media", message)
    }
    pub fn rate_limited(retry_after_secs: u64) -> AppError {
        AppError {
            retry_after: Some(retry_after_secs.max(1)),
            ..AppError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "too many requests, slow down",
            )
        }
    }
    pub fn csrf(message: impl Into<String>) -> AppError {
        AppError::new(StatusCode::FORBIDDEN, "csrf", message)
    }
    pub fn unavailable(message: impl Into<String>) -> AppError {
        AppError::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
    }
    /// 503 while the AI model is still loading: "ask again later", unlike
    /// `unavailable` (AI off or failed to load), which stays.
    pub fn ai_loading(message: impl Into<String>, retry_after_secs: u64) -> AppError {
        AppError {
            retry_after: Some(retry_after_secs.max(1)),
            ..AppError::new(StatusCode::SERVICE_UNAVAILABLE, "ai_loading", message)
        }
    }
    pub fn not_implemented(message: impl Into<String>) -> AppError {
        AppError::new(StatusCode::NOT_IMPLEMENTED, "not_implemented", message)
    }
    /// Logs `detail` and returns a generic 500.
    pub fn internal(detail: impl std::fmt::Display) -> AppError {
        tracing::error!("internal error: {detail}");
        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal server error")
    }

    pub fn to_api(&self) -> ApiError {
        ApiError {
            code: self.code.to_string(),
            message: self.message.clone(),
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.status.as_u16(), self.code, self.message)
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let mut resp = (self.status, Json(self.to_api())).into_response();
        if let Some(secs) = self.retry_after {
            if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
        }
        resp
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> AppError {
        match &e {
            sqlx::Error::RowNotFound => AppError::not_found(),
            sqlx::Error::Database(db) if db.is_unique_violation() => AppError::conflict("already exists"),
            _ => AppError::internal(format!("database: {e}")),
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> AppError {
        match e.downcast::<AppError>() {
            Ok(app) => app,
            Err(e) => AppError::internal(format!("{e:#}")),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> AppError {
        AppError::internal(format!("io: {e}"))
    }
}

impl From<tokio::task::JoinError> for AppError {
    fn from(e: tokio::task::JoinError) -> AppError {
        AppError::internal(format!("task: {e}"))
    }
}

impl From<stickpile_media::MediaError> for AppError {
    fn from(e: stickpile_media::MediaError) -> AppError {
        use stickpile_media::MediaError as M;
        match e {
            M::Unsupported => AppError::unsupported_media("unsupported or unrecognised file type"),
            M::TooLarge(m) => AppError::too_large(format!("file too large: {m}")),
            M::Invalid(m) => AppError::unsupported_media(format!("corrupt or invalid media: {m}")),
            M::Ffmpeg(m) => AppError::unsupported_media(format!("could not process video/animation: {m}")),
            M::Io(e) => AppError::internal(format!("media io: {e}")),
        }
    }
}

impl From<stickpile_setfmt::SetFmtError> for AppError {
    fn from(e: stickpile_setfmt::SetFmtError) -> AppError {
        use stickpile_setfmt::SetFmtError as S;
        match e {
            S::Io { .. } => AppError::internal(e.to_string()),
            _ => AppError::bad_request(e.to_string()),
        }
    }
}

/// Map a status code to the stable error code, for responses produced by
/// axum's own extractors (rejections) and the router fallback.
pub fn code_for_status(status: StatusCode) -> &'static str {
    match status {
        StatusCode::UNAUTHORIZED => "unauthorized",
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::METHOD_NOT_ALLOWED => "method_not_allowed",
        StatusCode::CONFLICT => "conflict",
        StatusCode::PAYLOAD_TOO_LARGE => "too_large",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media",
        StatusCode::TOO_MANY_REQUESTS => "rate_limited",
        StatusCode::NOT_IMPLEMENTED => "not_implemented",
        StatusCode::SERVICE_UNAVAILABLE => "unavailable",
        s if s.is_server_error() => "internal",
        _ => "bad_request",
    }
}
