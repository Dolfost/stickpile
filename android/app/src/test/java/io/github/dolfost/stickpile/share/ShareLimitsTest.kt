package io.github.dolfost.stickpile.share

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.InputStream
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class ShareLimitsTest {
    private fun bytes(n: Int) = ByteArray(n) { (it % 251).toByte() }

    /** A stream of [size] bytes that are never held in memory together. */
    private fun endless(size: Long) = object : InputStream() {
        var left = size

        override fun read(): Int = if (left-- > 0) 0 else -1

        override fun read(b: ByteArray, off: Int, len: Int): Int {
            if (left <= 0) return -1
            val n = minOf(len.toLong(), left).toInt()
            left -= n
            return n
        }
    }

    @Test
    fun aFileUpToTheLimitIsCopiedWhole() = runTest {
        for (size in listOf(0, 1, 1000, 200_000)) {
            val data = bytes(size)
            val out = ByteArrayOutputStream()
            assertEquals(size.toLong(), ShareLimits.copy(ByteArrayInputStream(data), out, maxBytes = 200_000))
            assertArrayEquals(data, out.toByteArray())
        }
    }

    @Test
    fun aLongerStreamStopsWithoutWritingPastTheLimit() = runTest {
        val out = ByteArrayOutputStream()
        try {
            ShareLimits.copy(ByteArrayInputStream(bytes(200_001)), out, maxBytes = 200_000)
            fail("copied past the limit")
        } catch (_: TooLargeException) {
        }
        assertTrue(out.size() <= 200_000)
    }

    @Test
    fun anEndlessStreamStopsAtTheLimit() = runTest {
        var written = 0L
        val counting = object : java.io.OutputStream() {
            override fun write(b: Int) {
                written++
            }

            override fun write(b: ByteArray, off: Int, len: Int) {
                written += len
            }
        }
        try {
            ShareLimits.copy(endless(Long.MAX_VALUE), counting, maxBytes = 5_000_000)
            fail("copied an endless stream")
        } catch (_: TooLargeException) {
        }
        assertTrue(written <= 5_000_000)
    }

    @Test
    fun aCancelledCopyStopsAtTheNextBlock() = runTest {
        val job = Job()
        var reads = 0
        // The screen goes while the first block is read.
        val stream = object : InputStream() {
            override fun read(): Int = 0

            override fun read(b: ByteArray, off: Int, len: Int): Int {
                reads++
                job.cancel()
                return len
            }
        }
        try {
            withContext(job) { ShareLimits.copy(stream, ByteArrayOutputStream()) }
            fail("a cancelled copy went on")
        } catch (_: CancellationException) {
        }
        assertEquals(1, reads)
    }

    @Test
    fun aSizeOverTheLimitIsRefusedBeforeCopying() {
        assertFalse(ShareLimits.tooLarge(null))
        assertFalse(ShareLimits.tooLarge(0))
        assertFalse(ShareLimits.tooLarge(ShareLimits.MAX_FILE_BYTES))
        assertTrue(ShareLimits.tooLarge(ShareLimits.MAX_FILE_BYTES + 1))
        assertTrue(ShareLimits.tooLarge(Long.MAX_VALUE))
        assertEquals(1_000_000_000L, ShareLimits.MAX_FILE_BYTES)
    }
}
