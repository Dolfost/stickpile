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
 * At most [capacity] tokens live at once, the oldest going first. Tokens
 * made for another app (through the provider's `prepare`) are also capped
 * per calling uid at [perCallerCapacity], so that one keyboard asking for
 * many cannot push out the tokens of the others and of the app itself.
 *
 * @param clock milliseconds of a monotonic clock (`SystemClock.elapsedRealtime`).
 * @param random fills an array with random bytes.
 */
class PreparedTokens(
    private val clock: () -> Long,
    private val random: (ByteArray) -> Unit = defaultRandom::nextBytes,
    private val lifetimeMillis: Long = LIFETIME_MILLIS,
    private val capacity: Int = CAPACITY,
    private val perCallerCapacity: Int = PER_CALLER_CAPACITY,
) {
    /** A prepared file: where it is, its MIME type and the name in its URI. */
    data class Entry(val path: String, val mimeType: String, val fileName: String)

    private class Registered(val token: ByteArray, val entry: Entry, val created: Long, val caller: Int?)

    // Oldest first.
    private val registered = ArrayDeque<Registered>()

    /**
     * A new token for [entry], as 32 lowercase hex characters. [caller] is
     * the uid of the other app it is made for, whose oldest token goes when
     * it has [perCallerCapacity] live ones; null for the app's own (the
     * picker, sharing), which only [capacity] limits.
     */
    @Synchronized
    fun register(entry: Entry, caller: Int? = null): String {
        val now = clock()
        dropExpired(now)
        if (caller != null) {
            while (registered.count { it.caller == caller } >= perCallerCapacity) {
                registered.removeAt(registered.indexOfFirst { it.caller == caller })
            }
        }
        while (registered.size >= capacity) registered.removeFirst()
        val bytes = ByteArray(TOKEN_BYTES)
        random(bytes)
        val token = hex(bytes)
        registered.addLast(Registered(token.toByteArray(Charsets.US_ASCII), entry, now, caller))
        return token
    }

    /** Revokes every token: no URI handed out before opens a file any more. */
    @Synchronized
    fun clear() {
        registered.clear()
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

        /** At most this many live tokens made for one other app (by uid); its oldest goes first. */
        const val PER_CALLER_CAPACITY = 64

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
