package io.github.dolfost.stickpile.share

import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.UploadFile
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class ShareBatchTest {
    private fun batch(n: Int) = ShareBatch(
        (0 until n).map { SharedFile(id = it, fileName = "f$it.png", isVideo = false, name = "f$it") },
    )

    private fun ready(n: Int): ShareBatch =
        (0 until n).fold(batch(n)) { b, id -> b.copied(id, "/cache/share/s/$id/f$id.png") }

    private val unreachable = UploadFailure(LibraryErrorKind.UNREACHABLE)
    private val refused = UploadFailure(LibraryErrorKind.INVALID_INPUT)

    @Test
    fun filesBecomeReadyOrUnreadable() {
        val b = batch(2).copied(0, "/c/0/f0.png").unreadable(1, ReadProblem.NO_ACCESS)
        assertEquals(FileStatus.Ready, b.file(0)?.status)
        assertEquals("/c/0/f0.png", b.file(0)?.path)
        assertEquals(FileStatus.Unreadable(ReadProblem.NO_ACCESS), b.file(1)?.status)
        assertFalse(b.copying)
        assertTrue(batch(1).copying)
        assertEquals(listOf(0), b.uploadable.map { it.id })
    }

    @Test
    fun theUploadRequestUsesTheFields() {
        val b = ready(1).edit(0, name = "  ", keywords = "cat, Cat, hat", emojis = "\uD83D\uDE00\uD83D\uDE02")
        assertEquals(
            UploadFile(
                path = "/cache/share/s/0/f0.png",
                name = null,
                keywords = listOf("cat", "hat"),
                emojis = listOf("\uD83D\uDE00", "\uD83D\uDE02"),
            ),
            b.file(0)?.toUpload(),
        )
        assertEquals("f0.png", b.file(0)?.label)
        val named = b.edit(0, name = " Cat ")
        assertEquals("Cat", named.file(0)?.toUpload()?.name)
        assertEquals("Cat", named.file(0)?.label)
        assertNull(batch(1).file(0)?.toUpload())
    }

    @Test
    fun onlyEditableFilesChange() {
        val b = batch(1).edit(0, name = "x")
        assertEquals("f0", b.file(0)?.name)
        val running = ready(1).startRun().edit(0, name = "x")
        assertEquals("f0", running.file(0)?.name)
    }

    @Test
    fun applyToAllCopiesToTheEditableFiles() {
        val b = batch(3).copied(0, "/c/0").copied(1, "/c/1").unreadable(2, ReadProblem.GONE)
            .edit(0, keywords = "cat", emojis = "\uD83D\uDE00")
        val withKeywords = b.keywordsToAll(0)
        assertEquals(listOf("cat", "cat", ""), withKeywords.files.map { it.keywords })
        assertEquals(listOf("\uD83D\uDE00", "", ""), withKeywords.files.map { it.emojis })
        assertEquals(listOf("\uD83D\uDE00", "\uD83D\uDE00", ""), b.emojisToAll(0).files.map { it.emojis })
    }

    @Test
    fun aRunGoesFileByFileWithProgress() {
        var b = ready(3).startRun()
        assertTrue(b.running)
        assertNull(b.summary)
        assertEquals(listOf(0, 1, 2), b.run)
        assertEquals(UploadProgress(1, 3, "f0"), b.progress)

        b = b.uploading(b.next()!!.id)
        assertEquals(UploadProgress(1, 3, "f0"), b.progress)
        b = b.uploaded(0, "f0")
        assertEquals(FileStatus.Uploaded("f0"), b.file(0)?.status)
        assertNull(b.file(0)?.path)
        assertEquals(UploadProgress(2, 3, "f1"), b.progress)

        b = b.uploading(1).failed(1, refused)
        assertEquals(2, b.next()?.id)
        assertEquals(UploadProgress(3, 3, "f2"), b.progress)
        b = b.uploading(2).uploaded(2, "f2")

        assertFalse(b.running)
        assertNull(b.next())
        assertNull(b.progress)
        assertEquals(UploadSummary(uploaded = 2, failed = 1, unreadable = 0, total = 3), b.summary)
        assertFalse(b.summary!!.complete)
        assertTrue(b.onlyRetries)
        assertEquals(listOf(1), b.uploadable.map { it.id })
    }

    @Test
    fun aFailureForAllStopsTheRun() {
        var b = ready(3).startRun()
        b = b.uploading(0).failed(0, unreachable)
        assertFalse(b.running)
        assertEquals(List(3) { FileStatus.Failed(unreachable) }, b.files.map { it.status })
        assertEquals(UploadSummary(uploaded = 0, failed = 3, unreadable = 0, total = 3), b.summary)
    }

    @Test
    fun aRetryRunsTheFailedFilesOnly() {
        var b = ready(3).startRun()
        b = b.uploading(0).uploaded(0, "f0")
        b = b.uploading(1).failed(1, refused)
        b = b.uploading(2).failed(2, refused)

        val one = b.startRun(listOf(2))
        assertEquals(listOf(2), one.run)
        assertEquals(UploadProgress(1, 1, "f2"), one.progress)
        assertTrue(one.file(1)?.status is FileStatus.Failed)

        b = b.startRun()
        assertEquals(listOf(1, 2), b.run)
        b = b.uploading(1).uploaded(1, "f1").uploading(2).uploaded(2, "f2")
        assertEquals(UploadSummary(uploaded = 3, failed = 0, unreadable = 0, total = 3), b.summary)
        assertTrue(b.summary!!.complete)
    }

    @Test
    fun noRunWithNothingToUploadOrWhileOneRuns() {
        val nothing = batch(2)
        assertSame(nothing, nothing.startRun())
        val running = ready(2).startRun()
        assertSame(running, running.startRun())
        val done = ready(1).startRun().uploading(0).uploaded(0, "f0")
        assertSame(done, done.startRun())
    }

    @Test
    fun theSummaryCountsUnreadableFiles() {
        val b = ready(2).let { it.copy(files = it.files + SharedFile(2, "x.txt", false)) }
            .unreadable(2, ReadProblem.NOT_MEDIA)
            .startRun()
            .uploading(0).uploaded(0, "a")
            .uploading(1).uploaded(1, "b")
        assertEquals(UploadSummary(uploaded = 2, failed = 0, unreadable = 1, total = 3), b.summary)
        assertTrue(b.summary!!.complete)
    }

    @Test
    fun removingAFile() {
        val b = ready(3).remove(1)
        assertEquals(listOf(0, 2), b.files.map { it.id })
        val running = ready(2).startRun()
        assertSame(running, running.remove(0))
        val afterRun = running.uploading(0).uploaded(0, "f0").uploading(1).failed(1, refused).remove(1)
        assertEquals(listOf(0), afterRun.run)
        assertEquals(UploadSummary(uploaded = 1, failed = 0, unreadable = 0, total = 1), afterRun.summary)
    }

    @Test
    fun failuresFromLibraryErrors() {
        assertEquals(UploadFailure(LibraryErrorKind.SERVER, 503), UploadFailure.of(LibraryException.Server(503u, "x")))
        assertEquals(
            UploadFailure(LibraryErrorKind.INVALID_INPUT),
            UploadFailure.of(LibraryException.InvalidInput("file", "x")),
        )
        assertTrue(UploadFailure.of(LibraryException.NotSignedIn("x")).stopsRun)
        assertTrue(UploadFailure.of(LibraryException.Unauthorized("x")).stopsRun)
        assertTrue(UploadFailure.of(LibraryException.NotFound("x")).stopsRun)
        assertTrue(UploadFailure.of(LibraryException.RateLimited("x")).stopsRun)
        assertFalse(UploadFailure.of(LibraryException.InvalidInput("file", "x")).stopsRun)
        assertFalse(UploadFailure.of(LibraryException.Io("x")).stopsRun)
        assertFalse(UploadFailure.of(LibraryException.Server(500u, "x")).stopsRun)
        assertFalse(UploadFailure.OTHER.stopsRun)
    }
}
