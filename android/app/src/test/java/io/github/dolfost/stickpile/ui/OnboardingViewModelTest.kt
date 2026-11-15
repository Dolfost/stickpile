package io.github.dolfost.stickpile.ui

import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.PairingLink
import io.github.dolfost.stickpile.ui.common.Doing
import io.github.dolfost.stickpile.ui.common.Message
import io.github.dolfost.stickpile.ui.onboarding.OnboardingStep
import io.github.dolfost.stickpile.ui.onboarding.OnboardingViewModel
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** Onboarding: pairing by QR code, link or typed code, and signing in (ui.onboarding). */
@OptIn(ExperimentalCoroutinesApi::class)
class OnboardingViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    private val library = FakeLibrary()
    private val logged = mutableListOf<Doing>()
    private val vm by lazy { OnboardingViewModel(library, io = main.dispatcher, log = { doing, _ -> logged += doing }) }

    private val link = PairingLink(serverUrl = "https://s.example", code = "7KQ2-M9XA-4FTR")

    @Test
    fun theTypedCodeShowsAsTheLibraryReadsIt() = runTest(main.dispatcher) {
        library.normalizePairingCode = { if (it.length >= 12) "7KQ2-M9XA-4FTR" else null }
        vm.showCode()
        assertEquals(OnboardingStep.CODE, vm.state.step)
        vm.setServerUrl("https://s.example")
        vm.setCode("7kq2")
        advanceUntilIdle()
        assertNull(vm.state.codeDisplay)
        assertFalse(vm.state.canPair)
        vm.setCode("7kq2 m9xa 4ftr")
        advanceUntilIdle()
        assertEquals("7KQ2-M9XA-4FTR", vm.state.codeDisplay)
        assertTrue(vm.state.canPair)
    }

    @Test
    fun anOlderReadingOfTheCodeDoesNotWin() = runTest(main.dispatcher) {
        library.normalizePairingCode = { "display of $it" }
        vm.setCode("a")
        vm.setCode("ab")
        advanceUntilIdle()
        assertEquals("display of ab", vm.state.codeDisplay)
    }

    @Test
    fun pairingWithTheTypedCode() = runTest(main.dispatcher) {
        library.normalizePairingCode = { "7KQ2-M9XA-4FTR" }
        library.loginPairing = { _, _ -> alice }
        vm.showCode()
        vm.setServerUrl("https://s.example")
        vm.setCode("7kq2m9xa4ftr")
        advanceUntilIdle()
        vm.pairWithCode()
        assertTrue(vm.state.busy)
        advanceUntilIdle()
        assertFalse(vm.state.busy)
        assertEquals("loginPairing https://s.example 7kq2m9xa4ftr", library.calls.last())
        assertEquals(alice, vm.state.account)
        assertEquals("", vm.state.code)
    }

    @Test
    fun aRefusedCodeSaysSo() = runTest(main.dispatcher) {
        library.normalizePairingCode = { "7KQ2-M9XA-4FTR" }
        library.loginPairing = { _, _ -> throw unauthorized() }
        vm.showCode()
        vm.setServerUrl("https://s.example")
        vm.setCode("7kq2m9xa4ftr")
        advanceUntilIdle()
        vm.pairWithCode()
        advanceUntilIdle()
        assertEquals(Message(R.string.app_error_pairing_code_refused), vm.state.error)
        assertNull(vm.state.account)
        assertEquals(listOf(Doing.PAIRING), logged)
        // Typing again clears the error.
        vm.setCode("7kq2m9xa4ftq")
        assertNull(vm.state.error)
    }

    @Test
    fun aScannedLinkPairsAtOnce() = runTest(main.dispatcher) {
        library.parsePairingLink = { link }
        library.loginPairing = { _, _ -> alice }
        vm.scanned("stickpile://pair?url=https://s.example&code=7KQ2M9XA4FTR")
        advanceUntilIdle()
        assertEquals("loginPairing https://s.example 7KQ2-M9XA-4FTR", library.calls.last())
        assertEquals(alice, vm.state.account)
    }

    @Test
    fun aCancelledScanDoesNothingAndAnotherCodeIsRefused() = runTest(main.dispatcher) {
        vm.scanned(null)
        advanceUntilIdle()
        assertEquals(emptyList<String>(), library.calls)
        vm.scanned("https://example.org")
        advanceUntilIdle()
        assertEquals(Message(R.string.app_error_not_a_pairing_qr), vm.state.error)
        assertEquals(listOf("parsePairingLink"), library.calls)
    }

    @Test
    fun anOpenedLinkAsksFirst() = runTest(main.dispatcher) {
        library.parsePairingLink = { link }
        library.loginPairing = { _, _ -> alice }
        vm.openLink("stickpile://pair?url=https://s.example&code=7KQ2M9XA4FTR")
        advanceUntilIdle()
        assertEquals(OnboardingStep.CONFIRM_LINK, vm.state.step)
        assertEquals(link, vm.state.link)
        assertEquals(listOf("parsePairingLink"), library.calls)
        // The same link again (a rotation) is not read again.
        vm.openLink("stickpile://pair?url=https://s.example&code=7KQ2M9XA4FTR")
        advanceUntilIdle()
        assertEquals(listOf("parsePairingLink"), library.calls)

        vm.confirmLink()
        advanceUntilIdle()
        assertEquals(alice, vm.state.account)
    }

    @Test
    fun anOpenedLinkCanBeRefused() = runTest(main.dispatcher) {
        library.parsePairingLink = { link }
        vm.openLink("stickpile://pair?url=https://s.example&code=7KQ2M9XA4FTR")
        advanceUntilIdle()
        assertTrue(vm.back())
        assertEquals(OnboardingStep.WELCOME, vm.state.step)
        assertNull(vm.state.link)
        assertFalse(vm.back())
    }

    @Test
    fun aBrokenLinkSaysSo() = runTest(main.dispatcher) {
        vm.openLink("stickpile://pair?nothing")
        advanceUntilIdle()
        assertEquals(OnboardingStep.WELCOME, vm.state.step)
        assertEquals(Message(R.string.app_error_not_a_pairing_link), vm.state.error)
    }

    @Test
    fun theServerAddressIsSavedBeforeSigningIn() = runTest(main.dispatcher) {
        vm.showServer()
        assertFalse(vm.state.canContinue)
        vm.setServerUrl("https://s.example")
        vm.continueToSignIn()
        advanceUntilIdle()
        assertEquals(listOf("setServerUrl https://s.example"), library.calls)
        assertEquals(OnboardingStep.SIGN_IN, vm.state.step)
        assertTrue(vm.back())
        assertEquals(OnboardingStep.SERVER, vm.state.step)
    }

    @Test
    fun aBadServerAddressStaysToBeFixed() = runTest(main.dispatcher) {
        library.setServerUrl = { throw invalid("server_url") }
        vm.showServer()
        vm.setServerUrl("ftp://s.example")
        vm.continueToSignIn()
        advanceUntilIdle()
        assertEquals(OnboardingStep.SERVER, vm.state.step)
        assertEquals(Message(R.string.app_error_invalid_server_url), vm.state.error)
    }

    @Test
    fun tailscaleThatDoesNotKnowThePhoneSaysSo() = runTest(main.dispatcher) {
        library.loginTailnet = { null }
        vm.signInWithTailscale()
        advanceUntilIdle()
        assertEquals(Message(R.string.onboarding_tailnet_unknown), vm.state.error)
        assertNull(vm.state.account)

        library.loginTailnet = { alice }
        vm.signInWithTailscale()
        advanceUntilIdle()
        assertEquals(alice, vm.state.account)
        assertNull(vm.state.error)
    }

    @Test
    fun aPasswordSignInAsksForTheCodeWhenTheServerWantsIt() = runTest(main.dispatcher) {
        library.loginPassword = { _, _, totp -> if (totp == null) throw invalid("totp") else throw unauthorized() }
        vm.setUsername("alice")
        assertFalse(vm.state.canSignIn)
        vm.setPassword("secret")
        assertTrue(vm.state.canSignIn)
        vm.signInWithPassword()
        advanceUntilIdle()
        assertEquals("loginPassword alice secret null", library.calls.last())
        assertTrue(vm.state.needsTotp)
        assertEquals(Message(R.string.onboarding_totp_notice), vm.state.notice)
        assertNull(vm.state.error)
        // Now the code is needed to go on.
        assertFalse(vm.state.canSignIn)
        vm.setTotp("123456")
        assertTrue(vm.state.canSignIn)
        vm.signInWithPassword()
        advanceUntilIdle()
        assertEquals("loginPassword alice secret 123456", library.calls.last())
        assertEquals(Message(R.string.app_error_wrong_password_or_code), vm.state.error)

        library.loginPassword = { _, _, _ -> alice }
        vm.signInWithPassword()
        advanceUntilIdle()
        assertEquals(alice, vm.state.account)
        assertEquals("", vm.state.password)
        assertEquals("", vm.state.totp)
    }

    @Test
    fun aWrongPasswordWithoutACodeSaysSo() = runTest(main.dispatcher) {
        library.loginPassword = { _, _, _ -> throw unauthorized() }
        vm.setUsername("alice")
        vm.setPassword("wrong")
        vm.signInWithPassword()
        advanceUntilIdle()
        assertEquals(Message(R.string.app_error_wrong_password), vm.state.error)
        assertFalse(vm.state.needsTotp)
    }

    @Test
    fun nothingStartsWhileACallRuns() = runTest(main.dispatcher) {
        val gate = CompletableDeferred<Unit>()
        library.loginTailnet = {
            gate.await()
            alice
        }
        vm.signInWithTailscale()
        advanceUntilIdle()
        assertTrue(vm.state.busy)
        vm.signInWithTailscale()
        vm.setUsername("alice")
        vm.setPassword("secret")
        vm.signInWithPassword()
        assertTrue(vm.back()) // swallowed while busy
        advanceUntilIdle()
        assertEquals(listOf("loginTailnet"), library.calls)
        gate.complete(Unit)
        advanceUntilIdle()
        assertFalse(vm.state.busy)
        assertEquals(alice, vm.state.account)
    }

    @Test
    fun theStateDoesNotPrintSecrets() {
        vm.setPassword("hunter2")
        vm.setTotp("424242")
        vm.setCode("7KQ2M9XA4FTR")
        val printed = vm.state.toString()
        listOf("hunter2", "424242", "7KQ2M9XA4FTR").forEach { assertFalse(printed, it in printed) }
    }

    @Test
    fun aRefusedCameraLeavesTheCodeAsTheWay() {
        vm.cameraRefused()
        assertEquals(Message(R.string.onboarding_camera_refused), vm.state.error)
    }
}
