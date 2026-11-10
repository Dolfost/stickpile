//! Shared client library for every stickpile client (CLI, GTK desktop; later
//! Android and macOS through UniFFI).
//!
//! * [`config`] — client TOML config (mirrored by the home-manager module)
//! * [`client`] — typed async REST client
//! * [`token`] — API token file storage (0600)
//! * [`cache`] — offline cache: item metadata (searchable offline with
//!   `stickpile-search`), thumbnails and recently used files under a size cap
//! * [`usage`] — offline usage-event queue, flushed when the server is back
//! * [`clipboard`] — which MIME offers to put on the clipboard for an item and
//!   target, plus CLI clipboard backends (Wayland data-control, X11)
//! * [`session`] — the high-level facade UI shells use (search with offline
//!   fallback, thumbnails, prepare a copy, record usage)
//!
//! The popup state machine and focused-window detection live in the
//! toolkit-free `stickpile-quick` crate.

pub mod cache;
pub mod client;
pub mod clipboard;
pub mod config;
pub mod session;
pub mod token;
pub mod usage;

pub use config::ClientConfig;
pub use session::{PreparedCopy, SearchOutcome, Session};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Server unreachable / timed out: callers fall back to the cache.
    #[error("server unreachable: {0}")]
    Offline(String),
    /// Non-2xx with an `ApiError` body.
    #[error("{status}: {} ({})", .error.message, .error.code)]
    Api { status: u16, error: stickpile_api::ApiError },
    #[error("not logged in (run `stickpile login`)")]
    NotLoggedIn,
    #[error("cache: {0}")]
    Cache(String),
    #[error("{0}")]
    Other(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = CoreError> = std::result::Result<T, E>;
