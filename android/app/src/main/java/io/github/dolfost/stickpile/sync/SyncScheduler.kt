package io.github.dolfost.stickpile.sync

import android.content.Context

/**
 * Schedules the periodic background sync ([SyncWorker], WorkManager, with
 * a network constraint). Stub: the sync work package fills it in; the
 * signatures are fixed, the onboarding and settings screens call them.
 */
object SyncScheduler {
    /** Make sure the periodic sync is scheduled (idempotent): at app start and after a login. */
    fun ensureScheduled(context: Context) {}

    /** Stop it: after a logout. */
    fun cancel(context: Context) {}
}
