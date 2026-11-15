package io.github.dolfost.stickpile.provider

import android.content.Context
import android.net.Uri
import io.github.dolfost.stickpile.rust.Prepared

/**
 * Prepared files as provider URIs (docs/android.md, "Provider contract",
 * `/prepared/<token>/<file name>`): what the input method commits and what
 * the app shares. Stub: the keyboard work package fills it in; the
 * signature is fixed, the library screen calls it too.
 */
object PreparedFiles {
    /**
     * A `content://io.github.dolfost.stickpile.stickers/prepared/<token>/<name>`
     * URI for a file [io.github.dolfost.stickpile.rust.StickpileLibrary.prepare]
     * returned, readable by anyone holding it for one hour.
     */
    fun uriFor(context: Context, prepared: Prepared): Uri = Uri.EMPTY
}
