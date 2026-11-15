package io.github.dolfost.stickpile.motion

/**
 * At most [max] holders at once, the others waiting in turn: the videos
 * that play at the same time (each holds a hardware decoder, of which a
 * phone has few). Not thread safe: used on the main thread.
 */
class Slots<T : Any>(private val max: Int) {
    private val holding = LinkedHashSet<T>()
    private val waiting = LinkedHashSet<T>()

    /** Whether [who] holds a slot now; if not, it waits for one. */
    fun take(who: T): Boolean {
        if (who in holding) return true
        if (holding.size < max) {
            waiting.remove(who)
            holding.add(who)
            return true
        }
        waiting.add(who)
        return false
    }

    /**
     * [who] gives its slot back, or stops waiting. Returns the one that
     * waited longest when a slot came free (it holds it now), else null.
     */
    fun give(who: T): T? {
        waiting.remove(who)
        if (!holding.remove(who)) return null
        val next = waiting.firstOrNull() ?: return null
        waiting.remove(next)
        holding.add(next)
        return next
    }

    fun holds(who: T): Boolean = who in holding

    val held: Int get() = holding.size
}
