package io.github.dolfost.stickpile.ime

import io.github.dolfost.stickpile.display.SyntaxHelp
import io.github.dolfost.stickpile.display.TileSize
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PickerSearchTest {
    @Test
    fun resultsAreATabOnlyWhileSomethingIsSearchedFor() {
        val lists = listOf(PickerTab.RECENT, PickerTab.LIKED, PickerTab.MOST_USED, PickerTab.SETS)
        assertEquals(lists, PickerSearch.tabs(""))
        assertEquals(lists, PickerSearch.tabs("  "))
        assertEquals(listOf(PickerTab.RESULTS) + lists, PickerSearch.tabs("cat"))
    }

    @Test
    fun aPickDeletesTheDraftOnlyWhileTheQueryIsTheDraft() {
        assertTrue(PickerSearch.fromDraft(draft = "cat", query = "cat", resultsFor = "cat"))
        assertTrue(PickerSearch.fromDraft(draft = "cat", query = "cat ", resultsFor = "cat"))
        assertFalse(PickerSearch.fromDraft(draft = "cat", query = "cats", resultsFor = "cat"))
        assertFalse(PickerSearch.fromDraft(draft = "cat", query = "ca", resultsFor = "cat"))
        assertFalse(PickerSearch.fromDraft(draft = "cat", query = "cat", resultsFor = "ca"))
        assertFalse(PickerSearch.fromDraft(draft = "cat", query = "cat", resultsFor = null))
        assertFalse(PickerSearch.fromDraft(draft = null, query = "cat", resultsFor = "cat"))
        assertFalse(PickerSearch.fromDraft(draft = null, query = "", resultsFor = null))
    }

    @Test
    fun theColumnsFollowTheTileSize() {
        assertEquals(4, PickerSearch.spanCount(352, TileSize.SMALL))
        assertEquals(3, PickerSearch.spanCount(352, TileSize.MEDIUM))
        assertEquals(3, PickerSearch.spanCount(352, TileSize.LARGE))
        assertEquals(5, PickerSearch.spanCount(800, TileSize.LARGE))
        assertEquals(PickerSearch.MIN_COLUMNS, PickerSearch.spanCount(200, TileSize.SMALL))
    }

    @Test
    fun theKeysMakeTheCellsSmallerNeverLarger() {
        assertEquals(6, PickerSearch.typingSpanCount(352, 3))
        assertEquals(8, PickerSearch.typingSpanCount(352, 8))
    }

    @Test
    fun aValueInTheDetailsSearchesForItself() {
        assertEquals("tag:reaction ", PickerSearch.filterQuery("tag", "reaction"))
        assertEquals("vibe:\"very sad\" ", PickerSearch.filterQuery("vibe", " very sad "))
        assertEquals("e:😂 ", PickerSearch.filterQuery("e", "😂"))
    }

    @Test
    fun anExampleOfTheSyntaxIsAQuery() {
        assertEquals("is:mine ", PickerSearch.exampleQuery("is:mine, is:liked, is:public"))
        assertEquals("q>70 ", PickerSearch.exampleQuery("q>70, likes>=2, copies<5"))
        assertEquals("vibe:", PickerSearch.exampleQuery("vibe: tag: set: by: e:"))
        assertEquals("cat \"big chungus\" ", PickerSearch.exampleQuery("cat \"big chungus\""))
        assertEquals("tag:a tag:b ", PickerSearch.exampleQuery("tag:a tag:b"))
        for (line in SyntaxHelp.SECTIONS.flatMap { it.lines }) {
            val q = PickerSearch.exampleQuery(line.example)
            assertTrue(q, q.isNotBlank() && (q.endsWith(" ") || q.endsWith(":")) && !q.contains(","))
        }
    }
}
