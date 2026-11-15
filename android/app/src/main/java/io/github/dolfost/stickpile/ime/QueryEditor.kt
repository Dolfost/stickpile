package io.github.dolfost.stickpile.ime

/**
 * Edits the picker's search query from its keys ([SearchKeys]): typing
 * appends, backspace takes off the last character as a person sees it (a
 * whole code point, an emoji with its modifiers, joiners and selectors, a
 * flag, a letter with its combining marks). The query never reaches the
 * editor's field. Pure.
 */
object QueryEditor {
    /** The longest query typed, in UTF-16 units. */
    const val MAX_LENGTH = 100

    private const val ZWJ = 0x200D

    /**
     * [query] with [text] typed at its end. A space at the start or after
     * another space is left out, and so is anything past [MAX_LENGTH].
     */
    fun type(query: String, text: String): String {
        if (text == " " && (query.isEmpty() || query.endsWith(" "))) return query
        val typed = query + text
        return if (typed.length > MAX_LENGTH) query else typed
    }

    /** [query] without its last character. */
    fun backspace(query: String): String = query.substring(0, lastCharacterStart(query))

    /** Where the last character as a person sees it starts in [text]: 0 for an empty text. */
    fun lastCharacterStart(text: String): Int {
        var i = text.length
        if (i == 0) return 0
        if (isRegionalIndicator(text.codePointBefore(i))) return flagStart(text)
        while (true) {
            while (i > 0 && isExtender(text.codePointBefore(i))) i -= Character.charCount(text.codePointBefore(i))
            if (i > 0) i -= Character.charCount(text.codePointBefore(i))
            if (i > 1 && text.codePointBefore(i) == ZWJ) {
                i -= 1
                continue
            }
            return i
        }
    }

    /** A run of regional indicators ends the text: they pair up into flags from its start. */
    private fun flagStart(text: String): Int {
        var i = text.length
        var count = 0
        while (i > 0 && isRegionalIndicator(text.codePointBefore(i))) {
            i -= Character.charCount(text.codePointBefore(i))
            count++
        }
        val last = if (count % 2 == 0) 2 else 1
        var start = text.length
        repeat(last) { start -= Character.charCount(text.codePointBefore(start)) }
        return start
    }

    private fun isRegionalIndicator(cp: Int): Boolean = cp in 0x1F1E6..0x1F1FF

    /** A code point that belongs to the one before it. */
    private fun isExtender(cp: Int): Boolean = when {
        cp == 0xFE0E || cp == 0xFE0F -> true // variation selectors
        cp in 0x1F3FB..0x1F3FF -> true // skin tones
        cp == 0x20E3 -> true // keycap
        cp in 0xE0020..0xE007F -> true // tags (subdivision flags)
        cp == ZWJ -> false
        else -> when (Character.getType(cp).toByte()) {
            Character.NON_SPACING_MARK, Character.ENCLOSING_MARK, Character.COMBINING_SPACING_MARK -> true
            else -> false
        }
    }
}
