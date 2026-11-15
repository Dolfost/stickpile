//! [`LibraryError`], the one error of the interface, and how
//! [`stickpile_core::CoreError`] maps onto it (see the crate docs for the
//! table).

use stickpile_core::config::ConfigError;
use stickpile_core::CoreError;

/// Everything that can go wrong in a `StickpileLibrary` call. Each variant is
/// a kind the app turns into its own (translated) text; `detail` says what
/// happened in English, for logs only, and never contains a token or a
/// password.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum LibraryError {
    /// No account: no token is stored and the server does not know this
    /// device by its tailnet identity. Sign in first.
    #[error("not signed in: {detail}")]
    NotSignedIn { detail: String },
    /// The server refused the credentials: a revoked or expired token, a
    /// wrong password or two-factor code, a wrong, expired or used pairing
    /// code (HTTP 401).
    #[error("unauthorized: {detail}")]
    Unauthorized { detail: String },
    /// The server could not be reached (no network, refused, timed out, a
    /// proxy's 502/503/504), and the offline cache cannot answer instead.
    #[error("server unreachable: {detail}")]
    Unreachable { detail: String },
    /// No such item or pack, or not one this account may see (HTTP 404).
    #[error("not found: {detail}")]
    NotFound { detail: String },
    /// Too many requests; try again later (HTTP 429).
    #[error("rate limited: {detail}")]
    RateLimited { detail: String },
    /// `prepare`: the item has no rendition of a type the text field accepts.
    #[error("nothing fits: {detail}")]
    NothingFits { detail: String },
    /// An argument is not acceptable. `field` names it: `server_url`,
    /// `code` (a pairing code), `token_name`, `totp` (the account needs a
    /// two-factor code and none was given), `variant`, `file` (an upload the
    /// server refused: not a picture or video, too large), `config_file` (it
    /// cannot be read as a stickpile config).
    #[error("invalid {field}: {detail}")]
    InvalidInput { field: String, detail: String },
    /// Any other HTTP error from the server, with its status.
    #[error("server error {status}: {detail}")]
    Server { status: u16, detail: String },
    /// Local storage failed: the cache database, a file, the token file.
    #[error("storage: {detail}")]
    Io { detail: String },
    /// Anything else.
    #[error("{detail}")]
    Other { detail: String },
}

/// The kind of a `LibraryError`, without its details
/// (`SyncState.last_error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum LibraryErrorKind {
    NotSignedIn,
    Unauthorized,
    Unreachable,
    NotFound,
    RateLimited,
    NothingFits,
    InvalidInput,
    Server,
    Io,
    Other,
}

impl LibraryError {
    /// The variant without its fields.
    pub fn kind(&self) -> LibraryErrorKind {
        match self {
            LibraryError::NotSignedIn { .. } => LibraryErrorKind::NotSignedIn,
            LibraryError::Unauthorized { .. } => LibraryErrorKind::Unauthorized,
            LibraryError::Unreachable { .. } => LibraryErrorKind::Unreachable,
            LibraryError::NotFound { .. } => LibraryErrorKind::NotFound,
            LibraryError::RateLimited { .. } => LibraryErrorKind::RateLimited,
            LibraryError::NothingFits { .. } => LibraryErrorKind::NothingFits,
            LibraryError::InvalidInput { .. } => LibraryErrorKind::InvalidInput,
            LibraryError::Server { .. } => LibraryErrorKind::Server,
            LibraryError::Io { .. } => LibraryErrorKind::Io,
            LibraryError::Other { .. } => LibraryErrorKind::Other,
        }
    }

    pub(crate) fn invalid(field: &str, detail: impl Into<String>) -> LibraryError {
        LibraryError::InvalidInput {
            field: field.into(),
            detail: detail.into(),
        }
    }

    pub(crate) fn not_found(detail: impl Into<String>) -> LibraryError {
        LibraryError::NotFound { detail: detail.into() }
    }

    pub(crate) fn other(detail: impl Into<String>) -> LibraryError {
        LibraryError::Other { detail: detail.into() }
    }

    /// [`From<CoreError>`], except that the server refusing the request as
    /// malformed (HTTP 400, 413, 415, 422) is [`LibraryError::InvalidInput`]
    /// for `field`: the argument of this call the request carried.
    pub(crate) fn input(e: CoreError, field: &str) -> LibraryError {
        match e.status() {
            Some(400 | 413 | 415 | 422) => LibraryError::invalid(field, e.to_string()),
            _ => e.into(),
        }
    }
}

impl From<CoreError> for LibraryError {
    fn from(e: CoreError) -> Self {
        let detail = e.to_string();
        match e {
            CoreError::NotLoggedIn => LibraryError::NotSignedIn { detail },
            CoreError::Offline(_) => LibraryError::Unreachable { detail },
            CoreError::Api { status, .. } => match status {
                401 => LibraryError::Unauthorized { detail },
                404 => LibraryError::NotFound { detail },
                429 => LibraryError::RateLimited { detail },
                status => LibraryError::Server { status, detail },
            },
            CoreError::Cache(_) | CoreError::Io(_) => LibraryError::Io { detail },
            CoreError::Other(_) => LibraryError::Other { detail },
        }
    }
}

impl From<ConfigError> for LibraryError {
    fn from(e: ConfigError) -> Self {
        let detail = e.to_string();
        match e {
            ConfigError::Parse { .. } | ConfigError::Invalid(_) | ConfigError::Edit { .. } => {
                LibraryError::invalid("config_file", detail)
            }
            ConfigError::Io { .. } | ConfigError::ReadOnly { .. } | ConfigError::Write { .. } => {
                LibraryError::Io { detail }
            }
        }
    }
}

impl From<tokio::task::JoinError> for LibraryError {
    fn from(e: tokio::task::JoinError) -> Self {
        LibraryError::other(format!("background task: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stickpile_api::ApiError;

    fn api(status: u16, code: &str) -> CoreError {
        CoreError::Api {
            status,
            error: ApiError {
                code: code.into(),
                message: "m".into(),
            },
        }
    }

    #[test]
    fn core_errors_map_to_kinds() {
        let kind = |e: CoreError| LibraryError::from(e).kind();
        assert_eq!(kind(CoreError::NotLoggedIn), LibraryErrorKind::NotSignedIn);
        assert_eq!(kind(CoreError::Offline("x".into())), LibraryErrorKind::Unreachable);
        assert_eq!(kind(api(401, "unauthorized")), LibraryErrorKind::Unauthorized);
        assert_eq!(kind(api(401, "totp_required")), LibraryErrorKind::Unauthorized);
        assert_eq!(kind(api(404, "not_found")), LibraryErrorKind::NotFound);
        assert_eq!(kind(api(429, "rate_limited")), LibraryErrorKind::RateLimited);
        assert_eq!(kind(api(400, "bad_request")), LibraryErrorKind::Server);
        assert_eq!(kind(api(403, "forbidden")), LibraryErrorKind::Server);
        assert_eq!(kind(api(500, "internal")), LibraryErrorKind::Server);
        assert_eq!(kind(CoreError::Cache("x".into())), LibraryErrorKind::Io);
        assert_eq!(kind(std::io::Error::other("x").into()), LibraryErrorKind::Io);
        assert_eq!(kind(CoreError::Other("x".into())), LibraryErrorKind::Other);
        assert_eq!(
            LibraryError::from(api(503, "unavailable")),
            LibraryError::Server {
                status: 503,
                detail: "503: m (unavailable)".into()
            }
        );
    }

    #[test]
    fn malformed_requests_name_the_field() {
        for status in [400, 413, 415, 422] {
            assert_eq!(
                LibraryError::input(api(status, "bad_request"), "file"),
                LibraryError::InvalidInput {
                    field: "file".into(),
                    detail: format!("{status}: m (bad_request)")
                }
            );
        }
        assert_eq!(
            LibraryError::input(api(401, "unauthorized"), "code").kind(),
            LibraryErrorKind::Unauthorized
        );
        assert_eq!(
            LibraryError::input(CoreError::Offline("x".into()), "code").kind(),
            LibraryErrorKind::Unreachable
        );
    }
}
