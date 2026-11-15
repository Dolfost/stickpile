package io.github.dolfost.stickpile.provider

import java.util.concurrent.Semaphore
import java.util.concurrent.TimeUnit

/**
 * At most [permits] blocks at once; a caller over the limit waits up to
 * [waitMillis] for a slot (first come, first served) and else gets null
 * without running its block.
 *
 * [StickerProvider] runs the library calls that may download (`/thumb/`
 * opens, `prepare`) through one gate: each holds a binder thread while it
 * waits for the network, and the process, which also runs the keyboard and
 * the app, has only about 15 of them.
 */
class DownloadGate(permits: Int, private val waitMillis: Long) {
    private val slots = Semaphore(permits, true)

    /** [block]'s result, or null when no slot was free within the wait. */
    fun <T : Any> run(block: () -> T): T? {
        val acquired = try {
            slots.tryAcquire(waitMillis, TimeUnit.MILLISECONDS)
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            false
        }
        if (!acquired) return null
        try {
            return block()
        } finally {
            slots.release()
        }
    }
}
