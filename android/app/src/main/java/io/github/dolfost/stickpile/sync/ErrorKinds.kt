package io.github.dolfost.stickpile.sync

import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException

/**
 * The kind of a library error without its details, as `SyncState.lastError`
 * gives it (pure: no call into the native library).
 */
fun LibraryException.kind(): LibraryErrorKind = when (this) {
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
