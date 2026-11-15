package io.github.dolfost.stickpile.provider

import java.security.MessageDigest

/**
 * The pure decisions of [KeyboardAllowlist]: an approval holds for the app
 * that was approved, by its signing certificates, not for any app that
 * later takes its package name; and only input methods are listed for
 * approval. The allowlist passes what `PackageManager` and
 * `InputMethodManager` said.
 */
object KeyboardTrust {
    /**
     * The SHA-256 of each signing certificate (its DER encoding, as
     * `Signature.toByteArray` gives it), as lowercase hex; null when there
     * is none (the package could not be read).
     */
    fun digests(certificates: List<ByteArray>): Set<String>? {
        if (certificates.isEmpty()) return null
        return certificates.map { cert -> hex(MessageDigest.getInstance("SHA-256").digest(cert)) }.toSet()
    }

    /**
     * Whether an approved package may read: the certificates stored when it
     * was approved are the ones it is signed with now, all of them (an app
     * signed by several keys needs every one). Nothing stored, or nothing
     * readable now, never matches.
     */
    fun matches(stored: Set<String>?, current: Set<String>?): Boolean =
        !stored.isNullOrEmpty() && stored == current

    /**
     * What an approval pins: the certificates of the app installed now,
     * else those it had when it asked (when the app is not visible to this
     * one any more); null when neither is known, and then nothing is
     * approved.
     */
    fun pinFor(current: Set<String>?, whenAsked: Set<String>?): Set<String>? =
        current?.takeIf { it.isNotEmpty() } ?: whenAsked?.takeIf { it.isNotEmpty() }

    /**
     * Whether a refused caller's package is listed for approval: only an
     * input method (a package with a service in
     * `InputMethodManager.getInputMethodList()`); any other app is refused
     * without being listed.
     */
    fun isKeyboard(packageName: String, inputMethodPackages: Collection<String>): Boolean =
        packageName in inputMethodPackages

    private fun hex(bytes: ByteArray): String = buildString(bytes.size * 2) {
        for (b in bytes) {
            val v = b.toInt() and 0xff
            append(DIGITS[v shr 4]).append(DIGITS[v and 0x0f])
        }
    }

    private const val DIGITS = "0123456789abcdef"
}
