//! Keeping stickers on this phone (docs/offline.md), the library's side:
//! the user keeps a sticker, a pack or the whole library, automatic rules
//! keep more, and the downloader (`offline_run`, which the app runs in a
//! WorkManager job) fetches the files, so they insert, share and play with
//! the server unreachable. Everything is `stickpile_core::offline`'s; this
//! converts its types and holds one [`OfflineControl`] for the library's
//! runs.
//!
//! [`OfflineControl`]: stickpile_core::offline::OfflineControl

use stickpile_core::offline::Scope;

use crate::library::{lock, on_device, Inner, StickpileLibrary};
use crate::types::*;
use crate::LibraryError;

impl Inner {
    /// Where the downloader stands, with whether a run goes on now.
    pub(crate) fn offline_status(&self) -> OfflineStatus {
        OfflineStatus::new(
            &self.offline.status(),
            *self.offline_running.borrow(),
            self.offline.is_paused(),
        )
    }

    /// Wake those waiting in `offline_status_change` (a pause, resume or
    /// cancel changes the status without a run publishing it).
    fn offline_nudge(&self) {
        self.offline_running.send_modify(|_| {});
    }
}

/// While it lives, a run goes on (`OfflineStatus.running`).
struct Running<'a>(&'a Inner);

impl<'a> Running<'a> {
    fn start(inner: &'a Inner) -> Running<'a> {
        inner.offline_running.send_replace(true);
        Running(inner)
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.offline_running.send_replace(false);
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl StickpileLibrary {
    /// The offline options in use: those `set_offline_options` gave, else
    /// the config's (the core's defaults on Android). No network.
    pub fn offline_options(&self) -> OfflineOptions {
        OfflineOptions::from(&self.inner.session().offline_options())
    }

    /// Use these offline options (the app's settings), from the next plan
    /// or run on; also after the server changes. The config's list of chat
    /// apps (`offline.targets`) stays. No network.
    pub fn set_offline_options(&self, options: OfflineOptions) {
        let session = self.inner.session();
        session.set_offline_options(options.over(&session.config().offline));
        *lock(&self.inner.offline_options) = Some(options);
    }

    /// Keep `scope` on this phone (again is fine). What of it is on the
    /// phone already is kept at once; `offline_run` downloads the rest.
    /// Stored in the offline cache, on this phone only. No network.
    pub async fn keep_offline(&self, scope: OfflineScope) -> Result<(), LibraryError> {
        self.write(move |inner| async move {
            let session = inner.session();
            tokio::task::spawn_blocking(move || session.offline_keep(&Scope::from(scope))).await??;
            Ok(())
        })
        .await
    }

    /// Stop keeping `scope`. Its files that nothing else keeps become
    /// ordinary cached files (the size limit may delete them later); none
    /// is deleted at once. No network.
    pub async fn release_offline(&self, scope: OfflineScope) -> Result<(), LibraryError> {
        self.write(move |inner| async move {
            let session = inner.session();
            tokio::task::spawn_blocking(move || session.offline_release(&Scope::from(scope))).await??;
            Ok(())
        })
        .await
    }

    /// What the user keeps, oldest first (the automatic rules are options,
    /// not listed here). No network.
    pub fn offline_keeps(&self) -> Result<Vec<OfflineScope>, LibraryError> {
        let keeps = self.inner.session().offline_keeps()?;
        Ok(keeps.into_iter().map(OfflineScope::from).collect())
    }

    /// Why the sticker is kept on this phone; empty when it is not. No
    /// network.
    pub fn offline_reasons(&self, item_id: String) -> Result<Vec<KeepReason>, LibraryError> {
        let session = self.inner.session();
        let Some(item) = self.inner.local_item(&session, &item_id)? else {
            // Not cached: only a keep of its own can name it.
            let kept = session.offline_keeps()?.contains(&Scope::Item(item_id));
            return Ok(if kept { vec![KeepReason::Sticker] } else { Vec::new() });
        };
        Ok(session
            .offline_reasons(&item)?
            .into_iter()
            .map(KeepReason::from)
            .collect())
    }

    /// What keeping `scope` would take with the options in use: its files,
    /// how much of them is on this phone, what the options leave out, the
    /// kept bytes with it, and whether that fits the budget. No network.
    pub async fn offline_estimate(&self, scope: OfflineScope) -> Result<OfflineEstimate, LibraryError> {
        self.read(move |inner| async move {
            let session = inner.session();
            let estimate = tokio::task::spawn_blocking(move || session.offline_estimate(&Scope::from(scope))).await??;
            Ok(estimate.into())
        })
        .await
    }

    /// What is kept and what that means in files: the totals, per kept
    /// scope and rule, what is skipped and what failed. No network.
    pub async fn offline_plan(&self) -> Result<OfflinePlan, LibraryError> {
        self.read(|inner| async move {
            let session = inner.session();
            let plan = tokio::task::spawn_blocking(move || session.offline_plan()).await??;
            Ok(plan.into())
        })
        .await
    }

    /// The files on this phone, kept and other, and the budget. No network.
    pub fn offline_usage(&self) -> Result<OfflineUsage, LibraryError> {
        Ok(self.inner.session().offline_usage()?.into())
    }

    /// The confirmed "Remove kept files": deletes every kept file and
    /// forgets what is kept (a run going on ends). The automatic rules stay
    /// as the options say. No network.
    pub fn remove_kept_files(&self) -> Result<CacheCleared, LibraryError> {
        Ok(self.inner.session().offline_remove_all()?.into())
    }

    /// For each of `item_ids`, whether picking it works with the server
    /// unreachable: a rendition `prepare` would choose for
    /// `editor_package`'s field taking `accepted_mime_types` is on this
    /// phone (kept, or cached). For the tiles' marks: nothing is downloaded
    /// or touched; false for an item neither shown nor cached. No network.
    pub fn available_offline(
        &self,
        item_ids: Vec<String>,
        editor_package: String,
        accepted_mime_types: Vec<String>,
        sticker: bool,
    ) -> Vec<bool> {
        let session = self.inner.session();
        let target = stickpile_core::target_for_package(&editor_package);
        item_ids
            .iter()
            .map(|id| match self.inner.local_item(&session, id) {
                Ok(Some(item)) => {
                    stickpile_core::variant_for(&on_device(&session, &item), target, &accepted_mime_types, sticker)
                        .is_some()
                }
                _ => false,
            })
            .collect()
    }

    /// A request found the server unreachable lately, and the lists answer
    /// from the offline cache (for five minutes, or until the server
    /// answers again). No network.
    pub fn server_unreachable(&self) -> bool {
        self.inner.session().client().believed_unreachable()
    }

    /// Download what is kept and not on this phone yet, until done,
    /// cancelled (`cancel_offline`, or the calling coroutine is
    /// cancelled), the server is unreachable, the budget is reached or the
    /// storage is full; while paused it waits. Lists the liked stickers and
    /// the packs' covers on the server first. Progress: `offline_status`.
    /// One run at a time: a second call waits for the first. The server is
    /// tried even if it was found away a moment ago: the app runs this when
    /// the platform says a network is there.
    pub async fn offline_run(&self) -> Result<OfflineOutcome, LibraryError> {
        self.read(|inner| async move {
            let _one = inner.offline_one.lock().await;
            let session = inner.session();
            session.client().forget_unreachable();
            let outcome = {
                let _running = Running::start(&inner);
                session.offline_run(&inner.offline).await?
            };
            Ok(OfflineOutcome {
                end: outcome.end.into(),
                complete: outcome.is_complete(),
                status: OfflineStatus::new(&outcome.status, false, inner.offline.is_paused()),
            })
        })
        .await
    }

    /// Where the downloader stands now. No network.
    pub fn offline_status(&self) -> OfflineStatus {
        self.inner.offline_status()
    }

    /// The status once it differs from `seen` (at once when it does
    /// already, or `seen` is null): for a screen that follows the progress.
    /// Stops waiting when the calling coroutine is cancelled.
    pub async fn offline_status_change(&self, seen: Option<OfflineStatus>) -> Result<OfflineStatus, LibraryError> {
        self.read(move |inner| async move {
            let mut status = inner.offline.subscribe();
            let mut running = inner.offline_running.subscribe();
            loop {
                status.borrow_and_update();
                running.borrow_and_update();
                let now = inner.offline_status();
                if seen.as_ref() != Some(&now) {
                    return Ok(now);
                }
                let closed = tokio::select! {
                    r = status.changed() => r.is_err(),
                    r = running.changed() => r.is_err(),
                };
                if closed {
                    return Ok(now);
                }
            }
        })
        .await
    }

    /// Hold the downloads: those going on stop (and start again first on
    /// resume), none starts, until `resume_offline`. No network.
    pub fn pause_offline(&self) {
        self.inner.offline.pause();
        self.inner.offline_nudge();
    }

    /// Let the downloads go on after `pause_offline`. No network.
    pub fn resume_offline(&self) {
        self.inner.offline.resume();
        self.inner.offline_nudge();
    }

    /// End the run going on, if any; the next one starts afresh. No network.
    pub fn cancel_offline(&self) {
        self.inner.offline.cancel();
        self.inner.offline_nudge();
    }
}
