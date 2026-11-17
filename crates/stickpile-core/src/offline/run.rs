//! The background downloader ([`crate::Session::offline_run`]): fetches
//! the missing files of the [`Plan`] in its order, four at a time.
//!
//! * A file is claimed in the cache (`offline_claims`) before it is fetched,
//!   so two processes on one cache (the CLI and the popup) never fetch the
//!   same file; a claim is renewed while the download runs and goes stale
//!   when its process dies.
//! * It is written to `<path>.part.<owner>` and renamed into place once its
//!   size (and the sha256 the server's `ETag` names) check out, so an
//!   interrupted download leaves nothing that looks complete. A part file
//!   is deleted when its download ends any other way, also when the run is
//!   dropped; one a killed process left behind goes when the file is
//!   claimed next (thumbnails and covers: when a run starts).
//! * A `429` is obeyed (every download waits as told); other failures are
//!   retried with a growing delay, a few times, then the file counts as
//!   failed with the reason kept (`offline_failures`) and the run goes on.
//! * The run stops at once when the server is unreachable
//!   ([`OfflineState::WaitingForNetwork`]; the next run after a successful
//!   sync goes on) and when the disk is full ([`OfflineState::DiskFull`], no
//!   retry). When the next file would exceed `offline.max_mb` no download
//!   starts any more and those running finish
//!   ([`OfflineState::LimitReached`]). Nothing is ever deleted to make room.
//! * A copy the user asked for goes first: while one is downloading, no
//!   background download starts and the running ones hold between chunks.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::Digest;
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, Notify};

use super::{Plan, PlanScope};
use crate::client::{map_reqwest, FileFetch};
use crate::{CoreError, Session};

// -- status --------------------------------------------------------------------

/// What the downloader is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfflineState {
    /// Not running (finished, cancelled, or not started).
    #[default]
    Idle,
    Downloading,
    /// Paused by [`OfflineControl::pause`]; goes on after
    /// [`OfflineControl::resume`].
    Paused,
    /// The server is unreachable; the next run after a successful sync
    /// goes on.
    WaitingForNetwork,
    /// The next file would exceed `offline.max_mb`; nothing was deleted.
    LimitReached,
    /// The disk is full; nothing was deleted.
    DiskFull,
}

/// Progress of one kept scope or automatic rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeProgress {
    pub scope: PlanScope,
    /// As [`super::ScopeTotals::name`].
    pub name: Option<String>,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// The downloader's progress, published while it runs
/// ([`OfflineControl::subscribe`]). "Done" counts the wanted files on the
/// device, those there before the run included.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfflineStatus {
    pub state: OfflineState,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Items and renditions not downloaded by the options (see
    /// [`super::Plan::skipped`]).
    pub skipped: u64,
    /// Wanted files whose download failed (this run or an earlier one).
    pub failed: u64,
    pub scopes: Vec<ScopeProgress>,
    /// What stopped or held the run (the network error, the budget, the
    /// disk), in English for logs; the state says which.
    pub message: Option<String>,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEnd {
    /// Every file was tried: on the device, or failed
    /// ([`OfflineStatus::failed`]).
    Complete,
    /// [`OfflineControl::cancel`], or the account's offline data was reset
    /// meanwhile.
    Cancelled,
    /// The server is unreachable.
    WaitingForNetwork,
    /// `offline.max_mb` is reached.
    LimitReached,
    /// The disk is full.
    DiskFull,
    /// The server refused the credentials (sign in again).
    Unauthorized,
}

/// What [`crate::Session::offline_run`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfflineOutcome {
    pub end: RunEnd,
    /// The last status published.
    pub status: OfflineStatus,
}

impl OfflineOutcome {
    /// Everything wanted is on the device.
    pub fn is_complete(&self) -> bool {
        self.end == RunEnd::Complete && self.status.files_done == self.status.files_total
    }
}

// -- control -------------------------------------------------------------------

/// Pauses, resumes and cancels the downloader, and carries its status.
/// Cheap to clone; one per shell, reused for every run. The status is a
/// [`tokio::sync::watch`] channel: [`OfflineControl::subscribe`] and wait
/// for changes (runtime-independent, so a GLib main loop can await it too),
/// or read [`OfflineControl::status`] when it suits.
#[derive(Debug, Clone)]
pub struct OfflineControl {
    inner: Arc<ControlInner>,
}

#[derive(Debug)]
struct ControlInner {
    paused: AtomicBool,
    cancelled: AtomicBool,
    /// Pause, resume, cancel.
    changed: Notify,
    status: watch::Sender<OfflineStatus>,
    tuning: Mutex<Tuning>,
}

impl Default for OfflineControl {
    fn default() -> Self {
        OfflineControl::new()
    }
}

impl OfflineControl {
    pub fn new() -> OfflineControl {
        OfflineControl {
            inner: Arc::new(ControlInner {
                paused: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
                changed: Notify::new(),
                status: watch::channel(OfflineStatus::default()).0,
                tuning: Mutex::new(Tuning::default()),
            }),
        }
    }

    /// Hold the downloads: those running are stopped (their files fetched
    /// again later), none starts. Stays paused across runs until
    /// [`OfflineControl::resume`].
    pub fn pause(&self) {
        self.inner.paused.store(true, Ordering::SeqCst);
        self.inner.changed.notify_waiters();
        self.inner.status.send_modify(|s| {
            if s.state == OfflineState::Downloading {
                s.state = OfflineState::Paused;
            }
        });
    }

    pub fn resume(&self) {
        self.inner.paused.store(false, Ordering::SeqCst);
        self.inner.changed.notify_waiters();
        self.inner.status.send_modify(|s| {
            if s.state == OfflineState::Paused {
                s.state = OfflineState::Downloading;
            }
        });
    }

    /// End the running run (the next one starts afresh).
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.changed.notify_waiters();
    }

    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::SeqCst)
    }

    /// The last status published.
    pub fn status(&self) -> OfflineStatus {
        self.inner.status.borrow().clone()
    }

    /// Status changes, from the current one on.
    pub fn subscribe(&self) -> watch::Receiver<OfflineStatus> {
        self.inner.status.subscribe()
    }

    fn cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// A run starts: a cancel was for the one before.
    pub(crate) fn begin(&self) {
        self.inner.cancelled.store(false, Ordering::SeqCst);
    }

    pub(crate) fn publish(&self, status: OfflineStatus) {
        self.inner.status.send_replace(status);
    }

    pub(crate) fn tuning(&self) -> Tuning {
        self.inner.tuning.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Change the downloader's limits and delays (tests).
    #[cfg(test)]
    pub(crate) fn tune(&self, f: impl FnOnce(&mut Tuning)) {
        f(&mut self.inner.tuning.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

/// The downloader's limits and delays.
#[derive(Debug, Clone)]
pub(crate) struct Tuning {
    /// Downloads at once.
    pub concurrency: usize,
    /// Tries per file and run (a file that failed in an earlier run gets one).
    pub attempts: u32,
    /// Delay before the second try, doubled for each further one.
    pub backoff: Duration,
    /// `429`s obeyed per file before it counts as failed.
    pub rate_limit_waits: u32,
    /// The wait after a `429` without `Retry-After`.
    pub default_retry_after: Duration,
    /// The longest `Retry-After` obeyed.
    pub max_retry_after: Duration,
    /// A claim older than this belongs to a process that died.
    pub claim_stale_secs: i64,
    /// How often a claim is renewed while its download runs.
    pub renew_every: Duration,
    /// How often a file another process downloads is looked at again.
    pub claim_poll: Duration,
    /// The longest a background download holds for a copy at once.
    pub foreground_wait: Duration,
    /// Test hook: write every download to this file instead of its part file
    /// (`/dev/full` makes the disk full).
    pub part_sink: Option<PathBuf>,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            concurrency: 4,
            attempts: 3,
            backoff: Duration::from_secs(2),
            rate_limit_waits: 5,
            default_retry_after: Duration::from_secs(30),
            max_retry_after: Duration::from_secs(600),
            claim_stale_secs: 120,
            renew_every: Duration::from_secs(30),
            claim_poll: Duration::from_millis(500),
            foreground_wait: Duration::from_secs(30),
            part_sink: None,
        }
    }
}

// -- a copy goes first -----------------------------------------------------------

/// Downloads the user asked for (a copy, an insert, a thumbnail on screen):
/// while any runs, the background downloader holds.
#[derive(Debug, Default)]
pub(crate) struct Foreground {
    active: AtomicUsize,
    idle: Notify,
}

/// Held while a user's download runs.
pub(crate) struct ForegroundGuard(Arc<Foreground>);

impl Drop for ForegroundGuard {
    fn drop(&mut self) {
        if self.0.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.idle.notify_waiters();
        }
    }
}

impl Foreground {
    pub(crate) fn enter(self: &Arc<Self>) -> ForegroundGuard {
        self.active.fetch_add(1, Ordering::SeqCst);
        ForegroundGuard(self.clone())
    }

    pub(crate) fn busy(&self) -> bool {
        self.active.load(Ordering::SeqCst) > 0
    }

    /// Wait until no user's download runs, at most `cap`.
    pub(crate) async fn wait_idle(&self, cap: Duration) {
        let wait = async {
            loop {
                let idle = self.idle.notified();
                tokio::pin!(idle);
                idle.as_mut().enable();
                if !self.busy() {
                    return;
                }
                idle.await;
            }
        };
        let _ = tokio::time::timeout(cap, wait).await;
    }
}

// -- the run -----------------------------------------------------------------------

/// Why one download did not finish.
#[derive(Debug)]
enum Fail {
    /// Paused, cancelled, or the run stopped.
    Interrupted,
    RateLimited(Option<Duration>),
    Offline(String),
    DiskFull(String),
    Unauthorized(String),
    /// The account's offline data was reset meanwhile.
    Reset,
    Other(String),
}

fn is_disk_full(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    if matches!(e.raw_os_error(), Some(libc::ENOSPC) | Some(libc::EDQUOT)) {
        return true;
    }
    matches!(
        e.kind(),
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
    )
}

fn io_fail(e: std::io::Error) -> Fail {
    if is_disk_full(&e) {
        Fail::DiskFull(e.to_string())
    } else {
        Fail::Other(e.to_string())
    }
}

fn core_fail(e: CoreError) -> Fail {
    match e {
        e if e.is_offline() => Fail::Offline(e.to_string()),
        e if e.is_unauthorized() => Fail::Unauthorized(e.to_string()),
        CoreError::Io(io) => io_fail(io),
        CoreError::Cache(msg) if msg.contains("disk is full") => Fail::DiskFull(msg),
        e => Fail::Other(e.to_string()),
    }
}

/// Deletes a part file unless it was renamed into place.
struct PartFile {
    path: PathBuf,
    armed: bool,
}

impl Drop for PartFile {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Gives a claim back when the file is done with, also when the run is
/// dropped.
struct ClaimGuard<'a> {
    session: &'a Session,
    rel: &'a str,
    owner: &'a str,
}

impl Drop for ClaimGuard<'_> {
    fn drop(&mut self) {
        if let Err(e) = self.session.cache().offline_unclaim(self.rel, self.owner) {
            tracing::debug!("giving back the claim on {}: {e}", self.rel);
        }
    }
}

/// What a worker does next.
enum Next {
    File(usize),
    /// Only files other processes are downloading are left: look again later.
    Wait,
    Done,
}

#[derive(Debug)]
struct RunState {
    queue: VecDeque<usize>,
    /// Files another process holds, with when to look again.
    deferred: Vec<(usize, Instant)>,
    /// Bytes of the downloads running (reserved against the budget).
    reserved: u64,
    /// Which files hold a reservation.
    holds: Vec<bool>,
    /// Bytes of the wanted files on the device.
    kept_bytes: u64,
    done: Vec<bool>,
    failed_now: HashSet<usize>,
    failed_before: HashSet<usize>,
    scope_done: Vec<(u64, u64)>,
    files_done: u64,
    bytes_done: u64,
    stop: Option<RunEnd>,
    /// The running downloads are to stop as well.
    halt: bool,
    message: Option<String>,
    /// No request before this (a `429`).
    not_before: Option<Instant>,
}

pub(crate) struct Run<'a> {
    session: &'a Session,
    control: &'a OfflineControl,
    plan: Plan,
    owner: String,
    epoch: u64,
    tuning: Tuning,
    st: Mutex<RunState>,
    /// The run stopped.
    stopped: Notify,
}

impl<'a> Run<'a> {
    pub(crate) fn new(session: &'a Session, control: &'a OfflineControl, plan: Plan, epoch: u64) -> Run<'a> {
        let n = plan.files.len();
        let mut scope_done = vec![(0u64, 0u64); plan.scopes.len()];
        let mut done = vec![false; n];
        let (mut files_done, mut bytes_done, mut kept_bytes) = (0, 0, 0);
        for (i, f) in plan.files.iter().enumerate() {
            if f.present {
                done[i] = true;
                files_done += 1;
                bytes_done += f.file.size;
                kept_bytes += f.file.size;
                for &s in &f.scopes {
                    scope_done[s].0 += 1;
                    scope_done[s].1 += f.file.size;
                }
            }
        }
        let failed_paths: HashSet<&str> = plan.failures.iter().map(|f| f.path.as_str()).collect();
        let failed_before = plan
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.present && failed_paths.contains(f.file.rel.as_str()))
            .map(|(i, _)| i)
            .collect();
        let queue = (0..n).filter(|&i| !done[i]).collect();
        let tuning = control.tuning();
        Run {
            session,
            control,
            owner: format!("{}-{}", std::process::id(), crate::new_id()),
            epoch,
            tuning,
            st: Mutex::new(RunState {
                queue,
                deferred: Vec::new(),
                reserved: 0,
                holds: vec![false; n],
                kept_bytes,
                done,
                failed_now: HashSet::new(),
                failed_before,
                scope_done,
                files_done,
                bytes_done,
                stop: None,
                halt: false,
                message: None,
                not_before: None,
            }),
            plan,
            stopped: Notify::new(),
        }
    }

    fn state(&self) -> MutexGuard<'_, RunState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The status now.
    fn status(&self, st: &RunState) -> OfflineStatus {
        let state = match st.stop {
            Some(RunEnd::WaitingForNetwork) => OfflineState::WaitingForNetwork,
            Some(RunEnd::LimitReached) => OfflineState::LimitReached,
            Some(RunEnd::DiskFull) => OfflineState::DiskFull,
            Some(_) => OfflineState::Idle,
            None if self.control.is_paused() => OfflineState::Paused,
            None => OfflineState::Downloading,
        };
        // Disjoint: a file failing again moves from `failed_before` to `failed_now`.
        let failed = st.failed_now.len() + st.failed_before.iter().filter(|i| !st.done[**i]).count();
        OfflineStatus {
            state,
            files_done: st.files_done,
            files_total: self.plan.totals.files,
            bytes_done: st.bytes_done,
            bytes_total: self.plan.totals.bytes,
            skipped: self.plan.skipped.len() as u64,
            failed: failed as u64,
            scopes: self
                .plan
                .scopes
                .iter()
                .zip(&st.scope_done)
                .map(|(s, (files, bytes))| ScopeProgress {
                    scope: s.scope.clone(),
                    name: s.name.clone(),
                    files_done: *files,
                    files_total: s.totals.files,
                    bytes_done: *bytes,
                    bytes_total: s.totals.bytes,
                })
                .collect(),
            message: st.message.clone(),
        }
    }

    fn publish(&self) {
        let status = {
            let st = self.state();
            self.status(&st)
        };
        self.control.publish(status);
    }

    /// End the run: no download starts any more. The running ones are
    /// stopped too, except at the budget: they were within it when they
    /// started, so they finish.
    fn stop(&self, end: RunEnd, message: Option<String>) {
        {
            let mut st = self.state();
            if st.stop.is_none() {
                st.stop = Some(end);
                st.message = message;
            }
            if end != RunEnd::LimitReached {
                st.halt = true;
            }
        }
        self.stopped.notify_waiters();
        self.publish();
    }

    /// No download is to start any more.
    fn stopping(&self) -> bool {
        self.control.cancelled() || self.state().stop.is_some()
    }

    /// Resolves when the running downloads must stop: paused, cancelled,
    /// or the run stopped (but for the budget).
    async fn interrupted(&self) {
        loop {
            let changed = self.control.inner.changed.notified();
            let stopped = self.stopped.notified();
            tokio::pin!(changed, stopped);
            changed.as_mut().enable();
            stopped.as_mut().enable();
            if self.control.cancelled() || self.state().halt || self.control.is_paused() {
                return;
            }
            tokio::select! {
                _ = changed => {}
                _ = stopped => {}
            }
        }
    }

    /// Wait while paused; false once the run is to end.
    async fn runnable(&self) -> bool {
        loop {
            let changed = self.control.inner.changed.notified();
            let stopped = self.stopped.notified();
            tokio::pin!(changed, stopped);
            changed.as_mut().enable();
            stopped.as_mut().enable();
            if self.stopping() {
                return false;
            }
            if !self.control.is_paused() {
                return true;
            }
            self.publish();
            tokio::select! {
                _ = changed => {}
                _ = stopped => {}
            }
        }
    }

    /// Sleep `d` unless the run is interrupted first; false if it was.
    async fn sleep(&self, d: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(d) => true,
            _ = self.interrupted() => false,
        }
    }

    fn next(&self) -> Next {
        let mut st = self.state();
        if st.stop.is_some() {
            return Next::Done;
        }
        let ix = match st.queue.pop_front() {
            Some(ix) => ix,
            None => {
                let now = Instant::now();
                match st.deferred.iter().position(|(_, at)| *at <= now) {
                    Some(pos) => st.deferred.remove(pos).0,
                    None if st.deferred.is_empty() => return Next::Done,
                    None => return Next::Wait,
                }
            }
        };
        let size = self.plan.files[ix].file.size;
        if let Some(budget) = self.plan.budget {
            if st.kept_bytes + st.reserved + size > budget {
                st.queue.push_front(ix);
                drop(st);
                self.stop(
                    RunEnd::LimitReached,
                    Some(format!(
                        "offline.max_mb ({} MiB) is reached; nothing more is downloaded",
                        budget / super::MIB
                    )),
                );
                return Next::Done;
            }
        }
        st.reserved += size;
        st.holds[ix] = true;
        Next::File(ix)
    }

    /// Give back the budget a file reserved (once).
    fn release(&self, st: &mut RunState, ix: usize) {
        if std::mem::take(&mut st.holds[ix]) {
            st.reserved = st.reserved.saturating_sub(self.plan.files[ix].file.size);
        }
    }

    /// Drive the workers until the run ends.
    pub(crate) async fn go(&self) -> OfflineOutcome {
        self.sweep_parts();
        self.publish();
        let workers = (0..self.tuning.concurrency.max(1)).map(|_| self.worker());
        futures::future::join_all(workers).await;
        let end = {
            let mut st = self.state();
            if self.control.cancelled() && st.stop.is_none() {
                st.stop = Some(RunEnd::Cancelled);
            }
            *st.stop.get_or_insert(RunEnd::Complete)
        };
        self.publish();
        OfflineOutcome {
            end,
            status: self.control.status(),
        }
    }

    async fn worker(&self) {
        loop {
            if !self.runnable().await {
                return;
            }
            self.session.foreground().wait_idle(self.tuning.foreground_wait).await;
            let wait = self
                .state()
                .not_before
                .map(|t| t.saturating_duration_since(Instant::now()));
            if let Some(wait) = wait.filter(|w| !w.is_zero()) {
                self.sleep(wait).await;
                continue;
            }
            match self.next() {
                Next::Done => return,
                Next::Wait => {
                    self.sleep(self.tuning.claim_poll).await;
                }
                Next::File(ix) => {
                    self.file(ix).await;
                    let mut st = self.state();
                    self.release(&mut st, ix);
                    drop(st);
                    self.publish();
                }
            }
        }
    }

    /// The file is on the device: count it.
    fn done(&self, ix: usize) {
        let f = &self.plan.files[ix];
        let mut st = self.state();
        // Counted as kept from now on, so no longer as reserved.
        self.release(&mut st, ix);
        if st.done[ix] {
            return;
        }
        st.done[ix] = true;
        st.files_done += 1;
        st.bytes_done += f.file.size;
        st.kept_bytes += f.file.size;
        st.failed_now.remove(&ix);
        for &s in &f.scopes {
            st.scope_done[s].0 += 1;
            st.scope_done[s].1 += f.file.size;
        }
    }

    async fn file(&self, ix: usize) {
        let file = &self.plan.files[ix].file;
        let cache = self.session.cache();
        let claimed = match cache.offline_claim(&file.rel, &self.owner, self.tuning.claim_stale_secs) {
            Ok(c) => c,
            Err(e) => {
                self.give_up(ix, Fail::Other(e.to_string()), 1);
                return;
            }
        };
        if !claimed {
            // Another process is downloading it; look again later.
            let mut st = self.state();
            st.deferred.push((ix, Instant::now() + self.tuning.claim_poll));
            return;
        }
        let _claim = ClaimGuard {
            session: self.session,
            rel: &file.rel,
            owner: &self.owner,
        };
        // It may have arrived meanwhile (another process, or a copy).
        if cache.file_present(&file.rel, Some(file.size)).unwrap_or(false) {
            match cache.register_kept_file(&file.rel, self.epoch) {
                Ok(true) => self.done(ix),
                Ok(false) => self.stop(RunEnd::Cancelled, Some("the offline data was reset".into())),
                Err(e) => match core_fail(e) {
                    Fail::DiskFull(msg) => self.stop(RunEnd::DiskFull, Some(msg)),
                    fail => self.give_up(ix, fail, 1),
                },
            }
            return;
        }
        if file.rel.starts_with("clip/") {
            // One small directory per rendition; `files/` is swept once per run.
            remove_stale_parts(&cache.abs(&file.rel), &self.owner);
        }
        let allowed = if self.state().failed_before.contains(&ix) {
            1
        } else {
            self.tuning.attempts.max(1)
        };
        let (mut attempts, mut waits) = (0u32, 0u32);
        loop {
            let result = tokio::select! {
                r = self.download(ix) => r,
                _ = self.interrupted() => Err(Fail::Interrupted),
            };
            let fail = match result {
                Ok(()) => match cache.register_kept_file(&file.rel, self.epoch) {
                    Ok(true) => {
                        self.done(ix);
                        return;
                    }
                    Ok(false) => Fail::Reset,
                    Err(e) => core_fail(e),
                },
                Err(f) => f,
            };
            match fail {
                Fail::Interrupted => {
                    if !self.stopping() {
                        // Paused: fetch it again first after the pause.
                        self.state().queue.push_front(ix);
                    }
                    return;
                }
                Fail::RateLimited(wait) => {
                    waits += 1;
                    if waits > self.tuning.rate_limit_waits {
                        self.give_up(
                            ix,
                            Fail::Other("the server kept answering 429 Too Many Requests".into()),
                            1,
                        );
                        return;
                    }
                    let wait = wait
                        .unwrap_or(self.tuning.default_retry_after)
                        .min(self.tuning.max_retry_after);
                    tracing::info!("offline download of {}: 429, waiting {wait:?}", file.rel);
                    let until = Instant::now() + wait;
                    {
                        let mut st = self.state();
                        st.not_before = Some(st.not_before.map_or(until, |t| t.max(until)));
                    }
                    if !self.sleep(wait).await {
                        if !self.stopping() {
                            self.state().queue.push_front(ix);
                        }
                        return;
                    }
                }
                Fail::Offline(msg) => {
                    self.state().queue.push_front(ix);
                    self.stop(RunEnd::WaitingForNetwork, Some(msg));
                    return;
                }
                Fail::DiskFull(msg) => {
                    self.state().queue.push_front(ix);
                    self.stop(RunEnd::DiskFull, Some(msg));
                    return;
                }
                Fail::Unauthorized(msg) => {
                    self.stop(RunEnd::Unauthorized, Some(msg));
                    return;
                }
                Fail::Reset => {
                    self.stop(RunEnd::Cancelled, Some("the offline data was reset".into()));
                    return;
                }
                Fail::Other(msg) => {
                    attempts += 1;
                    if attempts >= allowed {
                        self.give_up(ix, Fail::Other(msg), attempts);
                        return;
                    }
                    tracing::debug!("offline download of {} failed ({msg}); trying again", file.rel);
                    let delay = self.tuning.backoff.saturating_mul(1 << (attempts - 1).min(8));
                    if !self.sleep(delay).await {
                        if !self.stopping() {
                            self.state().queue.push_front(ix);
                        }
                        return;
                    }
                }
            }
        }
    }

    /// The file counts as failed, with the reason kept.
    fn give_up(&self, ix: usize, fail: Fail, attempts: u32) {
        let file = &self.plan.files[ix].file;
        let msg = match fail {
            Fail::Other(m) | Fail::Offline(m) | Fail::DiskFull(m) | Fail::Unauthorized(m) => m,
            other => format!("{other:?}"),
        };
        tracing::warn!("offline download of {} failed: {msg}", file.rel);
        if let Err(e) = self
            .session
            .cache()
            .offline_fail(&file.rel, &file.item_id, &msg, attempts)
        {
            tracing::warn!("recording the failure of {}: {e}", file.rel);
        }
        let mut st = self.state();
        st.failed_before.remove(&ix);
        st.failed_now.insert(ix);
    }

    /// Fetch one file into its part file and rename it into place.
    async fn download(&self, ix: usize) -> Result<(), Fail> {
        let file = &self.plan.files[ix].file;
        let cache = self.session.cache();
        let fetched = self
            .session
            .client()
            .fetch_file(&file.url_path())
            .await
            .map_err(core_fail)?;
        let (mut resp, sha256) = match fetched {
            FileFetch::RateLimited(wait) => return Err(Fail::RateLimited(wait)),
            FileFetch::Body { resp, sha256 } => (resp, sha256),
        };
        let dest = cache.abs(&file.rel);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(io_fail)?;
        }
        let name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut part = PartFile {
            path: dest.with_file_name(format!("{name}.part.{}", self.owner)),
            armed: true,
        };
        let target = self.tuning.part_sink.clone().unwrap_or_else(|| part.path.clone());
        let mut out = tokio::fs::File::create(&target).await.map_err(io_fail)?;
        let mut hasher = sha2::Sha256::new();
        let mut written = 0u64;
        let mut renewed = Instant::now();
        while let Some(chunk) = resp.chunk().await.map_err(|e| core_fail(map_reqwest(e)))? {
            out.write_all(&chunk).await.map_err(io_fail)?;
            hasher.update(&chunk);
            written += chunk.len() as u64;
            if renewed.elapsed() >= self.tuning.renew_every {
                renewed = Instant::now();
                if let Err(e) = cache.offline_renew_claim(&file.rel, &self.owner) {
                    tracing::debug!("renewing the claim on {}: {e}", file.rel);
                }
            }
            // A copy the user asked for goes first.
            self.session.foreground().wait_idle(self.tuning.foreground_wait).await;
        }
        out.flush().await.map_err(io_fail)?;
        drop(out);
        if file.size > 0 && written != file.size {
            return Err(Fail::Other(format!(
                "size mismatch: got {written} bytes, expected {}",
                file.size
            )));
        }
        if let Some(want) = sha256 {
            let got = hex::encode(hasher.finalize());
            if got != want {
                return Err(Fail::Other(format!(
                    "checksum mismatch: got sha256 {got}, expected {want}"
                )));
            }
        }
        tokio::fs::rename(&part.path, &dest).await.map_err(io_fail)?;
        part.armed = false;
        Ok(())
    }
}

impl Run<'_> {
    /// Delete the part files in `files/thumb` and `files/cover` that no live
    /// download writes: their file has no fresh claim of another process.
    fn sweep_parts(&self) {
        let cache = self.session.cache();
        for dir in ["files/thumb", "files/cover"] {
            let Ok(entries) = std::fs::read_dir(cache.abs(dir)) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let Some((stem, _)) = name.split_once(".part.") else {
                    continue;
                };
                let rel = format!("{dir}/{stem}");
                let live = cache
                    .offline_claimed_by_other(&rel, &self.owner, self.tuning.claim_stale_secs)
                    .unwrap_or(true);
                if !live {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

/// Delete part files of `dest` other processes left behind (the caller holds
/// the claim, so none of them is still being written).
fn remove_stale_parts(dest: &Path, owner: &str) {
    let (Some(dir), Some(name)) = (dest.parent(), dest.file_name()) else {
        return;
    };
    let prefix = format!("{}.part.", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let n = entry.file_name();
        let n = n.to_string_lossy();
        if let Some(other) = n.strip_prefix(&prefix) {
            if other != owner {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}
