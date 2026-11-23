package io.github.dolfost.stickpile.ime

import android.text.style.ForegroundColorSpan
import android.text.style.RelativeSizeSpan
import android.view.LayoutInflater
import android.view.View
import androidx.core.text.buildSpannedString
import androidx.core.text.inSpans
import androidx.core.view.isNotEmpty
import androidx.core.view.isVisible
import com.google.android.material.chip.Chip
import com.google.android.material.chip.ChipGroup
import com.google.android.material.color.MaterialColors
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.HintLabel
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

    /** The filters are being set by code: the listener does not narrow the grid. */
    private var quiet = false

    init {
        filters.setOnCheckedStateChangeListener { group, checked ->
            if (!quiet) onFilter(checked.firstOrNull()?.let { group.findViewById<Chip>(it)?.tag as? String })
        }
    }

    /** The emoji filter selected now; null: none. */
    val checkedFilter: String?
        get() = filters.findViewById<Chip>(filters.checkedChipId)?.tag as? String

    /** The offers show ([on]), or the filters; going to the offers clears the filter. */
    fun offer(on: Boolean) {
        offering = on
        update()
    }

    fun showOffers(hints: List<SearchHint>) {
        offers.removeAllViews()
        for (hint in hints) {
            val chip = inflater.inflate(R.layout.ime_hint_chip, offers, false) as Chip
            chip.text = offerText(chip, HintLabel.of(hint))
            chip.isCheckable = true
            chip.isChecked = hint.chosen
            chip.setOnClickListener { onOffer(hint) }
            offers.addView(chip)
        }
        offersScroll.scrollTo(0, 0)
        update()
    }

    /**
     * The filters for the items with these emoji lists; none selected, or
     * [keep] when it is still one of them (a list replaced in place keeps
     * what was chosen). Returns the filter selected; the caller narrows the
     * grid with it (the listener is not called).
     */
    fun showFilters(emojiLists: List<List<String>>, keep: String? = null): String? {
        quiet = true
        try {
            filters.clearCheck()
            filters.removeAllViews()
            var kept: String? = null
            for (emoji in EmojiFilters.top(emojiLists)) {
                val chip = inflater.inflate(R.layout.ime_filter_chip, filters, false) as Chip
                chip.id = View.generateViewId()
                chip.text = emoji
                chip.tag = emoji
                filters.addView(chip)
                if (emoji == keep && !offering) {
                    filters.check(chip.id)
                    kept = emoji
                }
            }
            update()
            return kept
        } finally {
            quiet = false
        }
    }

    /** An offer's text: an emoji of `e:` with its name after it, smaller and dimmer. */
    private fun offerText(chip: Chip, label: HintLabel): CharSequence {
        val name = label.name ?: return label.text
        val dim = MaterialColors.getColor(chip, com.google.android.material.R.attr.colorOnSurfaceVariant)
        return buildSpannedString {
            append(label.text)
            append(' ')
            inSpans(RelativeSizeSpan(NAME_SIZE), ForegroundColorSpan(dim)) { append(name) }
        }
    }

    private fun update() {
        if (offering && filters.checkedChipId != View.NO_ID) filters.clearCheck()
        offersScroll.isVisible = offering && offers.isNotEmpty()
        filtersScroll.isVisible = !offering && filters.isNotEmpty()
    }

    private companion object {
        /** An emoji's name, relative to the chip's text. */
        const val NAME_SIZE = 0.85f
    }
}
