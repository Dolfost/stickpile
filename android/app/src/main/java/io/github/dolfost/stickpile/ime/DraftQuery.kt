package io.github.dolfost.stickpile.ime

import android.text.InputType

/**
 * The text already in the field, used as the search (docs/android-plan.md
 * 4.7): the picker has no typing keys, so the query is the draft.
 *
 * @property query what to search for (trimmed).
 * @property text the field's whole text as read.
 * @property before characters before the cursor (or the selection).
 * @property selected characters selected.
 * @property after characters after the cursor (or the selection).
 */
data class Draft(val query: String, val text: String, val before: Int, val selected: Int, val after: Int)

/** Reads the draft from the text around the cursor. Pure: the service passes what the InputConnection gave. */
object DraftQuery {
    /** The longest draft searched for, in UTF-16 units. */
    const val MAX_LENGTH = 60

    /** How much to ask for on each side of the cursor: one more than fits, to see a longer text. */
    const val READ_LENGTH = MAX_LENGTH + 1

    /**
     * The draft, when the field holds one line of at most [MAX_LENGTH]
     * characters that is not blank; else null. Null [before] and [after]
     * mean the field could not be read.
     */
    fun read(before: CharSequence?, selected: CharSequence?, after: CharSequence?): Draft? {
        if (before == null && after == null) return null
        val b = before?.toString().orEmpty()
        val s = selected?.toString().orEmpty()
        val a = after?.toString().orEmpty()
        val text = b + s + a
        if (text.length > MAX_LENGTH || text.any { it == '\n' || it == '\r' || it == ' ' || it == ' ' }) {
            return null
        }
        val query = text.trim()
        if (query.isEmpty()) return null
        return Draft(query, text, b.length, s.length, a.length)
    }

    /** Fields whose text must not be read or sent anywhere: passwords. */
    fun isSensitive(inputType: Int): Boolean {
        val variation = inputType and InputType.TYPE_MASK_VARIATION
        return when (inputType and InputType.TYPE_MASK_CLASS) {
            InputType.TYPE_CLASS_TEXT -> variation == InputType.TYPE_TEXT_VARIATION_PASSWORD ||
                variation == InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD ||
                variation == InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD
            InputType.TYPE_CLASS_NUMBER -> variation == InputType.TYPE_NUMBER_VARIATION_PASSWORD
            else -> false
        }
    }
}
