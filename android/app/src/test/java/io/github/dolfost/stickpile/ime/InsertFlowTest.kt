package io.github.dolfost.stickpile.ime

import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class InsertFlowTest {
    private val fieldTypes = listOf("image/png", "image/gif")

    /**
     * Runs the flow to its end with scripted answers, and returns the steps
     * taken (without the final one) and the outcome.
     */
    private fun run(
        field: PrepareResult<String>,
        commit: Boolean = false,
        share: PrepareResult<String> = PrepareResult.Ready("shared.png"),
        canSend: Boolean = true,
        send: Boolean = false,
        copy: Boolean = true,
    ): Pair<List<String>, InsertOutcome<String>> {
        val steps = mutableListOf<String>()
        var step: InsertStep<String> = InsertFlow.start(fieldTypes)
        while (true) {
            step = when (step) {
                is InsertStep.PrepareForField -> {
                    assertEquals(fieldTypes, step.mimeTypes)
                    steps += "prepare field"
                    InsertFlow.afterFieldPrepared(field)
                }
                is InsertStep.Commit -> {
                    steps += "commit ${step.prepared}"
                    InsertFlow.afterCommit(step.prepared, commit)
                }
                InsertStep.PrepareForSharing -> {
                    steps += "prepare share"
                    InsertFlow.afterSharePrepared(share, canSend)
                }
                is InsertStep.Send -> {
                    steps += "send ${step.prepared}"
                    InsertFlow.afterSend(step.prepared, send)
                }
                is InsertStep.Copy -> {
                    steps += "copy ${step.prepared}"
                    InsertFlow.afterCopy(step.prepared, copy)
                }
                is InsertStep.Done -> return steps to step.outcome
            }
        }
    }

    @Test
    fun aRenditionTheFieldAcceptsIsCommitted() {
        assertEquals(
            listOf("prepare field", "commit field.png") to InsertOutcome.Committed("field.png"),
            run(PrepareResult.Ready("field.png"), commit = true),
        )
    }

    @Test
    fun whenNothingFitsItIsSentToTheEditorsApp() {
        assertEquals(
            listOf("prepare field", "prepare share", "send shared.png") to InsertOutcome.Sent("shared.png"),
            run(PrepareResult.Error(LibraryErrorKind.NOTHING_FITS), send = true),
        )
    }

    @Test
    fun whenTheFieldRefusesTheCommitItIsSent() {
        assertEquals(
            listOf("prepare field", "commit field.png", "prepare share", "send shared.png") to
                InsertOutcome.Sent("shared.png"),
            run(PrepareResult.Ready("field.png"), commit = false, send = true),
        )
    }

    @Test
    fun whenSendingFailsItIsCopied() {
        assertEquals(
            listOf("prepare field", "commit field.png", "prepare share", "send shared.png", "copy shared.png") to
                InsertOutcome.Copied("shared.png"),
            run(PrepareResult.Ready("field.png"), commit = false, send = false),
        )
    }

    @Test
    fun withNoAppToSendToItIsCopied() {
        assertEquals(
            listOf("prepare field", "prepare share", "copy shared.png") to InsertOutcome.Copied("shared.png"),
            run(PrepareResult.Error(LibraryErrorKind.NOTHING_FITS), canSend = false),
        )
    }

    @Test
    fun aFailedCopyIsAFailure() {
        assertEquals(
            listOf("prepare field", "prepare share", "copy shared.png") to InsertOutcome.Failed(LibraryErrorKind.OTHER),
            run(PrepareResult.Error(LibraryErrorKind.NOTHING_FITS), canSend = false, copy = false),
        )
    }

    @Test
    fun whenTheServerIsOutOfReachACachedFileIsStillShared() {
        val unreachable = PrepareResult.Error(LibraryErrorKind.UNREACHABLE)
        assertEquals(
            listOf("prepare field", "prepare share", "send shared.png") to InsertOutcome.Sent("shared.png"),
            run(unreachable, send = true),
        )
        assertEquals(
            listOf("prepare field", "prepare share", "send shared.png", "copy shared.png") to
                InsertOutcome.Copied("shared.png"),
            run(unreachable, send = false),
        )
        // Nothing cached either: the share's error is the failure.
        assertEquals(
            listOf("prepare field", "prepare share") to InsertOutcome.Failed(LibraryErrorKind.UNREACHABLE),
            run(unreachable, share = unreachable),
        )
    }

    /** The field's rendition is not kept offline: one for sharing may be; else that is the failure, explained. */
    @Test
    fun whenTheFieldsFileIsNotOnThePhoneASharedOneIsTried() {
        val notHere = PrepareResult.Error(LibraryErrorKind.NOT_ON_DEVICE)
        assertEquals(
            listOf("prepare field", "prepare share", "send shared.png") to InsertOutcome.Sent("shared.png"),
            run(notHere, send = true),
        )
        assertEquals(
            listOf("prepare field", "prepare share") to InsertOutcome.Failed(LibraryErrorKind.NOT_ON_DEVICE),
            run(notHere, share = notHere),
        )
        assertEquals(R.string.ime_error_not_on_device, ImeTexts.forError(LibraryErrorKind.NOT_ON_DEVICE))
    }

    @Test
    fun otherErrorsEndItAtOnce() {
        listOf(
            LibraryErrorKind.NOT_SIGNED_IN,
            LibraryErrorKind.UNAUTHORIZED,
            LibraryErrorKind.NOT_FOUND,
            LibraryErrorKind.RATE_LIMITED,
            LibraryErrorKind.INVALID_INPUT,
            LibraryErrorKind.SERVER,
            LibraryErrorKind.IO,
            LibraryErrorKind.OTHER,
        ).forEach { kind ->
            assertEquals(listOf("prepare field") to InsertOutcome.Failed(kind), run(PrepareResult.Error(kind)))
        }
    }

    @Test
    fun aFailedSharePreparationIsTheFailure() {
        assertEquals(
            listOf("prepare field", "prepare share") to InsertOutcome.Failed(LibraryErrorKind.UNREACHABLE),
            run(
                PrepareResult.Error(LibraryErrorKind.NOTHING_FITS),
                share = PrepareResult.Error(LibraryErrorKind.UNREACHABLE),
            ),
        )
        assertEquals(
            listOf("prepare field", "commit field.png", "prepare share") to
                InsertOutcome.Failed(LibraryErrorKind.NOTHING_FITS),
            run(PrepareResult.Ready("field.png"), share = PrepareResult.Error(LibraryErrorKind.NOTHING_FITS)),
        )
    }

    @Test
    fun everyHandOffIsAUse() {
        assertEquals("field.png", InsertFlow.used(InsertOutcome.Committed("field.png")))
        assertEquals("shared.png", InsertFlow.used(InsertOutcome.Sent("shared.png")))
        assertEquals("shared.png", InsertFlow.used(InsertOutcome.Copied("shared.png")))
        assertNull(InsertFlow.used(InsertOutcome.Failed(LibraryErrorKind.UNREACHABLE)))
    }

    @Test
    fun sharesAreAimedAtOtherAppsOnly() {
        assertTrue(InsertFlow.canSendTo("org.telegram.messenger", "io.github.dolfost.stickpile"))
        assertFalse(InsertFlow.canSendTo("io.github.dolfost.stickpile", "io.github.dolfost.stickpile"))
        assertFalse(InsertFlow.canSendTo("", "io.github.dolfost.stickpile"))
        assertFalse(InsertFlow.canSendTo(null, "io.github.dolfost.stickpile"))
    }

    @Test
    fun sharingAsksForPicturesAndVideos() {
        assertEquals(listOf("image/*", "video/*"), InsertFlow.SHARE_TYPES)
    }
}
