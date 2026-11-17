package io.github.dolfost.stickpile.ui.library

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.display.SafeMode
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.ItemList
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.Pack
import io.github.dolfost.stickpile.rust.Prepared
import io.github.dolfost.stickpile.rust.SearchHint
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import io.github.dolfost.stickpile.rust.Target
import io.github.dolfost.stickpile.sync.CacheChanges
import io.github.dolfost.stickpile.ui.common.Doing
import io.github.dolfost.stickpile.ui.common.Failure
import io.github.dolfost.stickpile.ui.common.FailureLog
import io.github.dolfost.stickpile.ui.common.Message
import io.github.dolfost.stickpile.ui.common.catching
import io.github.dolfost.stickpile.ui.common.messageFor
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** The library screen's tabs, in their order. */
enum class LibraryTab { RECENT, LIKED, PACKS, MOST_USED }

/** What the library screen asks the library for. */
sealed interface LibraryRequest {
    data class Search(val query: String) : LibraryRequest

    /** [query] searched inside the pack [packId] only, with the whole syntax. */
    data class PackSearch(val packId: String, val query: String) : LibraryRequest
    data object Recent : LibraryRequest
    data object Favorites : LibraryRequest
    data object MostUsed : LibraryRequest
    data object Packs : LibraryRequest
    data class PackItems(val packId: String) : LibraryRequest
}

/**
 * What to show for a [query] (searched when not blank, whatever the tab),
 * else the [tab], where [pack] is the pack opened in the packs tab: with
 * a pack open, a query searches inside it, and a blank one shows it whole.
 */
fun requestFor(query: String, tab: LibraryTab, pack: Pack?): LibraryRequest = when {
    pack != null && query.isNotBlank() -> LibraryRequest.PackSearch(pack.id, query)
    query.isNotBlank() -> LibraryRequest.Search(query)
    tab == LibraryTab.RECENT -> LibraryRequest.Recent
    tab == LibraryTab.LIKED -> LibraryRequest.Favorites
    tab == LibraryTab.MOST_USED -> LibraryRequest.MostUsed
    pack != null -> LibraryRequest.PackItems(pack.id)
    else -> LibraryRequest.Packs
}

/**
 * Whether [new] refines what [old] shows, so that the old list stays until
 * the new one is there: another search, a search in the same pack, and
 * that pack whole again (the search cleared).
 */
fun refines(old: LibraryRequest?, new: LibraryRequest): Boolean {
    val packOf = { r: LibraryRequest? ->
        when (r) {
            is LibraryRequest.PackSearch -> r.packId
            is LibraryRequest.PackItems -> r.packId
            else -> null
        }
    }
    if (new == old) return true
    return when (new) {
        is LibraryRequest.Search -> old is LibraryRequest.Search
        is LibraryRequest.PackSearch, is LibraryRequest.PackItems -> packOf(old) == packOf(new)
        else -> false
    }
}

/** What the grid area shows. */
sealed interface LibraryContent {
    data object Loading : LibraryContent

    /**
     * [offline]: the server could not be reached and the cache answered.
     * [hidden]: the items safe mode left out of [items].
     */
    data class Items(val items: List<Item>, val offline: Boolean, val hidden: Int = 0) : LibraryContent
    data class Packs(val packs: List<Pack>, val offline: Boolean) : LibraryContent

    /** [signInAgain]: the account is gone (signed out, or the token was refused). */
    data class Failed(val message: Message, val signInAgain: Boolean) : LibraryContent
}

/** The item whose details are open, and its page in the web UI (null: none, or not known yet). */
data class Details(val item: Item, val url: String? = null)

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
    /** The offers under the search field: for [query], or for a moment for the text before it. */
    val hints: List<SearchHint> = emptyList(),
    val details: Details? = null,
) {
    val request: LibraryRequest get() = requestFor(query, tab, pack)
}

/** What the screen does once: share a file, or show a short message. */
sealed interface LibraryEvent {
    data class Share(val prepared: Prepared) : LibraryEvent
    data class Notice(val message: Message) : LibraryEvent
}

/**
 * The library screen (docs/android-plan.md 4.8): a search (debounced, with
 * offers for what is typed; with a pack open, inside that pack only), tabs,
 * a grid; a tap shares an item, a long
 * press opens its details, a refresh syncs. In safe mode ([display], shared
 * with the keyboard) the searches leave out the items tagged nsfw and the
 * lists hide them. [refreshLikes] reads the account's likes in the
 * background ([io.github.dolfost.stickpile.StickpileApp.refreshLikes]).
 * The offers come from the offline cache: a sync that changed it (counted
 * in [cacheChanges], the refresh's too) asks for them again, the text as
 * it is. The non-suspend library calls run on [io].
 */
@OptIn(FlowPreview::class)
class LibraryViewModel(
    private val library: StickpileLibraryInterface,
    private val display: StateFlow<Display>,
    private val refreshLikes: () -> Job? = { null },
    private val cacheChanges: CacheChanges = CacheChanges(),
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

    /** The answer on screen as the library gave it, before safe mode filtered it. */
    private var answer: LibraryContent = LibraryContent.Loading
    private val loads = MutableStateFlow(Load(state.request, typed = false, generation = 0))

    /** The text to offer hints for; null until the search field is used. */
    private val hintQueries = MutableStateFlow<String?>(null)

    init {
        viewModelScope.launch {
            loads
                .debounce { if (it.typed) SEARCH_DELAY_MS else 0L }
                .collectLatest { load(it.request) }
        }
        viewModelScope.launch {
            combine(hintQueries.filterNotNull(), cacheChanges.count) { query, _ -> query }
                .collectLatest { loadHints(it) }
        }
        viewModelScope.launch {
            // Switched here or in the keyboard: the list is filtered again at
            // once, and asked for again (a search goes out with the filter).
            display.map { it.safeMode }.distinctUntilChanged().drop(1).collect {
                show(answer)
                reload()
            }
        }
    }

    fun setQuery(text: String) {
        state = state.copy(query = text)
        request(typed = true)
        hintQueries.value = text
    }

    /** Make [text] the query and search at once (an offer, an example or a tag was tapped). */
    fun searchNow(text: String) {
        state = state.copy(query = text)
        request(typed = false)
        hintQueries.value = text
    }

    /** Take an offer: its query replaces the text. */
    fun takeHint(hint: SearchHint) = searchNow(hint.query)

    /** The search field got the focus: offer what fits its text (the filter keys when empty). */
    fun searchFocused() {
        hintQueries.value = state.query
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

    /**
     * Back on the screen: the recent items may have changed (a chat, the
     * settings), and the likes are read now and then, after which the list
     * is loaded again to show them.
     */
    fun resumed() {
        if (state.request == LibraryRequest.Recent) reload()
        val reading = refreshLikes() ?: return
        viewModelScope.launch {
            reading.join()
            reload()
        }
    }

    /** Sync now, then load again; a failure is a message, the list stays. */
    fun refresh() {
        if (state.refreshing) return
        state = state.copy(refreshing = true)
        viewModelScope.launch {
            catching { library.syncNow() }
                .onSuccess(cacheChanges::after)
                .onFailure { e -> notice(Doing.SYNCING, e) }
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

    /** Like [item], or take the like back; the list and the details show it at once. */
    fun toggleLike(item: Item) {
        viewModelScope.launch {
            catching { library.toggleLike(item.id) }.fold(
                onSuccess = { liked ->
                    markLiked(item.id, liked)
                    val text = if (liked) R.string.library_liked else R.string.library_unliked
                    events.send(LibraryEvent.Notice(Message(text, listOf(item.name))))
                    if (state.request == LibraryRequest.Favorites) reload()
                },
                onFailure = { e -> notice(Doing.LIKING, e) },
            )
        }
    }

    /** Open [item]'s details; its web page is looked up meanwhile. */
    fun showDetails(item: Item) {
        state = state.copy(details = Details(item))
        viewModelScope.launch {
            val url = withContext(io) { catching { library.itemUrl(item.id) }.getOrNull() }
            val open = state.details
            if (open != null && open.item.id == item.id) state = state.copy(details = open.copy(url = url))
        }
    }

    fun hideDetails() {
        state = state.copy(details = null)
    }

    private fun request(typed: Boolean) {
        loads.value = Load(state.request, typed, generation)
    }

    private suspend fun load(request: LibraryRequest) {
        // A reload or a refined search keeps the old list until the new
        // one is there; anything else (another tab or pack) starts empty.
        if (!refines(shown, request)) show(LibraryContent.Loading)
        shown = request
        val result = catching { fetch(request) }
        show(
            result.getOrElse { e ->
                log.failed(Doing.LOADING, e)
                val failure = Failure.of(e)
                val gone = failure.kind == LibraryErrorKind.NOT_SIGNED_IN || failure.kind == LibraryErrorKind.UNAUTHORIZED
                LibraryContent.Failed(messageFor(failure, Doing.LOADING), signInAgain = gone)
            },
        )
    }

    private suspend fun fetch(request: LibraryRequest): LibraryContent = when (request) {
        is LibraryRequest.Search -> {
            val query = SafeMode.query(request.query, display.value.safeMode)
            items(library.search(query, Target.GENERIC, SEARCH_LIMIT))
        }
        is LibraryRequest.PackSearch -> {
            val query = SafeMode.query(request.query, display.value.safeMode)
            items(library.packSearch(request.packId, query))
        }
        LibraryRequest.Recent -> LibraryContent.Items(withContext(io) { library.recent(LIST_LIMIT) }, offline = false)
        LibraryRequest.Favorites -> items(library.favorites(LIST_LIMIT))
        LibraryRequest.MostUsed -> items(library.mostUsed(LIST_LIMIT))
        LibraryRequest.Packs -> library.packs().let { LibraryContent.Packs(it.packs, it.offline) }
        is LibraryRequest.PackItems -> items(library.packItems(request.packId))
    }

    private fun items(list: ItemList) = LibraryContent.Items(list.items, list.offline)

    /** Show [content] the way safe mode wants it, keeping it whole for when safe mode changes. */
    private fun show(content: LibraryContent) {
        answer = content
        state = state.copy(content = filtered(content))
    }

    private fun filtered(content: LibraryContent): LibraryContent {
        if (content !is LibraryContent.Items) return content
        val kept = SafeMode.filter(content.items, display.value.safeMode)
        return content.copy(items = kept.items, hidden = kept.hidden)
    }

    /** The account now likes [id] or not: the shown list and the details follow. */
    private fun markLiked(id: String, liked: Boolean) {
        val whole = answer
        if (whole is LibraryContent.Items) {
            show(whole.copy(items = whole.items.map { if (it.id == id) it.withLike(liked) else it }))
        }
        val open = state.details
        if (open != null && open.item.id == id) state = state.copy(details = open.copy(item = open.item.withLike(liked)))
    }

    /** Offers for [query]; an answer for text typed over meanwhile is dropped. */
    private suspend fun loadHints(query: String) {
        val hints = withContext(io) {
            catching { library.hints(query) }.getOrElse { e ->
                log.failed(Doing.LOADING, e)
                emptyList()
            }
        }
        if (query == state.query) state = state.copy(hints = hints)
    }

    private suspend fun notice(doing: Doing, e: Throwable) {
        log.failed(doing, e)
        events.send(LibraryEvent.Notice(messageFor(Failure.of(e), doing)))
    }

    companion object {
        /** How long the typing must pause before a search goes out (the desktop's). */
        const val SEARCH_DELAY_MS = 120L
        val SEARCH_LIMIT = 120u
        val LIST_LIMIT = 200u

        /** Sharing from the app: any picture or video (the library picks the rendition). */
        val SHARE_TYPES = listOf("image/*", "video/*")
    }
}

/**
 * [this] with the account's like set to [liked], its count of likes
 * following; unchanged when it was so already.
 */
fun Item.withLike(liked: Boolean): Item = when {
    liked == this.liked -> this
    liked -> copy(liked = true, likes = likes + 1u)
    else -> copy(liked = false, likes = if (likes > 0u) likes - 1u else 0u)
}
