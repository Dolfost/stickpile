package io.github.dolfost.stickpile.offline

import io.github.dolfost.stickpile.rust.OfflineState
import io.github.dolfost.stickpile.ui.samplePlan
import io.github.dolfost.stickpile.ui.sampleStatus
import io.github.dolfost.stickpile.ui.totals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** What the settings say about the downloads (offline.OfflineProgress). */
class OfflineProgressTest {
    private val half = samplePlan(totals(files = 10u, bytes = 1000u, presentFiles = 5u, presentBytes = 500u))
    private val done = samplePlan(totals(files = 10u, bytes = 1000u, presentFiles = 10u, presentBytes = 1000u))
    private val none = samplePlan()

    private fun shown(
        state: OfflineState = OfflineState.IDLE,
        running: Boolean = false,
        paused: Boolean = false,
        work: OfflineWork = OfflineWork.NONE,
        plan: io.github.dolfost.stickpile.rust.OfflinePlan? = half,
        mobileData: Boolean = false,
    ) = OfflineProgress.shown(sampleStatus(state, running, paused), work, plan, mobileData)

    @Test
    fun aRunGoingOnIsDownloading() {
        assertEquals(OfflineShown.DOWNLOADING, shown(OfflineState.DOWNLOADING, running = true))
        assertEquals(OfflineShown.DOWNLOADING, shown(work = OfflineWork.RUNNING))
    }

    @Test
    fun pausedWinsOverEverything() {
        assertEquals(OfflineShown.PAUSED, shown(OfflineState.DOWNLOADING, running = true, paused = true))
        assertEquals(OfflineShown.PAUSED, shown(paused = true, work = OfflineWork.WAITING))
    }

    @Test
    fun aWaitingJobWaitsForWifiUnlessMobileDataIsAllowed() {
        assertEquals(OfflineShown.WAITING_FOR_WIFI, shown(work = OfflineWork.WAITING))
        assertEquals(OfflineShown.WAITING_FOR_NETWORK, shown(work = OfflineWork.WAITING, mobileData = true))
        assertEquals(OfflineShown.UP_TO_DATE, shown(work = OfflineWork.WAITING, plan = done), "nothing to wait for")
    }

    @Test
    fun aRunThatStoppedSaysWhy() {
        assertEquals(OfflineShown.LIMIT_REACHED, shown(OfflineState.LIMIT_REACHED))
        assertEquals(OfflineShown.DISK_FULL, shown(OfflineState.DISK_FULL))
        assertEquals(OfflineShown.UNREACHABLE, shown(OfflineState.WAITING_FOR_NETWORK))
        // A run dropped while downloading (WorkManager stopped it): not downloading now.
        assertEquals(OfflineShown.PENDING, shown(OfflineState.DOWNLOADING, running = false))
    }

    @Test
    fun withoutARunThePlanSays() {
        assertEquals(OfflineShown.UP_TO_DATE, shown(plan = done))
        assertEquals(OfflineShown.PENDING, shown(plan = half))
        assertEquals(OfflineShown.NOTHING_KEPT, shown(plan = none))
        assertEquals(OfflineShown.NOTHING_KEPT, shown(plan = null))
        assertEquals(OfflineShown.UP_TO_DATE, shown(OfflineState.LIMIT_REACHED, plan = done), "since done")
    }

    @Test
    fun theCountsAreTheRunsWhileItGoesOnElseThePlans() {
        val running = sampleStatus(OfflineState.DOWNLOADING, running = true, filesDone = 2u, filesTotal = 8u)
        assertEquals(OfflineCounts(2u, 8u, 200u, 800u), OfflineProgress.counts(running, half))
        assertEquals(OfflineCounts(5u, 10u, 500u, 1000u), OfflineProgress.counts(sampleStatus(), half))
        assertNull(OfflineProgress.counts(sampleStatus(), none))
        assertEquals(0.5f, OfflineCounts(5u, 10u, 500u, 1000u).fraction)
        assertEquals(1f, OfflineCounts(0u, 0u, 0u, 0u).fraction)
    }

    @Test
    fun theButtonsFollowTheState() {
        assertTrue(OfflineProgress.canPause(OfflineShown.DOWNLOADING))
        assertTrue(OfflineProgress.canStop(OfflineShown.DOWNLOADING))
        assertFalse(OfflineProgress.canStart(OfflineShown.DOWNLOADING))
        assertTrue(OfflineProgress.canStart(OfflineShown.PENDING))
        assertTrue(OfflineProgress.canStart(OfflineShown.LIMIT_REACHED))
        assertFalse(OfflineProgress.canPause(OfflineShown.PAUSED))
        assertFalse(OfflineProgress.canStart(OfflineShown.UP_TO_DATE))
        assertFalse(OfflineProgress.canPause(OfflineShown.NOTHING_KEPT))
    }

    private fun assertEquals(expected: OfflineShown, actual: OfflineShown, message: String) =
        assertEquals(message, expected, actual)
}
