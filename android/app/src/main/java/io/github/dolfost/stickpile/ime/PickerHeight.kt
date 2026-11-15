package io.github.dolfost.stickpile.ime

/**
 * How tall the picker's grid is: its usual height scaled by the
 * keyboard height setting ([ImeSettings.heightPercent]), in steps, and
 * never more than half the screen (a phone held sideways). Pure.
 */
object PickerHeight {
    const val DEFAULT = 100
    const val MIN = 60
    const val MAX = 160
    const val STEP = 10

    /** The share of the screen's height the grid takes at most, whatever the setting. */
    const val MAX_SCREEN_SHARE = 0.5f

    /** [percent] on the nearest step, within [MIN] and [MAX]. */
    fun clamp(percent: Int): Int = ((percent.coerceIn(MIN, MAX) - MIN + STEP / 2) / STEP * STEP + MIN).coerceAtMost(MAX)

    /**
     * The grid's height for [percent]: [basePx] (its height at 100%)
     * scaled, at most [MAX_SCREEN_SHARE] of [screenPx], at least [minPx].
     */
    fun scaled(basePx: Int, percent: Int, screenPx: Int, minPx: Int): Int {
        val wanted = (basePx.toLong() * clamp(percent) / 100).toInt()
        val cap = (screenPx * MAX_SCREEN_SHARE).toInt()
        return wanted.coerceAtMost(cap).coerceAtLeast(minPx)
    }
}
