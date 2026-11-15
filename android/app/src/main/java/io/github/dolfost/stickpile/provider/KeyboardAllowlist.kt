package io.github.dolfost.stickpile.provider

import android.content.Context
import androidx.core.content.edit

/**
 * The keyboards (other apps, by package name) the user lets read the
 * stickers through [StickerProvider] (docs/android.md, "Provider
 * contract"). A refused package is remembered as pending, so that the
 * settings screen can offer it for approval. Stored in the app's private
 * preferences; this app's own uid needs no entry.
 */
class KeyboardAllowlist(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** Packages the user approved. */
    fun approved(): Set<String> = read(APPROVED)

    /** Packages that asked and were refused, not yet approved or dismissed. */
    fun pending(): Set<String> = read(PENDING)

    fun isApproved(packageName: String): Boolean = packageName in approved()

    /** A refused caller: remember it for the settings screen. */
    @Synchronized
    fun notePending(packageName: String) {
        if (!isApproved(packageName) && packageName !in pending()) write(PENDING, pending() + packageName)
    }

    @Synchronized
    fun approve(packageName: String) {
        write(APPROVED, approved() + packageName)
        write(PENDING, pending() - packageName)
    }

    @Synchronized
    fun revoke(packageName: String) {
        write(APPROVED, approved() - packageName)
    }

    @Synchronized
    fun dismiss(packageName: String) {
        write(PENDING, pending() - packageName)
    }

    // getStringSet's set must not be changed: always copy.
    private fun read(key: String): Set<String> = prefs.getStringSet(key, emptySet())?.toSet() ?: emptySet()

    private fun write(key: String, value: Set<String>) {
        prefs.edit { putStringSet(key, value.toSet()) }
    }

    private companion object {
        const val PREFS = "keyboard_allowlist"
        const val APPROVED = "approved"
        const val PENDING = "pending"
    }
}
