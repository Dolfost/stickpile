package io.github.dolfost.stickpile.ui.settings

import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import io.github.dolfost.stickpile.provider.KeyboardAllowlist
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** A keyboard (another app) by package; [label] is null when the app is not visible to this one. */
data class KeyboardEntry(val packageName: String, val label: String?) {
    val title: String get() = label ?: packageName
}

/** The keyboards the user let read the stickers, and those that asked and were refused. */
data class KeyboardAccess(val approved: List<KeyboardEntry>, val pending: List<KeyboardEntry>)

/** Order for the lists: by what is shown, case ignored. */
fun sortedEntries(entries: Collection<KeyboardEntry>): List<KeyboardEntry> =
    entries.sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.title })

/** [KeyboardAllowlist] with the apps' labels, read off the main thread. */
suspend fun readKeyboardAccess(context: Context): KeyboardAccess = withContext(Dispatchers.IO) {
    val allowlist = KeyboardAllowlist(context)
    val pm = context.packageManager
    fun entry(pkg: String) = KeyboardEntry(pkg, labelOf(pm, pkg))
    KeyboardAccess(
        approved = sortedEntries(allowlist.approved().map(::entry)),
        pending = sortedEntries(allowlist.pending().map(::entry)),
    )
}

/** Change the allowlist off the main thread (its preferences may still be loading). */
suspend fun changeKeyboardAccess(context: Context, change: KeyboardAllowlist.() -> Unit) {
    withContext(Dispatchers.IO) { KeyboardAllowlist(context).change() }
}

private fun labelOf(pm: PackageManager, pkg: String): String? = try {
    val info = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        pm.getApplicationInfo(pkg, PackageManager.ApplicationInfoFlags.of(0))
    } else {
        @Suppress("DEPRECATION")
        pm.getApplicationInfo(pkg, 0)
    }
    info.loadLabel(pm).toString()
} catch (e: PackageManager.NameNotFoundException) {
    null
}
