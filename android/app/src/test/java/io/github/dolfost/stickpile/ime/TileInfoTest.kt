package io.github.dolfost.stickpile.ime

import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.display.TileField
import io.github.dolfost.stickpile.ui.item
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class TileInfoTest {
    private val full = item("a", name = "grumpy cat", tags = listOf("cat"), emojis = listOf("😾"))
        .copy(vibe = "grumpy", text = "NO THOUGHTS\nhead empty")
    private val bare = item("b", name = " ").copy(copies = 0u, likes = 0u, quality = null)
    private val withText = Display().with(TileField.TEXT, true)

    @Test
    fun aSmallerCellHasRoomForFewerLines() {
        assertEquals(4, TileInfo.maxLines(120, 1f))
        assertEquals(3, TileInfo.maxLines(90, 1f))
        assertEquals(2, TileInfo.maxLines(60, 1f))
        assertEquals(1, TileInfo.maxLines(60, 2f))
        assertEquals(0, TileInfo.maxLines(10, 1f))
        assertEquals(TileLine.entries.size, TileInfo.maxLines(400, 1f))
    }

    @Test
    fun everyLineShowsWhenThereIsRoom() {
        assertEquals(TileLine.entries, TileInfo.lines(full, withText, maxLines = 5))
        assertEquals(
            "the text is off at first",
            TileLine.entries - TileLine.TEXT,
            TileInfo.lines(full, Display(), maxLines = 5),
        )
    }

    @Test
    fun withLessRoomTheNameAndTheStatsStay() {
        assertEquals(listOf(TileLine.NAME, TileLine.STATS), TileInfo.lines(full, Display(), maxLines = 2))
        assertEquals(listOf(TileLine.NAME, TileLine.TAGS, TileLine.STATS), TileInfo.lines(full, Display(), maxLines = 3))
        assertEquals(emptyList<TileLine>(), TileInfo.lines(full, Display(), maxLines = 0))
        assertEquals(
            "the text goes first: the picture shows it",
            TileLine.entries - TileLine.TEXT,
            TileInfo.lines(full, withText, maxLines = 4),
        )
    }

    @Test
    fun hiddenFieldsAndEmptyOnesMakeNoLine() {
        assertEquals(emptyList<TileLine>(), TileInfo.lines(full, Display(showInfo = false), maxLines = 4))
        val noTags = Display().with(TileField.TAGS, false)
        assertEquals(listOf(TileLine.NAME, TileLine.EMOJIS_VIBE, TileLine.STATS), TileInfo.lines(full, noTags, 4))
        val noSize = Display().with(TileField.SIZE, false)
        assertEquals(emptyList<TileLine>(), TileInfo.lines(bare, noSize, 4))
        assertEquals(listOf(TileLine.STATS), TileInfo.lines(bare.copy(copies = 5u), noSize, 4))
        assertEquals("no text, no line", emptyList<TileLine>(), TileInfo.lines(bare, noSize.with(TileField.TEXT, true), 4))
    }

    @Test
    fun theEmojisComeBeforeTheVibe() {
        assertEquals("😾 grumpy", TileInfo.emojisAndVibe(full, Display()))
        assertEquals("😾", TileInfo.emojisAndVibe(full, Display().with(TileField.VIBE, false)))
        assertEquals("grumpy", TileInfo.emojisAndVibe(full, Display().with(TileField.EMOJIS, false)))
        assertNull(TileInfo.emojisAndVibe(bare, Display()))
    }
}
