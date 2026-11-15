package io.github.dolfost.stickpile.provider

import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.pm.PackageManager
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.Binder
import android.os.Build
import android.os.Bundle
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.os.Process
import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.StickpileLibrary
import io.github.dolfost.stickpile.rust.targetForPackage
import java.io.File
import java.io.FileNotFoundException
import kotlinx.coroutines.runBlocking

/**
 * Serves stickers to keyboards (docs/android.md, "Provider contract").
 * Exported without a permission; access is checked here, at every entry
 * point ([ProviderAccess]): the app's own uid, or a package the user
 * approved in [KeyboardAllowlist]. A refused caller is noted as pending and
 * gets a [SecurityException]. The one exception is `/prepared/<token>/...`,
 * which anyone presenting a valid token may read ([PreparedTokens]).
 *
 * Paths are parsed by [ProviderRoutes]; anything else is no route. Nothing
 * can be written, and files open read-only.
 *
 * Calls from other apps arrive on binder threads and may wait for the
 * library there. The app's own parts (the input method, the screens) use
 * the library directly, never this provider: a call that needs the library
 * on the main thread is refused.
 */
class StickerProvider : ContentProvider() {
    private val allowlist by lazy { KeyboardAllowlist(ctx()) }

    override fun onCreate(): Boolean = true

    override fun query(
        uri: Uri,
        projection: Array<out String>?,
        selection: String?,
        selectionArgs: Array<out String>?,
        sortOrder: String?,
    ): Cursor? {
        val route = route(uri)
        if (route is ProviderRoute.PreparedFile) {
            val entry = PreparedFiles.tokens.lookup(route.token, route.fileName) ?: return null
            val (columns, values) = ProviderRows.openable(projection, entry.fileName, File(entry.path).length())
            return MatrixCursor(columns, 1).apply { addRow(values) }
        }
        enforceAccess()
        return when (route) {
            is ProviderRoute.Search -> items {
                it.search(route.query, targetForPackage(route.editorPackage), route.limit.toUInt()).items
            }
            is ProviderRoute.Recent -> items { it.recent(route.limit.toUInt()) }
            is ProviderRoute.Favorites -> items { it.favorites(route.limit.toUInt()).items }
            is ProviderRoute.PackItems -> items { it.packItems(route.packId).items }
            ProviderRoute.Packs -> packs()
            is ProviderRoute.Thumbnail, is ProviderRoute.PreparedFile, null -> null
        }
    }

    override fun getType(uri: Uri): String? {
        val route = route(uri)
        if (route is ProviderRoute.PreparedFile) {
            return PreparedFiles.tokens.lookup(route.token, route.fileName)?.mimeType
        }
        enforceAccess()
        return if (route is ProviderRoute.Thumbnail) THUMBNAIL_TYPE else null
    }

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        val route = route(uri)
        if (route is ProviderRoute.PreparedFile) {
            requireReadOnly(mode)
            // No detail in the message: the URI carries the token.
            val entry = PreparedFiles.tokens.lookup(route.token, route.fileName)
                ?: throw FileNotFoundException("no such prepared file")
            return ParcelFileDescriptor.open(File(entry.path), ParcelFileDescriptor.MODE_READ_ONLY)
        }
        enforceAccess()
        requireReadOnly(mode)
        if (route !is ProviderRoute.Thumbnail) throw FileNotFoundException("not a file")
        val path = try {
            withLibrary { it.thumbnail(route.itemId) }
        } catch (e: LibraryException) {
            throw FileNotFoundException("no thumbnail: ${e.kind}")
        }
        return ParcelFileDescriptor.open(File(path), ParcelFileDescriptor.MODE_READ_ONLY)
    }

    override fun call(method: String, arg: String?, extras: Bundle?): Bundle? {
        enforceAccess()
        return when (method) {
            METHOD_INFO -> Bundle().apply {
                putInt(KEY_CONTRACT_VERSION, CONTRACT_VERSION)
                putString(KEY_APP_VERSION, appVersion())
            }
            METHOD_PREPARE -> prepare(arg, extras)
            METHOD_USED -> used(arg, extras)
            else -> null
        }
    }

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

    private fun prepare(itemId: String?, extras: Bundle?): Bundle {
        if (itemId == null || !ProviderRoutes.isId(itemId)) return errorBundle(ProviderErrors.NOT_FOUND)
        val editorPackage = extras?.getString(KEY_PACKAGE).orEmpty()
        val mimeTypes = extras?.getStringArray(KEY_MIME_TYPES)?.toList().orEmpty()
        val sticker = extras?.getBoolean(KEY_STICKER, false) ?: false
        return try {
            val prepared = withLibrary { it.prepare(itemId, editorPackage, mimeTypes, sticker) }
            val uri = PreparedFiles.uriFor(ctx(), prepared)
            Bundle().apply {
                putString(KEY_URI, uri.toString())
                putString(KEY_MIME_TYPE, prepared.mimeType)
                putString(KEY_LABEL, prepared.displayName)
            }
        } catch (e: LibraryException) {
            errorBundle(ProviderErrors.code(e.kind))
        }
    }

    private fun used(itemId: String?, extras: Bundle?): Bundle {
        if (itemId == null || !ProviderRoutes.isId(itemId)) return errorBundle(ProviderErrors.NOT_FOUND)
        val variant = extras?.getString(KEY_VARIANT).orEmpty()
        val editorPackage = extras?.getString(KEY_PACKAGE).orEmpty()
        return try {
            withLibrary { it.recordUse(itemId, variant, editorPackage) }
            Bundle()
        } catch (e: LibraryException) {
            errorBundle(ProviderErrors.code(e.kind))
        }
    }

    /** A cursor of items; on a library error, no rows and the error's code in its extras. */
    private fun items(load: suspend (StickpileLibrary) -> List<Item>): Cursor {
        val cursor = MatrixCursor(ProviderRows.ITEM_COLUMNS)
        try {
            withLibrary(load).forEachIndexed { row, item -> cursor.addRow(ProviderRows.item(row, item)) }
        } catch (e: LibraryException) {
            cursor.extras = errorBundle(ProviderErrors.code(e.kind))
        }
        return cursor
    }

    private fun packs(): Cursor {
        val cursor = MatrixCursor(ProviderRows.PACK_COLUMNS)
        try {
            withLibrary { it.packs().packs }.forEachIndexed { row, pack -> cursor.addRow(ProviderRows.pack(row, pack)) }
        } catch (e: LibraryException) {
            cursor.extras = errorBundle(ProviderErrors.code(e.kind))
        }
        return cursor
    }

    /**
     * Runs [block] on the library and waits for it: on a binder thread, never
     * on the main thread (the app's own parts call the library directly).
     */
    private fun <T> withLibrary(block: suspend (StickpileLibrary) -> T): T {
        check(Looper.myLooper() != Looper.getMainLooper()) {
            "the app's own code calls the library directly, not through the provider"
        }
        val library = StickpileApp.from(ctx()).library
        return runBlocking { block(library) }
    }

    /** Allows the app itself and approved keyboards; refuses (and notes) anyone else. */
    private fun enforceAccess() {
        val decision = ProviderAccess.decide(
            callingUid = Binder.getCallingUid(),
            ownUid = Process.myUid(),
            callingPackage = callingPackage,
            packagesOf = ctx().packageManager::getPackagesForUid,
            isApproved = allowlist::isApproved,
        )
        if (decision is ProviderAccess.Decision.Deny) {
            decision.packages.forEach(allowlist::notePending)
            throw SecurityException("not a keyboard the user approved")
        }
    }

    private fun route(uri: Uri): ProviderRoute? = ProviderRoutes.parse(uri.authority, uri.encodedPath, uri.encodedQuery)

    private fun requireReadOnly(mode: String) {
        if (mode != "r") throw SecurityException("read-only")
    }

    private fun errorBundle(code: String) = Bundle().apply { putString(KEY_ERROR, code) }

    private fun appVersion(): String {
        val context = ctx()
        val info = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.packageManager.getPackageInfo(context.packageName, PackageManager.PackageInfoFlags.of(0))
        } else {
            @Suppress("DEPRECATION")
            context.packageManager.getPackageInfo(context.packageName, 0)
        }
        return info.versionName.orEmpty()
    }

    private fun ctx(): Context = context ?: throw IllegalStateException("provider not attached")

    companion object {
        /** The provider's authority; the manifest declares the same. */
        const val AUTHORITY = ProviderRoutes.AUTHORITY

        /** The contract's version, answered by `call("info")`. */
        const val CONTRACT_VERSION = 1

        /** Thumbnails are WebP. */
        const val THUMBNAIL_TYPE = "image/webp"

        const val METHOD_INFO = "info"
        const val METHOD_PREPARE = "prepare"
        const val METHOD_USED = "used"

        const val KEY_CONTRACT_VERSION = "contract_version"
        const val KEY_APP_VERSION = "app_version"
        const val KEY_PACKAGE = "package"
        const val KEY_MIME_TYPES = "mime_types"
        const val KEY_STICKER = "sticker"
        const val KEY_VARIANT = "variant"
        const val KEY_URI = "uri"
        const val KEY_MIME_TYPE = "mime_type"
        const val KEY_LABEL = "label"
        const val KEY_ERROR = "error"
    }
}
