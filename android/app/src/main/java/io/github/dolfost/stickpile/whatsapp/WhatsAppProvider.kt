package io.github.dolfost.stickpile.whatsapp

import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.res.AssetFileDescriptor
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.Binder
import android.os.ParcelFileDescriptor
import android.os.Process
import android.util.Log
import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.rust.WhatsappPart
import io.github.dolfost.stickpile.ui.common.FailureLog
import java.io.FileNotFoundException

/**
 * Who may read the WhatsApp provider, besides what the manifest asks
 * (`com.whatsapp.sticker.READ`, which WhatsApp holds): WhatsApp, WhatsApp
 * Business, the app itself, and root (`adb root` on an emulator, to look
 * at the rows). Without this, any app that declares a permission of that
 * name where WhatsApp is not installed could read the packs.
 */
object WhatsAppAccess {
    /** Root's uid. */
    private const val ROOT_UID = 0

    fun allowed(callingUid: Int, ownUid: Int, packages: Array<String>?): Boolean =
        callingUid == ownUid || callingUid == ROOT_UID || packages.orEmpty().any { it in WhatsApp.APPS }
}

/**
 * Serves the packs made ready for WhatsApp (the bridge's
 * `whatsapp_unpacked`) to WhatsApp, the way its sample sticker app does
 * (docs/android.md, "WhatsApp provider"): `/metadata`,
 * `/metadata/<identifier>`, `/stickers/<identifier>` and
 * `/stickers_asset/<identifier>/<file name>`, read-only. Declared exported
 * with `android:readPermission="com.whatsapp.sticker.READ"`, as the
 * sample is, and checked here once more ([WhatsAppAccess]).
 *
 * Unknown paths throw [IllegalArgumentException] like the sample's; an
 * unknown pack is a cursor without rows; a file that is not one of a
 * pack's is a [FileNotFoundException].
 */
class WhatsAppProvider : ContentProvider() {
    override fun onCreate(): Boolean = true

    override fun query(
        uri: Uri,
        projection: Array<out String>?,
        selection: String?,
        selectionArgs: Array<out String>?,
        sortOrder: String?,
    ): Cursor {
        enforceAccess()
        val cursor = when (val route = route(uri)) {
            WhatsAppRoute.Metadata -> packs(parts())
            is WhatsAppRoute.Pack -> packs(listOfNotNull(WhatsAppRows.find(parts(), route.identifier)))
            is WhatsAppRoute.Stickers -> MatrixCursor(WhatsAppRows.STICKER_COLUMNS).apply {
                WhatsAppRows.find(parts(), route.identifier)?.stickers?.forEach { addRow(WhatsAppRows.sticker(it)) }
            }
            is WhatsAppRoute.Asset, null -> throw IllegalArgumentException("Unknown URI: $uri")
        }
        cursor.setNotificationUri(ctx().contentResolver, uri)
        return cursor
    }

    /** By the path alone ([WhatsAppRoutes.type]): no data is read, so anyone may ask. */
    override fun getType(uri: Uri): String? = route(uri)?.let(WhatsAppRoutes::type)

    override fun getTypeAnonymous(uri: Uri): String? = getType(uri)

    override fun openAssetFile(uri: Uri, mode: String): AssetFileDescriptor =
        AssetFileDescriptor(openFile(uri, mode), 0, AssetFileDescriptor.UNKNOWN_LENGTH)

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        enforceAccess()
        if (mode != "r") throw SecurityException("read-only")
        val route = route(uri) as? WhatsAppRoute.Asset ?: throw FileNotFoundException("not a file")
        val file = WhatsAppRows.file(parts(), route.identifier, route.fileName)
            ?: throw FileNotFoundException("no such sticker")
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri? = throw UnsupportedOperationException("read-only")

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int =
        throw UnsupportedOperationException("read-only")

    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?): Int =
        throw UnsupportedOperationException("read-only")

    private fun packs(parts: List<WhatsappPart>): MatrixCursor =
        MatrixCursor(WhatsAppRows.PACK_COLUMNS).apply { parts.forEach { addRow(WhatsAppRows.pack(it)) } }

    /** The unpacked parts; none when the library cannot be opened. Reads files only, no network. */
    private fun parts(): List<WhatsappPart> = try {
        StickpileApp.from(ctx()).library.whatsappUnpacked()
    } catch (e: Exception) {
        Log.w(FailureLog.TAG, "WhatsApp provider: no packs: ${e.javaClass.simpleName}")
        emptyList()
    }

    private fun enforceAccess() {
        val uid = Binder.getCallingUid()
        if (!WhatsAppAccess.allowed(uid, Process.myUid(), ctx().packageManager.getPackagesForUid(uid))) {
            throw SecurityException("only WhatsApp reads these packs")
        }
    }

    private fun route(uri: Uri): WhatsAppRoute? = WhatsAppRoutes.parse(uri.authority, uri.encodedPath)

    private fun ctx(): Context = context ?: throw IllegalStateException("provider not attached")
}
