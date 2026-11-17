package io.github.dolfost.stickpile.ui.theme

import androidx.compose.material3.darkColorScheme
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ThemeTest {
    @Test
    fun oledPutsTheDarkThemeOnPureBlackAndKeepsTheContainersApart() {
        val dark = darkColorScheme()
        val oled = dark.onBlack()
        assertEquals(Color.Black, oled.background)
        assertEquals(Color.Black, oled.surface)
        assertEquals(Color.Black, oled.surfaceContainerLowest)
        listOf(
            dark.surfaceContainerLow to oled.surfaceContainerLow,
            dark.surfaceContainer to oled.surfaceContainer,
            dark.surfaceContainerHigh to oled.surfaceContainerHigh,
            dark.surfaceContainerHighest to oled.surfaceContainerHighest,
        ).forEach { (before, after) ->
            assertTrue("$after is darker than $before", after.luminance() < before.luminance())
            assertTrue("$after is not black", after.luminance() > 0f)
        }
        // The text and the accents stay the dark theme's.
        assertEquals(dark.onSurface, oled.onSurface)
        assertEquals(dark.primary, oled.primary)
    }
}
