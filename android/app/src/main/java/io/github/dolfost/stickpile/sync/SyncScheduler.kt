package io.github.dolfost.stickpile.sync

import android.content.Context
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequest
import androidx.work.WorkManager
import java.util.concurrent.TimeUnit

/**
 * Schedules the periodic background sync ([SyncWorker]) with WorkManager:
 * one unique periodic work, every [PeriodicWorkRequest.MIN_PERIODIC_INTERVAL_MILLIS]
 * (15 minutes in WorkManager 2.12, the shortest it allows; the library's
 * own policy would sync every 10), only with a connected network. The
 * onboarding and settings screens call it.
 */
object SyncScheduler {
    /** The unique work's name. */
    const val WORK_NAME = "stickpile-sync"

    /**
     * The delay before the one retry of a failed run ([SyncOutcome]): a
     * server that was briefly down gets a second chance well before the
     * next period.
     */
    const val RETRY_DELAY_MINUTES = 2L

    /**
     * Make sure the periodic sync is scheduled (idempotent: an existing
     * schedule is kept, `ExistingPeriodicWorkPolicy.KEEP`): at app start and
     * after a login.
     */
    fun ensureScheduled(context: Context) {
        val constraints = Constraints.Builder()
            .setRequiredNetworkType(NetworkType.CONNECTED)
            .build()
        val request = PeriodicWorkRequest.Builder(
            SyncWorker::class.java,
            PeriodicWorkRequest.MIN_PERIODIC_INTERVAL_MILLIS,
            TimeUnit.MILLISECONDS,
        )
            .setConstraints(constraints)
            .setBackoffCriteria(BackoffPolicy.LINEAR, RETRY_DELAY_MINUTES, TimeUnit.MINUTES)
            .build()
        WorkManager.getInstance(context)
            .enqueueUniquePeriodicWork(WORK_NAME, ExistingPeriodicWorkPolicy.KEEP, request)
    }

    /** Stop it: after a logout. */
    fun cancel(context: Context) {
        WorkManager.getInstance(context).cancelUniqueWork(WORK_NAME)
    }
}
