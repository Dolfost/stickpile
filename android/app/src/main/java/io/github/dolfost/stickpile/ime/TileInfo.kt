package io.github.dolfost.stickpile.ime

import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.display.ItemLabels
import io.github.dolfost.stickpile.display.TileField
import io.github.dolfost.stickpile.rust.Item

/** A line of text at the bottom of a tile, in the order they are drawn. */
enum class TileLine { NAME, EMOJIS_VIBE, TAGS, TEXT, STATS }

/**
 * What a tile of the picker's grid shows over the thumbnail, following
 * [Display]: the lines that have something to show, as many as fit the
 * cell without hiding the sticker. Pure.
 */
object TileInfo {
    /**
     * Which lines stay when not all fit: the first ones here. The words in
     * the picture go first, as the picture shows them anyway.
     */
    private val PRIORITY = listOf(TileLine.NAME, TileLine.STATS, TileLine.TAGS, TileLine.EMOJIS_VIBE, TileLine.TEXT)

    /** The share of a cell's height the lines may cover. */
    private const val COVER = 0.45f

    /** The height of a line at a font scale of 1, in dp. */
    private const val LINE_DP = 12f

    /** The most lines a cell [cellDp] wide (and tall) shows, at [fontScale]. */
    fun maxLines(cellDp: Int, fontScale: Float): Int =
        (cellDp * COVER / (LINE_DP * fontScale)).toInt().coerceIn(0, TileLine.entries.size)

    /** The lines [item]'s tile shows, at most [maxLines], in drawing order. */
    fun lines(item: Item, display: Display, maxLines: Int): List<TileLine> {
        val wanted = TileLine.entries.filter { has(item, display, it) }
        val kept = PRIORITY.filter { it in wanted }.take(maxLines).toSet()
        return wanted.filter { it in kept }
    }

    /** The emojis, then the vibe: "😂🐱 sarcastic"; null for none shown. */
    fun emojisAndVibe(item: Item, display: Display): String? = listOfNotNull(
        ItemLabels.emojis(item)?.takeIf { display.shows(TileField.EMOJIS) },
        ItemLabels.vibe(item)?.takeIf { display.shows(TileField.VIBE) },
    ).joinToString(" ").ifEmpty { null }

    private fun has(item: Item, display: Display, line: TileLine): Boolean = when (line) {
        TileLine.NAME -> display.shows(TileField.NAME) && item.name.isNotBlank()
        TileLine.EMOJIS_VIBE -> emojisAndVibe(item, display) != null
        TileLine.TAGS -> display.shows(TileField.TAGS) && ItemLabels.tags(item) != null
        TileLine.TEXT -> display.shows(TileField.TEXT) && ItemLabels.text(item) != null
        TileLine.STATS -> display.shows(TileField.SIZE) || ItemLabels.stats(item, display).isNotEmpty()
    }
}
