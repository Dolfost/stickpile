package io.github.dolfost.stickpile.ui

import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.ItemList
import io.github.dolfost.stickpile.rust.PackList
import io.github.dolfost.stickpile.ui.common.Message
import io.github.dolfost.stickpile.ui.library.LibraryContent
import io.github.dolfost.stickpile.ui.library.LibraryEvent
import io.github.dolfost.stickpile.ui.library.LibraryRequest
import io.github.dolfost.stickpile.ui.library.LibraryTab
import io.github.dolfost.stickpile.ui.library.LibraryViewModel
import io.github.dolfost.stickpile.ui.library.requestFor
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** The library screen: search, tabs, packs, sharing, likes and refresh (ui.library). */
@OptIn(ExperimentalCoroutinesApi::class)
class LibraryViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    private val library = FakeLibrary()
    private val vm by lazy { LibraryViewModel(library, io = main.dispatcher, log = { _, _ -> }) }

    private fun TestScope.started(): LibraryViewModel {
        vm.state // create it
        advanceUntilIdle()
        library.calls.clear()
        return vm
    }

    @Test
    fun aQuerySearchesWhateverTheTab() {
        val p = pack("p1")
        assertEquals(LibraryRequest.Search("cat"), requestFor("cat", LibraryTab.PACKS, p))
        assertEquals(LibraryRequest.Recent, requestFor("  ", LibraryTab.RECENT, null))
        assertEquals(LibraryRequest.Favorites, requestFor("", LibraryTab.LIKED, null))
        assertEquals(LibraryRequest.MostUsed, requestFor("", LibraryTab.MOST_USED, null))
        assertEquals(LibraryRequest.Packs, requestFor("", LibraryTab.PACKS, null))
        assertEquals(LibraryRequest.PackItems("p1"), requestFor("", LibraryTab.PACKS, p))
    }

    @Test
    fun theRecentItemsComeFirst() = runTest(main.dispatcher) {
        library.recent = { listOf(item("1"), item("2")) }
        assertEquals(LibraryContent.Loading, vm.state.content)
        advanceUntilIdle()
        assertEquals(listOf("recent"), library.calls)
        assertEquals(LibraryContent.Items(listOf(item("1"), item("2")), offline = false), vm.state.content)
    }

    @Test
    fun typingSearchesOnceItPauses() = runTest(main.dispatcher) {
        val vm = started()
        library.search = { ItemList(listOf(item("c")), offline = true) }
        vm.setQuery("c")
        advanceTimeBy(100)
        vm.setQuery("ca")
        advanceTimeBy(100)
        vm.setQuery("cat")
        advanceTimeBy(LibraryViewModel.SEARCH_DELAY_MS - 1)
        runCurrent()
        assertEquals(emptyList<String>(), library.calls)
        advanceTimeBy(2)
        runCurrent()
        assertEquals(listOf("search cat GENERIC"), library.calls)
        assertEquals(LibraryContent.Items(listOf(item("c")), offline = true), vm.state.content)

        // Clearing the query goes back to the tab.
        vm.setQuery("")
        advanceUntilIdle()
        assertEquals("recent", library.calls.last())
    }

    @Test
    fun theTabsAskForTheirLists() = runTest(main.dispatcher) {
        val vm = started()
        library.favorites = { ItemList(listOf(item("f")), offline = true) }
        vm.selectTab(LibraryTab.LIKED)
        advanceUntilIdle()
        assertEquals(LibraryContent.Items(listOf(item("f")), offline = true), vm.state.content)
        vm.selectTab(LibraryTab.MOST_USED)
        advanceUntilIdle()
        vm.selectTab(LibraryTab.MOST_USED) // the same tab again: nothing
        advanceUntilIdle()
        assertEquals(listOf("favorites", "mostUsed"), library.calls)
    }

    @Test
    fun packsOpenAndClose() = runTest(main.dispatcher) {
        val vm = started()
        library.packs = { PackList(listOf(pack("p1")), offline = false) }
        library.packItems = { ItemList(listOf(item("i")), offline = false) }
        vm.selectTab(LibraryTab.PACKS)
        advanceUntilIdle()
        assertEquals(LibraryContent.Packs(listOf(pack("p1")), offline = false), vm.state.content)
        vm.openPack(pack("p1"))
        advanceUntilIdle()
        assertEquals(LibraryContent.Items(listOf(item("i")), offline = false), vm.state.content)
        assertTrue(vm.closePack())
        assertFalse(vm.closePack())
        advanceUntilIdle()
        assertEquals(listOf("packs", "packItems p1", "packs"), library.calls)
        assertNull(vm.state.pack)
    }

    @Test
    fun aFailedListSaysWhyAndOffersASignInWhenTheAccountIsGone() = runTest(main.dispatcher) {
        val vm = started()
        library.favorites = { throw unreachable() }
        vm.selectTab(LibraryTab.LIKED)
        advanceUntilIdle()
        assertEquals(
            LibraryContent.Failed(Message(R.string.app_error_unreachable_nothing_saved), signInAgain = false),
            vm.state.content,
        )
        library.favorites = { throw unauthorized() }
        vm.reload()
        advanceUntilIdle()
        assertEquals(LibraryContent.Failed(Message(R.string.app_error_unauthorized), signInAgain = true), vm.state.content)
    }

    @Test
    fun aTapPreparesTheItemForTheShareSheet() = runTest(main.dispatcher) {
        val vm = started()
        library.prepare = { prepared() }
        vm.share(item("1"))
        assertEquals("1", vm.state.preparing)
        vm.share(item("2")) // one at a time
        advanceUntilIdle()
        assertEquals(LibraryEvent.Share(prepared()), vm.eventFlow.first())
        assertEquals(listOf("prepare 1 '' [image/*, video/*] false"), library.calls)
        assertNull(vm.state.preparing)
    }

    @Test
    fun aFailedShareIsAMessage() = runTest(main.dispatcher) {
        val vm = started()
        library.prepare = { throw unreachable() }
        vm.share(item("1"))
        advanceUntilIdle()
        assertEquals(LibraryEvent.Notice(Message(R.string.app_error_unreachable_not_saved)), vm.eventFlow.first())
        assertNull(vm.state.preparing)
    }

    @Test
    fun aLongPressLikesAndTheLikedTabFollows() = runTest(main.dispatcher) {
        val vm = started()
        var liked = false
        library.toggleLike = {
            liked = !liked
            liked
        }
        vm.toggleLike(item("1", name = "grumpy"))
        advanceUntilIdle()
        assertEquals(LibraryEvent.Notice(Message(R.string.library_liked, listOf("grumpy"))), vm.eventFlow.first())
        assertEquals(listOf("toggleLike 1"), library.calls)

        vm.selectTab(LibraryTab.LIKED)
        advanceUntilIdle()
        library.calls.clear()
        vm.toggleLike(item("1", name = "grumpy"))
        advanceUntilIdle()
        assertEquals(LibraryEvent.Notice(Message(R.string.library_unliked, listOf("grumpy"))), vm.eventFlow.first())
        assertEquals(listOf("toggleLike 1", "favorites"), library.calls)
    }

    @Test
    fun aRefreshSyncsThenLoadsAgain() = runTest(main.dispatcher) {
        val vm = started()
        vm.refresh()
        assertTrue(vm.state.refreshing)
        advanceUntilIdle()
        assertFalse(vm.state.refreshing)
        assertEquals(listOf("syncNow", "recent"), library.calls)

        library.calls.clear()
        library.syncNow = { throw unreachable() }
        vm.refresh()
        advanceUntilIdle()
        assertEquals(LibraryEvent.Notice(Message(R.string.app_error_unreachable)), vm.eventFlow.first())
        assertEquals(listOf("syncNow", "recent"), library.calls)
    }
}
