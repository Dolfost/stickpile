//! Keeping offline in the resident popup (display-free, shared with the
//! macOS app): the conversions between the core's offline types and the
//! model's, the downloader's schedule ([`Downloads`]: one run at a time,
//! after each successful sync and after a keep), and the "On this device"
//! block of the Settings page in words ([`Panel`]). See `docs/offline.md`.

use stickpile_core::cache::EvictReport;
use stickpile_core::offline::{
    Estimate as CoreEstimate, OfflineControl, OfflineOutcome, OfflineState, OfflineStatus, OfflineUsage, Reason,
    RunEnd, Scope,
};
use stickpile_i18n::global::{tf, tp};
use stickpile_quick::offline::{human_bytes, Estimate, KeepScope, KeptFor};

// -- the core's types and the model's ----------------------------------------------------

pub fn scope_to_core(s: &KeepScope) -> Scope {
    match s {
        KeepScope::Item(id) => Scope::Item(id.clone()),
        KeepScope::Set(id) => Scope::Set(id.clone()),
        KeepScope::Library => Scope::Library,
    }
}

pub fn scope_from_core(s: &Scope) -> KeepScope {
    match s {
        Scope::Item(id) => KeepScope::Item(id.clone()),
        Scope::Set(id) => KeepScope::Set(id.clone()),
        Scope::Library => KeepScope::Library,
    }
}

pub fn reason_from_core(r: Reason) -> KeptFor {
    match r {
        Reason::Item => KeptFor::Item,
        Reason::Set => KeptFor::Set,
        Reason::Library => KeptFor::Library,
        Reason::Used => KeptFor::Used,
        Reason::Liked => KeptFor::Liked,
        Reason::OwnPack => KeptFor::OwnPack,
    }
}

pub fn estimate_from_core(e: &CoreEstimate) -> Estimate {
    Estimate {
        items: e.items,
        files: e.totals.files,
        bytes: e.totals.bytes,
        present_bytes: e.totals.present_bytes,
        skipped: e.skipped.len() as u64,
        kept_bytes_after: e.kept_bytes_after,
        budget: e.budget,
    }
}

// -- the downloader's schedule ------------------------------------------------------------

/// When the resident popup runs `Session::offline_run`: after every
/// successful sync and after a keep, one run at a time; asked while one
/// runs, another follows it. Pause and cancel go to [`Downloads::control`]
/// (a pause holds across runs until resumed).
#[derive(Debug, Default)]
pub struct Downloads {
    pub control: OfflineControl,
    running: bool,
    again: bool,
    /// How the last run ended.
    pub last: Option<RunEnd>,
}

impl Downloads {
    /// A run is wanted: `true` when the shell should start one now.
    pub fn want(&mut self) -> bool {
        if self.running {
            self.again = true;
            return false;
        }
        self.running = true;
        true
    }

    /// The run ended (`None`: it failed before it started); `true` when
    /// another was asked for meanwhile and should start now.
    pub fn finished(&mut self, outcome: Option<&OfflineOutcome>) -> bool {
        self.running = false;
        self.last = outcome.map(|o| o.end);
        if std::mem::take(&mut self.again) {
            self.running = true;
            return true;
        }
        false
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// Another account or server: what ran was for the old one.
    pub fn forget(&mut self) {
        self.control.cancel();
        self.last = None;
    }
}

// -- the Settings page's block -------------------------------------------------------------

/// What the "On this device" block of the Settings page knows: the usage,
/// the downloader's last status, and the answer to the last action.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Panel {
    pub usage: Option<OfflineUsage>,
    /// The downloader's status, once a run published one.
    pub status: Option<OfflineStatus>,
    pub running: bool,
    pub paused: bool,
    /// What the last action of the block did ("Cleared 12 files..."), and
    /// whether it failed.
    pub message: Option<(String, bool)>,
}

/// The block in words, for the shells to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelView {
    /// The "Keep everything offline" switch.
    pub library: bool,
    /// "Kept offline: 120 files, 45.2 MB of 1.0 GB."
    pub kept: String,
    /// "Other cached files: 300, 12.1 MB (the cache size limit applies)."
    pub other: String,
    /// The downloader: "Downloading: 40 of 120 files, 12.0 of 45.2 MB.";
    /// empty before its first run.
    pub state: String,
    /// Draw the state as a problem (limit reached, disk full).
    pub state_warning: bool,
    /// The progress bar, 0..=1; `None`: no bar.
    pub fraction: Option<f64>,
    /// Show "Pause" (a run downloads), "Resume" (paused), or neither.
    pub can_pause: bool,
    pub can_resume: bool,
    /// Show "Cancel" (a run is on).
    pub can_cancel: bool,
    /// "3 files could not be downloaded...": empty when none.
    pub failures: String,
    /// "12 left out by the settings below.": empty when none.
    pub skipped: String,
    pub message: Option<String>,
    pub message_error: bool,
    /// "Remove kept files" has something to remove.
    pub can_remove: bool,
}

impl Panel {
    /// The block; `library`: the whole library is kept (the model knows).
    pub fn view(&self, library: bool) -> PanelView {
        let budget = self.usage.and_then(|u| u.budget);
        let (kept, other) = match &self.usage {
            Some(u) => (
                kept_text(u.kept_files, u.kept_bytes, u.budget),
                tf(
                    "Other cached files: {n}, {size} (the cache size limit applies).",
                    &[("n", &u.other_files), ("size", &human_bytes(u.other_bytes))],
                ),
            ),
            None => (String::new(), String::new()),
        };
        let (state, warning, fraction) = match &self.status {
            Some(s) => state_text(s, budget, self.paused),
            None => (String::new(), false, None),
        };
        let failed = self.status.as_ref().map_or(0, |s| s.failed);
        let skipped = self.status.as_ref().map_or(0, |s| s.skipped);
        PanelView {
            library,
            kept,
            other,
            state,
            state_warning: warning,
            fraction,
            can_pause: self.running && !self.paused,
            can_resume: self.paused,
            can_cancel: self.running,
            failures: if failed == 0 {
                String::new()
            } else {
                tp(
                    failed,
                    "{n} file could not be downloaded; it is tried again on the next run.",
                    "{n} files could not be downloaded; they are tried again on the next run.",
                )
            },
            skipped: if skipped == 0 {
                String::new()
            } else {
                tf(
                    "{n} left out by the settings below (kinds, size limit).",
                    &[("n", &skipped)],
                )
            },
            message: self.message.as_ref().map(|(m, _)| m.clone()),
            message_error: self.message.as_ref().is_some_and(|(_, e)| *e),
            can_remove: self.usage.is_some_and(|u| u.kept_files > 0) || library,
        }
    }

    /// What the plan says between runs (after a keep, a release, a
    /// removal): its numbers replace the last run's, whose stop (the limit,
    /// the disk, the network) still holds while the plan is not complete.
    pub fn planned(&mut self, mut s: OfflineStatus) {
        if self.running {
            return;
        }
        if let Some(last) = &self.status {
            let stopped = matches!(
                last.state,
                OfflineState::LimitReached | OfflineState::DiskFull | OfflineState::WaitingForNetwork
            );
            if stopped && s.files_done < s.files_total {
                s.state = last.state;
                s.message = last.message.clone();
            }
        }
        self.status = Some(s);
    }

    /// "Clear cache" went through.
    pub fn cleared(&mut self, r: &EvictReport) {
        let text = tp(
            r.removed_files as u64,
            "Cleared {n} file ({size}); {kept} kept files stayed.",
            "Cleared {n} files ({size}); {kept} kept files stayed.",
        )
        .replace("{size}", &human_bytes(r.freed_bytes))
        .replace("{kept}", &r.kept_files.to_string());
        self.message = Some((text, false));
    }

    pub fn failed(&mut self, error: String) {
        self.message = Some((error, true));
    }
}

fn kept_text(files: u64, bytes: u64, budget: Option<u64>) -> String {
    let base = tp(
        files,
        "Kept offline: {n} file, {size}",
        "Kept offline: {n} files, {size}",
    )
    .replace("{size}", &human_bytes(bytes));
    match budget {
        Some(b) => tf(
            "{kept} of {budget} allowed.",
            &[("kept", &base), ("budget", &human_bytes(b))],
        ),
        None => tf("{kept}, no limit.", &[("kept", &base)]),
    }
}

/// The downloader's line, whether it is a problem, and the bar.
fn state_text(s: &OfflineStatus, budget: Option<u64>, paused: bool) -> (String, bool, Option<f64>) {
    let files = tf(
        "{done} of {total} files, {bytes_done} of {bytes_total}",
        &[
            ("done", &s.files_done),
            ("total", &s.files_total),
            ("bytes_done", &human_bytes(s.bytes_done)),
            ("bytes_total", &human_bytes(s.bytes_total)),
        ],
    );
    let fraction = (s.bytes_total > 0).then(|| (s.bytes_done as f64 / s.bytes_total as f64).clamp(0.0, 1.0));
    let state = if paused && s.state != OfflineState::Idle {
        OfflineState::Paused
    } else {
        s.state
    };
    let (text, warning) = match state {
        OfflineState::Downloading => (tf("Downloading: {files}.", &[("files", &files)]), false),
        OfflineState::Paused => (tf("Paused: {files}.", &[("files", &files)]), false),
        OfflineState::WaitingForNetwork => (
            tf(
                "Waiting for the server: {files} on this device.",
                &[("files", &files)],
            ),
            false,
        ),
        OfflineState::LimitReached => (
            tf(
                "The limit of {budget} is reached: {files}. Nothing was deleted; raise the limit below to download the rest.",
                &[
                    ("budget", &budget.map_or_else(|| "?".to_string(), human_bytes)),
                    ("files", &files),
                ],
            ),
            true,
        ),
        OfflineState::DiskFull => (
            tf(
                "The disk is full: {files}. Nothing was deleted.",
                &[("files", &files)],
            ),
            true,
        ),
        OfflineState::Idle if s.files_done >= s.files_total => (
            tp(
                s.files_total,
                "Up to date: the {n} kept file is on this device.",
                "Up to date: all {n} kept files are on this device.",
            ),
            false,
        ),
        OfflineState::Idle => (
            tf(
                "{files} on this device; the rest downloads after the next sync.",
                &[("files", &files)],
            ),
            false,
        ),
    };
    (text, warning, fraction)
}

/// Remove the kept files: what the question says it removes.
pub fn remove_numbers(usage: Option<&OfflineUsage>) -> (u64, u64) {
    usage.map_or((0, 0), |u| (u.kept_files, u.kept_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use stickpile_core::offline::{PlanScope, ScopeProgress};

    fn status(state: OfflineState, done: u64, total: u64) -> OfflineStatus {
        OfflineStatus {
            state,
            files_done: done,
            files_total: total,
            bytes_done: done * 1_000_000,
            bytes_total: total * 1_000_000,
            skipped: 0,
            failed: 0,
            scopes: vec![ScopeProgress {
                scope: PlanScope::Thumbnails,
                name: None,
                files_done: done,
                files_total: total,
                bytes_done: 0,
                bytes_total: 0,
            }],
            message: None,
        }
    }

    #[test]
    fn scopes_and_reasons_convert_both_ways() {
        for s in [
            KeepScope::Item("a".into()),
            KeepScope::Set("s".into()),
            KeepScope::Library,
        ] {
            assert_eq!(scope_from_core(&scope_to_core(&s)), s);
        }
        assert_eq!(reason_from_core(Reason::OwnPack), KeptFor::OwnPack);
    }

    #[test]
    fn one_run_at_a_time_and_one_more_when_asked_meanwhile() {
        let mut d = Downloads::default();
        assert!(d.want());
        assert!(d.running());
        assert!(!d.want(), "one runs");
        assert!(!d.want());
        assert!(d.finished(None), "asked meanwhile: once more");
        assert!(d.running());
        assert!(!d.finished(None));
        assert!(!d.running());
    }

    #[test]
    fn the_block_in_words() {
        let mut p = Panel {
            usage: Some(OfflineUsage {
                kept_files: 120,
                kept_bytes: 45_200_000,
                other_files: 300,
                other_bytes: 12_100_000,
                budget: Some(1_000_000_000),
            }),
            status: Some(status(OfflineState::Downloading, 40, 120)),
            running: true,
            ..Panel::default()
        };
        let v = p.view(false);
        assert_eq!(v.kept, "Kept offline: 120 files, 45.2 MB of 1.0 GB allowed.");
        assert_eq!(
            v.other,
            "Other cached files: 300, 12.1 MB (the cache size limit applies)."
        );
        assert_eq!(v.state, "Downloading: 40 of 120 files, 40.0 MB of 120.0 MB.");
        assert_eq!(v.fraction.map(|f| (f * 3.0).round()), Some(1.0));
        assert!(v.can_pause && v.can_cancel && !v.can_resume && v.can_remove);
        assert!(v.failures.is_empty() && v.skipped.is_empty());

        p.paused = true;
        let v = p.view(false);
        assert!(v.state.starts_with("Paused: 40 of 120 files"), "{}", v.state);
        assert!(v.can_resume && !v.can_pause);

        p.paused = false;
        p.running = false;
        p.status = Some(OfflineStatus {
            failed: 3,
            skipped: 12,
            ..status(OfflineState::LimitReached, 100, 120)
        });
        let v = p.view(true);
        assert!(v.library);
        assert!(v.state_warning);
        assert!(
            v.state.starts_with("The limit of 1.0 GB is reached: 100 of 120 files"),
            "{}",
            v.state
        );
        assert!(
            v.failures.starts_with("3 files could not be downloaded"),
            "{}",
            v.failures
        );
        assert!(v.skipped.starts_with("12 left out"), "{}", v.skipped);
        assert!(!v.can_cancel);

        p.status = Some(status(OfflineState::Idle, 120, 120));
        assert_eq!(
            p.view(false).state,
            "Up to date: all 120 kept files are on this device."
        );
        p.status = Some(status(OfflineState::WaitingForNetwork, 10, 120));
        assert!(p
            .view(false)
            .state
            .starts_with("Waiting for the server: 10 of 120 files"));
        p.status = Some(status(OfflineState::DiskFull, 10, 120));
        assert!(p.view(false).state_warning);
    }

    #[test]
    fn the_plan_between_runs_replaces_the_numbers_and_keeps_a_stop() {
        let mut p = Panel {
            status: Some(status(OfflineState::Idle, 28, 28)),
            ..Panel::default()
        };
        // The kept files removed: the thumbnails are wanted again.
        p.planned(status(OfflineState::Idle, 0, 14));
        assert_eq!(
            p.view(false).state,
            "0 of 14 files, 0 B of 14.0 MB on this device; the rest downloads after the next sync."
        );
        p.status = Some(status(OfflineState::LimitReached, 10, 20));
        p.planned(status(OfflineState::Idle, 11, 20));
        assert!(p.view(false).state.starts_with("The limit of ? is reached: 11 of 20"));
        p.planned(status(OfflineState::Idle, 20, 20));
        assert!(p.view(false).state.starts_with("Up to date"), "complete: no stop");
        // A run publishes its own.
        p.running = true;
        p.planned(status(OfflineState::Idle, 0, 1));
        assert!(p.view(false).state.starts_with("Up to date"));
    }

    #[test]
    fn clearing_says_how_many_kept_files_stayed() {
        let mut p = Panel::default();
        p.cleared(&EvictReport {
            removed_files: 12,
            freed_bytes: 3_400_000,
            total_bytes: 0,
            kept_files: 40,
            kept_bytes: 9_000_000,
        });
        let v = p.view(false);
        assert_eq!(
            v.message.as_deref(),
            Some("Cleared 12 files (3.4 MB); 40 kept files stayed.")
        );
        assert!(!v.message_error);
        assert!(v.state.is_empty(), "no run yet: no state line");
        assert!(!v.can_remove);
        p.usage = Some(OfflineUsage {
            budget: None,
            ..OfflineUsage::default()
        });
        assert_eq!(p.view(false).kept, "Kept offline: 0 files, 0 B, no limit.");
    }
}
