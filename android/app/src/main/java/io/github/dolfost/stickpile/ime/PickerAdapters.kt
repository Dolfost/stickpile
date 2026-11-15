package io.github.dolfost.stickpile.ime

import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.ImageView
import android.widget.TextView
import androidx.recyclerview.widget.DiffUtil
import androidx.recyclerview.widget.ListAdapter
import androidx.recyclerview.widget.RecyclerView
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.Pack
import kotlinx.coroutines.Job

/** The grid of stickers. */
internal class ItemsAdapter(
    private val thumbnails: ThumbnailLoader,
    private val onClick: (Item) -> Unit,
    private val onLongClick: (Item, View) -> Unit,
) : ListAdapter<Item, ItemsAdapter.Holder>(DIFF) {

    class Holder(view: View) : RecyclerView.ViewHolder(view) {
        val image: ImageView = view.findViewById(R.id.ime_item_image)
        var job: Job? = null
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): Holder =
        Holder(LayoutInflater.from(parent.context).inflate(R.layout.ime_item, parent, false))

    override fun onBindViewHolder(holder: Holder, position: Int) {
        val item = getItem(position)
        holder.job?.cancel()
        holder.itemView.contentDescription = item.name
        holder.itemView.setOnClickListener { onClick(item) }
        holder.itemView.setOnLongClickListener {
            onLongClick(item, it)
            true
        }
        holder.job = thumbnails.bind(item.id, holder.image)
    }

    override fun onViewRecycled(holder: Holder) {
        holder.job?.cancel()
        holder.job = null
        thumbnails.unbind(holder.image)
    }

    private companion object {
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
