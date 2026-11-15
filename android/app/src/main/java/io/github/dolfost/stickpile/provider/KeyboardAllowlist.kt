package io.github.dolfost.stickpile.provider

import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import android.view.inputmethod.InputMethodManager
import androidx.core.content.edit

/**
 * The keyboards (other apps) the user lets read the stickers through
 * [StickerProvider] (docs/android.md, "Provider contract"). Stored in the
 * app's private preferences; this app's own uid needs no entry.
 *
 * An approval is for the app approved, not for its package name: [approve]
 * stores the SHA-256 of the app's signing certificates, and [isApproved]
 * holds only while the app installed under that name is signed with the
 * same ones ([KeyboardTrust]). A different app installed later under an
 * approved name is refused.
 *
 * Only input methods are listed: a refused caller that is a keyboard is
 * remembered as pending ([notePending]), so that the settings screen can
 * offer it for approval; any other app is refused without being listed.
 *
 * Reads the package manager: call off the main thread.
 */
class KeyboardAllowlist(context: Context) {
    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** Packages the user approved. */
    fun approved(): Set<String> = read(APPROVED)

    /** Keyboards that asked and were refused, not yet approved or dismissed. */
    fun pending(): Set<String> = read(PENDING)

    /** The user approved [packageName], and the app installed under it now is the one approved. */
    fun isApproved(packageName: String): Boolean =
        packageName in approved() && KeyboardTrust.matches(certificates(packageName), currentCertificates(packageName))

    /**
     * A refused caller: remember it for the settings screen, when it is a
     * keyboard. An approved name now signed with other certificates
     * (another app took the name) loses its approval and asks again.
     */
    @Synchronized
    fun notePending(packageName: String) {
        if (!KeyboardTrust.isKeyboard(packageName, inputMethodPackages())) return
        if (isApproved(packageName)) return
        val current = currentCertificates(packageName)
        // Unreadable now: no reason to drop an approval.
        if (current == null && packageName in approved()) return
        prefs.edit {
            putStringSet(APPROVED, approved() - packageName)
            putStringSet(PENDING, pending() + packageName)
            // What it was signed with when it asked, for [approve] when it cannot be read then.
            if (current != null) putStringSet(certificatesKey(packageName), current) else remove(certificatesKey(packageName))
        }
    }

    /**
     * Approves [packageName] for the app installed under it now (else the
     * one that asked), by its signing certificates. When neither can be
     * read, nothing is approved and it stays pending.
     */
    @Synchronized
    fun approve(packageName: String) {
        val pin = KeyboardTrust.pinFor(currentCertificates(packageName), certificates(packageName))
        if (pin == null) {
            Log.w(TAG, "approval not stored: no signing certificate to tie it to")
            return
        }
        prefs.edit {
            putStringSet(certificatesKey(packageName), pin)
            putStringSet(APPROVED, approved() + packageName)
            putStringSet(PENDING, pending() - packageName)
        }
    }

    @Synchronized
    fun revoke(packageName: String) {
        prefs.edit {
            putStringSet(APPROVED, approved() - packageName)
            if (packageName !in pending()) remove(certificatesKey(packageName))
        }
    }

    @Synchronized
    fun dismiss(packageName: String) {
        prefs.edit {
            putStringSet(PENDING, pending() - packageName)
            if (packageName !in approved()) remove(certificatesKey(packageName))
        }
    }

    // getStringSet's set must not be changed: always copy.
    private fun read(key: String): Set<String> = prefs.getStringSet(key, emptySet())?.toSet() ?: emptySet()

    /** The certificates stored for [packageName]: pinned when approved, seen when it asked. */
    private fun certificates(packageName: String): Set<String>? =
        read(certificatesKey(packageName)).ifEmpty { null }

    /** The SHA-256 of the signing certificates of the app installed as [packageName]; null when it cannot be read. */
    private fun currentCertificates(packageName: String): Set<String>? = try {
        KeyboardTrust.digests(signingCertificates(app.packageManager, packageName))
    } catch (e: PackageManager.NameNotFoundException) {
        // Not installed, or not visible to this app.
        null
    }

    /** The packages of the input methods installed (an app's own call makes it visible to this one). */
    private fun inputMethodPackages(): Set<String> =
        app.getSystemService(InputMethodManager::class.java)?.inputMethodList.orEmpty().map { it.packageName }.toSet()

    private companion object {
        const val TAG = "stickpile.provider"
        const val PREFS = "keyboard_allowlist"
        const val APPROVED = "approved"
        const val PENDING = "pending"

        /** Package names have no `/`. */
        fun certificatesKey(packageName: String) = "certificates/$packageName"

        /**
         * The certificates [packageName] is signed with now (on API 28+ the
         * current signers, not the past ones of a key rotation). Every one
         * of them is compared, not just one (the weakness the lint check is
         * about); below API 28 `GET_SIGNATURES` is the only way.
         */
        @SuppressLint("PackageManagerGetSignatures")
        @Suppress("DEPRECATION")
        fun signingCertificates(pm: PackageManager, packageName: String): List<ByteArray> {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                val info = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    pm.getPackageInfo(
                        packageName,
                        PackageManager.PackageInfoFlags.of(PackageManager.GET_SIGNING_CERTIFICATES.toLong()),
                    )
                } else {
                    pm.getPackageInfo(packageName, PackageManager.GET_SIGNING_CERTIFICATES)
                }
                return info.signingInfo?.apkContentsSigners.orEmpty().map { it.toByteArray() }
            }
            return pm.getPackageInfo(packageName, PackageManager.GET_SIGNATURES).signatures.orEmpty().map {
                it.toByteArray()
            }
        }
    }
}
