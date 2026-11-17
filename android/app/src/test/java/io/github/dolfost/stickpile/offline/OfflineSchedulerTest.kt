package io.github.dolfost.stickpile.offline

import androidx.work.NetworkType
import androidx.work.WorkInfo
import io.github.dolfost.stickpile.rust.OfflineEnd
import java.util.UUID
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** When the offline downloads may run (offline.OfflineScheduler). */
class OfflineSchedulerTest {
    @Test
    fun withoutMobileDataTheDownloadsWaitForANetworkThatIsNotMetered() {
        assertEquals(NetworkType.UNMETERED, OfflineScheduler.networkType(mobileData = false))
        val c = OfflineScheduler.constraints(mobileData = false)
        assertEquals(NetworkType.UNMETERED, c.requiredNetworkType)
        assertTrue("not on low storage", c.requiresStorageNotLow())
    }

    @Test
    fun withMobileDataAnyConnectedNetworkWill() {
        assertEquals(NetworkType.CONNECTED, OfflineScheduler.networkType(mobileData = true))
        val c = OfflineScheduler.constraints(mobileData = true)
        assertEquals(NetworkType.CONNECTED, c.requiredNetworkType)
        assertTrue("not on low storage", c.requiresStorageNotLow())
    }

    @Test
    fun nothingElseIsRequired() {
        for (mobileData in listOf(false, true)) {
            val c = OfflineScheduler.constraints(mobileData)
            assertFalse(c.requiresCharging())
            assertFalse(c.requiresBatteryNotLow())
            assertFalse(c.requiresDeviceIdle())
        }
    }

    @Test
    fun aRequestDuringARunRunsItAgainOnlyAfterACompleteRun() {
        assertTrue(OfflineScheduler.goesOn(OfflineEnd.COMPLETE, requested = true, runs = 1))
        assertFalse("nothing asked for", OfflineScheduler.goesOn(OfflineEnd.COMPLETE, requested = false, runs = 1))
        for (end in OfflineEnd.entries - OfflineEnd.COMPLETE) {
            assertFalse("$end", OfflineScheduler.goesOn(end, requested = true, runs = 1))
        }
        assertFalse(
            "not for ever",
            OfflineScheduler.goesOn(OfflineEnd.COMPLETE, requested = true, runs = OfflineScheduler.MAX_RUNS),
        )
    }

    @Test
    fun aRequestIsTakenOnce() {
        OfflineScheduler.takeRequest()
        assertFalse(OfflineScheduler.takeRequest())
    }

    @Test
    fun theJobsStateForTheSettings() {
        fun info(state: WorkInfo.State) = WorkInfo(UUID.randomUUID(), state, emptySet())
        assertEquals(OfflineWork.NONE, OfflineScheduler.work(emptyList()))
        assertEquals(OfflineWork.WAITING, OfflineScheduler.work(listOf(info(WorkInfo.State.ENQUEUED))))
        assertEquals(OfflineWork.RUNNING, OfflineScheduler.work(listOf(info(WorkInfo.State.RUNNING))))
        assertEquals(OfflineWork.NONE, OfflineScheduler.work(listOf(info(WorkInfo.State.SUCCEEDED))))
        assertEquals(OfflineWork.NONE, OfflineScheduler.work(listOf(info(WorkInfo.State.CANCELLED))))
    }
}
