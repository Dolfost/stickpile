package io.github.dolfost.stickpile.ime

import android.annotation.SuppressLint
import android.content.Context
import android.content.res.ColorStateList
import android.view.GestureDetector
import android.view.LayoutInflater
import android.view.MotionEvent
import android.view.View
import android.widget.Button
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView
import androidx.annotation.AttrRes
import androidx.annotation.StringRes
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.isEmpty
import androidx.core.view.isVisible
import androidx.core.view.updateLayoutParams
import androidx.core.view.updatePadding
import androidx.recyclerview.widget.GridLayoutManager
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.button.MaterialButton
import com.google.android.material.color.MaterialColors
import com.google.android.material.tabs.TabLayout
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.Columns
import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.display.DisplaySettings
import io.github.dolfost.stickpile.display.SafeMode
import io.github.dolfost.stickpile.provider.kind
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.Pack
import java.util.Locale
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * The picker's view (docs/android.md, the picker keyboard): the search bar
 * (the button back to the keyboard, the search box, safe mode, the menu),
 * the tabs (Results while something is searched for, Recent, Liked, Most
 * used, Sets), the offers for the query or the emoji filters, the grid of
 * tiles with what [Display] shows over them, a sticker's details and the
 * syntax help over the grid, and the picker's own search keys
 * ([SearchKeys]). Classic Views, made with a context themed with
 * `Theme.Stickpile.Views`. What it does with a picked item is the
 * service's ([Actions]).
 *
 * The query starts as the draft read from the field, if any. The keys and
 * the offers edit it here only: the editor never sees it, and it is never
 * logged. Each change asks for the offers and, after a short pause
 * ([PickerSearch.SEARCH_DELAY_MS]), searches. Safe mode (shared with the
 * app through [DisplaySettings]) leaves nsfw out of the search and of every
 * list.
 *
 * Runs on the main thread; it loads through [ImeLibrary] in coroutines of
 * its own scope (a child of the service's), cancelled by [stop] and
 * [dispose].
 */
internal class PickerView(
    private val context: Context,
    parentScope: CoroutineScope,
    private val library: ImeLibrary,
    private val displaySettings: DisplaySettings,
    private val settings: ImeSettings,
    private val actions: Actions,
) {
    interface Actions {
        /** Insert [item]; [fromDraft]: it was found by searching the draft, which may then be deleted. */
        fun pick(item: Item, sticker: Boolean, fromDraft: Boolean)
        fun toggleLike(item: Item)
        fun share(item: Item)
        fun backToKeyboard()
        fun openApp()
    }

    /** What the grid shows. */
    private sealed interface Shown {
        /**
         * @property hidden how many safe mode left out.
         * @property resultsFor the query these are the results of; null for a list.
         */
        data class Items(
            val items: List<Item>,
            val offline: Boolean,
            val empty: String,
            val hidden: Int = 0,
            val resultsFor: String? = null,
        ) : Shown

        data class Packs(val packs: List<Pack>, val offline: Boolean) : Shown

        data class Failed(val kind: LibraryErrorKind) : Shown
    }

    private val job = SupervisorJob(parentScope.coroutineContext[Job])
    private val scope = CoroutineScope(parentScope.coroutineContext + job)

    // The input view has no parent until the service hands it to the system.
    @SuppressLint("InflateParams")
    val root: View = LayoutInflater.from(context).inflate(R.layout.ime_picker, null)

    private val searchBox: View = root.findViewById(R.id.ime_search_box)
    private val queryView: TextView = root.findViewById(R.id.ime_query)
    private val clear: View = root.findViewById(R.id.ime_clear)
    private val safe: MaterialButton = root.findViewById(R.id.ime_safe)
    private val tabs: TabLayout = root.findViewById(R.id.ime_tabs)
    private val setHeader: View = root.findViewById(R.id.ime_set_header)
    private val setName: TextView = root.findViewById(R.id.ime_set_name)
    private val body: View = root.findViewById(R.id.ime_body)
    private val grid: RecyclerView = root.findViewById(R.id.ime_grid)
    private val messageBox: View = root.findViewById(R.id.ime_message_box)
    private val message: TextView = root.findViewById(R.id.ime_message)
    private val messageAction: Button = root.findViewById(R.id.ime_message_action)
    private val progress: View = root.findViewById(R.id.ime_progress)
    private val refreshing: View = root.findViewById(R.id.ime_refreshing)
    private val status: TextView = root.findViewById(R.id.ime_status)
    private val keysView: LinearLayout = root.findViewById(R.id.ime_keys)

    private val bodyHeightPx = context.resources.getDimensionPixelSize(R.dimen.ime_body_height)
    private val gridPaddingPx = context.resources.getDimensionPixelSize(R.dimen.ime_grid_padding)

    private var display: Display = displaySettings.current
    private val look = TileLook(context)
    private val thumbnails = ThumbnailLoader(scope, library, context.resources, largestCellPx())
    private val motions = MotionBinder(scope, library, context)
    private val itemsAdapter = ItemsAdapter(thumbnails, motions, look, ::onItemClick, ::onItemLongClick)
    private val packsAdapter = PacksAdapter(thumbnails, ::openPack)
    private val chipRows = ChipRows(
        root,
        onOffer = { setQuery(it.query) },
        onFilter = { emoji -> itemsAdapter.submitList(EmojiFilters.narrow(items, emoji) { it.emojis }) },
    )
    private val gridLayout = GridLayoutManager(context, Columns.DEFAULT)
    private val listLayout = LinearLayoutManager(context)

    private val details = DetailsPanel(
        root.findViewById(R.id.ime_details),
        thumbnails,
        motions,
        object : DetailsPanel.Listener {
            override fun send(item: Item, sticker: Boolean) {
                if (busy) return
                val fromDraft = pickFromDraft()
                closeDetails()
                actions.pick(item, sticker, fromDraft)
            }

            override fun toggleLike(item: Item) = actions.toggleLike(item)
            override fun share(item: Item) = actions.share(item)
            override fun search(query: String) = setQuery(query)
            override fun closed(item: Item) = itemsAdapter.rebind(item.id)
        },
    )
    private val syntax = SyntaxPanel(root.findViewById(R.id.ime_syntax), ::setQuery)
    private val columns = ColumnsPanel(root.findViewById(R.id.ime_columns), displaySettings)
    private val keys = SearchKeys(
        keysView,
        KeyLayout.forTag(settings.searchLayout) ?: KeyLayout.default(Locale.getDefault().language),
        object : SearchKeys.Listener {
            override fun type(text: String) = setQuery(QueryEditor.type(query, text))
            override fun backspace() = setQuery(QueryEditor.backspace(query))
            override fun done() = hideKeys()
            override fun layoutChanged(layout: KeyLayout) {
                settings.searchLayout = layout.tag
            }
        },
    )

    private var started = false
    private var draftQuery: String? = null
    private var query = ""
    private var tab = PickerTab.RECENT
    private var items: List<Item> = emptyList()
    private var resultsFor: String? = null
    private var pack: Pack? = null
    private var keysShown = false

    /** The keyboard height setting, as last read ([start]). */
    private var heightPercent = settings.heightPercent
    private var loadJob: Job? = null
    private var searchJob: Job? = null
    private var hintsJob: Job? = null
    private var likesJob: Job? = null
    private var loading = false
    private var reloading = false
    private var busy = false

    /** Tabs are being changed by code: their listener loads nothing. */
    private var quietTabs = false

    /** Said with the next list shown: safe mode was just turned on or off here. */
    @StringRes
    private var safeNote: Int? = null

    /** Like states learnt since the picker opened: from the Liked tab and from toggles. */
    private val likes = HashMap<String, Boolean>()

    private val hideStatus = Runnable { status.visibility = View.GONE }

    init {
        root.findViewById<View>(R.id.ime_back).setOnClickListener { actions.backToKeyboard() }
        root.findViewById<View>(R.id.ime_all_sets).setOnClickListener { show(PickerTab.SETS) }
        root.findViewById<View>(R.id.ime_more).setOnClickListener {
            PickerMenu.show(it, displaySettings, ::showColumns, ::showSyntax, actions::openApp)
        }
        root.findViewById<ImageView>(R.id.ime_search_icon).imageTintList =
            ColorStateList.valueOf(color(com.google.android.material.R.attr.colorOnSurfaceVariant))
        searchBox.setOnClickListener { showKeys() }
        clear.setOnClickListener { setQuery("") }
        safe.setOnClickListener { toggleSafeMode() }
        messageAction.setOnClickListener { actions.openApp() }
        messageBox.setOnClickListener { hideKeys() }
        tabs.addOnTabSelectedListener(object : TabLayout.OnTabSelectedListener {
            override fun onTabSelected(selected: TabLayout.Tab) {
                if (!quietTabs) (selected.tag as? PickerTab)?.let(::show)
            }

            override fun onTabUnselected(unselected: TabLayout.Tab) = Unit

            override fun onTabReselected(reselected: TabLayout.Tab) {
                if (!quietTabs && reselected.tag == PickerTab.SETS && pack != null) show(PickerTab.SETS)
            }
        })
        grid.layoutManager = gridLayout
        grid.adapter = itemsAdapter
        grid.addOnItemTouchListener(emptyTapHidesKeys())
        look.display = display
        look.likedOf = ::likedOf
        motions.autoplay = display.autoplay
        // Keep clear of the navigation bar where the keyboard's window reaches under it.
        ViewCompat.setOnApplyWindowInsetsListener(root) { view, insets ->
            view.updatePadding(bottom = insets.getInsets(WindowInsetsCompat.Type.navigationBars()).bottom)
            insets
        }
        applyColumns()
        showQuery()
        showSafeMode()
        scope.launch { displaySettings.state.collect { onDisplay(it) } }
    }

    /**
     * The picker is shown for a field: the draft's results first when
     * [query] is set, else Recent. A restart for the same field keeps what
     * is shown, unless there is a new draft. [likesRead] is a read of the
     * favorites going on ([io.github.dolfost.stickpile.StickpileApp.refreshLikes]):
     * the list is loaded again after it, for the hearts.
     */
    fun start(query: String?, restarting: Boolean, likesRead: Job?) {
        whenLikesRead(likesRead)
        val height = settings.heightPercent
        if (height != heightPercent) {
            heightPercent = height
            applyColumns()
        }
        if (InputRestart.keepsView(started, restarting, query, draftQuery)) return
        started = true
        draftQuery = query
        likes.clear()
        hideKeys()
        closeDetails()
        syntax.close()
        columns.close()
        this.query = query.orEmpty()
        showQuery()
        updateChipRows()
        requestHints()
        val first = PickerSearch.tabs(this.query).first()
        show(first)
    }

    /** Once [read] is over, what is shown loads again with the hearts it learnt (not the list of sets). */
    private fun whenLikesRead(read: Job?) {
        read ?: return
        likesJob?.cancel()
        likesJob = scope.launch {
            read.join()
            if (started && !(tab == PickerTab.SETS && pack == null)) reload(top = false)
        }
    }

    /** The picker was hidden: stop loading. */
    fun stop() {
        started = false
        loadJob?.cancel()
        searchJob?.cancel()
        hintsJob?.cancel()
        likesJob?.cancel()
        keys.stopRepeat()
        loading = false
        reloading = false
        updateProgress()
    }

    /** The view is gone for good. */
    fun dispose() {
        job.cancel()
        keys.stopRepeat()
        status.removeCallbacks(hideStatus)
        thumbnails.clear()
    }

    /** An insert is running: show it, and take no other pick. */
    fun setBusy(value: Boolean) {
        busy = value
        updateProgress()
    }

    fun showStatus(@StringRes text: Int) = showStatus(context.getString(text))

    private fun showStatus(text: CharSequence) {
        status.text = text
        status.visibility = View.VISIBLE
        status.removeCallbacks(hideStatus)
        status.postDelayed(hideStatus, STATUS_MILLIS)
    }

    /** A failure of an action: a short line, or the signed-out message with its button. */
    fun showError(kind: LibraryErrorKind) {
        if (ImeTexts.isSignedOut(kind)) display(Shown.Failed(kind)) else showStatus(ImeTexts.forError(kind))
    }

    fun likeChanged(item: Item, liked: Boolean) {
        likes[item.id] = liked
        if (tab == PickerTab.LIKED && pack == null && !liked && items.any { it.id == item.id }) {
            display(Shown.Items(items.filter { it.id != item.id }, false, context.getString(R.string.ime_empty_liked)))
        } else {
            itemsAdapter.refreshTile(item.id)
        }
        details.likeChanged(item.id, liked)
        showStatus(if (liked) R.string.ime_liked else R.string.ime_unliked)
    }

    // -- the query -----------------------------------------------------------------

    /**
     * The query is now [text]: the offers for it, and the results (after a
     * pause) while it is not blank; Recent again once it is cleared.
     */
    private fun setQuery(text: String) {
        if (text == query) return
        query = text
        showQuery()
        updateChipRows()
        requestHints()
        if (query.isNotBlank()) {
            closeDetails()
            syntax.close()
            if (tab != PickerTab.RESULTS || pack != null) enter(PickerTab.RESULTS) else setTabs(PickerTab.RESULTS)
            scheduleSearch()
        } else {
            searchJob?.cancel()
            if (tab == PickerTab.RESULTS) show(PickerTab.RECENT) else setTabs(tab)
        }
    }

    private fun showQuery() {
        queryView.text = query
        clear.isVisible = query.isNotEmpty()
    }

    /** The search after a pause; none when the results shown are already for this query (a space typed). */
    private fun scheduleSearch() {
        searchJob?.cancel()
        if (query.trim() == resultsFor && loadJob?.isActive != true) return
        searchJob = scope.launch {
            delay(PickerSearch.SEARCH_DELAY_MS)
            loadTab(PickerTab.RESULTS)
        }
    }

    /** The offers for the query as it is now; an answer for an older query is dropped. */
    private fun requestHints() {
        hintsJob?.cancel()
        val asked = query
        hintsJob = scope.launch {
            val offers = try {
                library.hints(asked)
            } catch (_: LibraryException) {
                emptyList()
            }
            if (asked == query) chipRows.showOffers(offers)
        }
    }

    /** The offers while typing or searching; the emoji filters otherwise. */
    private fun updateChipRows() {
        chipRows.offer(keysShown || query.isNotEmpty())
    }

    // -- the keys ------------------------------------------------------------------

    /** The search keys show under a smaller grid, in place of the tabs. */
    private fun showKeys() {
        if (keysShown) return
        closeDetails()
        syntax.close()
        columns.close()
        keysShown = true
        if (keysView.isEmpty()) keys.build()
        keysView.isVisible = true
        tabs.isVisible = false
        setHeader.isVisible = false
        searchBox.isActivated = true
        applyColumns()
        updateChipRows()
    }

    private fun hideKeys() {
        if (!keysShown) return
        keysShown = false
        keys.stopRepeat()
        keysView.isVisible = false
        tabs.isVisible = true
        setHeader.isVisible = pack != null
        searchBox.isActivated = false
        applyColumns()
        updateChipRows()
    }

    /** A tap on the grid where there is no sticker hides the keys. */
    private fun emptyTapHidesKeys(): RecyclerView.OnItemTouchListener {
        val taps = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
            override fun onSingleTapUp(e: MotionEvent): Boolean = true
        })
        return object : RecyclerView.SimpleOnItemTouchListener() {
            override fun onInterceptTouchEvent(rv: RecyclerView, e: MotionEvent): Boolean {
                if (taps.onTouchEvent(e) && keysShown && rv.findChildViewUnder(e.x, e.y) == null) hideKeys()
                return false
            }
        }
    }

    // -- tabs and loading ------------------------------------------------------------

    /** Shows [t] and loads it. */
    private fun show(t: PickerTab) {
        enter(t)
        loadTab(t)
    }

    /** [t] is the tab now (out of a set), without loading it. */
    private fun enter(t: PickerTab) {
        tab = t
        pack = null
        setHeader.visibility = View.GONE
        setTabs(t)
    }

    /** The tabs for the query, [selected] selected, without loading anything. */
    private fun setTabs(selected: PickerTab) {
        val wanted = PickerSearch.tabs(query)
        quietTabs = true
        try {
            val now = (0 until tabs.tabCount).map { tabs.getTabAt(it)?.tag }
            if (now != wanted) {
                tabs.removeAllTabs()
                for (t in wanted) tabs.addTab(tabs.newTab().setText(title(t)).setTag(t), false)
            }
            tabs.getTabAt(wanted.indexOf(selected))?.select()
        } finally {
            quietTabs = false
        }
    }

    /**
     * Loads [t]. [keep]: what is shown stays until the answer comes; [top]:
     * the answer is shown from its start.
     */
    private fun loadTab(t: PickerTab, keep: Boolean = false, top: Boolean = true) {
        when (t) {
            PickerTab.RESULTS -> {
                val q = query.trim()
                if (q.isEmpty()) return
                load(keep || resultsFor != null, top) { search(q) }
            }
            PickerTab.RECENT -> load(keep, top) {
                val recent = library.recent()
                if (recent.isEmpty() && !library.signedIn()) {
                    Shown.Failed(LibraryErrorKind.NOT_SIGNED_IN)
                } else {
                    items(recent, false, context.getString(R.string.ime_empty_recent))
                }
            }
            PickerTab.LIKED -> load(keep, top) {
                val list = library.favorites()
                list.items.forEach { likes[it.id] = true }
                items(list.items, list.offline, context.getString(R.string.ime_empty_liked))
            }
            PickerTab.MOST_USED -> load(keep, top) {
                val list = library.mostUsed()
                items(list.items, list.offline, context.getString(R.string.ime_empty_most_used))
            }
            PickerTab.SETS -> load(keep, top) {
                val list = library.packs()
                Shown.Packs(list.packs, list.offline)
            }
        }
    }

    private fun openPack(pack: Pack) = loadPack(pack)

    private fun loadPack(p: Pack, keep: Boolean = false, top: Boolean = true) {
        pack = p
        setName.text = p.name
        setHeader.isVisible = !keysShown
        load(keep, top) {
            val list = library.packItems(p.id)
            items(list.items, list.offline, context.getString(R.string.ime_empty_set))
        }
    }

    /** What is shown, loaded again over it (after safe mode or the likes changed). */
    private fun reload(top: Boolean) {
        val p = pack
        if (p != null) loadPack(p, keep = true, top = top) else loadTab(tab, keep = true, top = top)
    }

    /** The search, with safe mode's filter in safe mode. */
    private suspend fun search(q: String): Shown {
        val list = library.search(SafeMode.query(q, display.safeMode))
        return items(list.items, list.offline, context.getString(R.string.ime_empty_results, q), resultsFor = q)
    }

    /** What safe mode shows of [list]. */
    private fun items(list: List<Item>, offline: Boolean, empty: String, resultsFor: String? = null): Shown.Items {
        val shown = SafeMode.filter(list, display.safeMode)
        return Shown.Items(shown.items, offline, empty, shown.hidden, resultsFor)
    }

    private fun load(keep: Boolean, top: Boolean, block: suspend () -> Shown) {
        loadJob?.cancel()
        if (keep) {
            reloading = true
        } else {
            loading = true
            messageBox.visibility = View.GONE
            items = emptyList()
            resultsFor = null
            updateFilters()
            itemsAdapter.submitList(emptyList())
            packsAdapter.submitList(emptyList())
        }
        updateProgress()
        loadJob = scope.launch {
            val shown = try {
                block()
            } catch (e: LibraryException) {
                Shown.Failed(e.kind)
            }
            loading = false
            reloading = false
            updateProgress()
            display(shown, top)
        }
    }

    private fun display(shown: Shown, top: Boolean = false) {
        when (shown) {
            is Shown.Items -> {
                items = shown.items
                resultsFor = shown.resultsFor
                use(gridLayout, itemsAdapter)
                updateFilters()
                itemsAdapter.submitList(items) { if (top) grid.scrollToPosition(0) }
                when {
                    items.isNotEmpty() -> messageBox.visibility = View.GONE
                    shown.hidden > 0 -> showMessage(hiddenText(shown.hidden))
                    else -> showMessage(shown.empty)
                }
                notes(shown.offline, if (items.isEmpty()) 0 else shown.hidden)
            }
            is Shown.Packs -> {
                items = emptyList()
                resultsFor = null
                updateFilters()
                use(listLayout, packsAdapter)
                packsAdapter.submitList(shown.packs)
                if (shown.packs.isEmpty()) {
                    showMessage(context.getString(R.string.ime_empty_sets))
                } else {
                    messageBox.visibility = View.GONE
                }
                notes(shown.offline, 0)
            }
            is Shown.Failed -> {
                items = emptyList()
                resultsFor = null
                safeNote = null
                updateFilters()
                itemsAdapter.submitList(emptyList())
                packsAdapter.submitList(emptyList())
                showMessage(context.getString(ImeTexts.forError(shown.kind)), openApp = ImeTexts.isSignedOut(shown.kind))
            }
        }
    }

    /** The status line after a list: safe mode just changed, how many it hid, offline. */
    private fun notes(offline: Boolean, hidden: Int) {
        val lines = listOfNotNull(
            safeNote?.let(context::getString),
            hidden.takeIf { it > 0 }?.let(::hiddenText),
            context.getString(R.string.ime_offline).takeIf { offline },
        )
        safeNote = null
        if (lines.isNotEmpty()) showStatus(lines.joinToString("\n"))
    }

    private fun hiddenText(hidden: Int): String =
        context.resources.getQuantityString(R.plurals.display_hidden_by_safe_mode, hidden, hidden)

    private fun use(layout: RecyclerView.LayoutManager, adapter: RecyclerView.Adapter<*>) {
        if (grid.adapter !== adapter) {
            grid.layoutManager = layout
            grid.adapter = adapter
        }
    }

    /** The emoji filters of the items shown; none selected. */
    private fun updateFilters() {
        chipRows.showFilters(items.map { it.emojis })
    }

    private fun showMessage(text: String, openApp: Boolean = false) {
        message.text = text
        messageAction.visibility = if (openApp) View.VISIBLE else View.GONE
        messageBox.visibility = View.VISIBLE
    }

    private fun updateProgress() {
        progress.isVisible = loading || busy
        refreshing.isVisible = reloading && !loading && !busy
    }

    // -- display settings ----------------------------------------------------------

    /** The shared display settings changed (here or in the app). */
    private fun onDisplay(new: Display) {
        val old = display
        display = new
        look.display = new
        motions.autoplay = new.autoplay
        showSafeMode()
        when {
            old.autoplay != new.autoplay -> {
                if (old.columns != new.columns) applyColumns()
                itemsAdapter.rebindAll()
            }
            old.columns != new.columns -> applyColumns()
            else -> itemsAdapter.refreshTiles()
        }
        if (old.safeMode != new.safeMode && started) reload(top = true)
    }

    private fun toggleSafeMode() {
        val on = !display.safeMode
        val note = if (on) R.string.display_safe_on else R.string.display_safe_off
        safeNote = note
        showStatus(note)
        displaySettings.setSafeMode(on)
    }

    private fun showSafeMode() {
        val on = display.safeMode
        safe.setIconResource(if (on) R.drawable.ic_safe_on else R.drawable.ic_safe_off)
        safe.iconTint = ColorStateList.valueOf(
            color(if (on) android.R.attr.colorPrimary else com.google.android.material.R.attr.colorOnSurfaceVariant),
        )
        val label = context.getString(if (on) R.string.display_safe_on else R.string.display_safe_off)
        safe.contentDescription = label
        safe.tooltipText = label
    }

    /**
     * Columns for the tile size, more and smaller while the keys show (then
     * the grid is two rows of them tall), and the lines a tile has room for.
     */
    private fun applyColumns() {
        val density = context.resources.displayMetrics.density
        val widthPx = context.resources.displayMetrics.widthPixels - 2 * gridPaddingPx
        val widthDp = (widthPx / density).toInt()
        val normal = PickerSearch.spanCount(widthDp, display.columns, context.resources.configuration.smallestScreenWidthDp)
        val span = if (keysShown) PickerSearch.typingSpanCount(widthDp, normal) else normal
        if (gridLayout.spanCount != span) gridLayout.spanCount = span
        look.maxLines = TileInfo.maxLines(widthDp / span, context.resources.configuration.fontScale)
        look.cellPx = widthPx / span
        val usualPx = if (keysShown) TYPING_ROWS * (widthPx / span) + 2 * gridPaddingPx else bodyHeightPx
        val screenPx = context.resources.displayMetrics.heightPixels
        val bodyPx = PickerHeight.scaled(usualPx, heightPercent, screenPx, (MIN_BODY_DP * density).toInt())
        if (body.layoutParams.height != bodyPx) body.updateLayoutParams { height = bodyPx }
        itemsAdapter.refreshTiles()
    }

    // -- items -----------------------------------------------------------------------

    private fun likedOf(item: Item): Boolean = likes[item.id] ?: item.liked

    private fun pickFromDraft(): Boolean = PickerSearch.fromDraft(draftQuery, query, resultsFor)

    private fun onItemClick(item: Item) {
        if (!busy) actions.pick(item, sticker = false, fromDraft = pickFromDraft())
    }

    private fun onItemLongClick(item: Item) {
        if (busy) return
        hideKeys()
        syntax.close()
        columns.close()
        details.show(item, likedOf(item))
    }

    private fun closeDetails() {
        details.close()
    }

    private fun showSyntax() {
        hideKeys()
        closeDetails()
        columns.close()
        syntax.show()
    }

    private fun showColumns() {
        hideKeys()
        closeDetails()
        syntax.close()
        columns.show()
    }

    private fun title(t: PickerTab): String = context.getString(
        when (t) {
            PickerTab.RESULTS -> R.string.ime_tab_results
            PickerTab.RECENT -> R.string.ime_tab_recent
            PickerTab.LIKED -> R.string.ime_tab_liked
            PickerTab.MOST_USED -> R.string.ime_tab_most_used
            PickerTab.SETS -> R.string.ime_tab_sets
        },
    )

    private fun color(@AttrRes attr: Int): Int = MaterialColors.getColor(root, attr)

    /** The widest a grid cell gets (the fewest columns): the most a thumbnail is decoded at. */
    private fun largestCellPx(): Int =
        (context.resources.displayMetrics.widthPixels - 2 * gridPaddingPx) / Columns.MIN

    private companion object {
        const val STATUS_MILLIS = 5_000L

        /** Rows of the grid while the keys show (at 100%). */
        const val TYPING_ROWS = 2

        /** The lowest the grid gets, whatever the setting and the screen. */
        const val MIN_BODY_DP = 96
    }
}
