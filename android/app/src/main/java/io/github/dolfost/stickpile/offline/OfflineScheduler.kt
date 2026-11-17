package io.github.dolfost.stickpile.offline

import android.content.Context
import androidx.work.Constraints
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequest
import androidx.work.WorkInfo
import androidx.work.WorkManager
import io.github.dolfost.stickpile.rust.OfflineEnd
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map

/** What WorkManager does with the downloads' job, for the settings. */
enum class OfflineWork {
    /** No job, or it ended. */
    NONE,

    /** Waiting for its network and enough free storage. */
    WAITING,
    RUNNING,
}

/** Starting and stopping the offline downloads ([OfflineWorker]). */
interface OfflineJobs {
    /** Run the downloads when the platform allows it; [restart]: a run going on starts again (new options). */
    fun request(mobileData: Boolean, restart: Boolean = false)

    /** Stop the job (pause, stop, sign-out). */
    fun cancel()

    /** Where the job stands. */
    val work: Flow<OfflineWork>

    companion object {
        /** No job at all (tests, and screens made without the app's). */
        val NONE: OfflineJobs = object : OfflineJobs {
            override fun request(mobileData: Boolean, restart: Boolean) = Unit
            override fun cancel() = Unit
            override val work: Flow<OfflineWork> = flowOf(OfflineWork.NONE)
        }
    }
}

/**
 * Schedules the offline downloads ([OfflineWorker]) with WorkManager, like
 * the background sync ([io.github.dolfost.stickpile.sync.SyncScheduler]):
 * one unique one-time job, on a network that is not metered unless mobile
 * data is allowed, and not while storage is low. Asked for after each sync
 * that went through, after a keep, and when the options change; a request
 * while a run goes on makes the job run again after it ([takeRequest]).
 * No permission beyond WorkManager's own, no foreground service: when the
 * system stops the job (its network went, its time is up), the run stops
 * with it and WorkManager starts it again later.
 */
object OfflineScheduler {
    /** The unique work's name. */
    const val WORK_NAME = "stickpile-offline"

    /** Runs of one job at most: a request after each keeps it from going on for ever. */
    const val MAX_RUNS = 5

    private val requested = AtomicBoolean(false)

    /** The network the downloads wait for. */
    fun networkType(mobileData: Boolean): NetworkType =
        if (mobileData) NetworkType.CONNECTED else NetworkType.UNMETERED

    fun constraints(mobileData: Boolean): Constraints = Constraints.Builder()
        .setRequiredNetworkType(networkType(mobileData))
        .setRequiresStorageNotLow(true)
        .build()

    /** The job's state, for the settings. */
    fun work(infos: List<WorkInfo>): OfflineWork = when {
        infos.any { it.state == WorkInfo.State.RUNNING } -> OfflineWork.RUNNING
        infos.any { it.state == WorkInfo.State.ENQUEUED || it.state == WorkInfo.State.BLOCKED } -> OfflineWork.WAITING
        else -> OfflineWork.NONE
    }

    /**
     * Whether a request came since the last call: taken by the job before
     * each run, so that a keep or a sync during a run makes it run again.
     */
    fun takeRequest(): Boolean = requested.getAndSet(false)

    /**
     * Whether the job runs again after a run that ended with [end]: when
     * something was asked for meanwhile ([requested]) and the run got to its
     * end (not paused, stopped, offline, full), at most [MAX_RUNS] times.
     */
    fun goesOn(end: OfflineEnd, requested: Boolean, runs: Int): Boolean =
        requested && end == OfflineEnd.COMPLETE && runs < MAX_RUNS

    /** The jobs of [context]'s WorkManager. */
    fun jobs(context: Context): OfflineJobs = object : OfflineJobs {
        private val app = context.applicationContext

        override fun request(mobileData: Boolean, restart: Boolean) {
            requested.set(true)
            val request = OneTimeWorkRequest.Builder(OfflineWorker::class.java)
                .setConstraints(constraints(mobileData))
                .build()
            val policy = if (restart) ExistingWorkPolicy.REPLACE else ExistingWorkPolicy.KEEP
            WorkManager.getInstance(app).enqueueUniqueWork(WORK_NAME, policy, request)
        }

        override fun cancel() {
            WorkManager.getInstance(app).cancelUniqueWork(WORK_NAME)
        }

        override val work: Flow<OfflineWork>
            get() = WorkManager.getInstance(app).getWorkInfosForUniqueWorkFlow(WORK_NAME).map(::work)
    }
}
