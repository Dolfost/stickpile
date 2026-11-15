package io.github.dolfost.stickpile.provider

import java.security.MessageDigest
import java.security.SecureRandom

/**
 * The tokens of `/prepared/<token>/<file name>` URIs (docs/android.md,
 * "Provider contract"): 128 random bits from [SecureRandom], valid for one
 * hour, kept in memory only. A token is the capability to read one prepared
 * file; whoever presents it with the file name it was made for may read the
 * file, whatever app they are.
 *
 * Tokens are secrets: never log them, nor the URIs that carry them.
 *
 * @param clock milliseconds of a monotonic clock (`SystemClock.elapsedRealtime`).
 * @param random fills an array with random bytes.
 */
class PreparedTokens(
    private val clock: () -> Long,
    private val random: (ByteArray) -> Unit = defaultRandom::nextBytes,
    private val lifetimeMillis: Long = LIFETIME_MILLIS,
    private val capacity: Int = CAPACITY,
) {
    /** A prepared file: where it is, its MIME type and the name in its URI. */
    data class Entry(val path: String, val mimeType: String, val fileName: String)

    private class Registered(val token: ByteArray, val entry: Entry, val created: Long)

    // Oldest first.
    private val registered = ArrayDeque<Registered>()

    /** A new token for [entry], as 32 lowercase hex characters. */
    @Synchronized
    fun register(entry: Entry): String {
        val now = clock()
        dropExpired(now)
        while (registered.size >= capacity) registered.removeFirst()
        val bytes = ByteArray(TOKEN_BYTES)
        random(bytes)
        val token = hex(bytes)
        registered.addLast(Registered(token.toByteArray(Charsets.US_ASCII), entry, now))
        return token
    }

    /**
     * The file of [token], when the token is known, not expired, and was
     * made for [fileName]; else null. Compares tokens in constant time.
     */
    @Synchronized
    fun lookup(token: String, fileName: String): Entry? {
        dropExpired(clock())
        val presented = token.toByteArray(Charsets.US_ASCII)
        var found: Entry? = null
        for (r in registered) {
            if (MessageDigest.isEqual(r.token, presented)) found = r.entry
        }
        return found?.takeIf { it.fileName == fileName }
    }

    /** Tokens still valid (for tests). */
    @Synchronized
    fun size(): Int {
        dropExpired(clock())
        return registered.size
    }

    private fun dropExpired(now: Long) {
        while (registered.isNotEmpty() && now - registered.first().created >= lifetimeMillis) {
            registered.removeFirst()
        }
    }

    companion object {
        /** 128 bits. */
        const val TOKEN_BYTES = 16

        /** One hour. */
        const val LIFETIME_MILLIS = 60L * 60L * 1000L

        /** At most this many live tokens; the oldest goes first. */
        const val CAPACITY = 512

        private val defaultRandom = SecureRandom()

        private fun hex(bytes: ByteArray): String = buildString(bytes.size * 2) {
            for (b in bytes) {
                val v = b.toInt() and 0xff
                append(DIGITS[v shr 4]).append(DIGITS[v and 0x0f])
            }
        }

        private const val DIGITS = "0123456789abcdef"
    }
}
