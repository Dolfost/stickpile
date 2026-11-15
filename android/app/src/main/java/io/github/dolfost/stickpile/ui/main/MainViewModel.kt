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
import kotlinx.coroutines.Job
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
     * with; [canCancel] when someone is signed in already (a link came in,
     * or the settings asked to sign in elsewhere).
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

/** Whether a sign-in as [after] is another account or server than [before] (null: not known). */
fun isAnotherAccount(before: Account?, after: Account): Boolean =
    before == null || before.username != after.username || before.serverUrl != after.serverUrl

/**
 * The launcher's state: opens the library, decides the first screen, and
 * moves between onboarding, the keyboard walk-through and the library.
 *
 * [open] opens the library (off the main thread); [signedIn] runs when an
 * account is known (schedule the background sync, sync if due);
 * [accountChanged] when a sign-in changed the account or the server (what
 * was handed out for the last one goes). [accountVersion] is
 * [io.github.dolfost.stickpile.ui.common.AccountChanges.version] when the
 * view model is made.
 */
class MainViewModel(
    private val open: suspend () -> Result<StickpileLibraryInterface>,
    private val signedIn: (StickpileLibraryInterface) -> Unit,
    private val accountChanged: () -> Unit = {},
    private val log: FailureLog = FailureLog.android,
    accountVersion: Int = 0,
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

    /** The account onboarding signed in last, for the next screen to name; null before. */
    var signedInAs: Account? by mutableStateOf(null)
        private set

    /** Who is signed in as far as this knows; null when nobody or not known. */
    private var account: Account? = null
    private var pendingLink: String? = null
    private var pendingElsewhere = false
    private var started = false
    private var checking: Job? = null
    private var seenAccountVersion = accountVersion

    /** The account changed elsewhere during onboarding: leaving it asks again. */
    private var stale = false

    /** Decide the first screen; once per view model (a rotation does not repeat it). */
    fun start() {
        if (started) return
        started = true
        check()
    }

    /** Ask again who is signed in (retry, or after the account changed elsewhere). */
    fun check() {
        route = MainRoute.Starting
        stale = false
        checking?.cancel()
        checking = viewModelScope.launch {
            val lib = library ?: open().getOrElse { e ->
                log.failed(Doing.OPENING, e)
                route = MainRoute.Broken(Failure.of(e))
                return@launch
            }
            library = lib
            val result = catching { lib.account() }
            result.exceptionOrNull()?.let { log.failed(Doing.CHECKING_ACCOUNT, it) }
            account = result.getOrNull()
            var next = routeAfterAccountCheck(result)
            if (next == MainRoute.Library) signedIn(lib)
            val link = pendingLink
            if (link != null || pendingElsewhere) {
                pendingLink = null
                pendingElsewhere = false
                next = MainRoute.Onboarding(link, canCancel = next == MainRoute.Library)
            }
            route = next
        }
    }

    /**
     * [io.github.dolfost.stickpile.ui.common.AccountChanges.version], while
     * the launcher is resumed: when it moved, the account changed elsewhere
     * (a sign-out in the settings), so ask again who is signed in. Onboarding
     * stays up; leaving it asks.
     */
    fun onAccountChanges(version: Int) {
        if (version == seenAccountVersion) return
        seenAccountVersion = version
        account = null
        when (route) {
            is MainRoute.Onboarding -> stale = true
            else -> if (started) check()
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

    /**
     * The settings asked to sign in on another server or as someone else
     * (an account signed in by its tailnet identity cannot sign out).
     */
    fun signInElsewhere() {
        when (route) {
            MainRoute.Starting -> pendingElsewhere = true
            MainRoute.Library, MainRoute.KeyboardSetup -> route = MainRoute.Onboarding(canCancel = true)
            is MainRoute.Onboarding -> Unit
            is MainRoute.Broken, is MainRoute.AccountUnknown -> {
                pendingElsewhere = true
                check()
            }
        }
    }

    /** Onboarding signed [account] in: on to the keyboard walk-through, which names it. */
    fun onSignedIn(account: Account) {
        if (isAnotherAccount(this.account, account)) accountChanged()
        this.account = account
        stale = false
        signedInAs = account
        library?.let(signedIn)
        session++
        route = MainRoute.KeyboardSetup
    }

    /** Onboarding was left without signing in (only offered when someone is signed in). */
    fun onOnboardingCancelled() {
        session++
        if (stale) check() else route = MainRoute.Library
    }

    /** Skip the account check's failure: sign in anew. */
    fun signInAnew() {
        route = MainRoute.Onboarding()
    }

    fun onKeyboardSetupDone() {
        route = MainRoute.Library
    }
}
