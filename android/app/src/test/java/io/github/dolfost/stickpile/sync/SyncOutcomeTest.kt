package io.github.dolfost.stickpile.sync

import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SyncOutcomeTest {
    @Test
    fun aSyncThatWorkedIsDone() {
        assertEquals(SyncOutcome.DONE, SyncOutcome.after(null, 0))
        assertEquals(SyncOutcome.DONE, SyncOutcome.after(null, 1))
    }

    @Test
    fun aPassingFailureIsRetriedOnce() {
        for (kind in listOf(LibraryErrorKind.UNREACHABLE, LibraryErrorKind.RATE_LIMITED, LibraryErrorKind.SERVER)) {
            assertEquals(kind.name, SyncOutcome.RETRY, SyncOutcome.after(kind, 0))
            assertEquals(kind.name, SyncOutcome.DONE, SyncOutcome.after(kind, 1))
            assertEquals(kind.name, SyncOutcome.DONE, SyncOutcome.after(kind, 7))
        }
    }

    @Test
    fun otherFailuresWaitForTheNextPeriod() {
        val lasting = LibraryErrorKind.entries - SyncOutcome.TRANSIENT
        assertEquals(8, lasting.size)
        for (kind in lasting) {
            assertEquals(kind.name, SyncOutcome.DONE, SyncOutcome.after(kind, 0))
        }
    }

    @Test
    fun theRetryComesAfterTheLibrarysOwnRetryDelay() {
        // stickpile_core::sync_policy::RETRY_AFTER is 60 s.
        assertTrue(SyncScheduler.RETRY_DELAY_MINUTES * 60 > 60)
    }

    @Test
    fun everyErrorHasItsKind() {
        val kinds = listOf(
            LibraryException.NotSignedIn("") to LibraryErrorKind.NOT_SIGNED_IN,
            LibraryException.Unauthorized("") to LibraryErrorKind.UNAUTHORIZED,
            LibraryException.Unreachable("") to LibraryErrorKind.UNREACHABLE,
            LibraryException.NotFound("") to LibraryErrorKind.NOT_FOUND,
            LibraryException.RateLimited("") to LibraryErrorKind.RATE_LIMITED,
            LibraryException.NothingFits("") to LibraryErrorKind.NOTHING_FITS,
            LibraryException.InvalidInput("file", "") to LibraryErrorKind.INVALID_INPUT,
            LibraryException.Server(500u, "") to LibraryErrorKind.SERVER,
            LibraryException.Io("") to LibraryErrorKind.IO,
            LibraryException.Other("") to LibraryErrorKind.OTHER,
            LibraryException.NotOnDevice("") to LibraryErrorKind.NOT_ON_DEVICE,
        )
        kinds.forEach { (e, kind) -> assertEquals(kind, e.kind()) }
        assertEquals(LibraryErrorKind.entries.toSet(), kinds.map { it.second }.toSet())
    }
}
