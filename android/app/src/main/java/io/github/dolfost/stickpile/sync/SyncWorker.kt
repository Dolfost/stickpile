package io.github.dolfost.stickpile.sync

import android.content.Context
import android.util.Log
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters
import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.rust.LibraryException

/**
 * The periodic background sync (WorkManager, default initializer;
 * scheduled by [SyncScheduler]). One run is `StickpileLibrary.syncIfDue`:
 * when the library's policy says a sync is due (the last one is older than
 * 10 minutes, and no attempt started in the last 60 s), it pulls the item
 * changes into the offline cache and then sends the uses recorded on this
 * device (`Session::sync` flushes the usage queue after the item sync; a
 * failed flush does not fail the sync, the uses stay queued for the next
 * one). Otherwise, or when a sync is already running, it returns at once.
 *
 * A failure is recorded by the library (`SyncState.lastError`) and logged
 * here; [SyncOutcome.after] decides between one retry and the next period.
 */
class SyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val failure = try {
            StickpileApp.from(applicationContext).library.syncIfDue()
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
