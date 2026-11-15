package io.github.dolfost.stickpile.ui

import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import io.github.dolfost.stickpile.ui.common.Failure
import io.github.dolfost.stickpile.ui.main.MainRoute
import io.github.dolfost.stickpile.ui.main.MainViewModel
import io.github.dolfost.stickpile.ui.main.isAnotherAccount
import io.github.dolfost.stickpile.ui.main.routeAfterAccountCheck
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** The launcher's first screen and its moves (ui.main). */
@OptIn(ExperimentalCoroutinesApi::class)
class MainViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    private val library = FakeLibrary()
    private val signedIn = mutableListOf<StickpileLibraryInterface>()
    private var accountChanges = 0

    private fun model(open: suspend () -> Result<StickpileLibraryInterface> = { Result.success(library) }) =
        MainViewModel(
            open = open,
            signedIn = { signedIn += it },
            accountChanged = { accountChanges++ },
            log = { _, _ -> },
            accountVersion = 0,
        )

    @Test
    fun theAccountCheckDecidesTheFirstScreen() {
        assertEquals(MainRoute.Library, routeAfterAccountCheck(Result.success(alice)))
        assertEquals(MainRoute.Onboarding(), routeAfterAccountCheck(Result.success(null)))
        // A token, but the server is down and nothing was cached: retry, not onboarding.
        assertEquals(
            MainRoute.AccountUnknown(Failure(LibraryErrorKind.UNREACHABLE)),
            routeAfterAccountCheck(Result.failure(unreachable())),
        )
        assertEquals(MainRoute.Onboarding(), routeAfterAccountCheck(Result.failure(LibraryException.NotSignedIn("d"))))
        assertEquals(
            MainRoute.AccountUnknown(Failure(LibraryErrorKind.SERVER, status = 500)),
            routeAfterAccountCheck(Result.failure(LibraryException.Server(500u, "d"))),
        )
        assertEquals(
            MainRoute.AccountUnknown(Failure(LibraryErrorKind.IO)),
            routeAfterAccountCheck(Result.failure(LibraryException.Io("d"))),
        )
    }

    @Test
    fun signedInGoesToTheLibraryAndStartsTheSync() = runTest(main.dispatcher) {
        library.account = { alice }
        val vm = model()
        vm.start()
        assertEquals(MainRoute.Starting, vm.route)
        advanceUntilIdle()
        assertEquals(MainRoute.Library, vm.route)
        assertEquals(listOf<StickpileLibraryInterface>(library), signedIn)
        // A second start (a rotation) asks nothing again.
        vm.start()
        advanceUntilIdle()
        assertEquals(listOf("account"), library.calls)
    }

    @Test
    fun signedOutGoesToOnboardingWithoutSync() = runTest(main.dispatcher) {
        val vm = model()
        vm.start()
        advanceUntilIdle()
        assertEquals(MainRoute.Onboarding(), vm.route)
        assertEquals(emptyList<StickpileLibraryInterface>(), signedIn)
    }

    @Test
    fun aLibraryThatCannotOpenIsBrokenUntilItOpens() = runTest(main.dispatcher) {
        var fails = true
        val vm = model(open = {
            if (fails) Result.failure(LibraryException.InvalidInput("config_file", "d")) else Result.success(library)
        })
        vm.start()
        advanceUntilIdle()
        assertEquals(MainRoute.Broken(Failure(LibraryErrorKind.INVALID_INPUT, field = "config_file")), vm.route)
        fails = false
        vm.check()
        advanceUntilIdle()
        assertEquals(MainRoute.Onboarding(), vm.route)
    }

    @Test
    fun aLinkDuringTheStartPairsAndMayGoBackWhenSignedIn() = runTest(main.dispatcher) {
        library.account = { alice }
        val vm = model()
        vm.openLink("stickpile://pair?url=x&code=y")
        vm.start()
        advanceUntilIdle()
        assertEquals(MainRoute.Onboarding("stickpile://pair?url=x&code=y", canCancel = true), vm.route)
        vm.onOnboardingCancelled()
        assertEquals(MainRoute.Library, vm.route)
    }

    @Test
    fun aLinkWhileSignedOutCannotBeCancelled() = runTest(main.dispatcher) {
        val vm = model()
        vm.openLink("link")
        vm.start()
        advanceUntilIdle()
        assertEquals(MainRoute.Onboarding("link", canCancel = false), vm.route)
    }

    @Test
    fun aLinkInTheLibraryOpensPairing() = runTest(main.dispatcher) {
        library.account = { alice }
        val vm = model()
        vm.start()
        advanceUntilIdle()
        vm.openLink("link")
        assertEquals(MainRoute.Onboarding("link", canCancel = true), vm.route)
    }

    @Test
    fun aSignInGoesThroughTheKeyboardToTheLibrary() = runTest(main.dispatcher) {
        val vm = model()
        vm.start()
        advanceUntilIdle()
        val before = vm.session
        vm.onSignedIn(alice)
        assertEquals(MainRoute.KeyboardSetup, vm.route)
        // The walk-through names who is signed in.
        assertEquals(alice, vm.signedInAs)
        assertEquals(listOf<StickpileLibraryInterface>(library), signedIn)
        assertEquals(before + 1, vm.session)
        vm.onKeyboardSetupDone()
        assertEquals(MainRoute.Library, vm.route)
    }

    @Test
    fun aSignInAsAnotherAccountOrServerRevokesWhatWasHandedOut() = runTest(main.dispatcher) {
        library.account = { alice }
        val vm = model()
        vm.start()
        advanceUntilIdle()
        // Alice again, on her server (a link while signed in): nothing changed.
        vm.onSignedIn(alice)
        assertEquals(0, accountChanges)
        vm.onSignedIn(alice.copy(username = "bob"))
        assertEquals(1, accountChanges)
        vm.onSignedIn(alice.copy(username = "bob", serverUrl = "https://other.example"))
        assertEquals(2, accountChanges)
    }

    @Test
    fun aSignInAfterNobodyCountsAsAnotherAccount() = runTest(main.dispatcher) {
        val vm = model()
        vm.start()
        advanceUntilIdle()
        vm.onSignedIn(alice)
        assertEquals(1, accountChanges)
        assertTrue(isAnotherAccount(null, alice))
        assertFalse(isAnotherAccount(alice, alice.copy(displayName = "Alice", isAdmin = true)))
    }

    @Test
    fun aSignOutElsewhereShowsOnboardingWhenTheLauncherIsBack() = runTest(main.dispatcher) {
        library.account = { alice }
        val vm = model()
        vm.start()
        advanceUntilIdle()
        library.calls.clear()
        // Resumed, nothing changed: no new question.
        vm.onAccountChanges(0)
        advanceUntilIdle()
        assertEquals(emptyList<String>(), library.calls)
        // Signed out in the settings.
        library.account = { null }
        vm.onAccountChanges(1)
        advanceUntilIdle()
        assertEquals(listOf("account"), library.calls)
        assertEquals(MainRoute.Onboarding(), vm.route)
    }

    @Test
    fun anAccountChangeDuringOnboardingAsksAgainWhenItIsLeft() = runTest(main.dispatcher) {
        library.account = { alice }
        val vm = model()
        vm.start()
        advanceUntilIdle()
        vm.openLink("link")
        library.calls.clear()
        // Signed out elsewhere while onboarding was up.
        library.account = { null }
        vm.onAccountChanges(1)
        advanceUntilIdle()
        assertEquals(MainRoute.Onboarding("link", canCancel = true), vm.route)
        assertEquals(emptyList<String>(), library.calls)
        vm.onOnboardingCancelled()
        advanceUntilIdle()
        assertEquals(MainRoute.Onboarding(), vm.route)
        // Who signs in next is another account as far as the launcher knows.
        vm.onSignedIn(alice)
        assertEquals(1, accountChanges)
    }

    @Test
    fun anUnknownAccountOffersARetryOrANewSignIn() = runTest(main.dispatcher) {
        library.account = { throw LibraryException.Server(500u, "d") }
        val vm = model()
        vm.start()
        advanceUntilIdle()
        assertEquals(MainRoute.AccountUnknown(Failure(LibraryErrorKind.SERVER, status = 500)), vm.route)
        vm.signInAnew()
        assertEquals(MainRoute.Onboarding(), vm.route)
    }
}
