package io.github.dolfost.stickpile.provider

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DownloadGateTest {
    /** Starts [count] threads that each take a slot and hold it until [release] opens. */
    private fun hold(gate: DownloadGate, count: Int, release: CountDownLatch): List<Thread> {
        val holding = CountDownLatch(count)
        val threads = (1..count).map {
            thread {
                gate.run {
                    holding.countDown()
                    release.await()
                }
            }
        }
        assertTrue(holding.await(5, TimeUnit.SECONDS))
        return threads
    }

    @Test
    fun callsUnderTheLimitRun() {
        val gate = DownloadGate(permits = 2, waitMillis = 0)
        assertEquals("a", gate.run { "a" })
        assertEquals("b", gate.run { "b" })
    }

    @Test
    fun aCallOverTheLimitFailsAfterItsWaitWithoutRunning() {
        val gate = DownloadGate(permits = 2, waitMillis = 50)
        val release = CountDownLatch(1)
        val threads = hold(gate, 2, release)
        var ran = false
        val started = System.nanoTime()
        assertNull(
            gate.run {
                ran = true
                "late"
            },
        )
        assertTrue(System.nanoTime() - started >= TimeUnit.MILLISECONDS.toNanos(50))
        assertEquals(false, ran)
        release.countDown()
        threads.forEach { it.join() }
        // Free again.
        assertEquals("now", gate.run { "now" })
    }

    @Test
    fun aWaitingCallRunsWhenASlotFrees() {
        val gate = DownloadGate(permits = 1, waitMillis = 5_000)
        val release = CountDownLatch(1)
        val threads = hold(gate, 1, release)
        thread {
            Thread.sleep(50)
            release.countDown()
        }
        assertEquals("after", gate.run { "after" })
        threads.forEach { it.join() }
    }

    @Test
    fun aFailingCallGivesItsSlotBack() {
        val gate = DownloadGate(permits = 1, waitMillis = 0)
        val thrown = runCatching { gate.run<String> { throw IllegalStateException("library failed") } }
        assertTrue(thrown.exceptionOrNull() is IllegalStateException)
        assertEquals("next", gate.run { "next" })
    }

    @Test
    fun theProviderAllowsFourDownloadsAndWaitsBriefly() {
        assertEquals(4, StickerProvider.DOWNLOADS)
        assertTrue(StickerProvider.DOWNLOAD_WAIT_MILLIS in 1L..5_000L)
    }
}
