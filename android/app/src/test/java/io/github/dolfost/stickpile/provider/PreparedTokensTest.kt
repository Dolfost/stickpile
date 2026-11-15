package io.github.dolfost.stickpile.provider

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PreparedTokensTest {
    private var now = 1_000L
    private val entry = PreparedTokens.Entry("/data/cache/grumpy cat.png", "image/png", "grumpy cat.png")

    @Test
    fun tokensAre128RandomBitsAsHex() {
        val asked = mutableListOf<Int>()
        val tokens = PreparedTokens(clock = { now }, random = { bytes ->
            asked += bytes.size
            bytes.fill(0xab.toByte())
        })
        assertEquals("ab".repeat(16), tokens.register(entry))
        assertEquals(listOf(16), asked)
        assertEquals(PreparedTokens.TOKEN_BYTES * 8, 128)
    }

    @Test
    fun theDefaultRandomnessGivesDistinctWellFormedTokens() {
        val tokens = PreparedTokens(clock = { now })
        val made = (1..300).map { tokens.register(entry) }
        assertEquals(made.size, made.toSet().size)
        made.forEach { assertTrue(it, ProviderRoutes.isToken(it)) }
    }

    @Test
    fun aTokenOpensItsFileByItsNameOnly() {
        val tokens = PreparedTokens(clock = { now })
        val token = tokens.register(entry)
        assertEquals(entry, tokens.lookup(token, "grumpy cat.png"))
        assertNull(tokens.lookup(token, "other.png"))
        assertNull(tokens.lookup(token, "grumpy cat.PNG"))
        assertNull(tokens.lookup(token, ""))
    }

    @Test
    fun unknownTokensOpenNothing() {
        val tokens = PreparedTokens(clock = { now })
        val token = tokens.register(entry)
        val flipped = (if (token[0] == '0') "1" else "0") + token.substring(1)
        assertNull(tokens.lookup(flipped, entry.fileName))
        assertNull(tokens.lookup(token.substring(0, 31), entry.fileName))
        assertNull(tokens.lookup(token + "0", entry.fileName))
        assertNull(tokens.lookup("", entry.fileName))
        assertNull(PreparedTokens(clock = { now }).lookup(token, entry.fileName))
    }

    @Test
    fun tokensExpireAfterOneHour() {
        val tokens = PreparedTokens(clock = { now })
        val token = tokens.register(entry)
        now += PreparedTokens.LIFETIME_MILLIS - 1
        assertNotNull(tokens.lookup(token, entry.fileName))
        now += 1
        assertNull(tokens.lookup(token, entry.fileName))
        assertEquals(0, tokens.size())
    }

    @Test
    fun eachTokenHasItsOwnHour() {
        val tokens = PreparedTokens(clock = { now })
        val first = tokens.register(entry)
        now += 30 * 60 * 1000
        val second = tokens.register(entry.copy(fileName = "b.gif"))
        now += 30 * 60 * 1000
        assertNull(tokens.lookup(first, entry.fileName))
        assertNotNull(tokens.lookup(second, "b.gif"))
    }

    @Test
    fun theOldestTokensGoWhenTooManyAreLive() {
        val tokens = PreparedTokens(clock = { now }, capacity = 3)
        val made = (1..4).map { tokens.register(entry.copy(fileName = "$it.png")) }
        assertEquals(3, tokens.size())
        assertNull(tokens.lookup(made[0], "1.png"))
        assertNotNull(tokens.lookup(made[3], "4.png"))
    }
}
