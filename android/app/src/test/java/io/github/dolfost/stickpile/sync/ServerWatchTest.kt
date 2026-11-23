package io.github.dolfost.stickpile.sync

import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.ServerReach
import java.time.Instant
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * When the app and the keyboard run what they show again and sync
 * ([ServerWatch]): the server answers after it was found away, or while
 * what is shown came from the offline cache.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class ServerWatchTest {
    private val reach = MutableStateFlow<ServerReach>(ServerReach.Unknown)
    private var backs = 0
    private var stale = false

    private fun away(at: Long) = ServerReach.Unreachable(Instant.ofEpochSecond(at))

    private fun TestScope.following() = backgroundScope.launch {
        ServerWatch.follow(reach.value, { seen -> reach.first { it != seen } }, { stale }) { backs++ }
    }

    private fun TestScope.goes(to: ServerReach) {
        reach.value = to
        runCurrent()
    }

    @Test
    fun backAfterAnOutageRunsOncePerOutage() = runTest {
        reach.value = ServerReach.Reachable
        following()
        runCurrent()
        assertEquals(0, backs)

        goes(away(1))
        assertEquals("away: nothing yet", 0, backs)
        goes(ServerReach.Reachable)
        assertEquals(1, backs)

        // Forgotten and found again, no outage between: nothing.
        goes(ServerReach.Unknown)
        goes(ServerReach.Reachable)
        assertEquals(1, backs)

        // Away, forgotten (the user asked for a sync), back: a return.
        goes(away(2))
        goes(ServerReach.Unknown)
        goes(ServerReach.Reachable)
        assertEquals(2, backs)
    }

    @Test
    fun startingAwayCountsAsAnOutage() = runTest {
        reach.value = away(1)
        following()
        runCurrent()
        goes(ServerReach.Reachable)
        assertEquals(1, backs)
    }

    @Test
    fun anOfflineListIsReadAgainWhenTheServerAnswers() = runTest {
        // Shown from the cache, the server answering already: at once.
        reach.value = ServerReach.Reachable
        stale = true
        following()
        runCurrent()
        assertEquals(1, backs)

        // Unknown, then Reachable, with an offline list: again.
        goes(ServerReach.Unknown)
        goes(ServerReach.Reachable)
        assertEquals(2, backs)

        // Read from the server now: nothing more.
        stale = false
        goes(ServerReach.Unknown)
        goes(ServerReach.Reachable)
        assertEquals(2, backs)
    }

    @Test
    fun aFailedWaitEndsTheWatch() = runTest {
        var thrown: Throwable? = null
        val job = backgroundScope.launch {
            try {
                ServerWatch.follow(ServerReach.Unknown, { throw LibraryException.Other("gone") }, { false }) { backs++ }
            } catch (e: LibraryException) {
                thrown = e
            }
        }
        runCurrent()
        assertTrue(job.isCompleted)
        assertTrue(thrown is LibraryException.Other)
        assertEquals(0, backs)
    }
}
