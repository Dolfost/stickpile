package io.github.dolfost.stickpile.ime

import android.content.Context
import android.graphics.drawable.Drawable
import android.os.Build
import android.text.SpannableStringBuilder
import android.text.Spanned
import android.text.style.DynamicDrawableSpan
import android.text.style.ImageSpan
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.ImageView
import android.widget.TextView
import androidx.core.content.ContextCompat
import androidx.core.view.isVisible
import androidx.recyclerview.widget.DiffUtil
import androidx.recyclerview.widget.ListAdapter
import androidx.recyclerview.widget.RecyclerView
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.display.ItemLabels
import io.github.dolfost.stickpile.display.TileField
import io.github.dolfost.stickpile.display.TileIcons
import io.github.dolfost.stickpile.motion.LoopingVideoView
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.Pack
import kotlinx.coroutines.Job

/**
 * What the tiles show over the thumbnails, read on every bind: the
 * [display] settings, how many lines fit a cell ([maxLines]) and whether
 * the account liked an item ([likedOf]). After a change,
 * [ItemsAdapter.refreshTiles] binds them again.
 */
internal class TileLook(context: Context) {
    var display: Display = Display()
    var maxLines: Int = TileLine.entries.size

    /** The side of a grid cell, in pixels: what a moving picture is decoded at, at most. */
    var cellPx: Int = DecodeSize.MAX_THUMBNAIL_PX
    var likedOf: (Item) -> Boolean = { it.liked }

    private val iconPx = context.resources.getDimensionPixelSize(R.dimen.ime_tile_icon)
    private val icons: Map<TileField, Drawable> = listOf(TileField.COPIES, TileField.LIKES, TileField.QUALITY)
        .associateWith { field ->
            requireNotNull(ContextCompat.getDrawable(context, TileIcons.stat(field))).mutate().apply {
                setBounds(0, 0, iconPx, iconPx)
            }
        }

    /**
     * The stats line: the format and size, then each number after its icon
     * ("PNG 512x512  [copies]12  [heart]3").
     */
    fun stats(item: Item): CharSequence {
        val out = SpannableStringBuilder()
        if (display.shows(TileField.SIZE)) out.append(ItemLabels.shape(item))
        for (stat in ItemLabels.stats(item, display)) {
            if (out.isNotEmpty()) out.append("  ")
            icons[stat.kind]?.let { out.append(ICON_PLACEHOLDER, ImageSpan(it, ICON_ALIGN), SPAN_FLAGS) }
            out.append(stat.text)
        }
        return out
    }

    private companion object {
        const val ICON_PLACEHOLDER = "*"
        const val SPAN_FLAGS = Spanned.SPAN_EXCLUSIVE_EXCLUSIVE
        val ICON_ALIGN = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            DynamicDrawableSpan.ALIGN_CENTER
        } else {
            DynamicDrawableSpan.ALIGN_BASELINE
        }
    }
}

/** The grid of stickers, with what [look] shows over each. */
internal class ItemsAdapter(
    private val thumbnails: ThumbnailLoader,
    private val motions: MotionBinder,
    private val look: TileLook,
    private val onClick: (Item) -> Unit,
    private val onLongClick: (Item) -> Unit,
) : ListAdapter<Item, ItemsAdapter.Holder>(DIFF) {

    class Holder(view: View) : RecyclerView.ViewHolder(view) {
        val image: ImageView = view.findViewById(R.id.ime_item_image)
        val video: LoopingVideoView = view.findViewById(R.id.ime_item_video)
        val badge: ImageView = view.findViewById(R.id.ime_item_badge)
        val nsfw: ImageView = view.findViewById(R.id.ime_item_nsfw)
        val liked: ImageView = view.findViewById(R.id.ime_item_liked)
        val info: View = view.findViewById(R.id.ime_item_info)
        val lines: Map<TileLine, TextView> = mapOf(
            TileLine.NAME to view.findViewById(R.id.ime_item_name),
            TileLine.EMOJIS_VIBE to view.findViewById(R.id.ime_item_emojis),
            TileLine.TAGS to view.findViewById(R.id.ime_item_tags),
            TileLine.STATS to view.findViewById(R.id.ime_item_stats),
        )
        var job: Job? = null
        var motionJob: Job? = null
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): Holder =
        Holder(LayoutInflater.from(parent.context).inflate(R.layout.ime_item, parent, false))

    override fun onBindViewHolder(holder: Holder, position: Int) {
        val item = getItem(position)
        holder.job?.cancel()
        holder.motionJob?.cancel()
        holder.itemView.contentDescription = item.name
        holder.itemView.setOnClickListener { onClick(item) }
        holder.itemView.setOnLongClickListener {
            onLongClick(item)
            true
        }
        bindInfo(holder, item)
        holder.job = thumbnails.bind(item.id, holder.image)
        holder.motionJob = motions.bind(
            item,
            holder.image,
            holder.video,
            DecodeSize.thumbnailPx(look.cellPx),
            holder.job,
        )
    }

    override fun onBindViewHolder(holder: Holder, position: Int, payloads: MutableList<Any>) {
        if (payloads.isNotEmpty() && payloads.all { it == INFO }) {
            bindInfo(holder, getItem(position))
        } else {
            onBindViewHolder(holder, position)
        }
    }

    override fun onViewRecycled(holder: Holder) {
        holder.job?.cancel()
        holder.job = null
        holder.motionJob?.cancel()
        holder.motionJob = null
        thumbnails.unbind(holder.image)
        motions.unbind(holder.video)
    }

    /** Every tile binds again, thumbnail and all (whether GIFs and videos play changed). */
    fun rebindAll() {
        notifyItemRangeChanged(0, itemCount)
    }

    /** [look] changed: the tiles show it, the thumbnails stay. */
    fun refreshTiles() {
        notifyItemRangeChanged(0, itemCount, INFO)
    }

    /** The tile of [itemId], if shown, binds [look] again (its like changed). */
    fun refreshTile(itemId: String) {
        val position = currentList.indexOfFirst { it.id == itemId }
        if (position >= 0) notifyItemChanged(position, INFO)
    }

    /** The tile of [itemId], if shown, binds again, thumbnail and all (the details showed it). */
    fun rebind(itemId: String) {
        val position = currentList.indexOfFirst { it.id == itemId }
        if (position >= 0) notifyItemChanged(position)
    }

    private fun bindInfo(holder: Holder, item: Item) {
        val display = look.display
        val badge = ItemLabels.badge(item)?.takeIf { display.shows(TileField.BADGES) }
        if (badge != null) holder.badge.setImageResource(TileIcons.badge(badge))
        holder.badge.isVisible = badge != null
        holder.nsfw.isVisible = display.shows(TileField.BADGES) && item.nsfw && !display.safeMode
        holder.liked.isVisible = display.shows(TileField.LIKED) && look.likedOf(item)
        val lines = TileInfo.lines(item, display, look.maxLines)
        for ((line, view) in holder.lines) {
            val shown = line in lines
            view.isVisible = shown
            view.text = if (!shown) null else when (line) {
                TileLine.NAME -> item.name
                TileLine.EMOJIS_VIBE -> TileInfo.emojisAndVibe(item, display)
                TileLine.TAGS -> ItemLabels.tags(item)
                TileLine.STATS -> look.stats(item)
            }
        }
        holder.info.isVisible = lines.isNotEmpty()
    }

    private companion object {
        /** The payload of a bind that changes only what is shown over the thumbnail. */
        val INFO = Any()

        val DIFF = object : DiffUtil.ItemCallback<Item>() {
            override fun areItemsTheSame(oldItem: Item, newItem: Item) = oldItem.id == newItem.id
            override fun areContentsTheSame(oldItem: Item, newItem: Item) = oldItem == newItem
        }
    }
}

/** The list of sets. */
internal class PacksAdapter(
    private val onClick: (Pack) -> Unit,
) : ListAdapter<Pack, PacksAdapter.Holder>(DIFF) {

    class Holder(view: View) : RecyclerView.ViewHolder(view) {
        val name: TextView = view.findViewById(R.id.ime_pack_name)
        val summary: TextView = view.findViewById(R.id.ime_pack_summary)
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): Holder =
        Holder(LayoutInflater.from(parent.context).inflate(R.layout.ime_pack, parent, false))

    override fun onBindViewHolder(holder: Holder, position: Int) {
        val pack = getItem(position)
        val resources = holder.itemView.resources
        val count = pack.itemCount.toInt()
        holder.name.text = pack.name
        holder.summary.text = resources.getString(
            R.string.ime_set_summary,
            pack.owner,
            resources.getQuantityString(R.plurals.ime_set_items, count, count),
        )
        holder.itemView.setOnClickListener { onClick(pack) }
    }

    private companion object {
        val DIFF = object : DiffUtil.ItemCallback<Pack>() {
            override fun areItemsTheSame(oldItem: Pack, newItem: Pack) = oldItem.id == newItem.id
            override fun areContentsTheSame(oldItem: Pack, newItem: Pack) = oldItem == newItem
        }
    }
}
