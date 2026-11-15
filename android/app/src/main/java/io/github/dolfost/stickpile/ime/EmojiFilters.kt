package io.github.dolfost.stickpile.ime

/**
 * The row of emoji filters above the grid (docs/android-plan.md 4.7): the
 * most common emojis of the items shown, each narrowing the grid to the
 * items that have it. Pure.
 *
 * Emojis are compared without the emoji presentation selector (U+FE0F), so
 * that a heart with and without it are one filter.
 */
object EmojiFilters {
    /** The most filters shown. */
    const val MAX = 12

    /**
     * The emojis to offer for items with these emoji lists, most items
     * first (ties: the first seen first). An emoji every item has narrows
     * nothing and is left out; so is a blank one.
     */
    fun top(emojiLists: List<List<String>>, max: Int = MAX): List<String> {
        val counts = LinkedHashMap<String, Int>()
        val shown = HashMap<String, String>()
        for (emojis in emojiLists) {
            for (emoji in emojis.filter { key(it).isNotEmpty() }.distinctBy(::key)) {
                val k = key(emoji)
                counts[k] = (counts[k] ?: 0) + 1
                shown.putIfAbsent(k, emoji)
            }
        }
        return counts.entries
            .filter { it.value < emojiLists.size }
            .sortedByDescending { it.value } // stable: ties keep the first seen first
            .take(max)
            .map { shown.getValue(it.key) }
    }

    /** The items that have [emoji]; all of them when it is null. */
    fun <T> narrow(items: List<T>, emoji: String?, emojisOf: (T) -> List<String>): List<T> {
        if (emoji == null) return items
        val k = key(emoji)
        return items.filter { item -> emojisOf(item).any { key(it) == k } }
    }

    private fun key(emoji: String): String = emoji.replace("️", "").trim()
}
