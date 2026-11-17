package io.github.dolfost.stickpile.display

import io.github.dolfost.stickpile.rust.MediaFormat
import io.github.dolfost.stickpile.rust.MediaKind
import io.github.dolfost.stickpile.ui.item
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DisplayTest {
    @Test
    fun aThemeIsDarkLightOrThePhones() {
        assertNull(AppTheme.SYSTEM.night)
        assertEquals(true, AppTheme.DARK.night)
        assertEquals(true, AppTheme.OLED.night)
        assertEquals(false, AppTheme.WHITE.night)
        assertEquals(AppTheme.SYSTEM, Display().theme)
    }

    @Test
    fun theColumnsAreKeptInRangeAndFillAWiderGrid() {
        assertEquals(Columns.MIN, Columns.clamp(0))
        assertEquals(Columns.MAX, Columns.clamp(99))
        assertEquals(5, Columns.clamp(5))
        assertEquals(4, Columns.count(4, widthDp = 411, narrowDp = 411))
        assertEquals(4, Columns.count(4, widthDp = 380, narrowDp = 411))
        assertEquals(8, Columns.count(4, widthDp = 914, narrowDp = 411))
        assertEquals(Columns.MAX, Columns.count(12, widthDp = 411, narrowDp = 411))
        assertEquals(3, Columns.count(3, widthDp = 0, narrowDp = 0))
    }

    @Test
    fun safeModeLeavesNsfwOutOfTheSearchAndTheLists() {
        assertEquals("cat -tag:nsfw", SafeMode.query(" cat ", safe = true))
        assertEquals("cat", SafeMode.query("cat", safe = false))
        assertEquals("", SafeMode.query("  ", safe = true))
        val items = listOf(item("a"), item("x", tags = listOf("fun", " NSFW ")), item("b"))
        val shown = SafeMode.filter(items, safe = true)
        assertEquals(listOf("a", "b"), shown.items.map { it.id })
        assertEquals(1, shown.hidden)
        assertEquals(3, SafeMode.filter(items, safe = false).items.size)
        assertEquals(0, SafeMode.filter(items, safe = false).hidden)
    }

    @Test
    fun fieldsHideOneByOneOrAllAtOnce() {
        val d = Display()
        assertTrue(TileField.entries.all(d::shows))
        val noTags = d.with(TileField.TAGS, false)
        assertFalse(noTags.shows(TileField.TAGS))
        assertTrue(noTags.shows(TileField.NAME))
        assertTrue(noTags.with(TileField.TAGS, true).shows(TileField.TAGS))
        val off = d.copy(showInfo = false)
        assertTrue(TileField.entries.none(off::shows))
    }

    @Test
    fun labelsAreTheDesktops() {
        val gif = item("g").copy(kind = MediaKind.ANIMATION, format = MediaFormat.GIF, width = 512u, height = 384u)
        assertEquals("GIF", ItemLabels.badge(gif))
        assertEquals("GIF 512x384", ItemLabels.shape(gif))
        val webp = gif.copy(format = MediaFormat.WEBP)
        assertEquals("WEBP", ItemLabels.badge(webp))
        assertEquals("WEBP ANIM", ItemLabels.format(webp))
        val video = gif.copy(kind = MediaKind.VIDEO, format = MediaFormat.MP4)
        assertEquals("VIDEO", ItemLabels.badge(video))
        assertEquals("VIDEO", ItemLabels.format(video))
        val still = item("s")
        assertNull(ItemLabels.badge(still))
        assertEquals("no size known", "PNG", ItemLabels.shape(still))
    }

    @Test
    fun emojisTagsAndVibeAreTrimmed() {
        val it = item("a", tags = listOf("reaction", " ", " cat "), emojis = listOf("😂", "", "🐱"))
            .copy(vibe = " sarcastic ")
        assertEquals("#reaction #cat", ItemLabels.tags(it))
        assertEquals("😂🐱", ItemLabels.emojis(it))
        assertEquals("sarcastic", ItemLabels.vibe(it))
        assertNull(ItemLabels.tags(item("b")))
        assertNull(ItemLabels.emojis(item("b")))
        assertNull(ItemLabels.vibe(item("b").copy(vibe = " ")))
    }

    @Test
    fun statsShowWhatIsOnAndNotZero() {
        val it = item("a").copy(copies = 1234u, likes = 0u, quality = 87u)
        assertEquals(
            listOf(Stat(TileField.COPIES, "1.2k"), Stat(TileField.QUALITY, "87")),
            ItemLabels.stats(it, Display()),
        )
        assertEquals(
            listOf(Stat(TileField.QUALITY, "87")),
            ItemLabels.stats(it, Display().with(TileField.COPIES, false)),
        )
        assertEquals(emptyList<Stat>(), ItemLabels.stats(it, Display(showInfo = false)))
    }

    @Test
    fun countsAreShort() {
        assertEquals("0", ItemLabels.count(0u))
        assertEquals("999", ItemLabels.count(999u))
        assertEquals("1k", ItemLabels.count(1_000u))
        assertEquals("1.2k", ItemLabels.count(1_299u))
        assertEquals("12k", ItemLabels.count(12_345u))
        assertEquals("999k", ItemLabels.count(999_999u))
        assertEquals("1M", ItemLabels.count(1_000_000u))
        assertEquals("2.5M", ItemLabels.count(2_500_000u))
        assertEquals("25M", ItemLabels.count(25_000_000u))
    }
}
