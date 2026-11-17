//! stickpile for Android, the Rust side: a library over `stickpile-core`
//! offered to the Kotlin app in `android/` through UniFFI (Kotlin package
//! `io.github.dolfost.stickpile.rust`). The contract is docs/android.md,
//! "Bridge interface".
//!
//! The app holds one [`StickpileLibrary`] (the contract's `Library`, renamed
//! because Kotlin's `Library` is `com.sun.jna.Library`, which the generated
//! bindings import), a [`stickpile_core::Session`] (the
//! account, the offline cache, the usage queue) on a Tokio runtime of its
//! own, behind a lock so that changing the server can replace it. Its
//! `async` methods are Kotlin `suspend` functions (`async_runtime =
//! "tokio"`); the work runs on the library's runtime, and a read that the
//! caller stops waiting for is aborted (calls that write run to their end).
//! The other methods read local state only. Every record and enum of the
//! interface is defined here ([`types`]) and converted from the
//! `stickpile-api` types.
//!
//! # Errors
//!
//! Every call that can fail throws [`LibraryError`]. The `stickpile-core`
//! errors map onto it like this:
//!
//! | `CoreError` | `LibraryError` |
//! |---|---|
//! | `NotLoggedIn` (a 401 without a token) | `NotSignedIn` |
//! | `Offline` (no connection, timeout, a proxy's 502/503/504) | `Unreachable` |
//! | `NotOnDevice` (the server unreachable, the file not on this phone) | `NotOnDevice` |
//! | `Api` 401 | `Unauthorized`; `InvalidInput { field: "totp" }` for `totp_required` in `login_password` |
//! | `Api` 404 | `NotFound` |
//! | `Api` 429 | `RateLimited` |
//! | `Api` 400, 413, 415, 422 where the call sends input | `InvalidInput { field }`: `file` (upload), `token_name` (pairing), `username` (password login) |
//! | any other `Api` status | `Server { status }` |
//! | `Cache`, `Io` | `Io` |
//! | `Other` | `Other` |
//!
//! Reads that fall back to the cache (search, the lists, `account`) only
//! throw `Unreachable` when the cache cannot answer either. Input checked
//! before any request is `InvalidInput` with the argument's name
//! (`server_url`, `code`, `variant`), and `prepare` throws `NothingFits`
//! when no rendition fits the field. Config file errors: unreadable as a
//! config is `InvalidInput { field: "config_file" }`, failing to read or
//! write it is `Io`.
//!
//! Logging goes through `tracing` without a subscriber: nothing is printed
//! on Android.

uniffi::setup_scaffolding!();

mod device;
mod error;
mod hints;
mod library;
mod motion;
mod offline;
pub mod types;

pub use error::{LibraryError, LibraryErrorKind};
pub use library::StickpileLibrary;
pub use types::*;

/// The chat app behind an Android package name (the focused field's
/// editor package), as `search` and `prepare` see it; no network. Unknown
/// packages are `Generic`.
#[uniffi::export]
pub fn target_for_package(package: String) -> Target {
    stickpile_core::target_for_package(&package).into()
}

/// The stickpile version this library was built from (the workspace's).
#[uniffi::export]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The client name the library reports to the server, as the other
/// clients do (`desktop/0.5.2`, `cli/0.5.2`).
#[uniffi::export]
pub fn client_name() -> String {
    format!("android/{}", version())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_the_workspace_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(client_name(), format!("android/{}", env!("CARGO_PKG_VERSION")));
        // stickpile-core is linked: its default config resolves.
        let _ = stickpile_core::ClientConfig::default();
    }
}
