package io.github.dolfost.stickpile.ime

import android.text.InputType
import android.view.inputmethod.EditorInfo
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

    private val text = InputType.TYPE_CLASS_TEXT

    @Test
    fun plainTextFieldsAreRead() {
        assertTrue(DraftQuery.canRead(text, 0))
        assertTrue(DraftQuery.canRead(text or InputType.TYPE_TEXT_VARIATION_NORMAL, 0))
        assertTrue(DraftQuery.canRead(text or InputType.TYPE_TEXT_VARIATION_SHORT_MESSAGE, 0))
        assertTrue(DraftQuery.canRead(text or InputType.TYPE_TEXT_VARIATION_LONG_MESSAGE, 0))
        // A chat app's message field, as typically flagged.
        val chat = text or InputType.TYPE_TEXT_VARIATION_SHORT_MESSAGE or InputType.TYPE_TEXT_FLAG_CAP_SENTENCES or
            InputType.TYPE_TEXT_FLAG_MULTI_LINE or InputType.TYPE_TEXT_FLAG_AUTO_CORRECT
        assertTrue(DraftQuery.canRead(chat, EditorInfo.IME_ACTION_SEND or EditorInfo.IME_FLAG_NO_EXTRACT_UI))
    }

    @Test
    fun otherTextVariationsAreNotRead() {
        listOf(
            InputType.TYPE_TEXT_VARIATION_PASSWORD,
            InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD,
            InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD,
            InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS,
            InputType.TYPE_TEXT_VARIATION_WEB_EMAIL_ADDRESS,
            InputType.TYPE_TEXT_VARIATION_EMAIL_SUBJECT,
            InputType.TYPE_TEXT_VARIATION_URI,
            InputType.TYPE_TEXT_VARIATION_PERSON_NAME,
            InputType.TYPE_TEXT_VARIATION_POSTAL_ADDRESS,
            InputType.TYPE_TEXT_VARIATION_WEB_EDIT_TEXT,
            InputType.TYPE_TEXT_VARIATION_FILTER,
            InputType.TYPE_TEXT_VARIATION_PHONETIC,
        ).forEach { variation ->
            assertFalse("variation $variation", DraftQuery.canRead(text or variation, 0))
        }
    }

    @Test
    fun otherClassesAreNotRead() {
        listOf(
            InputType.TYPE_NULL,
            InputType.TYPE_CLASS_NUMBER,
            InputType.TYPE_CLASS_NUMBER or InputType.TYPE_NUMBER_VARIATION_PASSWORD,
            InputType.TYPE_CLASS_NUMBER or InputType.TYPE_NUMBER_FLAG_DECIMAL,
            InputType.TYPE_CLASS_PHONE,
            InputType.TYPE_CLASS_DATETIME,
            InputType.TYPE_CLASS_DATETIME or InputType.TYPE_DATETIME_VARIATION_TIME,
        ).forEach { type ->
            assertFalse("input type $type", DraftQuery.canRead(type, 0))
        }
    }

    @Test
    fun fieldsAskingForNoSuggestionsAreNotRead() {
        assertFalse(DraftQuery.canRead(text or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS, 0))
        assertFalse(
            DraftQuery.canRead(
                text or InputType.TYPE_TEXT_VARIATION_SHORT_MESSAGE or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS,
                0,
            ),
        )
    }

    @Test
    fun incognitoFieldsAreNotRead() {
        val incognito = EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING
        assertFalse(DraftQuery.canRead(text, incognito))
        assertFalse(DraftQuery.canRead(text or InputType.TYPE_TEXT_VARIATION_SHORT_MESSAGE, incognito))
        assertFalse(DraftQuery.canRead(text, incognito or EditorInfo.IME_ACTION_SEND))
    }
}
