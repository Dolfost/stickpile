package io.github.dolfost.stickpile.ime

import android.text.InputType
import android.view.inputmethod.EditorInfo

/**
 * The text already in the field, used as the search (docs/android-plan.md
 * 4.7): the picker's query starts as the draft, and its own keys edit it.
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

    /** The text variations whose draft may be read: what a person writes to others. */
    private val READABLE_VARIATIONS = setOf(
        InputType.TYPE_TEXT_VARIATION_NORMAL,
        InputType.TYPE_TEXT_VARIATION_SHORT_MESSAGE,
        InputType.TYPE_TEXT_VARIATION_LONG_MESSAGE,
    )

    /**
     * Whether the draft of a field may be read (and sent to the server as a
     * search), from its `EditorInfo.inputType` and `imeOptions`. Only plain
     * text: the text class with the variation normal, short message or long
     * message. Never any other class or variation (numbers such as one-time
     * codes, phone numbers, dates, email addresses, URLs, names, addresses,
     * passwords), never a field flagged for no suggestions (the flag apps
     * set so that keyboards keep nothing of what is typed), and never an
     * incognito field (`IME_FLAG_NO_PERSONALIZED_LEARNING`).
     */
    fun canRead(inputType: Int, imeOptions: Int): Boolean {
        if ((imeOptions and EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING) != 0) return false
        if ((inputType and InputType.TYPE_MASK_CLASS) != InputType.TYPE_CLASS_TEXT) return false
        if ((inputType and InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS) != 0) return false
        return (inputType and InputType.TYPE_MASK_VARIATION) in READABLE_VARIATIONS
    }
}
