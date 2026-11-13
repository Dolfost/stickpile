//! Shared client library for every stickpile client (CLI, GTK desktop; later
//! Android and macOS through UniFFI).
//!
//! * [`config`] — client TOML config (mirrored by the home-manager module)
//! * [`settings`] - the standard loader (`config.toml` layered with the GUI's
//!   `settings.toml`) and the Settings page's schema and writers
//! * [`client`] — typed async REST client
//! * [`token`] — API token file storage (0600)
//! * [`cache`] — offline cache: item metadata (searchable offline with
//!   `stickpile-search`), thumbnails and recently used files under a size cap
//! * [`usage`] — offline usage-event queue, flushed when the server is back
//! * [`clipboard`] — which MIME offers to put on the clipboard for an item and
//!   target, plus CLI clipboard backends (Wayland data-control, X11)
//! * [`session`] — the high-level facade UI shells use (search with offline
//!   fallback, thumbnails, prepare a copy, record usage)
//! * `mock` (feature `mock`) — an in-process axum mock of the REST API for tests
//!
//! The popup state machine and focused-window detection live in the
//! toolkit-free `stickpile-quick` crate.

pub mod cache;
pub mod client;
pub mod clipboard;
pub mod config;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
pub mod session;
pub mod settings;
pub mod token;
pub mod usage;

#[cfg(test)]
mod tests;

pub use client::ApiClient;
pub use config::ClientConfig;
pub use session::{PreparedCopy, SearchOutcome, Session};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Server unreachable / timed out: callers fall back to the cache.
    #[error("server unreachable: {0}")]
    Offline(String),
    /// Non-2xx with an `ApiError` body.
    #[error("{status}: {} ({})", .error.message, .error.code)]
    Api {
        status: u16,
        error: stickpile_api::ApiError,
    },
    #[error("not logged in (run `stickpile login`)")]
    NotLoggedIn,
    #[error("cache: {0}")]
    Cache(String),
    #[error("{0}")]
    Other(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl CoreError {
    /// True for [`CoreError::Offline`].
    pub fn is_offline(&self) -> bool {
        matches!(self, CoreError::Offline(_))
    }
    /// HTTP status of an [`CoreError::Api`] error.
    pub fn status(&self) -> Option<u16> {
        match self {
            CoreError::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
    /// Machine code of an [`CoreError::Api`] error (`not_found`, `unauthorized`, ...).
    pub fn api_code(&self) -> Option<&str> {
        match self {
            CoreError::Api { error, .. } => Some(error.code.as_str()),
            _ => None,
        }
    }
    /// The server rejected our credentials (missing, revoked or expired token).
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, CoreError::NotLoggedIn) || self.status() == Some(401)
    }
    /// Login needs a TOTP code (`401 totp_required`).
    pub fn is_totp_required(&self) -> bool {
        matches!(self, CoreError::Api { status: 401, error } if error.code == "totp_required")
    }
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Cache(e.to_string())
    }
}

impl From<stickpile_setfmt::SetFmtError> for CoreError {
    fn from(e: stickpile_setfmt::SetFmtError) -> Self {
        CoreError::Other(e.to_string())
    }
}

pub type Result<T, E = CoreError> = std::result::Result<T, E>;

/// Current unix time in seconds.
pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 26 lowercase base32 characters (128 random bits), the id format used by
/// the server and for client-generated usage event ids.
pub fn new_id() -> String {
    let bytes: [u8; 16] = rand::random();
    data_encoding::BASE32_NOPAD.encode(&bytes).to_ascii_lowercase()
}
