//! Keeping offline in the resident popup (display-free, shared with the
//! macOS app): the conversions between the core's offline types and the
//! model's, the downloader's schedule ([`Downloads`]: one run at a time,
//! after each successful sync and after a keep), and the "On this device"
//! block of the Settings page in words ([`Panel`]). See `docs/offline.md`.

use stickpile_core::cache::EvictReport;
use stickpile_core::offline::{
    Estimate as CoreEstimate, OfflineControl, OfflineOutcome, OfflineState, OfflineStatus, OfflineUsage, Reason,
    RunEnd, Scope, SkipReason, Skipped,
};
use stickpile_i18n::global::{tf, tp};
use stickpile_quick::offline::{human_bytes, Estimate, KeepScope, KeptFor, Skips};

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
        skips: skips_from_core(&e.skipped),
        kept_bytes_after: e.kept_bytes_after,
        budget: e.budget,
    }
}

/// What the core leaves out, counted by reason: a sticker of a kind
/// switched off is one entry, a file over the size limit is one entry.
pub fn skips_from_core(skipped: &[Skipped]) -> Skips {
    let mut out = Skips::default();
    for s in skipped {
        match s.reason {
            SkipReason::KindOff { .. } => out.kind_off += 1,
            SkipReason::TooLarge { .. } => out.too_large += 1,
            SkipReason::NotCached => out.not_cached += 1,
            SkipReason::NoRendition => out.no_rendition += 1,
        }
    }
    out
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
    /// What the plan leaves out, by reason (the downloader's status only
    /// counts entries, a sticker and a file alike).
    pub skips: Skips,
    /// What the last action of the block did, said in the language of the
    /// moment it is shown.
    pub note: Option<PanelNote>,
}

/// The answer to the last action of the block.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelNote {
    /// "Clear cache" went through.
    Cleared(EvictReport),
    /// An action failed (the whole message).
    Failed(String),
}

/// The block in words, for the shells to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelView {
    /// The "Keep everything offline" switch.
    pub library: bool,
    /// "Kept offline: 120 files, 45.2 MiB of 1.0 GiB."
    pub kept: String,
    /// "Other cached files: 300, 12.1 MiB (the cache size limit applies)."
    pub other: String,
    /// The downloader: "Downloading: 40 of 120 files, 12.0 of 45.2 MiB.";
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
    /// What is left out, one line per reason ("2 stickers are left out:
    /// their kind is off..."): empty when nothing is.
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
            skipped: self.skips.lines().join("\n"),
            message: self.note.as_ref().map(|n| match n {
                PanelNote::Cleared(r) => tp(
                    r.removed_files as u64,
                    "Cleared {n} file ({size}); {kept} kept files stayed.",
                    "Cleared {n} files ({size}); {kept} kept files stayed.",
                )
                .replace("{size}", &human_bytes(r.freed_bytes))
                .replace("{kept}", &r.kept_files.to_string()),
                PanelNote::Failed(e) => e.clone(),
            }),
            message_error: matches!(self.note, Some(PanelNote::Failed(_))),
            can_remove: self.usage.is_some_and(|u| u.kept_files > 0) || library,
        }
    }

    /// What the plan says between runs (after a keep, a release, a
    /// removal): its numbers replace the last run's, whose stop (the limit,
    /// the disk, the network) still holds while the plan is not complete;
    /// what it leaves out holds during a run too.
    pub fn planned(&mut self, mut s: OfflineStatus, skips: Skips) {
        self.skips = skips;
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
        self.note = Some(PanelNote::Cleared(r.clone()));
    }

    pub fn failed(&mut self, error: String) {
        self.note = Some(PanelNote::Failed(error));
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

    const MIB: u64 = 1 << 20;

    fn status(state: OfflineState, done: u64, total: u64) -> OfflineStatus {
        OfflineStatus {
            state,
            files_done: done,
            files_total: total,
            bytes_done: done * MIB,
            bytes_total: total * MIB,
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
    fn skips_are_counted_by_reason() {
        let skip = |reason| Skipped {
            item_id: "a".into(),
            name: "a".into(),
            variant: None,
            reason,
        };
        let skips = skips_from_core(&[
            skip(SkipReason::KindOff {
                kind: stickpile_api::MediaKind::Video,
            }),
            skip(SkipReason::KindOff {
                kind: stickpile_api::MediaKind::Video,
            }),
            skip(SkipReason::TooLarge { size: 9, limit: 1 }),
            skip(SkipReason::NotCached),
            skip(SkipReason::NoRendition),
        ]);
        assert_eq!(
            skips,
            Skips {
                kind_off: 2,
                too_large: 1,
                not_cached: 1,
                no_rendition: 1,
            }
        );
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
                kept_bytes: 452 * MIB / 10,
                other_files: 300,
                other_bytes: 121 * MIB / 10,
                budget: Some(1024 * MIB),
            }),
            status: Some(status(OfflineState::Downloading, 40, 120)),
            running: true,
            ..Panel::default()
        };
        let v = p.view(false);
        assert_eq!(v.kept, "Kept offline: 120 files, 45.2 MiB of 1.0 GiB allowed.");
        assert_eq!(
            v.other,
            "Other cached files: 300, 12.1 MiB (the cache size limit applies)."
        );
        assert_eq!(v.state, "Downloading: 40 of 120 files, 40.0 MiB of 120.0 MiB.");
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
            // Entries: two stickers of a kind off and ten files too large.
            skipped: 12,
            ..status(OfflineState::LimitReached, 100, 120)
        });
        p.skips = Skips {
            kind_off: 2,
            too_large: 10,
            ..Skips::default()
        };
        let v = p.view(true);
        assert!(v.library);
        assert!(v.state_warning);
        assert!(
            v.state.starts_with("The limit of 1.0 GiB is reached: 100 of 120 files"),
            "{}",
            v.state
        );
        assert!(
            v.failures.starts_with("3 files could not be downloaded"),
            "{}",
            v.failures
        );
        assert_eq!(
            v.skipped,
            "2 stickers are left out: their kind is off in the Offline settings.\n\
             10 files are left out: they are larger than the largest file of the Offline settings."
        );
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
        let kind_off = Skips {
            kind_off: 1,
            ..Skips::default()
        };
        p.planned(status(OfflineState::Idle, 0, 14), kind_off);
        let v = p.view(false);
        assert_eq!(
            v.state,
            "0 of 14 files, 0 B of 14.0 MiB on this device; the rest downloads after the next sync."
        );
        assert_eq!(
            v.skipped,
            "1 sticker is left out: its kind is off in the Offline settings."
        );
        p.status = Some(status(OfflineState::LimitReached, 10, 20));
        p.planned(status(OfflineState::Idle, 11, 20), Skips::default());
        assert!(p.view(false).state.starts_with("The limit of ? is reached: 11 of 20"));
        p.planned(status(OfflineState::Idle, 20, 20), Skips::default());
        assert!(p.view(false).state.starts_with("Up to date"), "complete: no stop");
        // A run publishes its own numbers; what is left out still comes from the plan.
        p.running = true;
        p.planned(status(OfflineState::Idle, 0, 1), kind_off);
        let v = p.view(false);
        assert!(v.state.starts_with("Up to date"));
        assert!(v.skipped.starts_with("1 sticker is left out"));
    }

    #[test]
    fn clearing_says_how_many_kept_files_stayed() {
        let mut p = Panel::default();
        p.cleared(&EvictReport {
            removed_files: 12,
            freed_bytes: 34 * MIB / 10,
            total_bytes: 0,
            kept_files: 40,
            kept_bytes: 9 * MIB,
        });
        let v = p.view(false);
        assert_eq!(
            v.message.as_deref(),
            Some("Cleared 12 files (3.4 MiB); 40 kept files stayed.")
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
