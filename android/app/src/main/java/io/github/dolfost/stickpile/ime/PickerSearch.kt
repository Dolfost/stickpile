package io.github.dolfost.stickpile.ime

import io.github.dolfost.stickpile.display.Columns

/** The picker's tabs. */
enum class PickerTab { RESULTS, RECENT, LIKED, MOST_USED, SETS }

/** What the picker's grid loads ([PickerSearch.load]). */
sealed interface PickerLoad {
    /** The search over everything (the Results tab). */
    data class Search(val query: String) : PickerLoad

    /** [query] searched inside the set [setId] only. */
    data class InSet(val setId: String, val query: String) : PickerLoad

    /** The set [setId], all of it. */
    data class WholeSet(val setId: String) : PickerLoad

    /** A tab's own list (Recent, Liked, Most used, the sets). */
    data class Tab(val tab: PickerTab) : PickerLoad
}

/**
 * The picker's decisions about its search, tabs and grid. Pure.
 */
object PickerSearch {
    /** How long typing pauses before the search goes out. */
    const val SEARCH_DELAY_MS = 120L

    /**
     * What the grid shows for [query] on [tab] with the set [setId] open
     * (in Sets; null: none): while a set is open the query searches inside
     * it, and a blank one shows it whole; else the Results tab searches
     * everything and the other tabs show their lists.
     */
    fun load(query: String, tab: PickerTab, setId: String?): PickerLoad {
        val q = query.trim()
        return when {
            setId != null && q.isNotEmpty() -> PickerLoad.InSet(setId, q)
            setId != null -> PickerLoad.WholeSet(setId)
            tab == PickerTab.RESULTS && q.isNotEmpty() -> PickerLoad.Search(q)
            else -> PickerLoad.Tab(tab)
        }
    }

    /** The size of a cell while the search keys show, in dp: small, so that two rows fit above the keys. */
    const val TYPING_CELL_DP = 56

    /** The tabs for [query]: Results first while it is not blank. */
    fun tabs(query: String): List<PickerTab> =
        if (query.isBlank()) PickerTab.entries - PickerTab.RESULTS else PickerTab.entries

    /**
     * Whether a pick from the results counts as found by the draft, so
     * that inserting it deletes the draft: the results shown are the
     * draft's ([resultsFor], null when no results are shown) and the query
     * is still the draft ([draft], null when none was read). Once the
     * query is edited, nothing picked deletes the draft.
     */
    fun fromDraft(draft: String?, query: String, resultsFor: String?): Boolean =
        draft != null && resultsFor != null && query.trim() == draft && resultsFor.trim() == draft

    /**
     * Columns of the grid [widthDp] wide for the setting [columns] (stickers
     * in a row with the phone upright), on a screen whose narrower side is
     * [narrowDp]: [Columns.count].
     */
    fun spanCount(widthDp: Int, columns: Int, narrowDp: Int): Int = Columns.count(columns, widthDp, narrowDp)

    /** Columns while the search keys show: smaller cells than [spanCount] gives, never fewer of them. */
    fun typingSpanCount(widthDp: Int, spanCount: Int): Int = maxOf(spanCount, widthDp / TYPING_CELL_DP)

    /**
     * The query a value in the details makes: `tag:reaction `, `vibe:"very sad" `
     * (quoted when it has a space, as the search syntax reads it).
     */
    fun filterQuery(key: String, value: String): String {
        val v = value.trim()
        return if (v.any(Char::isWhitespace)) "$key:\"$v\" " else "$key:$v "
    }

    /**
     * The query an example of the syntax help puts in the field: the first
     * of a list (`is:mine` of `is:mine, is:liked`), the first key of a row
     * of keys (`vibe:`); a space after it unless it is a key waiting for
     * its value.
     */
    fun exampleQuery(example: String): String {
        val first = example.split(", ").first().trim()
        val words = first.split(" ")
        val q = if (words.size > 1 && words.all { it.endsWith(":") }) words.first() else first
        return if (q.endsWith(":")) q else "$q "
    }
}
