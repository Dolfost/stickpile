package io.github.dolfost.stickpile.sync

import io.github.dolfost.stickpile.rust.SyncState
import org.junit.Assert.assertEquals
import org.junit.Test

class CacheChangesTest {
    private fun synced(cacheChanged: Boolean) =
        SyncState(lastSync = 1_700_000_000, lastError = null, pendingUsage = 0u, due = false, cacheChanged = cacheChanged)

    @Test
    fun onlySyncsThatChangedTheCacheCount() {
        val changes = CacheChanges()
        assertEquals(0, changes.count.value)
        changes.after(synced(cacheChanged = false))
        assertEquals(0, changes.count.value)
        changes.after(synced(cacheChanged = true))
        changes.after(synced(cacheChanged = true))
        assertEquals(2, changes.count.value)
    }

    /** The offline downloads follow each sync that went through, not a call that ran none. */
    @Test
    fun aSyncThatWentThroughIsHeardOf() {
        var heard = 0
        val changes = CacheChanges(synced = { heard++ })
        changes.after(synced(cacheChanged = false).copy(synced = true))
        changes.after(synced(cacheChanged = false))
        changes.after(synced(cacheChanged = true).copy(synced = true))
        assertEquals(2, heard)
    }
}
