package io.github.dolfost.stickpile.ime

import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.provider.kind
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.ItemList
import io.github.dolfost.stickpile.rust.KeepReason
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.Motion
import io.github.dolfost.stickpile.rust.OfflineScope
import io.github.dolfost.stickpile.rust.PackList
import io.github.dolfost.stickpile.rust.Prepared
import io.github.dolfost.stickpile.rust.SearchHint
import io.github.dolfost.stickpile.rust.StickpileLibrary
import io.github.dolfost.stickpile.rust.targetForPackage
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.withContext

/**
 * The picker's way to the app's one [StickpileLibrary], in process (never
 * through the provider). Every call runs on [Dispatchers.IO], the
 * non-suspending ones included, and so does opening the library on first
 * use: the main thread never waits for Rust. Throws [LibraryException].
 */
internal class ImeLibrary(private val app: StickpileApp) {
    private suspend fun <T> call(block: suspend (StickpileLibrary) -> T): T =
        withContext(Dispatchers.IO) { block(app.library) }

    suspend fun recent(): List<Item> = call { it.recent(LIMIT) }

    suspend fun favorites(): ItemList = call { it.favorites(LIMIT) }

    suspend fun mostUsed(): ItemList = call { it.mostUsed(LIMIT) }

    /** The offers under the search field for [query] as typed: no network, but native, so off the main thread too. */
    suspend fun hints(query: String): List<SearchHint> = call { it.hints(query) }

    /** Moves on after each sync that changed the cache, the app's included: the offers are asked for again then. */
    val cacheChanges: StateFlow<Int> get() = app.cacheChanges.count

    /** The package of the field being typed into, set by the service on each input start. */
    @Volatile
    var editorPackage: String = ""

    /**
     * Search ranked for the editor's app (the bridge maps its package to a
     * [io.github.dolfost.stickpile.rust.Target]; a native call too, so off
     * the main thread with the rest).
     */
    suspend fun search(query: String): ItemList {
        val editor = editorPackage
        return call { it.search(query, targetForPackage(editor), LIMIT) }
    }

    suspend fun packs(): PackList = call { it.packs() }

    suspend fun packItems(packId: String): ItemList = call { it.packItems(packId) }

    /** [query] inside the set [packId] only (the whole syntax; searched by the library, offline too). */
    suspend fun packSearch(packId: String, query: String): ItemList = call { it.packSearch(packId, query) }

    suspend fun thumbnail(itemId: String): String = call { it.thumbnail(itemId) }

    /** The file of a pack's cover; null for a pack without one. */
    suspend fun packCover(packId: String): String? = call { it.packCover(packId) }

    /** The file to play for a moving item; [download] false: only a cached one. */
    suspend fun motion(itemId: String, download: Boolean): Motion? = call { it.motion(itemId, download) }

    suspend fun prepare(itemId: String, editorPackage: String, mimeTypes: List<String>, sticker: Boolean): Prepared =
        call { it.prepare(itemId, editorPackage, mimeTypes, sticker) }

    suspend fun recordUse(itemId: String, variant: String, editorPackage: String) =
        call { it.recordUse(itemId, variant, editorPackage) }

    suspend fun toggleLike(itemId: String): Boolean = call { it.toggleLike(itemId) }

    /** What the field being typed into takes, set by the service on each input start. */
    @Volatile
    var fieldTypes: List<String> = emptyList()

    /**
     * Which of [ids] a pick would insert or share with the server away: a
     * rendition for the field (or, failing that, for sharing,
     * [InsertFlow.SHARE_TYPES]) is on the phone. The marks of the tiles;
     * nothing is downloaded.
     */
    suspend fun availableOffline(ids: List<String>): List<Boolean> {
        val editor = editorPackage
        val types = (fieldTypes + InsertFlow.SHARE_TYPES).distinct()
        return call { it.availableOffline(ids, editor, types, false) }
    }

    /** The lists answer from the cache now: the server was found away lately. */
    suspend fun serverUnreachable(): Boolean = call { it.serverUnreachable() }

    /** Why the sticker is kept offline (empty: it is not). */
    suspend fun offlineReasons(itemId: String): List<KeepReason> = call { it.offlineReasons(itemId) }

    /**
     * Keep the sticker offline by itself, or stop; after a keep the app's
     * downloads run (its WorkManager job, as the app's settings allow): the
     * keyboard downloads nothing itself.
     */
    suspend fun setKept(itemId: String, keep: Boolean) {
        val scope = OfflineScope.Sticker(itemId)
        call { if (keep) it.keepOffline(scope) else it.releaseOffline(scope) }
        if (keep) app.requestOfflineRun()
    }

    /**
     * Whether an account is signed in, asking the server (for when there is
     * nothing to show yet). Unknown (offline, no cached account) counts as yes.
     */
    suspend fun signedIn(): Boolean = try {
        call { it.account() } != null
    } catch (e: LibraryException) {
        !ImeTexts.isSignedOut(e.kind)
    }

    /** A sync if one is due; one that changed the cache counts in [cacheChanges]. */
    suspend fun syncIfDue() {
        app.cacheChanges.after(call { it.syncIfDue() })
    }

    private companion object {
        /** Items in a tab. */
        val LIMIT = 60u
    }
}
