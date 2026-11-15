//! When to run `Session::sync` (display-free): every [`SYNC_INTERVAL`] in
//! the background, and when the popup opens if the last successful sync is
//! older than that. Failures (offline) are retried after [`RETRY_AFTER`], so
//! opening the popup while the server is down does not fire a request every
//! time.
//!
//! The policy itself lives in [`stickpile_core::sync_policy`] (the Android
//! library uses it too) and is re-exported here; only the desktop timer's
//! [`TICK`] is defined in this crate.

use std::time::Duration;

pub use stickpile_core::sync_policy::{SyncPolicy, RETRY_AFTER, SYNC_INTERVAL};

/// How often the background timer checks [`SyncPolicy::due`].
pub const TICK: Duration = Duration::from_secs(60);
