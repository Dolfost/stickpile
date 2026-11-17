package io.github.dolfost.stickpile.ui

import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.rust.HintKind
import io.github.dolfost.stickpile.rust.ItemList
import io.github.dolfost.stickpile.rust.SearchHint
import io.github.dolfost.stickpile.sync.CacheChanges
import io.github.dolfost.stickpile.ui.library.Details
import io.github.dolfost.stickpile.ui.library.LibraryContent
import io.github.dolfost.stickpile.ui.library.LibraryTab
import io.github.dolfost.stickpile.ui.library.LibraryViewModel
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Rule
import org.junit.Test

/** The library screen's offers, safe mode, likes and details (ui.library). */
@OptIn(ExperimentalCoroutinesApi::class)
class LibrarySearchAndSafeModeTest {
    @get:Rule
    val main = MainDispatcherRule()

    private val library = FakeLibrary()
    private val display = MutableStateFlow(Display())

    /** What the next `refreshLikes` returns: null when the likes were read lately. */
    private var likesRead: Job? = null
    private val changes = CacheChanges()
    private val vm by lazy {
        LibraryViewModel(
            library,
            display,
            refreshLikes = { likesRead },
            cacheChanges = changes,
            io = main.dispatcher,
            log = { _, _ -> },
        )
    }

    private val cat = item("cat")
    private val lewd = item("lewd", tags = listOf("NSFW"))

    private fun TestScope.started(): LibraryViewModel {
        vm.state // create it
        advanceUntilIdle()
        library.calls.clear()
        return vm
    }

    private fun searches() = library.calls.filter { it.startsWith("search") }

    private fun hints() = library.calls.filter { it.startsWith("hints") }

    private fun hint(query: String, chosen: Boolean = false) = SearchHint(query.trim(), query, chosen, HintKind.VALUE)

    @Test
    fun safeModeSearchesWithoutNsfwAndHidesItFromEveryList() = runTest(main.dispatcher) {
        display.value = Display(safeMode = true)
        library.recent = { listOf(cat, lewd) }
        // The offline search may not know the filter: the list is filtered too.
        library.search = { ItemList(listOf(cat, lewd), offline = true) }
        val vm = started()
        assertEquals(LibraryContent.Items(listOf(cat), offline = false, hidden = 1), vm.state.content)

        vm.setQuery("cat ")
        advanceUntilIdle()
        assertEquals(listOf("search cat -tag:nsfw GENERIC"), searches())
        assertEquals(LibraryContent.Items(listOf(cat), offline = true, hidden = 1), vm.state.content)
    }

    @Test
    fun aListSafeModeEmptiedSaysHowManyItHid() = runTest(main.dispatcher) {
        display.value = Display(safeMode = true)
        library.recent = { listOf(lewd, item("lewder", tags = listOf("nsfw"))) }
        assertEquals(LibraryContent.Items(emptyList(), offline = false, hidden = 2), started().state.content)
    }

    @Test
    fun switchingSafeModeFiltersAtOnceAndAsksAgain() = runTest(main.dispatcher) {
        val vm = started()
        library.favorites = { ItemList(listOf(cat, lewd), offline = false) }
        vm.selectTab(LibraryTab.LIKED)
        advanceUntilIdle()
        assertEquals(LibraryContent.Items(listOf(cat, lewd), offline = false), vm.state.content)

        val gate = CompletableDeferred<Unit>()
        library.favorites = {
            gate.await()
            ItemList(listOf(cat, lewd), offline = false)
        }
        library.calls.clear()
        display.value = Display(safeMode = true)
        runCurrent()
        assertEquals(LibraryContent.Items(listOf(cat), offline = false, hidden = 1), vm.state.content)
        assertEquals(listOf("favorites"), library.calls)
        gate.complete(Unit)
        advanceUntilIdle()
        assertEquals(LibraryContent.Items(listOf(cat), offline = false, hidden = 1), vm.state.content)

        // A search goes out again, without the filter now.
        library.search = { ItemList(listOf(cat, lewd), offline = false) }
        vm.setQuery("cat")
        advanceUntilIdle()
        library.calls.clear()
        display.value = Display(safeMode = false)
        advanceUntilIdle()
        assertEquals(listOf("search cat GENERIC"), searches())
        assertEquals(LibraryContent.Items(listOf(cat, lewd), offline = false), vm.state.content)

        // Changing anything else asks for nothing.
        library.calls.clear()
        display.value = Display(safeMode = false, showInfo = false)
        advanceUntilIdle()
        assertEquals(emptyList<String>(), library.calls)
    }

    @Test
    fun typingOffersHintsForTheLatestText() = runTest(main.dispatcher) {
        val vm = started()
        library.hintsFor = { q -> listOf(hint("$q ")) }
        vm.setQuery("t")
        vm.setQuery("ta")
        advanceUntilIdle()
        // "t" was typed over before its turn came.
        assertEquals(listOf("hints ta"), hints())
        assertEquals(listOf(hint("ta ")), vm.state.hints)

        // An answer for text typed over while it was worked out is dropped.
        library.calls.clear()
        library.hintsFor = { q ->
            if (q == "tag") vm.setQuery("tag:")
            listOf(hint("$q "))
        }
        vm.setQuery("tag")
        advanceUntilIdle()
        assertEquals(listOf("hints tag", "hints tag:"), hints())
        assertEquals(listOf(hint("tag: ")), vm.state.hints)
    }

    @Test
    fun theEmptyFieldOffersTheKeysOnceFocused() = runTest(main.dispatcher) {
        val keys = listOf(SearchHint("tag:", "tag:", false, HintKind.KEY), SearchHint("vibe:", "vibe:", false, HintKind.KEY))
        library.hintsFor = { q -> if (q.isEmpty()) keys else emptyList() }
        val vm = started()
        assertEquals(emptyList<SearchHint>(), vm.state.hints)
        vm.searchFocused()
        advanceUntilIdle()
        assertEquals(listOf("hints "), hints())
        assertEquals(keys, vm.state.hints)
    }

    @Test
    fun aSyncThatChangedTheCacheOffersAgainForTheTextAsItIs() = runTest(main.dispatcher) {
        val vm = started()
        changes.after(library.syncState.copy(cacheChanged = true))
        advanceUntilIdle()
        assertEquals("the field was never used", emptyList<String>(), hints())

        // The first opening: the cache is still empty.
        vm.setQuery("v:")
        advanceUntilIdle()
        assertEquals(emptyList<SearchHint>(), vm.state.hints)
        // The sync brings the vibes.
        library.hintsFor = { q -> listOf(hint("${q}tired ")) }
        changes.after(library.syncState.copy(cacheChanged = false))
        advanceUntilIdle()
        assertEquals("nothing changed: not asked again", listOf("hints v:"), hints())
        changes.after(library.syncState.copy(cacheChanged = true))
        advanceUntilIdle()
        assertEquals(listOf("hints v:", "hints v:"), hints())
        assertEquals(listOf(hint("v:tired ")), vm.state.hints)
        assertEquals("v:", vm.state.query)
    }

    @Test
    fun aRefreshThatChangedTheCacheOffersAgain() = runTest(main.dispatcher) {
        val vm = started()
        vm.setQuery("v:")
        advanceUntilIdle()
        library.syncNow = { library.syncState.copy(cacheChanged = true) }
        library.hintsFor = { q -> listOf(hint("${q}tired ")) }
        vm.refresh()
        advanceUntilIdle()
        assertEquals(1, changes.count.value)
        assertEquals(listOf(hint("v:tired ")), vm.state.hints)
    }

    @Test
    fun takingAHintSearchesAtOnce() = runTest(main.dispatcher) {
        val vm = started()
        vm.setQuery("vibe:sar")
        vm.takeHint(hint("vibe:sarcastic ", chosen = true))
        runCurrent() // no time passes: the typed search never went out
        assertEquals("vibe:sarcastic ", vm.state.query)
        assertEquals(listOf("search vibe:sarcastic GENERIC"), searches())
        advanceUntilIdle()
        assertEquals(listOf("search vibe:sarcastic GENERIC"), searches())
        assertEquals("hints vibe:sarcastic ", hints().last())
    }

    @Test
    fun aLikeShowsAtOnceInTheListAndTheDetails() = runTest(main.dispatcher) {
        library.recent = { listOf(cat, item("dog")) }
        val vm = started()
        library.toggleLike = { true }
        vm.showDetails(cat)
        vm.toggleLike(cat)
        advanceUntilIdle()
        val liked = cat.copy(liked = true, likes = 1u)
        assertEquals(LibraryContent.Items(listOf(liked, item("dog")), offline = false), vm.state.content)
        assertEquals(Details(liked, url = "https://s.example/items/cat", reasons = emptyList()), vm.state.details)
        assertEquals(listOf("toggleLike cat"), library.calls)

        library.toggleLike = { false }
        vm.toggleLike(liked)
        advanceUntilIdle()
        assertEquals(LibraryContent.Items(listOf(cat, item("dog")), offline = false), vm.state.content)
    }

    @Test
    fun theDetailsKnowTheItemsPage() = runTest(main.dispatcher) {
        val vm = started()
        vm.showDetails(cat)
        assertEquals(Details(cat), vm.state.details)
        advanceUntilIdle()
        assertEquals(Details(cat, url = "https://s.example/items/cat", reasons = emptyList()), vm.state.details)
        vm.hideDetails()
        assertNull(vm.state.details)
    }

    @Test
    fun theListLoadsAgainOnceTheLikesAreRead() = runTest(main.dispatcher) {
        val vm = started()
        vm.selectTab(LibraryTab.LIKED)
        advanceUntilIdle()
        library.calls.clear()

        vm.resumed() // read lately: nothing to wait for
        advanceUntilIdle()
        assertEquals(emptyList<String>(), library.calls)

        val reading = Job()
        likesRead = reading
        vm.resumed()
        advanceUntilIdle()
        assertEquals(emptyList<String>(), library.calls)
        reading.complete()
        advanceUntilIdle()
        assertEquals(listOf("favorites"), library.calls)
    }

    @Test
    fun theRecentItemsLoadAgainOnResume() = runTest(main.dispatcher) {
        val vm = started()
        vm.resumed()
        advanceUntilIdle()
        assertEquals(listOf("recent"), library.calls)
    }
}
