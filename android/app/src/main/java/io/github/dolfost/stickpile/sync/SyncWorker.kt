package io.github.dolfost.stickpile.sync

import android.content.Context
import android.util.Log
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters
import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.rust.LibraryException

/**
 * The periodic background sync (WorkManager, default initializer;
 * scheduled by [SyncScheduler]). One run is `StickpileLibrary.syncNow`: it
 * pulls the item changes into the offline cache and then sends the uses
 * recorded on this device (`Session::sync` flushes the usage queue after
 * the item sync; a failed flush does not fail the sync, the uses stay
 * queued for the next one).
 *
 * Not `syncIfDue`: WorkManager's schedule (every 15 minutes, with a
 * network) is the background policy. The library's own policy measures time
 * with a clock that stops while the phone sleeps (Rust's `Instant`,
 * `CLOCK_MONOTONIC` on Android), so in a process that stays alive across
 * sleep it could keep finding nothing due.
 *
 * A failure is recorded by the library (`SyncState.lastError`) and logged
 * here; [SyncOutcome.after] decides between one retry and the next period.
 */
class SyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val failure = try {
            StickpileApp.from(applicationContext).library.syncNow()
            null
        } catch (e: LibraryException) {
            Log.w(TAG, "background sync failed: ${e.javaClass.simpleName}: ${e.message}")
            e.kind()
        }
        return when (SyncOutcome.after(failure, runAttemptCount)) {
            SyncOutcome.DONE -> Result.success()
            SyncOutcome.RETRY -> Result.retry()
        }
    }

    private companion object {
        const val TAG = "stickpile.sync"
    }
}
