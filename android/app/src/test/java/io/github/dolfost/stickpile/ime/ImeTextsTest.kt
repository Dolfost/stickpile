package io.github.dolfost.stickpile.ime

import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ImeTextsTest {
    @Test
    fun onlyAMissingOrRefusedAccountIsSignedOut() {
        val signedOut = LibraryErrorKind.entries.filter(ImeTexts::isSignedOut)
        assertEquals(listOf(LibraryErrorKind.NOT_SIGNED_IN, LibraryErrorKind.UNAUTHORIZED), signedOut)
        signedOut.forEach { assertEquals(R.string.ime_signed_out, ImeTexts.forError(it)) }
    }

    @Test
    fun everyOtherErrorHasItsOwnLine() {
        val others = LibraryErrorKind.entries.filterNot(ImeTexts::isSignedOut)
        others.forEach { assertTrue(it.name, ImeTexts.forError(it) != R.string.ime_signed_out) }
        assertEquals(R.string.ime_error_unreachable, ImeTexts.forError(LibraryErrorKind.UNREACHABLE))
        assertEquals(R.string.ime_error_nothing_fits, ImeTexts.forError(LibraryErrorKind.NOTHING_FITS))
        assertFalse(ImeTexts.isSignedOut(LibraryErrorKind.UNREACHABLE))
    }
}
