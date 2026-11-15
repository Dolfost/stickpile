package io.github.dolfost.stickpile.sync

import io.github.dolfost.stickpile.rust.LibraryErrorKind

/**
 * What [SyncWorker] tells WorkManager after a run (pure, no Android).
 *
 * A failed sync is already logged and recorded by the library
 * (`SyncState.lastError`), and the app and the keyboard call `syncIfDue`
 * whenever they open, so the worker mostly waits for its next period.
 * Only a failure that may be gone in a minute (the server unreachable, say
 * while a VPN comes up after the network did, busy, or answering with an
 * error) is retried, once, [SyncScheduler.RETRY_DELAY_MINUTES] later.
 *
 * Not more often: WorkManager backs a periodic work off from one retry to
 * the next (the delay grows with every attempt, up to five hours) and runs
 * it only after that delay, so retrying every failure would make the sync
 * rarer than its period during a long outage. Not sooner: the library
 * starts no attempt within 60 s of the last one (`SyncPolicy`'s
 * `RETRY_AFTER`), so an earlier retry would find no sync due and do
 * nothing.
 */
enum class SyncOutcome {
    /** Done until the next period (WorkManager: `Result.success()`). */
    DONE,

    /** Run again after the backoff delay (WorkManager: `Result.retry()`). */
    RETRY,
    ;

    companion object {
        /** Retries after one failed run of a period. */
        const val MAX_RETRIES = 1

        /** The failures worth trying again soon. */
        val TRANSIENT: Set<LibraryErrorKind> = setOf(
            LibraryErrorKind.UNREACHABLE,
            LibraryErrorKind.RATE_LIMITED,
            LibraryErrorKind.SERVER,
        )

        /**
         * The outcome of a run that failed with [failure] (null: it did not
         * fail), when WorkManager has already run this period's work
         * [runAttemptCount] times before (0 on the first run).
         */
        fun after(failure: LibraryErrorKind?, runAttemptCount: Int): SyncOutcome =
            if (failure in TRANSIENT && runAttemptCount < MAX_RETRIES) RETRY else DONE
    }
}
