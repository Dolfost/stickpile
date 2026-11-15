package io.github.dolfost.stickpile.ui

import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.ui.common.Doing
import io.github.dolfost.stickpile.ui.common.Failure
import io.github.dolfost.stickpile.ui.common.Message
import io.github.dolfost.stickpile.ui.common.catching
import io.github.dolfost.stickpile.ui.common.messageFor
import kotlin.coroutines.cancellation.CancellationException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** Which text each kind of library error gets (ui.common.Failures). */
class FailuresTest {

    @Test
    fun everyExceptionHasItsKind() {
        val cases = mapOf(
            LibraryException.NotSignedIn("d") to Failure(LibraryErrorKind.NOT_SIGNED_IN),
            LibraryException.Unauthorized("d") to Failure(LibraryErrorKind.UNAUTHORIZED),
            LibraryException.Unreachable("d") to Failure(LibraryErrorKind.UNREACHABLE),
            LibraryException.NotFound("d") to Failure(LibraryErrorKind.NOT_FOUND),
            LibraryException.RateLimited("d") to Failure(LibraryErrorKind.RATE_LIMITED),
            LibraryException.NothingFits("d") to Failure(LibraryErrorKind.NOTHING_FITS),
            LibraryException.InvalidInput("totp", "d") to Failure(LibraryErrorKind.INVALID_INPUT, field = "totp"),
            LibraryException.Server(503u, "d") to Failure(LibraryErrorKind.SERVER, status = 503),
            LibraryException.Io("d") to Failure(LibraryErrorKind.IO),
            LibraryException.Other("d") to Failure(LibraryErrorKind.OTHER),
        )
        cases.forEach { (e, failure) -> assertEquals(e.toString(), failure, Failure.of(e)) }
        assertEquals(Failure(LibraryErrorKind.OTHER), Failure.of(IllegalStateException("not from the library")))
    }

    @Test
    fun everyKindHasATextInEveryActivity() {
        for (kind in LibraryErrorKind.entries) {
            for (doing in Doing.entries) {
                val message = messageFor(Failure(kind), doing)
                assertTrue("$kind while $doing", message.id != 0)
            }
        }
    }

    @Test
    fun aRefusedSignInSaysWhatWasWrong() {
        val refused = Failure(LibraryErrorKind.UNAUTHORIZED)
        assertEquals(Message(R.string.app_error_pairing_code_refused), messageFor(refused, Doing.PAIRING))
        assertEquals(Message(R.string.app_error_wrong_password), messageFor(refused, Doing.PASSWORD_LOGIN))
        assertEquals(Message(R.string.app_error_wrong_password_or_code), messageFor(refused, Doing.PASSWORD_AND_CODE_LOGIN))
        assertEquals(Message(R.string.app_error_unauthorized), messageFor(refused, Doing.LOADING))
        assertEquals(Message(R.string.app_error_unauthorized), messageFor(refused, Doing.SYNCING))
    }

    @Test
    fun unreachableDependsOnWhatWasAskedFor() {
        val offline = Failure(LibraryErrorKind.UNREACHABLE)
        assertEquals(Message(R.string.app_error_unreachable_nothing_saved), messageFor(offline, Doing.LOADING))
        assertEquals(Message(R.string.app_error_unreachable_not_saved), messageFor(offline, Doing.SHARING))
        assertEquals(Message(R.string.app_error_unreachable), messageFor(offline, Doing.PAIRING))
        assertEquals(Message(R.string.app_error_unreachable), messageFor(LibraryErrorKind.UNREACHABLE, Doing.SYNCING))
    }

    @Test
    fun invalidInputNamesTheField() {
        fun text(field: String?) = messageFor(Failure(LibraryErrorKind.INVALID_INPUT, field = field), Doing.PAIRING).id
        assertEquals(R.string.app_error_invalid_server_url, text("server_url"))
        assertEquals(R.string.app_error_invalid_pairing_code, text("code"))
        assertEquals(R.string.app_error_totp_needed, text("totp"))
        assertEquals(R.string.app_error_login_refused, text("username"))
        assertEquals(R.string.app_error_token_name_refused, text("token_name"))
        assertEquals(R.string.app_error_config_file, text("config_file"))
        assertEquals(R.string.app_error_invalid_input, text("file"))
        assertEquals(R.string.app_error_invalid_input, text(null))
    }

    @Test
    fun aServerErrorShowsItsStatus() {
        assertEquals(
            Message(R.string.app_error_server, listOf(502)),
            messageFor(Failure.of(LibraryException.Server(502u, "bad gateway")), Doing.LOADING),
        )
    }

    @Test
    fun catchingKeepsFailuresButNotCancellation() {
        assertEquals(Result.success(1), catching { 1 })
        val failed = catching { throw LibraryException.Io("disk") }
        assertTrue(failed.exceptionOrNull() is LibraryException.Io)
        try {
            catching { throw CancellationException("stop") }
            fail("a cancellation must go on")
        } catch (e: CancellationException) {
            assertEquals("stop", e.message)
        }
    }
}
