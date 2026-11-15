package io.github.dolfost.stickpile.ui.library

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.Pack
import io.github.dolfost.stickpile.rust.Prepared
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import io.github.dolfost.stickpile.rust.Target
import io.github.dolfost.stickpile.ui.common.Doing
import io.github.dolfost.stickpile.ui.common.Failure
import io.github.dolfost.stickpile.ui.common.FailureLog
import io.github.dolfost.stickpile.ui.common.Message
import io.github.dolfost.stickpile.ui.common.catching
import io.github.dolfost.stickpile.ui.common.messageFor
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** The library screen's tabs, in their order. */
enum class LibraryTab { RECENT, LIKED, PACKS, MOST_USED }

/** What the library screen asks the library for. */
sealed interface LibraryRequest {
    data class Search(val query: String) : LibraryRequest
    data object Recent : LibraryRequest
    data object Favorites : LibraryRequest
    data object MostUsed : LibraryRequest
    data object Packs : LibraryRequest
    data class PackItems(val packId: String) : LibraryRequest
}

/**
 * What to show for a [query] (searched when not blank, whatever the tab),
 * else the [tab], where [pack] is the pack opened in the packs tab.
 */
fun requestFor(query: String, tab: LibraryTab, pack: Pack?): LibraryRequest = when {
    query.isNotBlank() -> LibraryRequest.Search(query)
    tab == LibraryTab.RECENT -> LibraryRequest.Recent
    tab == LibraryTab.LIKED -> LibraryRequest.Favorites
    tab == LibraryTab.MOST_USED -> LibraryRequest.MostUsed
    pack != null -> LibraryRequest.PackItems(pack.id)
    else -> LibraryRequest.Packs
}

/** What the grid area shows. */
sealed interface LibraryContent {
    data object Loading : LibraryContent

    /** [offline]: the server could not be reached and the cache answered. */
    data class Items(val items: List<Item>, val offline: Boolean) : LibraryContent
    data class Packs(val packs: List<Pack>, val offline: Boolean) : LibraryContent

    /** [signInAgain]: the account is gone (signed out, or the token was refused). */
    data class Failed(val message: Message, val signInAgain: Boolean) : LibraryContent
}

data class LibraryState(
    val query: String = "",
    val tab: LibraryTab = LibraryTab.RECENT,
    /** The pack opened in the packs tab. */
    val pack: Pack? = null,
    val content: LibraryContent = LibraryContent.Loading,
    /** A sync asked for by the user is running. */
    val refreshing: Boolean = false,
    /** The item being prepared for sharing. */
    val preparing: String? = null,
) {
    val request: LibraryRequest get() = requestFor(query, tab, pack)
}

/** What the screen does once: share a file, or show a short message. */
sealed interface LibraryEvent {
    data class Share(val prepared: Prepared) : LibraryEvent
    data class Notice(val message: Message) : LibraryEvent
}

/**
 * The library screen (docs/android-plan.md 4.8): a search (debounced),
 * tabs, a grid; a tap shares an item, a long press likes or unlikes it,
 * a refresh syncs. The non-suspend library calls run on [io].
 */
@OptIn(FlowPreview::class)
class LibraryViewModel(
    private val library: StickpileLibraryInterface,
    private val io: CoroutineDispatcher = Dispatchers.IO,
    private val log: FailureLog = FailureLog.android,
) : ViewModel() {

    var state: LibraryState by mutableStateOf(LibraryState())
        private set

    private val events = Channel<LibraryEvent>(Channel.BUFFERED)

    /** Share intents and messages, each to be handled once. */
    val eventFlow: Flow<LibraryEvent> = events.receiveAsFlow()

    /** The request to load; [typed] ones wait for the typing to pause. */
    private data class Load(val request: LibraryRequest, val typed: Boolean, val generation: Int)

    private var generation = 0

    /** The request whose answer is on screen (or loading). */
    private var shown: LibraryRequest? = null
    private val loads = MutableStateFlow(Load(state.request, typed = false, generation = 0))

    init {
        viewModelScope.launch {
            loads
                .debounce { if (it.typed) SEARCH_DELAY_MS else 0L }
                .collectLatest { load(it.request) }
        }
    }

    fun setQuery(text: String) {
        state = state.copy(query = text)
        request(typed = true)
    }

    fun selectTab(tab: LibraryTab) {
        if (tab == state.tab && state.pack == null) return
        state = state.copy(tab = tab, pack = null)
        request(typed = false)
    }

    fun openPack(pack: Pack) {
        state = state.copy(pack = pack)
        request(typed = false)
    }

    /** Back from a pack to the list of packs; false when no pack is open. */
    fun closePack(): Boolean {
        if (state.pack == null) return false
        state = state.copy(pack = null)
        request(typed = false)
        return true
    }

    /** Load what is shown again (after a sync, or back on the screen). */
    fun reload() {
        generation++
        loads.value = Load(state.request, typed = false, generation = generation)
    }

    /** Sync now, then load again; a failure is a message, the list stays. */
    fun refresh() {
        if (state.refreshing) return
        state = state.copy(refreshing = true)
        viewModelScope.launch {
            catching { library.syncNow() }.onFailure { e -> notice(Doing.SYNCING, e) }
            state = state.copy(refreshing = false)
            reload()
        }
    }

    /** Prepare [item] as a file any app takes, for the share sheet. */
    fun share(item: Item) {
        if (state.preparing != null) return
        state = state.copy(preparing = item.id)
        viewModelScope.launch {
            catching { library.prepare(item.id, "", SHARE_TYPES, false) }.fold(
                onSuccess = { events.send(LibraryEvent.Share(it)) },
                onFailure = { e -> notice(Doing.SHARING, e) },
            )
            state = state.copy(preparing = null)
        }
    }

    /** Like [item], or take the like back. */
    fun toggleLike(item: Item) {
        viewModelScope.launch {
            catching { library.toggleLike(item.id) }.fold(
                onSuccess = { liked ->
                    val text = if (liked) R.string.library_liked else R.string.library_unliked
                    events.send(LibraryEvent.Notice(Message(text, listOf(item.name))))
                    if (state.request == LibraryRequest.Favorites) reload()
                },
                onFailure = { e -> notice(Doing.LIKING, e) },
            )
        }
    }

    private fun request(typed: Boolean) {
        loads.value = Load(state.request, typed, generation)
    }

    private suspend fun load(request: LibraryRequest) {
        // A new kind of list starts empty; the same kind (a refined search,
        // a reload) keeps the old one until the new one is there.
        if (shown?.let { it::class } != request::class) state = state.copy(content = LibraryContent.Loading)
        shown = request
        val result = catching { fetch(request) }
        state = state.copy(
            content = result.getOrElse { e ->
                log.failed(Doing.LOADING, e)
                val failure = Failure.of(e)
                val gone = failure.kind == LibraryErrorKind.NOT_SIGNED_IN || failure.kind == LibraryErrorKind.UNAUTHORIZED
                LibraryContent.Failed(messageFor(failure, Doing.LOADING), signInAgain = gone)
            },
        )
    }

    private suspend fun fetch(request: LibraryRequest): LibraryContent = when (request) {
        is LibraryRequest.Search -> library.search(request.query, Target.GENERIC, SEARCH_LIMIT).let {
            LibraryContent.Items(it.items, it.offline)
        }
        LibraryRequest.Recent -> LibraryContent.Items(withContext(io) { library.recent(LIST_LIMIT) }, offline = false)
        LibraryRequest.Favorites -> library.favorites(LIST_LIMIT).let { LibraryContent.Items(it.items, it.offline) }
        LibraryRequest.MostUsed -> library.mostUsed(LIST_LIMIT).let { LibraryContent.Items(it.items, it.offline) }
        LibraryRequest.Packs -> library.packs().let { LibraryContent.Packs(it.packs, it.offline) }
        is LibraryRequest.PackItems -> library.packItems(request.packId).let { LibraryContent.Items(it.items, it.offline) }
    }

    private suspend fun notice(doing: Doing, e: Throwable) {
        log.failed(doing, e)
        events.send(LibraryEvent.Notice(messageFor(Failure.of(e), doing)))
    }

    companion object {
        /** How long the typing must pause before a search goes out. */
        const val SEARCH_DELAY_MS = 300L
        val SEARCH_LIMIT = 120u
        val LIST_LIMIT = 200u

        /** Sharing from the app: any picture or video (the library picks the rendition). */
        val SHARE_TYPES = listOf("image/*", "video/*")
    }
}
