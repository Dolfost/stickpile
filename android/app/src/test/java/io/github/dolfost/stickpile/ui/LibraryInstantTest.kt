package io.github.dolfost.stickpile.ui

import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.rust.ItemList
import io.github.dolfost.stickpile.rust.ServerReach
import io.github.dolfost.stickpile.ui.library.LibraryContent
import io.github.dolfost.stickpile.ui.library.LibraryEvent
import io.github.dolfost.stickpile.ui.library.LibraryTab
import io.github.dolfost.stickpile.ui.library.LibraryViewModel
import java.time.Instant
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/**
 * Results without waiting in the library screen (docs/offline.md): the
 * offline results at once with the notice, the server's in their place
 * when they come, nothing moved under a finger, and the server's return.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class LibraryInstantTest {
    @get:Rule
    val main = MainDispatcherRule()

    private val library = FakeLibrary()
    private var syncs = 0
    private val vm by lazy {
        LibraryViewModel(
            library,
            MutableStateFlow(Display()),
            io = main.dispatcher,
            log = { _, _ -> },
            serverBack = { syncs++ },
        )
    }

    private val offline = listOf(item("a"), item("b"))
    private val online = listOf(item("c"), item("a"), item("b"))

    private fun TestScope.started(): LibraryViewModel {
        vm.state // create it
        advanceUntilIdle()
        library.calls.clear()
        return vm
    }

    /** "cat" answers [offline] from the cache first; the server's [online] come when [gate] opens. */
    private fun slowServer(gate: CompletableDeferred<ItemList?>): FakePending {
        val pending = FakePending { gate.await() }
        library.search = { ItemList(offline, offline = true) }
        library.pendingFor = { pending }
        return pending
    }

    /** Typed: shown after the search delay. */
    private fun TestScope.typed(vm: LibraryViewModel, text: String) {
        vm.setQuery(text)
        advanceTimeBy(LibraryViewModel.SEARCH_DELAY_MS + 1)
        runCurrent()
    }

    @Test
    fun theOfflineResultsShowAtOnceWithTheNoticeAndTheServersReplaceThem() = runTest(main.dispatcher) {
        val vm = started()
        val gate = CompletableDeferred<ItemList?>()
        val pending = slowServer(gate)

        typed(vm, "cat")
        assertEquals("the offline results and the notice", LibraryContent.Items(offline, offline = true), vm.state.content)
        assertEquals(listOf("search cat GENERIC"), library.calls.filter { !it.startsWith("hints") })

        gate.complete(ItemList(online, offline = false))
        runCurrent()
        assertEquals("the server's, without the notice", LibraryContent.Items(online, offline = false), vm.state.content)
        assertTrue("let go", pending.closed)
        assertEquals("one search", 1, library.calls.count { it.startsWith("search") })
    }

    @Test
    fun noServerResultsKeepTheOfflineOnesAndTheNotice() = runTest(main.dispatcher) {
        val vm = started()
        val gate = CompletableDeferred<ItemList?>()
        val pending = slowServer(gate)
        typed(vm, "cat")
        gate.complete(null)
        advanceUntilIdle()
        assertEquals(LibraryContent.Items(offline, offline = true), vm.state.content)
        assertTrue(pending.closed)

        // Signed out meanwhile: the screen says so, as a failed search would.
        val refused = FakePending { throw unauthorized() }
        library.pendingFor = { refused }
        typed(vm, "dog")
        advanceUntilIdle()
        assertTrue((vm.state.content as LibraryContent.Failed).signInAgain)
    }

    @Test
    fun theSwapWaitsWhileAFingerIsOnTheGrid() = runTest(main.dispatcher) {
        val vm = started()
        val gate = CompletableDeferred<ItemList?>()
        slowServer(gate)
        typed(vm, "cat")

        vm.pressing(true)
        gate.complete(ItemList(online, offline = false))
        advanceUntilIdle()
        assertEquals("nothing moves under the finger", LibraryContent.Items(offline, offline = true), vm.state.content)
        vm.pressing(false)
        runCurrent()
        assertEquals(LibraryContent.Items(online, offline = false), vm.state.content)

        // A newer search while the finger is down drops the swap waiting.
        val second = CompletableDeferred<ItemList?>()
        slowServer(second)
        typed(vm, "ca")
        vm.pressing(true)
        second.complete(ItemList(online, offline = false))
        runCurrent()
        library.search = { ItemList(listOf(item("z")), offline = true) }
        library.pendingFor = { null }
        typed(vm, "cats")
        vm.pressing(false)
        advanceUntilIdle()
        assertEquals(LibraryContent.Items(listOf(item("z")), offline = true), vm.state.content)
    }

    /**
     * What the user is on stays: the details open, a share going on, and
     * the grid itself (no loading in between, so it keeps its scroll
     * position; the screen keeps it at the top or on the sticker first in
     * view).
     */
    @Test
    fun theSwapKeepsTheDetailsTheShareAndTheGrid() = runTest(main.dispatcher) {
        val vm = started()
        val gate = CompletableDeferred<ItemList?>()
        slowServer(gate)
        typed(vm, "cat")
        val shareGate = CompletableDeferred<Unit>()
        library.prepare = {
            shareGate.await()
            prepared()
        }
        vm.showDetails(item("a"))
        vm.share(item("b"))
        runCurrent()

        library.calls.clear()
        gate.complete(ItemList(online, offline = false))
        runCurrent()
        assertEquals(LibraryContent.Items(online, offline = false), vm.state.content)
        assertEquals("no search again, no loading: the list is replaced", emptyList<String>(), library.calls)
        assertEquals("a", vm.state.details?.item?.id)
        assertEquals("b", vm.state.preparing)

        shareGate.complete(Unit)
        advanceUntilIdle()
        assertEquals(LibraryEvent.Share(prepared()), vm.eventFlow.first())
        assertNull(vm.state.preparing)
        assertEquals("a", vm.state.details?.item?.id)
    }

    @Test
    fun theServerBackRunsWhatIsShownAgainAndStartsASync() = runTest(main.dispatcher) {
        library.reach.value = ServerReach.Reachable
        val vm = started()
        library.search = { ItemList(offline, offline = true) }
        typed(vm, "cat")
        library.calls.clear()

        // Found away: nothing runs yet.
        library.reach.value = ServerReach.Unreachable(Instant.ofEpochSecond(1_000))
        advanceUntilIdle()
        assertEquals(emptyList<String>(), library.calls)
        assertEquals(0, syncs)

        // Back: the search again in place, and a sync.
        library.search = { ItemList(online, offline = false) }
        library.reach.value = ServerReach.Reachable
        advanceUntilIdle()
        assertEquals(listOf("search cat GENERIC"), library.calls)
        assertEquals(1, syncs)
        assertEquals(LibraryContent.Items(online, offline = false), vm.state.content)

        // Forgotten (Unknown) and found again without an outage between:
        // nothing to do.
        library.calls.clear()
        library.reach.value = ServerReach.Unknown
        advanceUntilIdle()
        library.reach.value = ServerReach.Reachable
        advanceUntilIdle()
        assertEquals(emptyList<String>(), library.calls)
        assertEquals(1, syncs)

        // Away, forgotten (a sync asked for), back: that is a return.
        library.reach.value = ServerReach.Unreachable(Instant.ofEpochSecond(2_000))
        advanceUntilIdle()
        library.reach.value = ServerReach.Unknown
        advanceUntilIdle()
        library.reach.value = ServerReach.Reachable
        advanceUntilIdle()
        assertEquals(listOf("search cat GENERIC"), library.calls)
        assertEquals(2, syncs)
    }

    @Test
    fun aListReadOfflineIsReadAgainWhenTheServerAnswers() = runTest(main.dispatcher) {
        library.reach.value = ServerReach.Unknown
        library.favorites = { ItemList(offline, offline = true) }
        val vm = started()
        vm.selectTab(LibraryTab.LIKED)
        advanceUntilIdle()
        assertEquals(LibraryContent.Items(offline, offline = true), vm.state.content)

        // The watch never saw the outage (the list was read while it was
        // Unknown), but what is shown is the cache's.
        library.calls.clear()
        library.favorites = { ItemList(online, offline = false) }
        library.reach.value = ServerReach.Reachable
        advanceUntilIdle()
        assertEquals(listOf("favorites"), library.calls)
        assertEquals(LibraryContent.Items(online, offline = false), vm.state.content)
        assertEquals(1, syncs)
    }

    @Test
    fun backOnTheScreenTheServerIsChecked() = runTest(main.dispatcher) {
        val vm = started()
        val before = library.checks
        vm.resumed()
        advanceUntilIdle()
        assertEquals(before + 1, library.checks)
    }
}
