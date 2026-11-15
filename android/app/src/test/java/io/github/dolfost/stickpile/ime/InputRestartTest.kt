package io.github.dolfost.stickpile.ime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class InputRestartTest {
    private val cat = Draft("cat", "cat", 3, 0, 0)
    private val dog = Draft("dog", "dog", 3, 0, 0)

    @Test
    fun aRestartWithoutANewDraftKeepsTheView() {
        assertTrue(InputRestart.keepsView(started = true, restarting = true, query = null, shownQuery = "cat"))
        assertTrue(InputRestart.keepsView(started = true, restarting = true, query = "cat", shownQuery = "cat"))
        assertTrue(InputRestart.keepsView(started = true, restarting = true, query = null, shownQuery = null))
    }

    @Test
    fun aNewDraftANewInputOrAFirstStartShowsAfresh() {
        assertFalse(InputRestart.keepsView(started = true, restarting = true, query = "dog", shownQuery = "cat"))
        assertFalse(InputRestart.keepsView(started = true, restarting = false, query = null, shownQuery = "cat"))
        assertFalse(InputRestart.keepsView(started = false, restarting = true, query = null, shownQuery = null))
    }

    @Test
    fun theDraftIsForgottenOnlyByANewInput() {
        assertEquals(cat, InputRestart.draft(restarting = true, read = null, remembered = cat))
        assertEquals(dog, InputRestart.draft(restarting = true, read = dog, remembered = cat))
        assertNull(InputRestart.draft(restarting = false, read = null, remembered = cat))
        assertEquals(dog, InputRestart.draft(restarting = false, read = dog, remembered = cat))
    }
}
