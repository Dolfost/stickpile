package io.github.dolfost.stickpile.ime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DecodeSizeTest {
    @Test
    fun smallPicturesKeepTheirSize() {
        assertEquals(256 to 128, DecodeSize.fit(256, 128, 300))
        assertEquals(300 to 300, DecodeSize.fit(300, 300, 300))
        assertEquals(1 to 1, DecodeSize.fit(1, 1, 300))
    }

    @Test
    fun largePicturesShrinkToTheLongerSideKeepingTheAspectRatio() {
        assertEquals(512 to 256, DecodeSize.fit(4096, 2048, 512))
        assertEquals(256 to 512, DecodeSize.fit(2048, 4096, 512))
        assertEquals(512 to 512, DecodeSize.fit(100_000, 100_000, 512))
        assertEquals(300 to 168, DecodeSize.fit(1920, 1080, 300))
    }

    @Test
    fun noSideGoesBelowOneOrAboveTheLimit() {
        assertEquals(512 to 1, DecodeSize.fit(1_000_000, 1, 512))
        assertEquals(1 to 512, DecodeSize.fit(1, Int.MAX_VALUE, 512))
        assertEquals(512 to 512, DecodeSize.fit(Int.MAX_VALUE, Int.MAX_VALUE, 512))
    }

    @Test
    fun aSizeThatIsNotPositiveIsNoSize() {
        assertNull(DecodeSize.fit(0, 100, 512))
        assertNull(DecodeSize.fit(100, 0, 512))
        assertNull(DecodeSize.fit(-1, -1, 512))
    }

    @Test
    fun samplingKeepsTheLongerSideBetweenTheLimitAndTwiceIt() {
        assertEquals(1, DecodeSize.sampleSize(512, 512, 512))
        assertEquals(1, DecodeSize.sampleSize(1023, 10, 512))
        assertEquals(2, DecodeSize.sampleSize(1024, 10, 512))
        assertEquals(8, DecodeSize.sampleSize(4096, 4096, 512))
        for (side in listOf(513, 1000, 5000, 65_535, 1_000_000, Int.MAX_VALUE)) {
            val sample = DecodeSize.sampleSize(side, 1, 512)
            assertEquals("power of two for $side", 0, sample and (sample - 1))
            val decoded = side / sample
            assertTrue("$side / $sample = $decoded", decoded in 512 until 1024)
        }
    }

    @Test
    fun samplingANonsenseSizeDoesNothing() {
        assertEquals(1, DecodeSize.sampleSize(0, 0, 512))
        assertEquals(1, DecodeSize.sampleSize(-5, 100, 0))
        assertEquals(1, DecodeSize.sampleSize(100, 100, 512))
    }

    @Test
    fun thumbnailsAreNoLargerThanTheirCellAndAtMost512() {
        assertEquals(240, DecodeSize.thumbnailPx(240))
        assertEquals(512, DecodeSize.thumbnailPx(720))
        assertEquals(1, DecodeSize.thumbnailPx(0))
    }
}
