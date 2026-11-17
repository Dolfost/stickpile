package io.github.dolfost.stickpile.ime

import io.github.dolfost.stickpile.rust.LibraryErrorKind

/**
 * The order in which the picker tries to get an item into the chat
 * (docs/android.md, "Choosing what to insert"), as a pure state machine:
 *
 * 1. a rendition the field accepts, committed with `commitContent`;
 * 2. else (nothing fits, the server is out of reach, or the field refused
 *    it) a rendition for sharing, sent with `ACTION_SEND` to the editor's
 *    app;
 * 3. else that file's URI on the clipboard, saying so.
 *
 * Which rendition fits is the library's decision; this only orders the
 * attempts. [P] is the prepared file (`Prepared` in the app).
 */
sealed interface InsertStep<out P> {
    /** Ask the library for a rendition of one of [mimeTypes] (the field's). */
    data class PrepareForField(val mimeTypes: List<String>) : InsertStep<Nothing>

    /** Commit [prepared] into the field. */
    data class Commit<P>(val prepared: P) : InsertStep<P>

    /** Ask the library for a rendition of [InsertFlow.SHARE_TYPES]. */
    data object PrepareForSharing : InsertStep<Nothing>

    /** Send [prepared] to the editor's app. */
    data class Send<P>(val prepared: P) : InsertStep<P>

    /** Put [prepared]'s URI on the clipboard. */
    data class Copy<P>(val prepared: P) : InsertStep<P>

    data class Done<P>(val outcome: InsertOutcome<P>) : InsertStep<P>
}

/** How an insert ended; every outcome but [Failed] is a use ([InsertFlow.used]). */
sealed interface InsertOutcome<out P> {
    /** In the field. */
    data class Committed<P>(val prepared: P) : InsertOutcome<P>

    /** Handed to the editor's app (its share screen was started). */
    data class Sent<P>(val prepared: P) : InsertOutcome<P>

    /** On the clipboard: tell the user to paste it. */
    data class Copied<P>(val prepared: P) : InsertOutcome<P>

    /** Nothing worked; [kind] says why. */
    data class Failed(val kind: LibraryErrorKind) : InsertOutcome<Nothing>
}

/** What the library answered to `prepare`. */
sealed interface PrepareResult<out P> {
    data class Ready<P>(val prepared: P) : PrepareResult<P>

    data class Error(val kind: LibraryErrorKind) : PrepareResult<Nothing>
}

object InsertFlow {
    /** What a file shared to another app may be. */
    val SHARE_TYPES = listOf("image/*", "video/*")

    /** Errors of the field's rendition after which a rendition for sharing is still tried. */
    private val TRY_SHARING =
        setOf(LibraryErrorKind.NOTHING_FITS, LibraryErrorKind.UNREACHABLE, LibraryErrorKind.NOT_ON_DEVICE)

    fun <P> start(fieldMimeTypes: List<String>): InsertStep<P> = InsertStep.PrepareForField(fieldMimeTypes)

    /**
     * Nothing fits the field, the server is out of reach, or the field's
     * file is not on the phone (one for sharing may still be in the
     * cache): share instead. Any other error ends it (sharing would fail
     * the same way).
     */
    fun <P> afterFieldPrepared(result: PrepareResult<P>): InsertStep<P> = when (result) {
        is PrepareResult.Ready -> InsertStep.Commit(result.prepared)
        is PrepareResult.Error ->
            if (result.kind in TRY_SHARING) {
                InsertStep.PrepareForSharing
            } else {
                InsertStep.Done(InsertOutcome.Failed(result.kind))
            }
    }

    fun <P> afterCommit(prepared: P, committed: Boolean): InsertStep<P> =
        if (committed) InsertStep.Done(InsertOutcome.Committed(prepared)) else InsertStep.PrepareForSharing

    /** [canSend]: the editor is another app, which a share can be aimed at. */
    fun <P> afterSharePrepared(result: PrepareResult<P>, canSend: Boolean): InsertStep<P> = when (result) {
        is PrepareResult.Ready -> if (canSend) InsertStep.Send(result.prepared) else InsertStep.Copy(result.prepared)
        is PrepareResult.Error -> InsertStep.Done(InsertOutcome.Failed(result.kind))
    }

    fun <P> afterSend(prepared: P, sent: Boolean): InsertStep<P> =
        if (sent) InsertStep.Done(InsertOutcome.Sent(prepared)) else InsertStep.Copy(prepared)

    fun <P> afterCopy(prepared: P, copied: Boolean): InsertStep<P> =
        InsertStep.Done(if (copied) InsertOutcome.Copied(prepared) else InsertOutcome.Failed(LibraryErrorKind.OTHER))

    /**
     * The rendition to record a use of: every hand-off counts (in the
     * field, sent to the editor's app, or on the clipboard, as the desktop
     * counts a copy); null when nothing worked.
     */
    fun <P> used(outcome: InsertOutcome<P>): P? = when (outcome) {
        is InsertOutcome.Committed -> outcome.prepared
        is InsertOutcome.Sent -> outcome.prepared
        is InsertOutcome.Copied -> outcome.prepared
        is InsertOutcome.Failed -> null
    }

    /** Whether a share can be aimed at the editor: a package, and not this app (whose share target uploads). */
    fun canSendTo(editorPackage: String?, ownPackage: String): Boolean =
        !editorPackage.isNullOrEmpty() && editorPackage != ownPackage
}
