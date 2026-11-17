package io.github.dolfost.stickpile.ui

import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.offline.KeepAsk
import io.github.dolfost.stickpile.rust.ItemList
import io.github.dolfost.stickpile.rust.KeepReason
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.OfflineScope
import io.github.dolfost.stickpile.rust.PackList
import io.github.dolfost.stickpile.ui.common.Message
import io.github.dolfost.stickpile.ui.library.LibraryContent
import io.github.dolfost.stickpile.ui.library.LibraryEvent
import io.github.dolfost.stickpile.ui.library.LibraryRequest
import io.github.dolfost.stickpile.ui.library.LibraryTab
import io.github.dolfost.stickpile.ui.library.LibraryViewModel
import io.github.dolfost.stickpile.ui.library.refines
import io.github.dolfost.stickpile.ui.library.requestFor
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
    private val vm by lazy { LibraryViewModel(library, MutableStateFlow(Display()), io = main.dispatcher, log = { _, _ -> }) }

    private fun TestScope.started(): LibraryViewModel {
        vm.state // create it
        advanceUntilIdle()
        library.calls.clear()
        return vm
    }

    @Test
    fun aQuerySearchesWhateverTheTabAndInsideAnOpenPack() {
        val p = pack("p1")
        assertEquals(LibraryRequest.Search("cat"), requestFor("cat", LibraryTab.PACKS, null))
        assertEquals(LibraryRequest.Search("cat"), requestFor("cat", LibraryTab.RECENT, null))
        assertEquals(LibraryRequest.PackSearch("p1", "v:tired"), requestFor("v:tired", LibraryTab.PACKS, p))
        assertEquals(LibraryRequest.Recent, requestFor("  ", LibraryTab.RECENT, null))
        assertEquals(LibraryRequest.Favorites, requestFor("", LibraryTab.LIKED, null))
        assertEquals(LibraryRequest.MostUsed, requestFor("", LibraryTab.MOST_USED, null))
        assertEquals(LibraryRequest.Packs, requestFor("", LibraryTab.PACKS, null))
        assertEquals(LibraryRequest.PackItems("p1"), requestFor("", LibraryTab.PACKS, p))
        assertEquals(LibraryRequest.PackItems("p1"), requestFor(" ", LibraryTab.PACKS, p))
    }

    @Test
    fun searchesInsideAPackRefineIt() {
        val inP1 = LibraryRequest.PackSearch("p1", "cat")
        assertTrue(refines(LibraryRequest.PackItems("p1"), inP1))
        assertTrue(refines(LibraryRequest.PackSearch("p1", "ca"), inP1))
        assertTrue("cleared: the pack again", refines(inP1, LibraryRequest.PackItems("p1")))
        assertFalse(refines(LibraryRequest.PackItems("p2"), inP1))
        assertFalse(refines(LibraryRequest.Search("cat"), inP1))
        assertFalse("out of the pack", refines(inP1, LibraryRequest.Search("cat")))
        assertTrue(refines(LibraryRequest.Search("ca"), LibraryRequest.Search("cat")))
        assertTrue(refines(LibraryRequest.Recent, LibraryRequest.Recent))
        assertFalse(refines(null, LibraryRequest.Recent))
        assertFalse(refines(LibraryRequest.Packs, LibraryRequest.PackItems("p1")))
    }

    @Test
    fun typingWithAPackOpenSearchesThatPack() = runTest(main.dispatcher) {
        val vm = started()
        library.packs = { PackList(listOf(pack("p1")), offline = false) }
        library.packItems = { ItemList(listOf(item("a"), item("b")), offline = false) }
        val gate = CompletableDeferred<Unit>()
        library.packSearch = { _, _ ->
            gate.await()
            ItemList(listOf(item("b")), offline = true)
        }
        vm.selectTab(LibraryTab.PACKS)
        advanceUntilIdle()
        vm.openPack(pack("p1"))
        advanceUntilIdle()
        library.calls.clear()

        vm.setQuery("v:tired")
        advanceUntilIdle()
        assertEquals(listOf("packSearch p1 v:tired"), library.calls.filter { !it.startsWith("hints") })
        assertEquals(
            "the pack stays until the answer comes",
            LibraryContent.Items(listOf(item("a"), item("b")), offline = false),
            vm.state.content,
        )
        gate.complete(Unit)
        advanceUntilIdle()
        assertEquals(LibraryContent.Items(listOf(item("b")), offline = true), vm.state.content)
        assertEquals(pack("p1"), vm.state.pack)

        // Cleared: the pack again; left: the search over everything.
        library.calls.clear()
        vm.setQuery("")
        advanceUntilIdle()
        assertEquals(listOf("packItems p1"), library.calls.filter { !it.startsWith("hints") })
        vm.setQuery("cat")
        advanceUntilIdle()
        library.calls.clear()
        assertTrue(vm.closePack())
        advanceUntilIdle()
        assertEquals(listOf("search cat GENERIC"), library.calls)
        assertEquals(LibraryRequest.Search("cat"), vm.state.request)
    }

    @Test
    fun aPackSearchInSafeModeLeavesNsfwOut() = runTest(main.dispatcher) {
        val display = MutableStateFlow(Display(safeMode = true))
        val vm = LibraryViewModel(library, display, io = main.dispatcher, log = { _, _ -> })
        library.packSearch = { _, _ -> ItemList(listOf(item("a"), item("x", tags = listOf("nsfw"))), offline = false) }
        vm.selectTab(LibraryTab.PACKS)
        vm.openPack(pack("p1"))
        advanceUntilIdle()
        vm.setQuery("type:gif ")
        advanceUntilIdle()
        assertEquals("packSearch p1 type:gif -tag:nsfw", library.calls.last { it.startsWith("packSearch") })
        assertEquals(LibraryContent.Items(listOf(item("a")), offline = false, hidden = 1), vm.state.content)
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
        assertEquals(emptyList<String>(), library.calls.filter { it.startsWith("search") })
        advanceTimeBy(2)
        runCurrent()
        assertEquals(listOf("search cat GENERIC"), library.calls.filter { it.startsWith("search") })
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
    fun aRefinedSearchKeepsTheOldResultsButAnotherPackStartsEmpty() = runTest(main.dispatcher) {
        val vm = started()
        val gate = CompletableDeferred<Unit>()
        library.search = { q ->
            if (q == "cats") gate.await()
            ItemList(listOf(item(q)), offline = false)
        }
        vm.setQuery("cat")
        advanceUntilIdle()
        vm.setQuery("cats")
        advanceUntilIdle()
        assertEquals(LibraryContent.Items(listOf(item("cat")), offline = false), vm.state.content)
        gate.complete(Unit)
        advanceUntilIdle()
        assertEquals(LibraryContent.Items(listOf(item("cats")), offline = false), vm.state.content)

        vm.setQuery("")
        vm.selectTab(LibraryTab.PACKS)
        advanceUntilIdle()
        val slow = CompletableDeferred<Unit>()
        library.packItems = { id ->
            if (id == "p2") slow.await()
            ItemList(listOf(item(id)), offline = false)
        }
        vm.openPack(pack("p1"))
        advanceUntilIdle()
        vm.closePack()
        vm.openPack(pack("p2"))
        advanceUntilIdle()
        assertEquals(LibraryContent.Loading, vm.state.content)
        slow.complete(Unit)
        advanceUntilIdle()
        assertEquals(LibraryContent.Items(listOf(item("p2")), offline = false), vm.state.content)
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

    // -- keeping offline ------------------------------------------------------------

    @Test
    fun aStickersDetailsKeepItOfflineAndThenOfferToStop() = runTest(main.dispatcher) {
        var runs = 0
        val vm = LibraryViewModel(
            library,
            MutableStateFlow(Display()),
            io = main.dispatcher,
            log = { _, _ -> },
            requestOfflineRun = { runs++ },
        )
        vm.state // create it
        advanceUntilIdle()
        val cat = item("cat", name = "grumpy cat")
        vm.showDetails(cat)
        advanceUntilIdle()
        assertEquals(emptyList<KeepReason>(), vm.state.details?.reasons)

        vm.toggleKeep(cat)
        advanceUntilIdle()
        assertEquals(listOf<OfflineScope>(OfflineScope.Sticker("cat")), library.keeps)
        assertEquals(1, runs)
        assertEquals(listOf(KeepReason.STICKER), vm.state.details?.reasons)
        assertEquals(
            LibraryEvent.Notice(Message(R.string.offline_kept_notice, listOf("grumpy cat"))),
            vm.eventFlow.first(),
        )

        vm.toggleKeep(cat)
        advanceUntilIdle()
        assertTrue(library.keeps.isEmpty())
        assertEquals(1, runs)
        assertEquals(
            LibraryEvent.Notice(Message(R.string.offline_released_notice, listOf("grumpy cat"))),
            vm.eventFlow.first(),
        )
    }

    @Test
    fun aPackIsKeptAfterItsEstimate() = runTest(main.dispatcher) {
        var runs = 0
        val vm = LibraryViewModel(
            library,
            MutableStateFlow(Display()),
            io = main.dispatcher,
            log = { _, _ -> },
            requestOfflineRun = { runs++ },
        )
        vm.state // create it
        advanceUntilIdle()
        val cats = pack("cats")
        vm.openPack(cats)
        advanceUntilIdle()
        assertEquals(false, vm.state.packKept)

        vm.askKeepPack()
        assertEquals(KeepAsk(OfflineScope.Pack("cats"), "Pack cats"), vm.state.keepAsk)
        advanceUntilIdle()
        assertEquals(sampleEstimate(), vm.state.keepAsk?.estimate)
        assertTrue("offlineEstimate Pack(packId=cats)" in library.calls)
        assertTrue("nothing kept yet", library.keeps.isEmpty())

        vm.confirmKeep()
        advanceUntilIdle()
        assertEquals(listOf<OfflineScope>(OfflineScope.Pack("cats")), library.keeps)
        assertEquals(true, vm.state.packKept)
        assertNull(vm.state.keepAsk)
        assertEquals(1, runs)

        vm.releasePack()
        advanceUntilIdle()
        assertTrue(library.keeps.isEmpty())
        assertEquals(false, vm.state.packKept)
    }

    @Test
    fun eachListAsksWhichOfItsStickersWorkOffline() = runTest(main.dispatcher) {
        library.recent = { listOf(item("a"), item("b")) }
        library.available = setOf("a")
        val vm = started()
        val marks = vm.state.marks
        assertTrue(marks.known)
        assertEquals(setOf("a"), marks.available)
        assertFalse(marks.unreachable)
        vm.state.content.let { assertTrue(it is LibraryContent.Items) }

        // A list from the cache: the server is away, so what is not here is dimmed.
        library.search = { ItemList(listOf(item("a"), item("b")), offline = true) }
        vm.searchNow("cat")
        advanceUntilIdle()
        assertTrue(vm.state.marks.unreachable)
        assertEquals("'' [image/*, video/*]", library.marksAsked.last())
    }

    @Test
    fun aStickerNotOnThePhoneIsExplainedWhenShared() = runTest(main.dispatcher) {
        library.prepare = { throw LibraryException.NotOnDevice("grumpy cat: not on this device") }
        val vm = started()
        vm.share(item("cat"))
        advanceUntilIdle()
        assertEquals(LibraryEvent.Notice(Message(R.string.app_error_not_on_device)), vm.eventFlow.first())
    }
}
