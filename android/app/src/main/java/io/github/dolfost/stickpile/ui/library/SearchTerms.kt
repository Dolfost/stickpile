package io.github.dolfost.stickpile.ui.library

/**
 * Search texts the library screen writes for the user, in the syntax of
 * `stickpile_api::query`. Pure.
 */
object SearchTerms {
    /**
     * The query that searches for [value] of the filter [key]
     * (`tag:reaction `), quoted when it has a space (`tag:"big chungus" `),
     * with a space after it, ready for more.
     */
    fun filter(key: String, value: String): String {
        val v = value.trim().replace("\"", "")
        val quoted = if (v.any { it.isWhitespace() }) "\"$v\"" else v
        return "$key:$quoted "
    }

    /**
     * The parts of a syntax example ([io.github.dolfost.stickpile.display.SyntaxHelp])
     * a tap can search for: "q>70, likes>=2" is two, "vibe: tag:" two keys,
     * `cat "big chungus"` one.
     */
    fun examples(example: String): List<String> = example.split(", ").flatMap { part ->
        val words = part.split(' ').filter { it.isNotEmpty() }
        if (words.size > 1 && words.all { it.endsWith(':') }) words else listOf(part)
    }

    /** What a tapped example puts in the search field: a key stays open for its value, anything else gets a space. */
    fun typed(example: String): String = if (example.endsWith(':')) example else "$example "
}
