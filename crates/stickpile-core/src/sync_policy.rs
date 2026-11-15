//! When to run `Session::sync` (display-free, shared by every client): every
//! [`SYNC_INTERVAL`] in the background, and when the client is opened if the
//! last successful sync is older than that. Failures (offline) are retried
//! after [`RETRY_AFTER`], so opening the client while the server is down does
//! not fire a request every time.

use std::time::{Duration, Instant};

/// How old the last successful sync may get before the next one is due.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// The least time between the starts of two attempts, so a failed sync is
/// retried after this delay.
pub const RETRY_AFTER: Duration = Duration::from_secs(60);

/// The sync schedule's state: when the last attempt started, when the last
/// one succeeded, and whether one is running. Time is passed in, so the
/// policy is testable without waiting.
#[derive(Debug, Clone)]
pub struct SyncPolicy {
    interval: Duration,
    retry_after: Duration,
    last_ok: Option<Instant>,
    last_attempt: Option<Instant>,
    running: bool,
}

impl Default for SyncPolicy {
    /// [`SYNC_INTERVAL`] and [`RETRY_AFTER`], nothing synced yet.
    fn default() -> Self {
        SyncPolicy::new(SYNC_INTERVAL, RETRY_AFTER)
    }
}

impl SyncPolicy {
    /// A policy that has not synced yet: a sync is due at once.
    pub fn new(interval: Duration, retry_after: Duration) -> SyncPolicy {
        SyncPolicy {
            interval,
            retry_after,
            last_ok: None,
            last_attempt: None,
            running: false,
        }
    }

    /// True when a sync should start at `now`: none is running, the last
    /// success is older than the interval (or there was none), and the last
    /// attempt is older than the retry delay.
    pub fn due(&self, now: Instant) -> bool {
        if self.running {
            return false;
        }
        let stale = self
            .last_ok
            .is_none_or(|t| now.saturating_duration_since(t) >= self.interval);
        let backoff_over = self
            .last_attempt
            .is_none_or(|t| now.saturating_duration_since(t) >= self.retry_after);
        stale && backoff_over
    }

    /// Record that a sync started at `now`.
    pub fn started(&mut self, now: Instant) {
        self.running = true;
        self.last_attempt = Some(now);
    }

    /// Record that the running sync ended at `now`, successfully when `ok`.
    pub fn finished(&mut self, now: Instant, ok: bool) {
        self.running = false;
        if ok {
            self.last_ok = Some(now);
        }
    }

    /// True between [`SyncPolicy::started`] and [`SyncPolicy::finished`].
    pub fn is_running(&self) -> bool {
        self.running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn first_sync_is_due_immediately() {
        assert!(SyncPolicy::default().due(Instant::now()));
    }

    #[test]
    fn not_due_while_running_or_fresh() {
        let t0 = Instant::now();
        let mut p = SyncPolicy::new(10 * MIN, MIN);
        p.started(t0);
        assert!(p.is_running());
        assert!(!p.due(t0 + 20 * MIN));
        p.finished(t0 + Duration::from_secs(2), true);
        assert!(!p.due(t0 + 5 * MIN));
        assert!(p.due(t0 + 11 * MIN));
    }

    #[test]
    fn failures_back_off() {
        let t0 = Instant::now();
        let mut p = SyncPolicy::new(10 * MIN, MIN);
        p.started(t0);
        p.finished(t0, false);
        assert!(!p.due(t0 + Duration::from_secs(30)), "retry not before backoff");
        assert!(p.due(t0 + MIN));
    }
}
