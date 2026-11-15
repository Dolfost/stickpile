package io.github.dolfost.stickpile.ime

import android.view.LayoutInflater
import android.view.View
import androidx.core.view.isNotEmpty
import androidx.core.view.isVisible
import com.google.android.material.chip.Chip
import com.google.android.material.chip.ChipGroup
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.SearchHint

/**
 * The row of chips under the picker's search bar: the offers for the query
 * ([SearchHint]s, the one the search uses checked) while typing or
 * searching, else the emoji filters of the items shown ([EmojiFilters]).
 * One shows at a time, so the picker keeps its height.
 */
internal class ChipRows(
    root: View,
    private val onOffer: (SearchHint) -> Unit,
    private val onFilter: (emoji: String?) -> Unit,
) {
    private val inflater = LayoutInflater.from(root.context)
    private val offersScroll: View = root.findViewById(R.id.ime_hints_scroll)
    private val offers: ChipGroup = root.findViewById(R.id.ime_hints)
    private val filtersScroll: View = root.findViewById(R.id.ime_filters_scroll)
    private val filters: ChipGroup = root.findViewById(R.id.ime_filters)
    private var offering = false

    init {
        filters.setOnCheckedStateChangeListener { group, checked ->
            onFilter(checked.firstOrNull()?.let { group.findViewById<Chip>(it)?.tag as? String })
        }
    }

    /** The offers show ([on]), or the filters; going to the offers clears the filter. */
    fun offer(on: Boolean) {
        offering = on
        update()
    }

    fun showOffers(hints: List<SearchHint>) {
        offers.removeAllViews()
        for (hint in hints) {
            val chip = inflater.inflate(R.layout.ime_hint_chip, offers, false) as Chip
            chip.text = hint.label
            chip.isCheckable = true
            chip.isChecked = hint.chosen
            chip.setOnClickListener { onOffer(hint) }
            offers.addView(chip)
        }
        offersScroll.scrollTo(0, 0)
        update()
    }

    /** The filters for the items with these emoji lists; none selected. */
    fun showFilters(emojiLists: List<List<String>>) {
        filters.clearCheck()
        filters.removeAllViews()
        for (emoji in EmojiFilters.top(emojiLists)) {
            val chip = inflater.inflate(R.layout.ime_filter_chip, filters, false) as Chip
            chip.id = View.generateViewId()
            chip.text = emoji
            chip.tag = emoji
            filters.addView(chip)
        }
        update()
    }

    private fun update() {
        if (offering && filters.checkedChipId != View.NO_ID) filters.clearCheck()
        offersScroll.isVisible = offering && offers.isNotEmpty()
        filtersScroll.isVisible = !offering && filters.isNotEmpty()
    }
}
