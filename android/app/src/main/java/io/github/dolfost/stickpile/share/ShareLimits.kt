package io.github.dolfost.stickpile.share

import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive

/** A shared file is larger than [ShareLimits.MAX_FILE_BYTES]. */
class TooLargeException : IOException("larger than the share limit")

/** How much of a shared file is copied. Pure (streams only). */
object ShareLimits {
    /**
     * The largest file copied for upload: 1 GB. The server's own limit is
     * in its configuration (50 MB unless changed) and the bridge does not
     * tell it, so this only keeps a huge or endless stream from filling
     * the phone. The text `share_unreadable_too_large` names it.
     */
    const val MAX_FILE_BYTES = 1_000_000_000L

    private const val BUFFER_BYTES = 64 * 1024

    /** Whether the size a provider gives (`OpenableColumns.SIZE`; null: none) is over [maxBytes]. */
    fun tooLarge(size: Long?, maxBytes: Long = MAX_FILE_BYTES): Boolean = size != null && size > maxBytes

    /**
     * Copies [from] to [to] and returns the bytes copied; throws
     * [TooLargeException] as soon as the stream turns out longer than
     * [maxBytes] (a provider's size may be wrong or missing), having
     * written no more than that. Stops between blocks when the calling
     * coroutine is cancelled.
     */
    suspend fun copy(from: InputStream, to: OutputStream, maxBytes: Long = MAX_FILE_BYTES): Long {
        val buffer = ByteArray(BUFFER_BYTES)
        var total = 0L
        while (true) {
            currentCoroutineContext().ensureActive()
            val n = from.read(buffer)
            if (n < 0) return total
            total += n
            if (total > maxBytes) throw TooLargeException()
            to.write(buffer, 0, n)
        }
    }
}
