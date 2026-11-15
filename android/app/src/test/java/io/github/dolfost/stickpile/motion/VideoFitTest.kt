package io.github.dolfost.stickpile.motion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class VideoFitTest {
    private fun assertScale(x: Float, y: Float, actual: Pair<Float, Float>?) {
        requireNotNull(actual)
        assertEquals(x, actual.first, 0.001f)
        assertEquals(y, actual.second, 0.001f)
    }

    @Test
    fun aWideVideoFillsTheWidth() {
        // 200x100 in a 100x100 view: 100x50, half as tall as stretched.
        assertScale(1f, 0.5f, VideoFit.scale(100, 100, 200, 100))
    }

    @Test
    fun aTallVideoFillsTheHeight() {
        assertScale(0.5f, 1f, VideoFit.scale(100, 100, 100, 200))
        assertScale(0.5625f, 1f, VideoFit.scale(160, 90, 90, 90))
    }

    @Test
    fun aMatchingVideoIsLeftAsItIs() {
        assertScale(1f, 1f, VideoFit.scale(120, 120, 512, 512))
    }

    @Test
    fun anUnknownSizeLeavesItAlone() {
        assertNull(VideoFit.scale(0, 100, 10, 10))
        assertNull(VideoFit.scale(100, 100, 0, 10))
    }
}
