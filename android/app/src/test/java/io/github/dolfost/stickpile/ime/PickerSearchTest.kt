package io.github.dolfost.stickpile.ime

import io.github.dolfost.stickpile.display.SyntaxHelp
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
    fun withASetOpenTheQuerySearchesInsideIt() {
        assertEquals(PickerLoad.InSet("s1", "v:tired"), PickerSearch.load(" v:tired ", PickerTab.SETS, "s1"))
        assertEquals("cleared: the set whole", PickerLoad.WholeSet("s1"), PickerSearch.load("  ", PickerTab.SETS, "s1"))
        assertEquals(PickerLoad.Search("cat"), PickerSearch.load("cat", PickerTab.RESULTS, null))
        assertEquals("left the set: the lists", PickerLoad.Tab(PickerTab.SETS), PickerSearch.load("cat", PickerTab.SETS, null))
        assertEquals(PickerLoad.Tab(PickerTab.RECENT), PickerSearch.load("cat", PickerTab.RECENT, null))
        assertEquals(PickerLoad.Tab(PickerTab.LIKED), PickerSearch.load("", PickerTab.LIKED, null))
        assertEquals(PickerLoad.Tab(PickerTab.RESULTS), PickerSearch.load("", PickerTab.RESULTS, null))
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
    fun theColumnsAreTheStickersInARow() {
        assertEquals(3, PickerSearch.spanCount(411, 3, narrowDp = 411))
        assertEquals(8, PickerSearch.spanCount(411, 8, narrowDp = 411))
        assertEquals("sideways: more of the same size", 6, PickerSearch.spanCount(914, 3, narrowDp = 411))
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
        assertEquals("v:", PickerSearch.exampleQuery("v: t: s: b:"))
        assertEquals("cat \"big chungus\" ", PickerSearch.exampleQuery("cat \"big chungus\""))
        assertEquals("tag:a tag:b ", PickerSearch.exampleQuery("tag:a tag:b"))
        for (line in SyntaxHelp.SECTIONS.flatMap { it.lines }) {
            val q = PickerSearch.exampleQuery(line.example)
            assertTrue(q, q.isNotBlank() && (q.endsWith(" ") || q.endsWith(":")) && !q.contains(","))
        }
    }
}
