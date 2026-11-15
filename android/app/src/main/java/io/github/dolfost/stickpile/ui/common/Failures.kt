package io.github.dolfost.stickpile.ui.common

import android.util.Log
import androidx.annotation.StringRes
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException
import kotlin.coroutines.cancellation.CancellationException

/**
 * A failed library call as the screens need it: the kind (docs/android.md,
 * "Bridge interface"), and the field or HTTP status when the kind has one.
 * The exception's `detail` is for the log only, never shown.
 */
data class Failure(val kind: LibraryErrorKind, val field: String? = null, val status: Int? = null) {
    companion object {
        /** Any exception a library call threw; anything not from the library is [LibraryErrorKind.OTHER]. */
        fun of(e: Throwable): Failure = when (e) {
            is LibraryException.NotSignedIn -> Failure(LibraryErrorKind.NOT_SIGNED_IN)
            is LibraryException.Unauthorized -> Failure(LibraryErrorKind.UNAUTHORIZED)
            is LibraryException.Unreachable -> Failure(LibraryErrorKind.UNREACHABLE)
            is LibraryException.NotFound -> Failure(LibraryErrorKind.NOT_FOUND)
            is LibraryException.RateLimited -> Failure(LibraryErrorKind.RATE_LIMITED)
            is LibraryException.NothingFits -> Failure(LibraryErrorKind.NOTHING_FITS)
            is LibraryException.InvalidInput -> Failure(LibraryErrorKind.INVALID_INPUT, field = e.field)
            is LibraryException.Server -> Failure(LibraryErrorKind.SERVER, status = e.status.toInt())
            is LibraryException.Io -> Failure(LibraryErrorKind.IO)
            else -> Failure(LibraryErrorKind.OTHER)
        }
    }
}

/** What the user was doing when a call failed: the same kind reads differently in each. */
enum class Doing {
    /** Opening the library (reading its config and cache). */
    OPENING,

    /** Asking who is signed in. */
    CHECKING_ACCOUNT,

    /** Reading a list or searching. */
    LOADING,
    SHARING,
    LIKING,
    SYNCING,
    CLEARING_CACHE,
    SIGNING_OUT,

    /** Saving the server address before a Tailscale or password sign-in. */
    SETTING_SERVER,
    PAIRING,
    TAILNET_LOGIN,
    PASSWORD_LOGIN,

    /** A password sign-in with the two-factor code the server asked for. */
    PASSWORD_AND_CODE_LOGIN,
}

/** A text to show: a string resource and its format arguments. */
data class Message(@StringRes val id: Int, val args: List<Any> = emptyList())

/** The text for [failure] while [doing]. */
fun messageFor(failure: Failure, doing: Doing): Message = when (failure.kind) {
    LibraryErrorKind.NOT_SIGNED_IN -> Message(R.string.app_error_not_signed_in)
    LibraryErrorKind.UNAUTHORIZED -> Message(
        when (doing) {
            Doing.PAIRING -> R.string.app_error_pairing_code_refused
            Doing.PASSWORD_LOGIN -> R.string.app_error_wrong_password
            Doing.PASSWORD_AND_CODE_LOGIN -> R.string.app_error_wrong_password_or_code
            else -> R.string.app_error_unauthorized
        },
    )
    LibraryErrorKind.UNREACHABLE -> Message(
        when (doing) {
            Doing.LOADING -> R.string.app_error_unreachable_nothing_saved
            Doing.SHARING -> R.string.app_error_unreachable_not_saved
            else -> R.string.app_error_unreachable
        },
    )
    LibraryErrorKind.NOT_FOUND -> Message(R.string.app_error_not_found)
    LibraryErrorKind.RATE_LIMITED -> Message(R.string.app_error_rate_limited)
    LibraryErrorKind.NOTHING_FITS -> Message(R.string.app_error_nothing_fits)
    LibraryErrorKind.INVALID_INPUT -> Message(
        when (failure.field) {
            "server_url" -> R.string.app_error_invalid_server_url
            "code" -> R.string.app_error_invalid_pairing_code
            "totp" -> R.string.app_error_totp_needed
            "username" -> R.string.app_error_login_refused
            "token_name" -> R.string.app_error_token_name_refused
            "config_file" -> R.string.app_error_config_file
            else -> R.string.app_error_invalid_input
        },
    )
    LibraryErrorKind.SERVER -> Message(R.string.app_error_server, listOf(failure.status ?: 0))
    LibraryErrorKind.IO -> Message(R.string.app_error_storage)
    LibraryErrorKind.OTHER -> Message(R.string.app_error_other)
}

/** The text for the kind a sync failed with ([io.github.dolfost.stickpile.rust.SyncState.lastError]). */
fun messageFor(kind: LibraryErrorKind, doing: Doing): Message = messageFor(Failure(kind), doing)

/**
 * Where failures go: the log, with the library's detail (which never holds
 * a token, password or code). An interface so that tests need no Android.
 */
fun interface FailureLog {
    fun failed(doing: Doing, e: Throwable)

    companion object {
        const val TAG = "stickpile"

        /** To `android.util.Log`. */
        val android = FailureLog { doing, e -> Log.w(TAG, "$doing failed: ${e.javaClass.simpleName}: ${e.message}") }
    }
}

/**
 * [block]'s value or what it threw, except a cancellation, which goes on
 * (unlike `runCatching`), so that a cancelled coroutine stops.
 */
inline fun <T> catching(block: () -> T): Result<T> = try {
    Result.success(block())
} catch (e: CancellationException) {
    throw e
} catch (e: Exception) {
    Result.failure(e)
}
