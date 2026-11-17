package io.github.dolfost.stickpile.provider

import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException

/** The kind of a library error, without its fields. */
val LibraryException.kind: LibraryErrorKind
    get() = when (this) {
        is LibraryException.NotSignedIn -> LibraryErrorKind.NOT_SIGNED_IN
        is LibraryException.Unauthorized -> LibraryErrorKind.UNAUTHORIZED
        is LibraryException.Unreachable -> LibraryErrorKind.UNREACHABLE
        is LibraryException.NotFound -> LibraryErrorKind.NOT_FOUND
        is LibraryException.RateLimited -> LibraryErrorKind.RATE_LIMITED
        is LibraryException.NothingFits -> LibraryErrorKind.NOTHING_FITS
        is LibraryException.InvalidInput -> LibraryErrorKind.INVALID_INPUT
        is LibraryException.Server -> LibraryErrorKind.SERVER
        is LibraryException.Io -> LibraryErrorKind.IO
        is LibraryException.Other -> LibraryErrorKind.OTHER
        is LibraryException.NotOnDevice -> LibraryErrorKind.NOT_ON_DEVICE
    }

/** The `error` values of the provider's calls (docs/android.md, "Provider contract", "Calls"). */
object ProviderErrors {
    const val NOTHING_FITS = "nothing_fits"
    const val NOT_FOUND = "not_found"
    const val NOT_SIGNED_IN = "not_signed_in"
    const val UNREACHABLE = "unreachable"
    const val FAILED = "failed"

    /** A refused token counts as signed out: the user has to sign in again. */
    fun code(kind: LibraryErrorKind): String = when (kind) {
        LibraryErrorKind.NOTHING_FITS -> NOTHING_FITS
        LibraryErrorKind.NOT_FOUND -> NOT_FOUND
        LibraryErrorKind.NOT_SIGNED_IN, LibraryErrorKind.UNAUTHORIZED -> NOT_SIGNED_IN
        // Not kept offline, and the server away: unreachable to a keyboard
        // (the contract has no code of its own for it).
        LibraryErrorKind.UNREACHABLE, LibraryErrorKind.NOT_ON_DEVICE -> UNREACHABLE
        LibraryErrorKind.RATE_LIMITED,
        LibraryErrorKind.INVALID_INPUT,
        LibraryErrorKind.SERVER,
        LibraryErrorKind.IO,
        LibraryErrorKind.OTHER,
        -> FAILED
    }
}
