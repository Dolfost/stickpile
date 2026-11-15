package io.github.dolfost.stickpile.motion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SlotsTest {
    @Test
    fun theFirstTakeTheSlotsAndTheRestWaitInTurn() {
        val slots = Slots<String>(2)
        assertTrue(slots.take("a"))
        assertTrue(slots.take("b"))
        assertTrue("taking again keeps it", slots.take("a"))
        assertFalse(slots.take("c"))
        assertFalse(slots.take("d"))
        assertEquals(2, slots.held)

        assertEquals("the one waiting longest", "c", slots.give("a"))
        assertTrue(slots.holds("c"))
        assertFalse(slots.holds("a"))
        assertEquals("d", slots.give("b"))
        assertNull("nobody waits", slots.give("c"))
        assertEquals(1, slots.held)
    }

    @Test
    fun oneThatStopsWaitingIsNotHandedASlot() {
        val slots = Slots<String>(1)
        slots.take("a")
        slots.take("b")
        slots.take("c")
        assertNull("b held nothing", slots.give("b"))
        assertEquals("c", slots.give("a"))
        assertNull("a stranger gives nothing back", slots.give("x"))
        assertEquals(1, slots.held)
    }
}
