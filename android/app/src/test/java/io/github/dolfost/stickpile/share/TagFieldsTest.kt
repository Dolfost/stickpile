package io.github.dolfost.stickpile.share

import org.junit.Assert.assertEquals
import org.junit.Test

class TagFieldsTest {
    @Test
    fun keywordsAreSeparatedByCommas() {
        assertEquals(listOf("cat", "funny cat", "hat"), TagFields.keywords("cat, funny cat,hat"))
        assertEquals(listOf("a", "b"), TagFields.keywords("a\nb"))
    }

    @Test
    fun keywordsAreTrimmedAndCollapsed() {
        assertEquals(listOf("funny cat"), TagFields.keywords("   funny \t  cat   "))
    }

    @Test
    fun emptyKeywordsGo() {
        assertEquals(emptyList<String>(), TagFields.keywords(""))
        assertEquals(emptyList<String>(), TagFields.keywords(" , ,,  "))
        assertEquals(listOf("cat"), TagFields.keywords(",cat,"))
    }

    @Test
    fun repeatedKeywordsGoIgnoringCase() {
        assertEquals(listOf("Cat", "dog"), TagFields.keywords("Cat, dog, cat, CAT , dog"))
        assertEquals(listOf("Кіт"), TagFields.keywords("Кіт, кіт"))
    }

    @Test
    fun emojisAreSeparatedByCommasOrSpaces() {
        assertEquals(listOf("\uD83D\uDE00", "\uD83D\uDE02", "\uD83C\uDF89"), TagFields.emojis("\uD83D\uDE00, \uD83D\uDE02 \uD83C\uDF89"))
        assertEquals(listOf("\uD83D\uDE00", "\uD83D\uDE02"), TagFields.emojis("  \uD83D\uDE00\n,\t\uD83D\uDE02  "))
    }

    @Test
    fun emojisTypedTogetherAreSplit() {
        assertEquals(listOf("\uD83D\uDE00", "\uD83D\uDE02", "\uD83C\uDF89"), TagFields.emojis("\uD83D\uDE00\uD83D\uDE02\uD83C\uDF89"))
        // A heart with its emoji presentation selector stays one.
        assertEquals(listOf("\u2764\uFE0F", "\uD83D\uDD25"), TagFields.emojis("\u2764\uFE0F\uD83D\uDD25"))
    }

    @Test
    fun wordsInTheEmojisFieldStayWhole() {
        assertEquals(listOf("lol", "сміх"), TagFields.emojis("lol сміх"))
    }

    @Test
    fun emptyAndRepeatedEmojisGo() {
        assertEquals(emptyList<String>(), TagFields.emojis(""))
        assertEquals(emptyList<String>(), TagFields.emojis(" ,, "))
        assertEquals(listOf("\uD83D\uDE00"), TagFields.emojis("\uD83D\uDE00 \uD83D\uDE00\uD83D\uDE00, \uD83D\uDE00"))
    }

    @Test
    fun listsAsTheFieldsShowThem() {
        assertEquals("cat, funny cat", TagFields.keywordsText(listOf("cat", "funny cat")))
        assertEquals("\uD83D\uDE00 \uD83D\uDE02", TagFields.emojisText(listOf("\uD83D\uDE00", "\uD83D\uDE02")))
        assertEquals(listOf("cat", "funny cat"), TagFields.keywords(TagFields.keywordsText(listOf("cat", "funny cat"))))
    }
}
