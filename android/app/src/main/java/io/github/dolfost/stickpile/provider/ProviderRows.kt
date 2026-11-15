package io.github.dolfost.stickpile.provider

import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.MediaKind
import io.github.dolfost.stickpile.rust.Pack

/**
 * The columns and rows of the provider's cursors (docs/android.md,
 * "Provider contract", "Queries"). Every cursor has `_id`, the row number,
 * so that list adapters work.
 */
object ProviderRows {
    val ITEM_COLUMNS = arrayOf(
        "_id", "item_id", "name", "set_name", "emojis", "kind", "width", "height", "thumbnail_uri",
    )

    val PACK_COLUMNS = arrayOf("_id", "pack_id", "name", "owner", "item_count")

    /** `_display_name` and `_size` (`OpenableColumns`) of a prepared file. */
    val OPENABLE_COLUMNS = arrayOf(DISPLAY_NAME, SIZE)

    fun item(row: Int, item: Item): Array<Any?> = arrayOf(
        row.toLong(),
        item.id,
        item.name,
        item.setName,
        item.emojis.joinToString(""),
        kindName(item.kind),
        item.width.toLong(),
        item.height.toLong(),
        ProviderRoutes.thumbnailUri(item.id),
    )

    fun pack(row: Int, pack: Pack): Array<Any?> = arrayOf(
        row.toLong(),
        pack.id,
        pack.name,
        pack.owner,
        pack.itemCount.toLong(),
    )

    /**
     * The `OpenableColumns` row of a prepared file, as many apps ask for a
     * content URI's name and size: the requested columns that are known,
     * in the requested order (all when [projection] is null).
     */
    fun openable(projection: Array<out String>?, fileName: String, size: Long): Pair<Array<String>, Array<Any?>> {
        val columns = (projection ?: OPENABLE_COLUMNS).filter { it in OPENABLE_COLUMNS }.distinct()
        val values = columns.map { if (it == DISPLAY_NAME) fileName else size }
        return columns.toTypedArray() to values.toTypedArray<Any?>()
    }

    fun kindName(kind: MediaKind): String = when (kind) {
        MediaKind.STILL -> "still"
        MediaKind.ANIMATION -> "animation"
        MediaKind.VIDEO -> "video"
    }

    // The values of android.provider.OpenableColumns, here so that the JVM
    // tests need no Android class.
    private const val DISPLAY_NAME = "_display_name"
    private const val SIZE = "_size"
}
