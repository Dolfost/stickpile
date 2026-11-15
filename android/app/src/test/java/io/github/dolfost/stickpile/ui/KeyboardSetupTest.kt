package io.github.dolfost.stickpile.ui

import io.github.dolfost.stickpile.ui.keyboard.PICKER_SERVICE
import io.github.dolfost.stickpile.ui.keyboard.isPickerEnabled
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Telling whether the picker keyboard is turned on (ui.keyboard). */
class KeyboardSetupTest {
    private val app = "io.github.dolfost.stickpile"

    @Test
    fun thePickerIsTheServiceInTheManifest() {
        assertEquals("io.github.dolfost.stickpile.ime.PickerInputMethodService", PICKER_SERVICE)
    }

    @Test
    fun enabledMeansThisAppsPickerIsInTheList() {
        val gboard = "com.google.android.inputmethod.latin" to "com.android.inputmethod.latin.LatinIME"
        assertFalse(isPickerEnabled(emptyList(), app))
        assertFalse(isPickerEnabled(listOf(gboard), app))
        assertTrue(isPickerEnabled(listOf(gboard, app to PICKER_SERVICE), app))
        // The same class name from another package (a debug build, a fork) is not this one.
        assertFalse(isPickerEnabled(listOf("$app.debug" to PICKER_SERVICE), app))
        // Another service of this package is not the picker.
        assertFalse(isPickerEnabled(listOf(app to "$app.ime.Other"), app))
    }
}
