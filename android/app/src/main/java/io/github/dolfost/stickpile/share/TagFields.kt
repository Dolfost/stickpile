package io.github.dolfost.stickpile.share

import java.text.BreakIterator

/**
 * The keywords and emojis text fields of the share screen (pure, no
 * Android): what the user typed, as the lists `UploadFile` takes, split
 * like the web UI splits them (`split_list` and `split_emojis` in
 * `stickpile-server`'s `web/items.rs`).
 */
object TagFields {
    /**
     * Keywords from the keywords field: separated by commas (or line
     * breaks), trimmed, white space inside one collapsed to a space ("funny
     * cat" stays one keyword), empty ones dropped, and a repeat of an
     * earlier one (ignoring case) dropped.
     */
    fun keywords(text: String): List<String> =
        text.split(',', '\n')
            .map { it.replace(WHITE_SPACE, " ").trim() }
            .filter { it.isNotEmpty() }
            .distinctBy { it.lowercase() }

    /**
     * Emojis from the emojis field: separated by commas or white space, and
     * a run of emojis typed without a separator (as emoji keyboards type
     * them) split into its characters as the user sees them (grapheme
     * clusters, so a skin tone or a joined family stays one). A run that
     * holds a letter or a digit stays whole, as typed. Repeats dropped.
     */
    fun emojis(text: String): List<String> =
        text.split(SEPARATORS)
            .filter { it.isNotEmpty() }
            .flatMap { if (it.any(Char::isLetterOrDigit)) listOf(it) else graphemes(it) }
            .distinct()

    /** A list of keywords as the keywords field shows it. */
    fun keywordsText(keywords: List<String>): String = keywords.joinToString(", ")

    /** A list of emojis as the emojis field shows it. */
    fun emojisText(emojis: List<String>): String = emojis.joinToString(" ")

    private val WHITE_SPACE = Regex("\\s+")
    private val SEPARATORS = Regex("[,\\s]+")

    private fun graphemes(s: String): List<String> {
        val it = BreakIterator.getCharacterInstance()
        it.setText(s)
        val out = mutableListOf<String>()
        var start = it.first()
        var end = it.next()
        while (end != BreakIterator.DONE) {
            out += s.substring(start, end)
            start = end
            end = it.next()
        }
        return out
    }
}
