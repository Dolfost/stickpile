package io.github.dolfost.stickpile.ime

import android.view.LayoutInflater
import android.view.View
import android.widget.ImageView
import android.widget.TextView
import androidx.annotation.PluralsRes
import androidx.core.view.isNotEmpty
import androidx.core.view.isVisible
import com.google.android.material.button.MaterialButton
import com.google.android.material.chip.Chip
import com.google.android.material.chip.ChipGroup
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.ItemLabels
import io.github.dolfost.stickpile.rust.Item
import kotlinx.coroutines.Job

/**
 * A sticker's details over the picker's grid, opened by a long press
 * (`ime_details.xml`): a bigger thumbnail, its name, set and owner, format
 * and size, the numbers, and its vibe, emojis and tags as chips that
 * search for them; and the buttons to send it (as a picture or as a
 * sticker), like or unlike it, share it, or close the panel. It shows all
 * of it, whatever the tiles show.
 */
internal class DetailsPanel(
    private val view: View,
    private val thumbnails: ThumbnailLoader,
    private val listener: Listener,
) {
    interface Listener {
        fun send(item: Item, sticker: Boolean)
        fun toggleLike(item: Item)
        fun share(item: Item)

        /** A chip was tapped: search for [query]. */
        fun search(query: String)

        /** The panel no longer shows [item]. */
        fun closed(item: Item)
    }

    private val context = view.context
    private val image: ImageView = view.findViewById(R.id.ime_details_image)
    private val name: TextView = view.findViewById(R.id.ime_details_name)
    private val set: TextView = view.findViewById(R.id.ime_details_set)
    private val facts: TextView = view.findViewById(R.id.ime_details_facts)
    private val chips: ChipGroup = view.findViewById(R.id.ime_details_chips)
    private val like: MaterialButton = view.findViewById(R.id.ime_details_like)
    private var thumbnailJob: Job? = null

    /** The item shown; null while the panel is closed. */
    var item: Item? = null
        private set

    init {
        view.findViewById<View>(R.id.ime_details_send).setOnClickListener { item?.let { listener.send(it, false) } }
        view.findViewById<View>(R.id.ime_details_sticker).setOnClickListener { item?.let { listener.send(it, true) } }
        view.findViewById<View>(R.id.ime_details_share).setOnClickListener { item?.let(listener::share) }
        view.findViewById<View>(R.id.ime_details_close).setOnClickListener { close() }
        like.setOnClickListener { item?.let(listener::toggleLike) }
    }

    fun show(item: Item, liked: Boolean) {
        if (this.item?.id != item.id) {
            thumbnailJob?.cancel()
            thumbnailJob = thumbnails.bind(item.id, image)
        }
        this.item = item
        name.text = item.name
        set.text = context.getString(R.string.display_pack_by, item.setName, item.owner)
        showLike(item, liked)
        showChips(item)
        view.isVisible = true
    }

    /** The like of [itemId] changed: shown when the panel shows it. */
    fun likeChanged(itemId: String, liked: Boolean) {
        val shown = item?.takeIf { it.id == itemId } ?: return
        showLike(shown, liked)
    }

    fun close() {
        val shown = item ?: return
        item = null
        view.isVisible = false
        thumbnailJob?.cancel()
        thumbnailJob = null
        thumbnails.unbind(image)
        listener.closed(shown)
    }

    private fun showLike(item: Item, liked: Boolean) {
        facts.text = facts(item, liked)
        like.setIconResource(if (liked) R.drawable.ic_heart else R.drawable.ic_heart_outline)
        val label = context.getString(if (liked) R.string.ime_menu_unlike else R.string.ime_menu_like)
        like.contentDescription = label
        like.tooltipText = label
    }

    /** The format and size, times sent, likes, quality, and the account's like; a line each. */
    private fun facts(item: Item, liked: Boolean): String = listOfNotNull(
        ItemLabels.shape(item),
        if (item.copies == 0uL) context.getString(R.string.display_never_copied) else count(R.plurals.display_copied, item.copies),
        item.likes.takeIf { it > 0u }?.let { count(R.plurals.display_likes, it) },
        item.quality?.let { context.getString(R.string.display_quality, it.toInt()) },
        context.getString(R.string.display_liked_by_you).takeIf { liked },
    ).joinToString("\n")

    private fun count(@PluralsRes plurals: Int, n: ULong): String {
        val count = minOf(n, Int.MAX_VALUE.toULong()).toInt()
        return context.resources.getQuantityString(plurals, count, count)
    }

    private fun showChips(item: Item) {
        chips.removeAllViews()
        val inflater = LayoutInflater.from(context)
        fun chip(label: String, query: String) {
            val chip = inflater.inflate(R.layout.ime_detail_chip, chips, false) as Chip
            chip.text = label
            chip.setOnClickListener { listener.search(query) }
            chips.addView(chip)
        }
        ItemLabels.vibe(item)?.let { chip(it, PickerSearch.filterQuery("vibe", it)) }
        for (emoji in item.emojis.map { it.trim() }.filter { it.isNotEmpty() }.distinct()) {
            chip(emoji, PickerSearch.filterQuery("e", emoji))
        }
        for (tag in item.tags.map { it.trim() }.filter { it.isNotEmpty() }.distinct()) {
            chip("#$tag", PickerSearch.filterQuery("tag", tag))
        }
        chips.isVisible = chips.isNotEmpty()
    }
}
