package io.github.dolfost.stickpile.ime

import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.provider.kind
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.ItemList
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.PackList
import io.github.dolfost.stickpile.rust.Prepared
import io.github.dolfost.stickpile.rust.StickpileLibrary
import io.github.dolfost.stickpile.rust.targetForPackage
import kotlinx.coroutines.Dispatchers
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

    /** The package of the field being typed into, set by the service on each input start. */
    @Volatile
    var editorPackage: String = ""

    /** Search ranked for the editor's app (the bridge maps its package to a [Target]). */
    suspend fun search(query: String): ItemList {
        val target = targetForPackage(editorPackage)
        return call { it.search(query, target, LIMIT) }
    }

    suspend fun packs(): PackList = call { it.packs() }

    suspend fun packItems(packId: String): ItemList = call { it.packItems(packId) }

    suspend fun thumbnail(itemId: String): String = call { it.thumbnail(itemId) }

    suspend fun prepare(itemId: String, editorPackage: String, mimeTypes: List<String>, sticker: Boolean): Prepared =
        call { it.prepare(itemId, editorPackage, mimeTypes, sticker) }

    suspend fun recordUse(itemId: String, variant: String, editorPackage: String) =
        call { it.recordUse(itemId, variant, editorPackage) }

    suspend fun toggleLike(itemId: String): Boolean = call { it.toggleLike(itemId) }

    /**
     * Whether an account is signed in, asking the server (for when there is
     * nothing to show yet). Unknown (offline, no cached account) counts as yes.
     */
    suspend fun signedIn(): Boolean = try {
        call { it.account() } != null
    } catch (e: LibraryException) {
        !ImeTexts.isSignedOut(e.kind)
    }

    suspend fun syncIfDue() {
        call { it.syncIfDue() }
    }

    private companion object {
        /** Items in a tab. */
        val LIMIT = 60u
    }
}
