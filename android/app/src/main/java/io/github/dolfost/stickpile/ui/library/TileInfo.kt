package io.github.dolfost.stickpile.ui.library

import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.display.ItemLabels
import io.github.dolfost.stickpile.display.Stat
import io.github.dolfost.stickpile.display.TileField
import io.github.dolfost.stickpile.ime.TileInfo as LineFit
import io.github.dolfost.stickpile.ime.TileLine
import io.github.dolfost.stickpile.rust.Item

/**
 * What a grid tile shows over its thumbnail, as [Display] asks: a text is
 * null when its field is off or the item has none. Pure.
 *
 * @property badge the kind of a moving item ("GIF", "VIDEO").
 * @property nsfw the NSFW mark: tagged nsfw with safe mode off (in safe mode the item is not shown).
 * @property mood the emojis and the vibe, on one line.
 * @property shape the format and resolution ("PNG 512x512").
 */
data class TileInfo(
    val badge: String?,
    val nsfw: Boolean,
    val liked: Boolean,
    val name: String?,
    val mood: String?,
    val tags: String?,
    val shape: String?,
    val stats: List<Stat>,
) {
    /** Something to show on the dark band at the bottom. */
    val hasBand: Boolean get() = name != null || mood != null || tags != null || hasStatsLine

    /** The last line of the band: the shape, then the numbers. */
    val hasStatsLine: Boolean get() = shape != null || stats.isNotEmpty()

    companion object {
        /**
         * What [item]'s tile shows with [display], at most [maxLines] lines
         * of text (the ones the keyboard's tiles keep, [LineFit.lines]).
         */
        fun of(item: Item, display: Display, maxLines: Int = TileLine.entries.size): TileInfo {
            val lines = LineFit.lines(item, display, maxLines).toSet()
            fun text(field: TileField, value: () -> String?): String? = if (display.shows(field)) value() else null
            fun <T> on(line: TileLine, value: T): T? = value.takeIf { line in lines }
            val mood = listOfNotNull(
                text(TileField.EMOJIS) { ItemLabels.emojis(item) },
                text(TileField.VIBE) { ItemLabels.vibe(item) },
            )
            return TileInfo(
                badge = text(TileField.BADGES) { ItemLabels.badge(item) },
                nsfw = display.shows(TileField.BADGES) && item.nsfw && !display.safeMode,
                liked = display.shows(TileField.LIKED) && item.liked,
                name = on(TileLine.NAME, text(TileField.NAME) { item.name.trim().takeIf { it.isNotEmpty() } }),
                mood = on(TileLine.EMOJIS_VIBE, mood.takeIf { it.isNotEmpty() }?.joinToString(" ")),
                tags = on(TileLine.TAGS, text(TileField.TAGS) { ItemLabels.tags(item) }),
                shape = on(TileLine.STATS, text(TileField.SIZE) { ItemLabels.shape(item) }),
                stats = on(TileLine.STATS, ItemLabels.stats(item, display)).orEmpty(),
            )
        }
    }
}
