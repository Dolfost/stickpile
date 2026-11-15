package io.github.dolfost.stickpile.ime

import org.junit.Assert.assertEquals
import org.junit.Test

class EmojiFiltersTest {
    private val cat = "😺"
    private val heart = "❤"
    private val heartEmoji = "❤️"
    private val fire = "🔥"
    private val sad = "😢"

    @Test
    fun theMostCommonComeFirstAndTiesKeepTheirOrder() {
        val items = listOf(
            listOf(fire, cat),
            listOf(cat),
            listOf(sad, cat),
            listOf(fire),
            listOf(),
        )
        assertEquals(listOf(cat, fire, sad), EmojiFilters.top(items))
    }

    @Test
    fun anItemCountsOnceForEachEmoji() {
        val items = listOf(listOf(cat, cat, cat), listOf(fire), listOf(fire))
        assertEquals(listOf(fire, cat), EmojiFilters.top(items))
    }

    @Test
    fun theEmojiPresentationSelectorDoesNotMakeAnotherFilter() {
        val items = listOf(listOf(heart), listOf(heartEmoji), listOf(cat))
        assertEquals(listOf(heart, cat), EmojiFilters.top(items))
        assertEquals(listOf(listOf(heart), listOf(heartEmoji)), EmojiFilters.narrow(items, heartEmoji) { it })
    }

    @Test
    fun anEmojiEveryItemHasIsNoFilter() {
        assertEquals(listOf(fire), EmojiFilters.top(listOf(listOf(cat, fire), listOf(cat))))
        assertEquals(emptyList<String>(), EmojiFilters.top(listOf(listOf(cat, fire))))
        assertEquals(emptyList<String>(), EmojiFilters.top(emptyList()))
    }

    @Test
    fun blankEmojisAreSkipped() {
        assertEquals(listOf(cat), EmojiFilters.top(listOf(listOf("", " ", "️", cat), listOf(fire, cat), listOf(fire))).take(1))
        assertEquals(listOf(cat, fire), EmojiFilters.top(listOf(listOf("", cat), listOf(fire, cat), listOf(fire), listOf())))
    }

    @Test
    fun atMostTwelve() {
        val many = (0 until 20).map { listOf("e$it") }
        assertEquals(12, EmojiFilters.top(many).size)
        assertEquals(3, EmojiFilters.top(many, max = 3).size)
        assertEquals(listOf("e0", "e1", "e2"), EmojiFilters.top(many, max = 3))
    }

    @Test
    fun aFilterNarrowsTheGridAndNoneShowsAll() {
        val items = listOf("a" to listOf(cat), "b" to listOf(fire), "c" to listOf(fire, cat), "d" to emptyList())
        assertEquals(listOf("a", "c"), EmojiFilters.narrow(items, cat) { it.second }.map { it.first })
        assertEquals(listOf("b", "c"), EmojiFilters.narrow(items, fire) { it.second }.map { it.first })
        assertEquals(items, EmojiFilters.narrow(items, null) { it.second })
        assertEquals(emptyList<Pair<String, List<String>>>(), EmojiFilters.narrow(items, sad) { it.second })
    }
}
