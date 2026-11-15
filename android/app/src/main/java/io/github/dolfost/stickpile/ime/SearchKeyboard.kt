package io.github.dolfost.stickpile.ime

import androidx.annotation.StringRes
import io.github.dolfost.stickpile.R

/**
 * A letter layout of the picker's own search keys ([SearchKeys]). The
 * letters are lower case: the search ignores case.
 *
 * @property tag what [ImeSettings.searchLayout] stores.
 * @property rows the three rows of letters.
 * @property letters the label of the key back from the symbols.
 * @property label the layout's name, on the space bar.
 */
enum class KeyLayout(
    val tag: String,
    val rows: List<String>,
    @StringRes val letters: Int,
    @StringRes val label: Int,
) {
    ENGLISH(
        "en",
        listOf("qwertyuiop", "asdfghjkl", "zxcvbnm"),
        R.string.ime_key_letters_english,
        R.string.ime_layout_english,
    ),
    UKRAINIAN(
        "uk",
        listOf("йцукенгшщзхї", "фівапролджє", "ячсмитьбюґ"),
        R.string.ime_key_letters_ukrainian,
        R.string.ime_layout_ukrainian,
    ),
    ;

    /** The layout the globe key switches to. */
    fun next(): KeyLayout = entries[(ordinal + 1) % entries.size]

    companion object {
        fun forTag(tag: String?): KeyLayout? = entries.firstOrNull { it.tag == tag }

        /** The layout for a phone in [language] (an ISO 639 code) that has not chosen one. */
        fun default(language: String): KeyLayout = if (language == "uk") UKRAINIAN else ENGLISH
    }
}

/** One key of the search keys. */
sealed interface Key {
    /** Types [text]. */
    data class Text(val text: String) : Key

    /** Deletes the last character; repeats while held. */
    data object Backspace : Key

    /** Between the letters and the symbols. */
    data object Symbols : Key

    /** The next [KeyLayout]. */
    data object NextLayout : Key
    data object Space : Key

    /** Hides the keys. */
    data object Done : Key

    /** Room left empty, to centre a shorter row. */
    data object Gap : Key
}

/** A key and its share of the row's width. */
data class KeySpec(val key: Key, val weight: Float = 1f)

/**
 * The rows of the picker's search keys: three of letters (or of digits and
 * the symbols the search syntax uses), then
 * `[123][globe][:][space][-][Done]`. Every row is equally wide in
 * weights, the widest row of letters. Pure.
 */
object SearchKeyboard {
    /** The symbols page, without the `:` and `-` of the bottom row. */
    val SYMBOLS: List<String> = listOf("1234567890", "@#&_=<>\"'", ".,!?()/+")

    private const val BACKSPACE_WEIGHT = 1.5f
    private const val SIDE_KEY_WEIGHT = 1.5f

    fun rows(layout: KeyLayout, symbols: Boolean): List<List<KeySpec>> {
        val chars = if (symbols) SYMBOLS else layout.rows
        val width = layout.rows.maxOf { it.length }.toFloat()
        val rows = chars.mapIndexed { i, row ->
            val keys = row.map { KeySpec(Key.Text(it.toString())) }
            if (i == chars.lastIndex) withBackspace(keys, width) else centred(keys, width)
        }
        return rows + listOf(bottomRow(width))
    }

    /** [keys] with gaps on both sides to fill [width]. */
    private fun centred(keys: List<KeySpec>, width: Float): List<KeySpec> {
        val side = (width - keys.size) / 2
        if (side <= 0f) return keys
        return listOf(KeySpec(Key.Gap, side)) + keys + KeySpec(Key.Gap, side)
    }

    /** [keys] then the backspace at the end, which takes what the gap on the left leaves. */
    private fun withBackspace(keys: List<KeySpec>, width: Float): List<KeySpec> {
        val left = ((width - keys.size - BACKSPACE_WEIGHT) / 2).coerceAtLeast(0f)
        val backspace = KeySpec(Key.Backspace, width - keys.size - left)
        return (if (left > 0f) listOf(KeySpec(Key.Gap, left)) else emptyList()) + keys + backspace
    }

    private fun bottomRow(width: Float): List<KeySpec> {
        val fixed = listOf(
            KeySpec(Key.Symbols, SIDE_KEY_WEIGHT),
            KeySpec(Key.NextLayout),
            KeySpec(Key.Text(":")),
            KeySpec(Key.Text("-")),
            KeySpec(Key.Done, SIDE_KEY_WEIGHT),
        )
        val space = KeySpec(Key.Space, width - fixed.sumOf { it.weight.toDouble() }.toFloat())
        return fixed.take(3) + space + fixed.drop(3)
    }
}
