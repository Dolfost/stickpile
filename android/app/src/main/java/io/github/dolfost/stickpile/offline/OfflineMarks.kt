package io.github.dolfost.stickpile.offline

import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.display.TileField

/** What a tile shows of keeping offline (docs/offline.md, 4.8). */
enum class OfflineMark {
    NONE,

    /** The server answers, and the sticker would also work without it (a tile mark that can be turned off). */
    AVAILABLE,

    /** The server is unreachable, and the sticker's file is not here: dimmed, with its mark. */
    UNAVAILABLE,
}

/**
 * Which stickers of a list work without the server (the library's
 * `availableOffline`), and whether the server is unreachable now (the
 * list came from the cache, or the library found it away lately). Pure.
 *
 * @property known the library answered for this list; until then no tile is marked.
 */
data class OfflineMarks(
    val available: Set<String> = emptySet(),
    val unreachable: Boolean = false,
    val known: Boolean = false,
) {
    /** The mark of [itemId]'s tile with [display]. */
    fun of(itemId: String, display: Display): OfflineMark = when {
        !known -> OfflineMark.NONE
        unreachable -> if (itemId in available) OfflineMark.NONE else OfflineMark.UNAVAILABLE
        itemId in available && display.shows(TileField.OFFLINE) -> OfflineMark.AVAILABLE
        else -> OfflineMark.NONE
    }

    companion object {
        /** From the library's answer: [available] is parallel to [ids]. */
        fun of(ids: List<String>, available: List<Boolean>, unreachable: Boolean): OfflineMarks = OfflineMarks(
            available = ids.zip(available).filter { it.second }.map { it.first }.toSet(),
            unreachable = unreachable,
            known = true,
        )
    }
}
