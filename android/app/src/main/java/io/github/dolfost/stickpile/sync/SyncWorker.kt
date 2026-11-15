package io.github.dolfost.stickpile.sync

import android.content.Context
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters

/**
 * The periodic background sync (WorkManager, default initializer).
 * Stub: not scheduled anywhere, and does nothing when run.
 */
class SyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result = Result.success()
}
