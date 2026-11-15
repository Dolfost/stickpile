package io.github.dolfost.stickpile.motion

import io.github.dolfost.stickpile.display.Autoplay
import io.github.dolfost.stickpile.motion.MotionPolicy.Source
import io.github.dolfost.stickpile.rust.MediaKind
import io.github.dolfost.stickpile.rust.Motion
import io.github.dolfost.stickpile.rust.MotionKind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MotionPolicyTest {
    private val modern = 34
    private val oreo = 27

    @Test
    fun aStillNeverPlays() {
        for (autoplay in Autoplay.entries) {
            assertNull(MotionPolicy.source(MediaKind.STILL, autoplay, metered = false, sdk = modern))
        }
    }

    @Test
    fun alwaysDownloadsOnAnyNetwork() {
        assertEquals(Source.ANY, MotionPolicy.source(MediaKind.ANIMATION, Autoplay.ALWAYS, metered = true, sdk = modern))
        assertEquals(Source.ANY, MotionPolicy.source(MediaKind.VIDEO, Autoplay.ALWAYS, metered = false, sdk = modern))
    }

    @Test
    fun wifiOnlyPlaysTheCachedOnMobileData() {
        assertEquals(Source.CACHED, MotionPolicy.source(MediaKind.VIDEO, Autoplay.WIFI, metered = true, sdk = modern))
        assertEquals(Source.ANY, MotionPolicy.source(MediaKind.VIDEO, Autoplay.WIFI, metered = false, sdk = modern))
    }

    @Test
    fun neverPlaysNothing() {
        assertNull(MotionPolicy.source(MediaKind.ANIMATION, Autoplay.NEVER, metered = false, sdk = modern))
        assertNull(MotionPolicy.source(MediaKind.VIDEO, Autoplay.NEVER, metered = false, sdk = modern))
    }

    @Test
    fun animatedPicturesNeedApi28() {
        assertNull(MotionPolicy.source(MediaKind.ANIMATION, Autoplay.ALWAYS, metered = false, sdk = oreo))
        assertEquals(Source.ANY, MotionPolicy.source(MediaKind.VIDEO, Autoplay.ALWAYS, metered = false, sdk = oreo))
        val picture = Motion("/c/a.gif", MotionKind.PICTURE, 1u, 1u)
        val video = Motion("/c/a.mp4", MotionKind.VIDEO, 1u, 1u)
        assertFalse(MotionPolicy.playable(picture, sdk = oreo))
        assertTrue(MotionPolicy.playable(picture, sdk = modern))
        assertTrue(MotionPolicy.playable(video, sdk = oreo))
    }
}
