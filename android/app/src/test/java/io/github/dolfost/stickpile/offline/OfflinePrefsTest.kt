package io.github.dolfost.stickpile.offline

import io.github.dolfost.stickpile.rust.OfflineOptions
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

/** How the offline options and the pause are stored next to the display settings (offline.OfflinePrefs). */
class OfflinePrefsTest {
    @Test
    fun nothingStoredIsTheCoresDefaults() {
        val read = OfflinePrefs.read(emptyMap<String, Any>())
        assertEquals(OfflineChoices(), read)
        // docs/offline.md: stills and animations on, videos off, 1024 MiB,
        // thumbnails kept, the rules off, no mobile data.
        val o = read.options
        assertEquals(
            listOf(true, true, true, false, 0uL, 1024uL, true, false, 200u, false, false, false),
            listOf(
                o.stickerRenditions, o.stills, o.animations, o.videos, o.maxFileMb, o.maxMb,
                o.autoThumbnails, o.autoUsed, o.autoUsedCount, o.autoLiked, o.autoOwnPacks, o.mobileData,
            ),
        )
        assertFalse(read.paused)
    }

    @Test
    fun whatIsWrittenIsReadBack() {
        val choices = OfflineChoices(
            OfflineOptions(
                stickerRenditions = false,
                stills = false,
                animations = false,
                videos = true,
                maxFileMb = 5u,
                maxMb = 0u,
                autoThumbnails = false,
                autoUsed = true,
                autoUsedCount = 50u,
                autoLiked = true,
                autoOwnPacks = true,
                mobileData = true,
            ),
            paused = true,
        )
        val stored = OfflinePrefs.write(choices)
        assertEquals(choices, OfflinePrefs.read(stored))
        // One key each, of the types SharedPreferences keeps.
        assertEquals(13, stored.size)
        assertEquals(5L, stored[OfflinePrefs.MAX_FILE_MB])
        assertEquals(50, stored[OfflinePrefs.AUTO_USED_COUNT])
        assertEquals(true, stored[OfflinePrefs.MOBILE_DATA])
    }

    @Test
    fun aMissingOrWrongValueIsItsDefault() {
        val read = OfflinePrefs.read(
            mapOf(
                OfflinePrefs.VIDEOS to "yes",
                OfflinePrefs.MAX_MB to -1L,
                OfflinePrefs.MAX_FILE_MB to 7,
                OfflinePrefs.MOBILE_DATA to true,
            ),
        )
        assertEquals(OfflineOptions(mobileData = true), read.options)
    }

    @Test
    fun theUsedCountStaysWithinTheHistory() {
        assertEquals(2000u, OfflinePrefs.read(mapOf(OfflinePrefs.AUTO_USED_COUNT to 50_000)).options.autoUsedCount)
        assertEquals(1u, OfflinePrefs.read(mapOf(OfflinePrefs.AUTO_USED_COUNT to 0)).options.autoUsedCount)
    }

    @Test
    fun theMemoryStoreKeepsWhatIsSet() {
        val store = MemoryOfflineStore()
        store.setOptions(OfflineOptions(videos = true))
        store.setPaused(true)
        assertEquals(OfflineChoices(OfflineOptions(videos = true), paused = true), store.current)
    }
}
