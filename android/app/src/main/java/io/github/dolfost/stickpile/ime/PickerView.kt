package io.github.dolfost.stickpile.ime

import android.annotation.SuppressLint
import android.content.Context
import android.view.LayoutInflater
import android.view.Menu
import android.view.View
import android.widget.Button
import android.widget.PopupMenu
import android.widget.TextView
import androidx.annotation.StringRes
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.updatePadding
import androidx.recyclerview.widget.GridLayoutManager
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.chip.Chip
import com.google.android.material.chip.ChipGroup
import com.google.android.material.tabs.TabLayout
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.provider.kind
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.Pack
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * The picker's view (docs/android-plan.md 4.7): the button back to the
 * keyboard, the tabs (the draft's results when there is a draft, Recent,
 * Liked, Sets), a row of emoji filters, and the grid. Classic Views, made
 * with a context themed with `Theme.Stickpile.Views`. What it does with a
 * picked item is the service's ([Actions]).
 *
 * Runs on the main thread; it loads through [ImeLibrary] in coroutines of
 * its own scope (a child of the service's), cancelled by [stop] and
 * [dispose].
 */
internal class PickerView(
    private val context: Context,
    parentScope: CoroutineScope,
    private val library: ImeLibrary,
    private val actions: Actions,
) {
    interface Actions {
        /** Insert [item]; [fromDraft]: it was found by searching the draft. */
        fun pick(item: Item, sticker: Boolean, fromDraft: Boolean)
        fun toggleLike(item: Item)
        fun share(item: Item)
        fun backToKeyboard()
        fun openApp()
    }

    private enum class Tab { RESULTS, RECENT, LIKED, SETS }

    /** What the grid shows. */
    private sealed interface Shown {
        data class Items(val items: List<Item>, val offline: Boolean, val empty: String) : Shown

        data class Packs(val packs: List<Pack>, val offline: Boolean) : Shown

        data class Failed(val kind: LibraryErrorKind) : Shown
    }

    private val job = SupervisorJob(parentScope.coroutineContext[Job])
    private val scope = CoroutineScope(parentScope.coroutineContext + job)

    // The input view has no parent until the service hands it to the system.
    @SuppressLint("InflateParams")
    val root: View = LayoutInflater.from(context).inflate(R.layout.ime_picker, null)

    private val tabs: TabLayout = root.findViewById(R.id.ime_tabs)
    private val setHeader: View = root.findViewById(R.id.ime_set_header)
    private val setName: TextView = root.findViewById(R.id.ime_set_name)
    private val filtersScroll: View = root.findViewById(R.id.ime_filters_scroll)
    private val filters: ChipGroup = root.findViewById(R.id.ime_filters)
    private val grid: RecyclerView = root.findViewById(R.id.ime_grid)
    private val messageBox: View = root.findViewById(R.id.ime_message_box)
    private val message: TextView = root.findViewById(R.id.ime_message)
    private val messageAction: Button = root.findViewById(R.id.ime_message_action)
    private val progress: View = root.findViewById(R.id.ime_progress)
    private val status: TextView = root.findViewById(R.id.ime_status)

    private val thumbnails = ThumbnailLoader(scope, library, context.resources)
    private val itemsAdapter = ItemsAdapter(thumbnails, ::onItemClick, ::onItemLongClick)
    private val packsAdapter = PacksAdapter(::openPack)
    private val gridLayout = GridLayoutManager(context, spanCount())
    private val listLayout = LinearLayoutManager(context)

    private var started = false
    private var draftQuery: String? = null
    private var tab = Tab.RECENT
    private var items: List<Item> = emptyList()
    private var inPack = false
    private var loadJob: Job? = null
    private var loading = false
    private var busy = false

    /** Like states learnt since the picker opened: from the Liked tab and from toggles. */
    private val likes = HashMap<String, Boolean>()

    private val hideStatus = Runnable { status.visibility = View.GONE }

    init {
        root.findViewById<View>(R.id.ime_back).setOnClickListener { actions.backToKeyboard() }
        root.findViewById<View>(R.id.ime_all_sets).setOnClickListener { show(Tab.SETS) }
        messageAction.setOnClickListener { actions.openApp() }
        filters.setOnCheckedStateChangeListener { group, checked ->
            val emoji = checked.firstOrNull()?.let { group.findViewById<Chip>(it)?.tag as? String }
            itemsAdapter.submitList(EmojiFilters.narrow(items, emoji) { it.emojis })
        }
        tabs.addOnTabSelectedListener(object : TabLayout.OnTabSelectedListener {
            override fun onTabSelected(selected: TabLayout.Tab) {
                (selected.tag as? Tab)?.let(::show)
            }

            override fun onTabUnselected(unselected: TabLayout.Tab) = Unit

            override fun onTabReselected(reselected: TabLayout.Tab) {
                if (reselected.tag == Tab.SETS && inPack) show(Tab.SETS)
            }
        })
        grid.layoutManager = gridLayout
        grid.adapter = itemsAdapter
        // Keep clear of the navigation bar where the keyboard's window reaches under it.
        ViewCompat.setOnApplyWindowInsetsListener(root) { view, insets ->
            view.updatePadding(bottom = insets.getInsets(WindowInsetsCompat.Type.navigationBars()).bottom)
            insets
        }
    }

    /**
     * The picker is shown for a field: the draft's results first when
     * [query] is set, else Recent. A restart for the same field keeps what
     * is shown, unless there is a new draft.
     */
    fun start(query: String?, restarting: Boolean) {
        if (InputRestart.keepsView(started, restarting, query, draftQuery)) return
        started = true
        draftQuery = query
        likes.clear()
        tabs.removeAllTabs()
        for (t in Tab.entries) {
            if (t == Tab.RESULTS && query == null) continue
            tabs.addTab(tabs.newTab().setText(title(t)).setTag(t), false)
        }
        tabs.getTabAt(0)?.select()
    }

    /** The picker was hidden: stop loading. */
    fun stop() {
        started = false
        loadJob?.cancel()
        loading = false
        updateProgress()
    }

    /** The view is gone for good. */
    fun dispose() {
        job.cancel()
        status.removeCallbacks(hideStatus)
        thumbnails.clear()
    }

    /** An insert is running: show it, and take no other pick. */
    fun setBusy(value: Boolean) {
        busy = value
        updateProgress()
    }

    fun showStatus(@StringRes text: Int) {
        status.setText(text)
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
        if (tab == Tab.LIKED && !liked && items.any { it.id == item.id }) {
            display(Shown.Items(items.filter { it.id != item.id }, false, context.getString(R.string.ime_empty_liked)))
        }
        showStatus(if (liked) R.string.ime_liked else R.string.ime_unliked)
    }

    private fun show(t: Tab) {
        tab = t
        inPack = false
        setHeader.visibility = View.GONE
        when (t) {
            Tab.RESULTS -> {
                val query = draftQuery ?: return
                load {
                    val list = library.search(query)
                    Shown.Items(list.items, list.offline, context.getString(R.string.ime_empty_results, query))
                }
            }
            Tab.RECENT -> load {
                val recent = library.recent()
                if (recent.isEmpty() && !library.signedIn()) {
                    Shown.Failed(LibraryErrorKind.NOT_SIGNED_IN)
                } else {
                    Shown.Items(recent, false, context.getString(R.string.ime_empty_recent))
                }
            }
            Tab.LIKED -> load {
                val list = library.favorites()
                list.items.forEach { likes[it.id] = true }
                Shown.Items(list.items, list.offline, context.getString(R.string.ime_empty_liked))
            }
            Tab.SETS -> load {
                val list = library.packs()
                Shown.Packs(list.packs, list.offline)
            }
        }
    }

    private fun openPack(pack: Pack) {
        inPack = true
        setName.text = pack.name
        setHeader.visibility = View.VISIBLE
        load {
            val list = library.packItems(pack.id)
            Shown.Items(list.items, list.offline, context.getString(R.string.ime_empty_set))
        }
    }

    private fun load(block: suspend () -> Shown) {
        loadJob?.cancel()
        loading = true
        updateProgress()
        messageBox.visibility = View.GONE
        items = emptyList()
        updateFilters()
        itemsAdapter.submitList(emptyList())
        packsAdapter.submitList(emptyList())
        loadJob = scope.launch {
            val shown = try {
                block()
            } catch (e: LibraryException) {
                Shown.Failed(e.kind)
            }
            loading = false
            updateProgress()
            display(shown)
        }
    }

    private fun display(shown: Shown) {
        when (shown) {
            is Shown.Items -> {
                items = shown.items
                use(gridLayout, itemsAdapter)
                updateFilters()
                itemsAdapter.submitList(items)
                if (items.isEmpty()) showMessage(shown.empty) else messageBox.visibility = View.GONE
                if (shown.offline) showStatus(R.string.ime_offline)
            }
            is Shown.Packs -> {
                items = emptyList()
                updateFilters()
                use(listLayout, packsAdapter)
                packsAdapter.submitList(shown.packs)
                if (shown.packs.isEmpty()) {
                    showMessage(context.getString(R.string.ime_empty_sets))
                } else {
                    messageBox.visibility = View.GONE
                }
                if (shown.offline) showStatus(R.string.ime_offline)
            }
            is Shown.Failed -> {
                items = emptyList()
                updateFilters()
                itemsAdapter.submitList(emptyList())
                packsAdapter.submitList(emptyList())
                showMessage(context.getString(ImeTexts.forError(shown.kind)), openApp = ImeTexts.isSignedOut(shown.kind))
            }
        }
    }

    private fun use(layout: RecyclerView.LayoutManager, adapter: RecyclerView.Adapter<*>) {
        if (grid.adapter !== adapter) {
            grid.layoutManager = layout
            grid.adapter = adapter
        }
    }

    /** The emoji filters of the items shown; none selected. */
    private fun updateFilters() {
        filters.clearCheck()
        filters.removeAllViews()
        val top = EmojiFilters.top(items.map { it.emojis })
        val inflater = LayoutInflater.from(context)
        for (emoji in top) {
            val chip = inflater.inflate(R.layout.ime_filter_chip, filters, false) as Chip
            chip.id = View.generateViewId()
            chip.text = emoji
            chip.tag = emoji
            filters.addView(chip)
        }
        filtersScroll.visibility = if (top.isEmpty()) View.GONE else View.VISIBLE
    }

    private fun showMessage(text: String, openApp: Boolean = false) {
        message.text = text
        messageAction.visibility = if (openApp) View.VISIBLE else View.GONE
        messageBox.visibility = View.VISIBLE
    }

    private fun updateProgress() {
        progress.visibility = if (loading || busy) View.VISIBLE else View.GONE
    }

    private fun onItemClick(item: Item) {
        if (!busy) actions.pick(item, sticker = false, fromDraft = tab == Tab.RESULTS)
    }

    private fun onItemLongClick(item: Item, anchor: View) {
        if (busy) return
        val fromDraft = tab == Tab.RESULTS
        val liked = likes[item.id] ?: false
        val popup = PopupMenu(context, anchor)
        popup.menu.add(Menu.NONE, MENU_LIKE, 0, if (liked) R.string.ime_menu_unlike else R.string.ime_menu_like)
        popup.menu.add(Menu.NONE, MENU_STICKER, 1, R.string.ime_menu_send_sticker)
        popup.menu.add(Menu.NONE, MENU_SHARE, 2, R.string.ime_menu_share)
        popup.setOnMenuItemClickListener { menuItem ->
            when (menuItem.itemId) {
                MENU_LIKE -> actions.toggleLike(item)
                MENU_STICKER -> actions.pick(item, sticker = true, fromDraft = fromDraft)
                MENU_SHARE -> actions.share(item)
            }
            true
        }
        popup.show()
    }

    private fun title(t: Tab): String = when (t) {
        Tab.RESULTS -> context.getString(R.string.ime_tab_results, draftQuery.orEmpty())
        Tab.RECENT -> context.getString(R.string.ime_tab_recent)
        Tab.LIKED -> context.getString(R.string.ime_tab_liked)
        Tab.SETS -> context.getString(R.string.ime_tab_sets)
    }

    /** Columns of the grid: about 84 dp each, at least three. */
    private fun spanCount(): Int = maxOf(3, context.resources.configuration.screenWidthDp / 84)

    private companion object {
        const val STATUS_MILLIS = 5_000L
        const val MENU_LIKE = 1
        const val MENU_STICKER = 2
        const val MENU_SHARE = 3
    }
}
