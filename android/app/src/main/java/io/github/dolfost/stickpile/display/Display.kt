package io.github.dolfost.stickpile.display

import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.MediaFormat
import io.github.dolfost.stickpile.rust.MediaKind

/**
 * One piece of information the item tiles can show over the thumbnail, in
 * the app and in the keyboard alike (the desktop's `zf`, `ze`, `zt`, `zv`
 * and `zi`, split finer). Each can be hidden on its own.
 */
enum class TileField {
    /** The kind of a moving item (GIF, WEBP, VIDEO) and the NSFW mark, in the top corner. */
    BADGES,

    /** A heart on the items the account liked ([Item.liked]). */
    LIKED,
    NAME,
    EMOJIS,

    /** The item's vibe label ("sarcastic"). */
    VIBE,
    TAGS,

    /** The format and resolution: "PNG 512x512". */
    SIZE,

    /** Times it was sent, by all users ([Item.copies]). */
    COPIES,

    /** Likes by all users ([Item.likes]). */
    LIKES,

    /** The server's quality score ([Item.quality]). */
    QUALITY,
}

/** How large the tiles are: the smallest width of a grid column. */
enum class TileSize(val minDp: Int) {
    SMALL(80),
    MEDIUM(112),
    LARGE(150),
}

/**
 * When the GIFs, animated WebPs and videos move in the grids and the
 * details, instead of showing the still of their first frame. Playing one
 * downloads its file the first time.
 */
enum class Autoplay {
    ALWAYS,

    /** Downloads only on a network that is not metered; what is cached plays anywhere. */
    WIFI,
    NEVER,
}

/**
 * What the grids show, shared by the app and the keyboard
 * ([DisplaySettings] keeps it).
 *
 * @property safeMode items tagged nsfw are left out of every list and search.
 * @property showInfo the tiles show the [fields] that are on; off hides them all.
 */
data class Display(
    val safeMode: Boolean = false,
    val showInfo: Boolean = true,
    val fields: Set<TileField> = TileField.entries.toSet(),
    val tileSize: TileSize = TileSize.MEDIUM,
    val autoplay: Autoplay = Autoplay.ALWAYS,
) {
    /** The tiles show [field] now. */
    fun shows(field: TileField): Boolean = showInfo && field in fields

    fun with(field: TileField, on: Boolean): Display = copy(fields = if (on) fields + field else fields - field)
}

/**
 * Safe mode, as on the desktop (`S`): items tagged nsfw are hidden from
 * every list, and the search asks the server to leave them out so that the
 * results still fill the list. Pure.
 */
object SafeMode {
    /** The tag safe mode hides, compared without case. */
    const val TAG = "nsfw"

    /** The filter added to a search in safe mode. */
    const val FILTER = "-tag:$TAG"

    /** The query to send: [query] with [FILTER] after it in safe mode (a blank query stays blank). */
    fun query(query: String, safe: Boolean): String {
        val q = query.trim()
        return if (!safe || q.isEmpty()) q else "$q $FILTER"
    }

    /** What safe mode shows of [items], and how many it hides. */
    fun <T> filter(items: List<T>, safe: Boolean, nsfw: (T) -> Boolean): Filtered<T> {
        if (!safe) return Filtered(items, 0)
        val kept = items.filterNot(nsfw)
        return Filtered(kept, items.size - kept.size)
    }

    fun filter(items: List<Item>, safe: Boolean): Filtered<Item> = filter(items, safe) { it.nsfw }

    data class Filtered<T>(val items: List<T>, val hidden: Int)
}

/** One number of the stats line, drawn with its own icon. */
data class Stat(val kind: TileField, val text: String)

/**
 * The texts of an item's tile and details, the way the desktop writes them
 * (`stickpile_quick::model::item_details`), in ASCII. Pure.
 */
object ItemLabels {
    /** The corner badge of anything that moves; null for a still. */
    fun badge(item: Item): String? = when (item.kind) {
        MediaKind.STILL -> null
        MediaKind.ANIMATION -> when (item.format) {
            MediaFormat.GIF -> "GIF"
            MediaFormat.WEBP -> "WEBP"
            else -> "ANIM"
        }
        MediaKind.VIDEO -> "VIDEO"
    }

    /** "PNG", "WEBP ANIM", "VIDEO": the kind of an item for the stats. */
    fun format(item: Item): String = when {
        item.kind == MediaKind.VIDEO -> "VIDEO"
        item.format == MediaFormat.PNG -> "PNG"
        item.format == MediaFormat.JPEG -> "JPEG"
        item.format == MediaFormat.WEBP && item.kind == MediaKind.ANIMATION -> "WEBP ANIM"
        item.format == MediaFormat.WEBP -> "WEBP"
        item.format == MediaFormat.GIF -> "GIF"
        item.format == MediaFormat.MP4 -> "MP4"
        else -> "WEBM"
    }

    /** "512x512"; null when the size is unknown. */
    fun resolution(item: Item): String? =
        if (item.width > 0u && item.height > 0u) "${item.width}x${item.height}" else null

    /** "PNG 512x512", or the format alone when the size is unknown. */
    fun shape(item: Item): String = listOfNotNull(format(item), resolution(item)).joinToString(" ")

    /** The vibe, trimmed; null for none. */
    fun vibe(item: Item): String? = item.vibe?.trim()?.takeIf { it.isNotEmpty() }

    /** The emojis run together: "😂🐱"; null for none. */
    fun emojis(item: Item): String? = item.emojis.map { it.trim() }.filter { it.isNotEmpty() }
        .takeIf { it.isNotEmpty() }?.joinToString("")

    /** The tags as "#reaction #cat"; null for none. */
    fun tags(item: Item): String? = item.tags.map { it.trim() }.filter { it.isNotEmpty() }
        .takeIf { it.isNotEmpty() }?.joinToString(" ") { "#$it" }

    /**
     * The numbers the stats line shows with [display]: times sent and likes
     * (when not zero), quality (when scored); short forms ("1.2k").
     */
    fun stats(item: Item, display: Display): List<Stat> = buildList {
        if (display.shows(TileField.COPIES) && item.copies > 0u) add(Stat(TileField.COPIES, count(item.copies)))
        if (display.shows(TileField.LIKES) && item.likes > 0u) add(Stat(TileField.LIKES, count(item.likes)))
        val q = item.quality
        if (display.shows(TileField.QUALITY) && q != null) add(Stat(TileField.QUALITY, q.toString()))
    }

    /** A count in at most four characters: 999, 1.2k, 12k, 1.2M. */
    fun count(n: ULong): String = when {
        n < 1_000u -> n.toString()
        n < 10_000u -> tenths(n, 1_000u) + "k"
        n < 1_000_000u -> "${n / 1_000u}k"
        n < 10_000_000u -> tenths(n, 1_000_000u) + "M"
        else -> "${n / 1_000_000u}M"
    }

    /** n / unit with one decimal, rounded down, without a trailing ".0". */
    private fun tenths(n: ULong, unit: ULong): String {
        val t = n * 10u / unit
        return if (t % 10u == 0uL) "${t / 10u}" else "${t / 10u}.${t % 10u}"
    }
}
