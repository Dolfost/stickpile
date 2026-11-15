package io.github.dolfost.stickpile.ime

import androidx.annotation.StringRes
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.LibraryErrorKind

/** What the picker says about a library error: a short line in the keyboard. */
object ImeTexts {
    /** Errors after which only signing in (in the app) helps. */
    fun isSignedOut(kind: LibraryErrorKind): Boolean =
        kind == LibraryErrorKind.NOT_SIGNED_IN || kind == LibraryErrorKind.UNAUTHORIZED

    @StringRes
    fun forError(kind: LibraryErrorKind): Int = when (kind) {
        LibraryErrorKind.NOT_SIGNED_IN, LibraryErrorKind.UNAUTHORIZED -> R.string.ime_signed_out
        LibraryErrorKind.UNREACHABLE -> R.string.ime_error_unreachable
        LibraryErrorKind.NOT_FOUND -> R.string.ime_error_not_found
        LibraryErrorKind.RATE_LIMITED -> R.string.ime_error_rate_limited
        LibraryErrorKind.NOTHING_FITS -> R.string.ime_error_nothing_fits
        LibraryErrorKind.SERVER -> R.string.ime_error_server
        LibraryErrorKind.IO -> R.string.ime_error_io
        LibraryErrorKind.INVALID_INPUT, LibraryErrorKind.OTHER -> R.string.ime_error_failed
    }
}
