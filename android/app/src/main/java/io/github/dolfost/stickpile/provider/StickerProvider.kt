package io.github.dolfost.stickpile.provider

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.net.Uri

/**
 * Serves stickers to keyboards (docs/android.md, "Provider contract").
 * Exported; access is checked in code at every entry point.
 *
 * Stub: answers nothing. Queries return null (as for an unknown path),
 * files cannot be opened, writes are refused.
 */
class StickerProvider : ContentProvider() {
    override fun onCreate(): Boolean = true

    override fun query(
        uri: Uri,
        projection: Array<out String>?,
        selection: String?,
        selectionArgs: Array<out String>?,
        sortOrder: String?,
    ): Cursor? = null

    override fun getType(uri: Uri): String? = null

    override fun insert(uri: Uri, values: ContentValues?): Uri? =
        throw UnsupportedOperationException("read-only")

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int =
        throw UnsupportedOperationException("read-only")

    override fun update(
        uri: Uri,
        values: ContentValues?,
        selection: String?,
        selectionArgs: Array<out String>?,
    ): Int = throw UnsupportedOperationException("read-only")

    companion object {
        /** The provider's authority; the manifest declares the same. */
        const val AUTHORITY = "io.github.dolfost.stickpile.stickers"
    }
}
