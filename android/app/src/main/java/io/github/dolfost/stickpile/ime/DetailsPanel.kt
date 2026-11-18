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
import io.github.dolfost.stickpile.motion.LoopingVideoView
import io.github.dolfost.stickpile.offline.KeepAction
import io.github.dolfost.stickpile.offline.OfflineLabels
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.KeepReason
import kotlinx.coroutines.Job

/**
 * A sticker's details over the picker's grid, opened by a long press
 * (`ime_details.xml`): a bigger picture (moving, as [MotionBinder] makes
 * it), its name, set and owner, format and size, the numbers, its vibe,
 * emojis and tags as chips that search for them, and the words in the
 * picture (line breaks kept); and the buttons to
 * send it (Send as a tap does, and the other way: as a sticker, or as a
 * picture when a tap sends stickers, [SendChoice]), like or unlike it,
 * keep it offline or stop (why it is kept is among the facts), share it,
 * or close the panel. It shows all of it, whatever the tiles show.
 */
internal class DetailsPanel(
    private val view: View,
    private val thumbnails: ThumbnailLoader,
    private val motions: MotionBinder,
    private val listener: Listener,
) {
    interface Listener {
        fun send(item: Item, sticker: Boolean)
        fun toggleLike(item: Item)
        fun share(item: Item)

        /** Keep [item] offline by itself ([keep]), or stop. */
        fun keep(item: Item, keep: Boolean)

        /** A chip was tapped: search for [query]. */
        fun search(query: String)

        /** The panel no longer shows [item]. */
        fun closed(item: Item)
    }

    private val context = view.context
    private val image: ImageView = view.findViewById(R.id.ime_details_image)
    private val video: LoopingVideoView = view.findViewById(R.id.ime_details_video)
    private val picturePx = context.resources.getDimensionPixelSize(R.dimen.ime_details_picture)
    private val name: TextView = view.findViewById(R.id.ime_details_name)
    private val set: TextView = view.findViewById(R.id.ime_details_set)
    private val facts: TextView = view.findViewById(R.id.ime_details_facts)
    private val chips: ChipGroup = view.findViewById(R.id.ime_details_chips)
    private val textLabel: TextView = view.findViewById(R.id.ime_details_text_label)
    private val text: TextView = view.findViewById(R.id.ime_details_text)
    private val like: MaterialButton = view.findViewById(R.id.ime_details_like)
    private val keep: MaterialButton = view.findViewById(R.id.ime_details_keep)
    private val other: MaterialButton = view.findViewById(R.id.ime_details_other)

    /** "Prefer stickers" as it was when the panel opened: what Send and [other] send. */
    private var preferStickers = false

    /** Why the item shown is kept offline; null until the library said. */
    private var reasons: List<KeepReason>? = null
    private var liked = false
    private var thumbnailJob: Job? = null
    private var motionJob: Job? = null

    /** The item shown; null while the panel is closed. */
    var item: Item? = null
        private set

    init {
        view.findViewById<View>(R.id.ime_details_send).setOnClickListener {
            item?.let { listener.send(it, SendChoice.tap(preferStickers)) }
        }
        other.setOnClickListener { item?.let { listener.send(it, SendChoice.other(preferStickers)) } }
        view.findViewById<View>(R.id.ime_details_share).setOnClickListener { item?.let(listener::share) }
        view.findViewById<View>(R.id.ime_details_close).setOnClickListener { close() }
        like.setOnClickListener { item?.let(listener::toggleLike) }
        keep.setOnClickListener {
            val shown = item ?: return@setOnClickListener
            val why = reasons ?: return@setOnClickListener
            listener.keep(shown, OfflineLabels.stickerAction(why) == KeepAction.KEEP)
        }
    }

    /** Shows [item]; [preferStickers]: a tap sends stickers ([ImeSettings.preferStickers]). */
    fun show(item: Item, liked: Boolean, preferStickers: Boolean) {
        showOther(preferStickers)
        if (this.item?.id != item.id) {
            thumbnailJob?.cancel()
            motionJob?.cancel()
            thumbnailJob = thumbnails.bind(item.id, image)
            motionJob = motions.bind(item, image, video, picturePx, thumbnailJob)
            reasons = null
        }
        this.item = item
        name.text = item.name
        set.text = context.getString(R.string.display_pack_by, item.setName, item.owner)
        showLike(item, liked)
        showKeep()
        showChips(item)
        // The words in the picture as they are, line breaks kept.
        val words = item.text.trim()
        text.text = words
        text.isVisible = words.isNotEmpty()
        textLabel.isVisible = words.isNotEmpty()
        view.isVisible = true
    }

    /** The like of [itemId] changed: shown when the panel shows it. */
    fun likeChanged(itemId: String, liked: Boolean) {
        val shown = item?.takeIf { it.id == itemId } ?: return
        showLike(shown, liked)
    }

    /** Why [itemId] is kept offline (the library's answer): shown when the panel shows it. */
    fun showReasons(itemId: String, reasons: List<KeepReason>) {
        val shown = item?.takeIf { it.id == itemId } ?: return
        this.reasons = reasons
        facts.text = facts(shown, liked)
        showKeep()
    }

    /** The other way to send, as [SendChoice] says for [preferStickers]. */
    private fun showOther(preferStickers: Boolean) {
        this.preferStickers = preferStickers
        other.setIconResource(SendChoice.otherIcon(preferStickers))
        val label = context.getString(SendChoice.otherLabel(preferStickers))
        other.contentDescription = label
        other.tooltipText = label
    }

    /** The keep button as the reasons say: keep, or stop keeping (disabled until known). */
    private fun showKeep() {
        val why = reasons
        keep.isEnabled = why != null
        val action = OfflineLabels.stickerAction(why.orEmpty())
        keep.setIconResource(if (action == KeepAction.KEEP) R.drawable.ic_offline else R.drawable.ic_offline_done)
        val label = context.getString(OfflineLabels.label(action))
        keep.contentDescription = label
        keep.tooltipText = label
    }

    fun close() {
        val shown = item ?: return
        item = null
        view.isVisible = false
        thumbnailJob?.cancel()
        thumbnailJob = null
        motionJob?.cancel()
        motionJob = null
        thumbnails.unbind(image)
        motions.unbind(video)
        listener.closed(shown)
    }

    private fun showLike(item: Item, liked: Boolean) {
        this.liked = liked
        facts.text = facts(item, liked)
        like.setIconResource(if (liked) R.drawable.ic_heart else R.drawable.ic_heart_outline)
        val label = context.getString(if (liked) R.string.ime_menu_unlike else R.string.ime_menu_like)
        like.contentDescription = label
        like.tooltipText = label
    }

    /**
     * The format and size, times sent, likes, quality, the account's like,
     * and why it is kept offline; a line each.
     */
    private fun facts(item: Item, liked: Boolean): String = listOfNotNull(
        ItemLabels.shape(item),
        if (item.copies == 0uL) context.getString(R.string.display_never_copied) else count(R.plurals.display_copied, item.copies),
        item.likes.takeIf { it > 0u }?.let { count(R.plurals.display_likes, it) },
        item.quality?.let { context.getString(R.string.display_quality, it.toInt()) },
        context.getString(R.string.display_liked_by_you).takeIf { liked },
        reasons?.takeIf { it.isNotEmpty() }?.let { why ->
            val words = OfflineLabels.reasons(why).joinToString(", ") { context.getString(it) }
            context.getString(R.string.offline_reasons, words)
        },
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
