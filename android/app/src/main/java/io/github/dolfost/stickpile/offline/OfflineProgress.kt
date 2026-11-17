package io.github.dolfost.stickpile.offline

import androidx.annotation.StringRes
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.OfflinePlan
import io.github.dolfost.stickpile.rust.OfflineState
import io.github.dolfost.stickpile.rust.OfflineStatus

/** What the settings say about the downloads. */
enum class OfflineShown(@StringRes val text: Int) {
    NOTHING_KEPT(R.string.offline_state_nothing),
    UP_TO_DATE(R.string.offline_state_up_to_date),

    /** Files are still to download, and no job runs or waits. */
    PENDING(R.string.offline_state_pending),
    DOWNLOADING(R.string.offline_state_downloading),
    PAUSED(R.string.offline_state_paused),
    WAITING_FOR_WIFI(R.string.offline_state_waiting_wifi),
    WAITING_FOR_NETWORK(R.string.offline_state_waiting_network),

    /** The last run found the server unreachable. */
    UNREACHABLE(R.string.offline_state_unreachable),
    LIMIT_REACHED(R.string.offline_state_limit),
    DISK_FULL(R.string.offline_state_disk_full),
}

/** Files and bytes on the phone of those wanted. */
data class OfflineCounts(val filesDone: ULong, val filesTotal: ULong, val bytesDone: ULong, val bytesTotal: ULong) {
    /** Done, 0 to 1 (1 when nothing is wanted). */
    val fraction: Float
        get() = if (bytesTotal > 0u) {
            (bytesDone.toDouble() / bytesTotal.toDouble()).toFloat().coerceIn(0f, 1f)
        } else if (filesTotal > 0u) {
            (filesDone.toDouble() / filesTotal.toDouble()).toFloat().coerceIn(0f, 1f)
        } else {
            1f
        }
}

/**
 * Where the downloads stand, from the library's status, WorkManager's job
 * and the plan (pure): the library says what a run did, WorkManager whether
 * one runs or waits for its network.
 */
object OfflineProgress {
    fun shown(status: OfflineStatus, work: OfflineWork, plan: OfflinePlan?, mobileData: Boolean): OfflineShown = when {
        status.paused -> OfflineShown.PAUSED
        status.running || work == OfflineWork.RUNNING -> OfflineShown.DOWNLOADING
        work == OfflineWork.WAITING && !complete(plan) ->
            if (mobileData) OfflineShown.WAITING_FOR_NETWORK else OfflineShown.WAITING_FOR_WIFI
        status.state == OfflineState.LIMIT_REACHED && !complete(plan) -> OfflineShown.LIMIT_REACHED
        status.state == OfflineState.DISK_FULL && !complete(plan) -> OfflineShown.DISK_FULL
        status.state == OfflineState.WAITING_FOR_NETWORK && !complete(plan) -> OfflineShown.UNREACHABLE
        plan == null || plan.totals.files == 0uL -> OfflineShown.NOTHING_KEPT
        complete(plan) -> OfflineShown.UP_TO_DATE
        else -> OfflineShown.PENDING
    }

    /** The counts to show: the run's while one goes on, else the plan's. */
    fun counts(status: OfflineStatus, plan: OfflinePlan?): OfflineCounts? = when {
        status.running && status.filesTotal > 0u ->
            OfflineCounts(status.filesDone, status.filesTotal, status.bytesDone, status.bytesTotal)
        plan != null && plan.totals.files > 0u -> plan.totals.let {
            OfflineCounts(it.presentFiles, it.files, it.presentBytes, it.bytes)
        }
        else -> null
    }

    /** Pause is offered while downloads go on or wait; resume while paused. */
    fun canPause(shown: OfflineShown): Boolean = shown in setOf(
        OfflineShown.DOWNLOADING,
        OfflineShown.WAITING_FOR_WIFI,
        OfflineShown.WAITING_FOR_NETWORK,
        OfflineShown.PENDING,
        OfflineShown.UNREACHABLE,
    )

    /** Stop is offered while a run goes on or waits. */
    fun canStop(shown: OfflineShown): Boolean = shown in setOf(
        OfflineShown.DOWNLOADING,
        OfflineShown.WAITING_FOR_WIFI,
        OfflineShown.WAITING_FOR_NETWORK,
    )

    /** "Download now" is offered when files are missing and nothing runs or waits. */
    fun canStart(shown: OfflineShown): Boolean = shown in setOf(
        OfflineShown.PENDING,
        OfflineShown.UNREACHABLE,
        OfflineShown.LIMIT_REACHED,
        OfflineShown.DISK_FULL,
    )

    private fun complete(plan: OfflinePlan?): Boolean =
        plan != null && plan.totals.presentFiles == plan.totals.files
}
