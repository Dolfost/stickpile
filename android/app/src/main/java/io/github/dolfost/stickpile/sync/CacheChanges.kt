package io.github.dolfost.stickpile.sync

import io.github.dolfost.stickpile.rust.SyncState
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

/**
 * The syncs that changed the offline cache, counted (pure, no Android).
 * The offers under the search fields of the app and the keyboard
 * (`StickpileLibrary.hints`) come from the cache, so offers shown while the
 * first sync still ran stay empty until the text changes, unless they are
 * asked for again when [count] moves. Every sync reports to [after]: the
 * app's, the keyboard's and the periodic one; they share one process, so
 * one of these ([io.github.dolfost.stickpile.StickpileApp.cacheChanges]).
 * A sync that went through also calls [synced]: the offline downloads
 * follow each sync (docs/offline.md).
 */
class CacheChanges(private val synced: () -> Unit = {}) {
    private val changes = MutableStateFlow(0)

    /** Moves on after each sync that changed the cache. */
    val count: StateFlow<Int> = changes.asStateFlow()

    /** [state] is what a sync returned: counted when it changed the cache. */
    fun after(state: SyncState) {
        if (state.cacheChanged) changes.update { it + 1 }
        if (state.synced) synced()
    }
}
