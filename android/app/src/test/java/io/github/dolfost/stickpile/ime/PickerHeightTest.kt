package io.github.dolfost.stickpile.ime

import org.junit.Assert.assertEquals
import org.junit.Test

class PickerHeightTest {
    @Test
    fun percentsGoInSteps() {
        assertEquals(100, PickerHeight.clamp(100))
        assertEquals(110, PickerHeight.clamp(106))
        assertEquals(100, PickerHeight.clamp(104))
        assertEquals(PickerHeight.MIN, PickerHeight.clamp(0))
        assertEquals(PickerHeight.MAX, PickerHeight.clamp(1_000))
        assertEquals(PickerHeight.MAX, PickerHeight.clamp(PickerHeight.MAX - 1))
    }

    @Test
    fun theGridIsScaled() {
        assertEquals(600, PickerHeight.scaled(600, 100, 2400, 200))
        assertEquals(900, PickerHeight.scaled(600, 150, 2400, 200))
        assertEquals(360, PickerHeight.scaled(600, 60, 2400, 200))
    }

    @Test
    fun theGridNeverTakesMoreThanHalfTheScreen() {
        // A phone held sideways: 1080 px tall.
        assertEquals(540, PickerHeight.scaled(600, 100, 1080, 200))
        assertEquals(540, PickerHeight.scaled(600, 160, 1080, 200))
    }

    @Test
    fun norLessThanItsLeast() {
        assertEquals(250, PickerHeight.scaled(300, 60, 400, 250))
    }
}
