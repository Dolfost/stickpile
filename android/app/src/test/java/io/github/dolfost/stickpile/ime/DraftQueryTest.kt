package io.github.dolfost.stickpile.ime

import android.text.InputType
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DraftQueryTest {
    @Test
    fun theWholeFieldIsTheQuery() {
        assertEquals(Draft("grumpy cat", "grumpy cat", 10, 0, 0), DraftQuery.read("grumpy cat", null, ""))
        assertEquals(Draft("grumpy cat", "grumpy cat", 3, 0, 7), DraftQuery.read("gru", "", "mpy cat"))
        assertEquals(Draft("grumpy cat", "grumpy cat", 0, 0, 10), DraftQuery.read("", null, "grumpy cat"))
    }

    @Test
    fun theSelectionIsPartOfIt() {
        assertEquals(Draft("grumpy cat", "grumpy cat", 2, 4, 4), DraftQuery.read("gr", "umpy", " cat"))
    }

    @Test
    fun spacesAroundAreTrimmedButCounted() {
        assertEquals(Draft("cat", "  cat ", 5, 0, 1), DraftQuery.read("  cat", null, " "))
    }

    @Test
    fun anEmptyOrBlankFieldHasNoDraft() {
        assertNull(DraftQuery.read("", null, ""))
        assertNull(DraftQuery.read("   ", "", "\t"))
        assertNull(DraftQuery.read(null, null, null))
    }

    @Test
    fun anUnreadableSideCountsAsEmpty() {
        assertEquals(Draft("cat", "cat", 3, 0, 0), DraftQuery.read("cat", null, null))
    }

    @Test
    fun moreThanOneLineIsNoDraft() {
        assertNull(DraftQuery.read("hello\nworld", null, ""))
        assertNull(DraftQuery.read("hello", null, "\r\nworld"))
        assertNull(DraftQuery.read("a", "b\nc", ""))
        assertNull(DraftQuery.read("a b", null, ""))
        assertNull(DraftQuery.read("cat\n", null, ""))
    }

    @Test
    fun sixtyCharactersAtMost() {
        val sixty = "x".repeat(60)
        assertEquals(sixty, DraftQuery.read(sixty, null, "")?.query)
        assertEquals(sixty, DraftQuery.read(sixty.take(25), null, sixty.drop(25))?.query)
        assertNull(DraftQuery.read(sixty + "y", null, ""))
        assertNull(DraftQuery.read(sixty.take(30), "", sixty.take(31)))
        // What the service reads: one more than fits on each side.
        assertEquals(61, DraftQuery.READ_LENGTH)
    }

    @Test
    fun passwordFieldsAreNotRead() {
        val text = InputType.TYPE_CLASS_TEXT
        val number = InputType.TYPE_CLASS_NUMBER
        assertTrue(DraftQuery.isSensitive(text or InputType.TYPE_TEXT_VARIATION_PASSWORD))
        assertTrue(DraftQuery.isSensitive(text or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD))
        assertTrue(DraftQuery.isSensitive(text or InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD))
        assertTrue(DraftQuery.isSensitive(number or InputType.TYPE_NUMBER_VARIATION_PASSWORD))
        assertFalse(DraftQuery.isSensitive(text or InputType.TYPE_TEXT_FLAG_MULTI_LINE))
        assertFalse(DraftQuery.isSensitive(text or InputType.TYPE_TEXT_VARIATION_SHORT_MESSAGE))
        assertFalse(DraftQuery.isSensitive(number))
        assertFalse(DraftQuery.isSensitive(InputType.TYPE_NULL))
    }
}
