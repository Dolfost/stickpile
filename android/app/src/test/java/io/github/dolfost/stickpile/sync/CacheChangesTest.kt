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
}
