package io.github.dolfost.stickpile.ime

/**
 * What survives when the editor restarts the input of the same field (for
 * example after taking a sticker): the picker should not jump back to its
 * first tab, nor forget the draft it searched for. Pure.
 */
object InputRestart {
    /**
     * The view keeps what it shows: it was started, this is a restart, and
     * there is no new draft (none, or the one already searched for).
     */
    fun keepsView(started: Boolean, restarting: Boolean, query: String?, shownQuery: String?): Boolean =
        started && restarting && (query == null || query == shownQuery)

    /** The draft to remember: a new input's (even none), or a restart's when it has one. */
    fun draft(restarting: Boolean, read: Draft?, remembered: Draft?): Draft? =
        if (!restarting || read != null) read else remembered
}
