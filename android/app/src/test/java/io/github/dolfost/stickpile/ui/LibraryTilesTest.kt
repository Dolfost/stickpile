package io.github.dolfost.stickpile.ui

import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.display.Stat
import io.github.dolfost.stickpile.display.TileField
import io.github.dolfost.stickpile.rust.MediaFormat
import io.github.dolfost.stickpile.rust.MediaKind
import io.github.dolfost.stickpile.ui.library.SearchTerms
import io.github.dolfost.stickpile.ui.library.TileInfo
import io.github.dolfost.stickpile.ui.library.withLike
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** What the tiles show, and the search texts the library screen writes (ui.library). */
class LibraryTilesTest {
    private val gif = item("1", name = " Grumpy ", tags = listOf("cat", "nsfw"), emojis = listOf("😾", " "), liked = true).copy(
        kind = MediaKind.ANIMATION,
        format = MediaFormat.GIF,
        width = 320u,
        height = 240u,
        vibe = "grumpy",
        copies = 1234u,
        likes = 7u,
        quality = 81u,
        text = "NO THOUGHTS\nhead empty",
    )

    @Test
    fun aTileShowsEveryFieldThatIsOn() {
        val info = TileInfo.of(gif, Display().with(TileField.TEXT, true))
        assertEquals(
            TileInfo(
                badge = "GIF",
                nsfw = true,
                liked = true,
                name = "Grumpy",
                mood = "😾 grumpy",
                tags = "#cat #nsfw",
                text = "NO THOUGHTS / head empty",
                shape = "GIF 320x240",
                stats = listOf(Stat(TileField.COPIES, "1.2k"), Stat(TileField.LIKES, "7"), Stat(TileField.QUALITY, "81")),
            ),
            info,
        )
        assertTrue(info.hasBand)
        assertEquals("the text is off at first", null, TileInfo.of(gif, Display()).text)
    }

    @Test
    fun fieldsTurnOffOneByOneOrAllAtOnce() {
        val some = Display(fields = setOf(TileField.VIBE, TileField.LIKES))
        assertEquals(
            TileInfo(null, false, false, null, "grumpy", null, null, null, listOf(Stat(TileField.LIKES, "7"))),
            TileInfo.of(gif, some),
        )
        val none = TileInfo.of(gif, Display(showInfo = false))
        assertEquals(TileInfo(null, false, false, null, null, null, null, null, emptyList()), none)
        assertFalse(none.hasBand)
        val text = TileInfo.of(gif, Display(fields = setOf(TileField.TEXT)))
        assertEquals(TileInfo(null, false, false, null, null, null, "NO THOUGHTS / head empty", null, emptyList()), text)
        assertTrue(text.hasBand)
    }

    @Test
    fun aSmallTileKeepsTheLinesThatFit() {
        val two = TileInfo.of(gif, Display(), maxLines = 2)
        assertEquals("Grumpy", two.name)
        assertEquals("GIF 320x240", two.shape)
        assertEquals(null, two.mood)
        assertEquals(null, two.tags)
        assertEquals("GIF", two.badge)
        val none = TileInfo.of(gif, Display(), maxLines = 0)
        assertFalse(none.hasBand)
        assertTrue(none.liked)
    }

    @Test
    fun safeModeDropsTheNsfwMarkAndAStillHasNoBadge() {
        assertFalse(TileInfo.of(gif, Display(safeMode = true)).nsfw)
        val still = TileInfo.of(item("2"), Display())
        assertEquals(null, still.badge)
        assertEquals("PNG", still.shape) // no size known
        assertEquals(null, still.mood)
        assertEquals(emptyList<Stat>(), still.stats)
    }

    @Test
    fun aLikeCountsAndAnUnlikeTakesItBack() {
        val dog = item("dog").copy(likes = 2u)
        assertEquals(dog.copy(liked = true, likes = 3u), dog.withLike(true))
        assertEquals(dog, dog.withLike(true).withLike(false))
        assertSame(dog, dog.withLike(false))
        assertEquals(0uL, item("x", liked = true).withLike(false).likes)
    }

    @Test
    fun filtersAreWrittenAsTheSyntaxWantsThem() {
        assertEquals("tag:reaction ", SearchTerms.filter("tag", " reaction "))
        assertEquals("tag:\"big chungus\" ", SearchTerms.filter("tag", "big chungus"))
        assertEquals("e:😂 ", SearchTerms.filter("e", "😂"))
        assertEquals("vibe:\"so done\" ", SearchTerms.filter("vibe", "so \"done\""))
    }

    @Test
    fun syntaxExamplesSplitIntoWhatATapSearches() {
        assertEquals(listOf("q>70", "likes>=2", "copies<5"), SearchTerms.examples("q>70, likes>=2, copies<5"))
        assertEquals(listOf("vibe:", "tag:", "set:", "by:", "e:"), SearchTerms.examples("vibe: tag: set: by: e:"))
        assertEquals(listOf("cat \"big chungus\""), SearchTerms.examples("cat \"big chungus\""))
        assertEquals(listOf("tag:a tag:b"), SearchTerms.examples("tag:a tag:b"))
        assertEquals(listOf("x:", "x:lo"), SearchTerms.examples("x:, x:lo"))
        assertEquals("vibe:", SearchTerms.typed("vibe:"))
        assertEquals("-tag:nsfw ", SearchTerms.typed("-tag:nsfw"))
    }
}
