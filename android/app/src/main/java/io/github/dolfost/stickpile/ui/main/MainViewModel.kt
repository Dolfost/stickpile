package io.github.dolfost.stickpile.ui.main

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.dolfost.stickpile.rust.Account
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import io.github.dolfost.stickpile.ui.common.Doing
import io.github.dolfost.stickpile.ui.common.Failure
import io.github.dolfost.stickpile.ui.common.FailureLog
import io.github.dolfost.stickpile.ui.common.catching
import kotlinx.coroutines.launch

/** Which screen the launcher shows. */
sealed interface MainRoute {
    /** Opening the library and asking who is signed in. */
    data object Starting : MainRoute

    /** The library cannot be opened (its config is unreadable). */
    data class Broken(val failure: Failure) : MainRoute

    /** Asking who is signed in failed for another reason than being signed out or offline. */
    data class AccountUnknown(val failure: Failure) : MainRoute

    /**
     * Pairing or signing in. [link] is a pairing link the app was opened
     * with; [canCancel] when someone is signed in already (a link came in).
     */
    data class Onboarding(val link: String? = null, val canCancel: Boolean = false) : MainRoute

    /** Turning the picker keyboard on, after a sign-in. */
    data object KeyboardSetup : MainRoute

    /** Search, tabs and the grid. */
    data object Library : MainRoute
}

/**
 * Where to go once `account()` answered: the library when someone is
 * signed in, else onboarding (a new install answers null, also offline).
 * `NotSignedIn` goes to onboarding too. `Unreachable` means a token is
 * stored but the server cannot be reached and no account was cached: the
 * user is probably signed in, so it gets the screen with a retry, like the
 * other failures.
 */
fun routeAfterAccountCheck(result: Result<Account?>): MainRoute = result.fold(
    onSuccess = { account -> if (account != null) MainRoute.Library else MainRoute.Onboarding() },
    onFailure = { e ->
        val failure = Failure.of(e)
        when (failure.kind) {
            LibraryErrorKind.NOT_SIGNED_IN -> MainRoute.Onboarding()
            else -> MainRoute.AccountUnknown(failure)
        }
    },
)

/**
 * The launcher's state: opens the library, decides the first screen, and
 * moves between onboarding, the keyboard walk-through and the library.
 *
 * [open] opens the library (off the main thread); [signedIn] runs when an
 * account is known (schedule the background sync, sync if due).
 */
class MainViewModel(
    private val open: suspend () -> Result<StickpileLibraryInterface>,
    private val signedIn: (StickpileLibraryInterface) -> Unit,
    private val log: FailureLog = FailureLog.android,
) : ViewModel() {

    var route: MainRoute by mutableStateOf(MainRoute.Starting)
        private set

    /** The library once open. */
    var library: StickpileLibraryInterface? by mutableStateOf(null)
        private set

    /**
     * Counts the onboardings finished (signed in or left): the onboarding
     * and library view models are keyed by it, so that each account and
     * each onboarding start with fresh ones.
     */
    var session: Int by mutableIntStateOf(0)
        private set

    private var pendingLink: String? = null
    private var started = false

    /** Decide the first screen; once per view model (a rotation does not repeat it). */
    fun start() {
        if (started) return
        started = true
        check()
    }

    /** Ask again who is signed in (retry, or after the account changed elsewhere). */
    fun check() {
        route = MainRoute.Starting
        viewModelScope.launch {
            val lib = library ?: open().getOrElse { e ->
                log.failed(Doing.OPENING, e)
                route = MainRoute.Broken(Failure.of(e))
                return@launch
            }
            library = lib
            val result = catching { lib.account() }
            result.exceptionOrNull()?.let { log.failed(Doing.CHECKING_ACCOUNT, it) }
            var next = routeAfterAccountCheck(result)
            if (next == MainRoute.Library) signedIn(lib)
            val link = pendingLink
            if (link != null) {
                pendingLink = null
                next = MainRoute.Onboarding(link, canCancel = next == MainRoute.Library)
            }
            route = next
        }
    }

    /** The app was opened with a pairing link (`stickpile://pair?...`): pair with it. */
    fun openLink(link: String) {
        when (val now = route) {
            MainRoute.Starting -> pendingLink = link
            MainRoute.Library, MainRoute.KeyboardSetup -> route = MainRoute.Onboarding(link, canCancel = true)
            is MainRoute.Onboarding -> route = MainRoute.Onboarding(link, now.canCancel)
            is MainRoute.Broken, is MainRoute.AccountUnknown -> {
                pendingLink = link
                check()
            }
        }
    }

    /** Onboarding signed someone in: on to the keyboard walk-through. */
    fun onSignedIn() {
        library?.let(signedIn)
        session++
        route = MainRoute.KeyboardSetup
    }

    /** Onboarding was left without signing in (only offered when someone is signed in). */
    fun onOnboardingCancelled() {
        session++
        route = MainRoute.Library
    }

    /** Skip the account check's failure: sign in anew. */
    fun signInAnew() {
        route = MainRoute.Onboarding()
    }

    fun onKeyboardSetupDone() {
        route = MainRoute.Library
    }
}
