package io.github.dolfost.stickpile.ime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SearchKeyboardTest {
    private fun texts(row: List<KeySpec>): String =
        row.mapNotNull { (it.key as? Key.Text)?.text }.joinToString("")

    @Test
    fun everyRowIsAsWide() {
        for (layout in KeyLayout.entries) {
            for (symbols in listOf(false, true)) {
                val widths = SearchKeyboard.rows(layout, symbols).map { row -> row.sumOf { it.weight.toDouble() } }
                val widest = layout.rows.maxOf { it.length }.toDouble()
                for (width in widths) assertEquals("$layout symbols=$symbols", widest, width, 0.001)
                assertTrue(SearchKeyboard.rows(layout, symbols).flatten().all { it.weight > 0f })
            }
        }
    }

    @Test
    fun theLettersAreTheLayoutsInLowerCase() {
        val english = SearchKeyboard.rows(KeyLayout.ENGLISH, symbols = false)
        assertEquals(listOf("qwertyuiop", "asdfghjkl", "zxcvbnm"), english.take(3).map(::texts))
        val ukrainian = SearchKeyboard.rows(KeyLayout.UKRAINIAN, symbols = false)
        assertEquals(listOf("йцукенгшщзхї", "фівапролджє", "ячсмитьбюґ"), ukrainian.take(3).map(::texts))
        for (layout in KeyLayout.entries) {
            val letters = SearchKeyboard.rows(layout, symbols = false).take(3).joinToString("") { texts(it) }
            assertEquals(letters.lowercase(), letters)
        }
    }

    @Test
    fun theThirdRowEndsWithBackspaceAndTheLastIsTheBottomRow() {
        for (layout in KeyLayout.entries) {
            for (symbols in listOf(false, true)) {
                val rows = SearchKeyboard.rows(layout, symbols)
                assertEquals(4, rows.size)
                assertEquals(Key.Backspace, rows[2].last().key)
                assertEquals(
                    listOf(Key.Symbols, Key.NextLayout, Key.Text(":"), Key.Space, Key.Text("-"), Key.Done),
                    rows[3].map { it.key },
                )
            }
        }
    }

    @Test
    fun theSymbolsHaveTheDigitsAndWhatTheSyntaxUses() {
        val typed = SearchKeyboard.rows(KeyLayout.ENGLISH, symbols = true).joinToString("") { texts(it) }
        for (c in "0123456789:-\"_'#><=.,!?@&") assertTrue("$c", c in typed)
        assertEquals(typed.toSet().size, typed.length)
    }

    @Test
    fun theGlobeKeyCyclesThroughTheLayouts() {
        assertEquals(KeyLayout.UKRAINIAN, KeyLayout.ENGLISH.next())
        assertEquals(KeyLayout.ENGLISH, KeyLayout.UKRAINIAN.next())
    }

    @Test
    fun theChosenLayoutIsReadBackAndTheDefaultFollowsThePhone() {
        for (layout in KeyLayout.entries) assertEquals(layout, KeyLayout.forTag(layout.tag))
        assertNull(KeyLayout.forTag(null))
        assertNull(KeyLayout.forTag("de"))
        assertEquals(KeyLayout.UKRAINIAN, KeyLayout.default("uk"))
        assertEquals(KeyLayout.ENGLISH, KeyLayout.default("en"))
        assertEquals(KeyLayout.ENGLISH, KeyLayout.default("de"))
    }
}
