package io.github.dolfost.stickpile.provider

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class KeyboardTrustTest {
    private val keyA = "certificate A".toByteArray()
    private val keyB = "certificate B".toByteArray()

    private val a = KeyboardTrust.digests(listOf(keyA))!!
    private val b = KeyboardTrust.digests(listOf(keyB))!!
    private val ab = KeyboardTrust.digests(listOf(keyA, keyB))!!

    @Test
    fun certificatesAreKeptAsTheirSha256InHex() {
        assertEquals(
            setOf("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            KeyboardTrust.digests(listOf("abc".toByteArray())),
        )
        assertEquals(2, ab.size)
        assertTrue(ab.containsAll(a + b))
        assertEquals(a, KeyboardTrust.digests(listOf(keyA, keyA.copyOf())))
    }

    @Test
    fun noCertificateIsNoDigest() {
        assertNull(KeyboardTrust.digests(emptyList()))
    }

    @Test
    fun theApprovedAppMatches() {
        assertTrue(KeyboardTrust.matches(a, a))
        assertTrue(KeyboardTrust.matches(ab, KeyboardTrust.digests(listOf(keyB, keyA))))
    }

    @Test
    fun anotherAppUnderTheSameNameDoesNot() {
        assertFalse(KeyboardTrust.matches(a, b))
        // Every key of an app signed by several must match, no more and no less.
        assertFalse(KeyboardTrust.matches(ab, a))
        assertFalse(KeyboardTrust.matches(a, ab))
    }

    @Test
    fun nothingStoredOrNothingReadableNeverMatches() {
        assertFalse(KeyboardTrust.matches(null, a))
        assertFalse(KeyboardTrust.matches(emptySet(), a))
        assertFalse(KeyboardTrust.matches(a, null))
        assertFalse(KeyboardTrust.matches(null, null))
        assertFalse(KeyboardTrust.matches(emptySet(), emptySet()))
    }

    @Test
    fun anApprovalPinsTheInstalledAppElseTheOneThatAsked() {
        assertEquals(b, KeyboardTrust.pinFor(current = b, whenAsked = a))
        assertEquals(a, KeyboardTrust.pinFor(current = null, whenAsked = a))
        assertEquals(a, KeyboardTrust.pinFor(current = emptySet(), whenAsked = a))
        assertNull(KeyboardTrust.pinFor(current = null, whenAsked = null))
        assertNull(KeyboardTrust.pinFor(current = emptySet(), whenAsked = emptySet()))
    }

    @Test
    fun onlyInputMethodsAreListed() {
        val inputMethods = setOf("juloo.keyboard2", "com.google.android.inputmethod.latin")
        assertTrue(KeyboardTrust.isKeyboard("juloo.keyboard2", inputMethods))
        assertFalse(KeyboardTrust.isKeyboard("org.telegram.messenger", inputMethods))
        assertFalse(KeyboardTrust.isKeyboard("juloo.keyboard2", emptySet()))
    }
}
