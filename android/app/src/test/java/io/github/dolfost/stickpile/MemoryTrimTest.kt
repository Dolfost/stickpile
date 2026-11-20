package io.github.dolfost.stickpile

import android.content.ComponentCallbacks2
import org.junit.Assert.assertEquals
import org.junit.Test

@Suppress("DEPRECATION")
class MemoryTrimTest {
    @Test
    fun inTheBackgroundNothingIsKept() {
        assertEquals(0f, MemoryTrim.keep(ComponentCallbacks2.TRIM_MEMORY_BACKGROUND))
        assertEquals(0f, MemoryTrim.keep(ComponentCallbacks2.TRIM_MEMORY_MODERATE))
        assertEquals(0f, MemoryTrim.keep(ComponentCallbacks2.TRIM_MEMORY_COMPLETE))
    }

    @Test
    fun theKeyboardKeepsItsPicturesWhenTheAppIsHidden() {
        assertEquals(1f, MemoryTrim.keep(ComponentCallbacks2.TRIM_MEMORY_UI_HIDDEN))
    }

    @Test
    fun whileRunningLowHalfIsKeptAndCriticalNone() {
        assertEquals(1f, MemoryTrim.keep(ComponentCallbacks2.TRIM_MEMORY_RUNNING_MODERATE))
        assertEquals(0.5f, MemoryTrim.keep(ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW))
        assertEquals(0f, MemoryTrim.keep(ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL))
    }
}
