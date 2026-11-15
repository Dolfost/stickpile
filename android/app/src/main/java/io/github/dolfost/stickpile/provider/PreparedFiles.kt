package io.github.dolfost.stickpile.provider

import android.content.Context
import android.net.Uri
import android.os.SystemClock
import androidx.core.net.toUri
import io.github.dolfost.stickpile.rust.Prepared
import java.io.File

/**
 * Prepared files as provider URIs (docs/android.md, "Provider contract",
 * `/prepared/<token>/<file name>`): what the input method commits and what
 * the app shares. The tokens live in this process's memory, where
 * [StickerProvider] (same process) checks them.
 */
object PreparedFiles {
    /** Every token of this process, valid for one hour. */
    internal val tokens = PreparedTokens(clock = SystemClock::elapsedRealtime)

    /**
     * A `content://io.github.dolfost.stickpile.stickers/prepared/<token>/<name>`
     * URI for a file [io.github.dolfost.stickpile.rust.StickpileLibrary.prepare]
     * returned, readable by anyone holding it for one hour. Each call makes
     * a new token. The URI carries the token: never log it.
     */
    @Suppress("UNUSED_PARAMETER")
    fun uriFor(context: Context, prepared: Prepared): Uri {
        val name = fileName(prepared)
        val token = tokens.register(PreparedTokens.Entry(prepared.path, prepared.mimeType, name))
        return ProviderRoutes.preparedUri(token, name).toUri()
    }

    /**
     * Revokes every token at once, so no URI handed out before opens a file
     * any more: after a sign-out, a change of server or account, and when the
     * cache is cleared. Stub until the keyboard fixes fill it in; the
     * signature is fixed, the settings and onboarding screens call it.
     */
    fun revokeAll() {}

    private fun fileName(prepared: Prepared): String =
        ProviderRoutes.fileName(prepared.displayName.ifEmpty { File(prepared.path).name })
}
