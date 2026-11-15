package io.github.dolfost.stickpile.ime

import org.junit.Assert.assertEquals
import org.junit.Test

class QueryEditorTest {
    /** The text of these code points. */
    private fun cp(vararg points: Int): String = points.joinToString("") { String(Character.toChars(it)) }

    private val joy = cp(0x1F602)
    private val thumbsUpMedium = cp(0x1F44D, 0x1F3FD)
    private val redHeart = cp(0x2764, 0xFE0F)
    private val family = cp(0x1F468, 0x200D, 0x1F469, 0x200D, 0x1F467)
    private val keycapOne = cp(0x31, 0xFE0F, 0x20E3)
    private val ukraine = cp(0x1F1FA, 0x1F1E6)
    private val usa = cp(0x1F1FA, 0x1F1F8)
    private val scotland = cp(0x1F3F4, 0xE0067, 0xE0062, 0xE0073, 0xE0063, 0xE0074, 0xE007F)

    @Test
    fun typingAppends() {
        assertEquals("c", QueryEditor.type("", "c"))
        assertEquals("tag:", QueryEditor.type("tag", ":"))
        assertEquals("кіт", QueryEditor.type("кі", "т"))
    }

    @Test
    fun aSpaceIsNotTypedFirstNorAfterAnother() {
        assertEquals("", QueryEditor.type("", " "))
        assertEquals("cat ", QueryEditor.type("cat", " "))
        assertEquals("cat ", QueryEditor.type("cat ", " "))
    }

    @Test
    fun nothingIsTypedPastTheLongestQuery() {
        val almost = "a".repeat(QueryEditor.MAX_LENGTH - 1)
        val full = QueryEditor.type(almost, "a")
        assertEquals(QueryEditor.MAX_LENGTH, full.length)
        assertEquals(full, QueryEditor.type(full, "b"))
        assertEquals(almost, QueryEditor.type(almost, joy))
    }

    @Test
    fun backspaceTakesOffTheLastLetter() {
        assertEquals("ca", QueryEditor.backspace("cat"))
        assertEquals("кі", QueryEditor.backspace("кіт"))
        assertEquals("", QueryEditor.backspace("a"))
        assertEquals("", QueryEditor.backspace(""))
    }

    @Test
    fun backspaceTakesOffAWholeEmojiNotHalfOfIt() {
        assertEquals("e:", QueryEditor.backspace("e:$joy"))
        assertEquals("e:", QueryEditor.backspace("e:$thumbsUpMedium"))
        assertEquals("e:", QueryEditor.backspace("e:$redHeart"))
        assertEquals("e:", QueryEditor.backspace("e:$family"))
        assertEquals("e:", QueryEditor.backspace("e:$keycapOne"))
        assertEquals("e:", QueryEditor.backspace("e:$scotland"))
        assertEquals("e:$joy", QueryEditor.backspace("e:$joy$redHeart"))
    }

    @Test
    fun flagsGoOneAtATime() {
        assertEquals(ukraine, QueryEditor.backspace(ukraine + usa))
        assertEquals("x", QueryEditor.backspace("x$ukraine"))
        assertEquals(ukraine, QueryEditor.backspace(ukraine + cp(0x1F1FA)))
    }

    @Test
    fun aCombiningMarkGoesWithItsLetter() {
        assertEquals("", QueryEditor.backspace("ї"))
        assertEquals("a", QueryEditor.backspace("aé"))
    }

    @Test
    fun aLoneHalfOfAPairGoesAlone() {
        assertEquals("a", QueryEditor.backspace("a\uD83D"))
        assertEquals("a", QueryEditor.backspace("a\uDE02"))
    }

    @Test
    fun lastCharacterStartIsAtTheCharacter() {
        assertEquals(2, QueryEditor.lastCharacterStart("e:$joy"))
        assertEquals(0, QueryEditor.lastCharacterStart(""))
    }
}
