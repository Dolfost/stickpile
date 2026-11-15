package io.github.dolfost.stickpile.ui

import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.AuthKind
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.SyncState
import io.github.dolfost.stickpile.ui.common.Message
import io.github.dolfost.stickpile.ui.settings.KeyboardEntry
import io.github.dolfost.stickpile.ui.settings.SettingsEvent
import io.github.dolfost.stickpile.ui.settings.SettingsViewModel
import io.github.dolfost.stickpile.ui.settings.sortedEntries
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** The settings: account, sync, cache and signing out (ui.settings). */
@OptIn(ExperimentalCoroutinesApi::class)
class SettingsViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    private val library = FakeLibrary()
    private var cacheCleared = 0
    private var signedOut = 0

    /** The app's scope: it outlives the view model. */
    private val background = CoroutineScope(SupervisorJob() + main.dispatcher)

    private fun model() = SettingsViewModel(
        library,
        background = background,
        io = main.dispatcher,
        log = { _, _ -> },
        onCacheCleared = { cacheCleared++ },
        onSignedOut = { signedOut++ },
    )

    private val vm by lazy { model() }

    @Test
    fun showsTheAccountTheSyncAndTheCache() = runTest(main.dispatcher) {
        library.account = { alice }
        library.serverUrl = "https://s.example"
        library.syncState = SyncState(lastSync = 1_700_000_000, lastError = LibraryErrorKind.UNREACHABLE, pendingUsage = 2u, due = false)
        library.pendingUsage = 3u
        library.cacheBytes = { 12_345u }
        assertTrue(vm.state.checking)
        advanceUntilIdle()
        val s = vm.state
        assertFalse(s.checking)
        assertEquals(alice, s.account)
        assertNull(s.accountFailure)
        assertEquals("https://s.example", s.serverUrl)
        assertEquals(1_700_000_000L, s.sync?.lastSync)
        assertEquals(LibraryErrorKind.UNREACHABLE, s.sync?.lastError)
        assertEquals(3L, s.pendingUsage)
        assertEquals(12_345L, s.cacheBytes)
    }

    @Test
    fun anUnknownAccountSaysWhy() = runTest(main.dispatcher) {
        library.account = { throw LibraryException.Unreachable("d") }
        library.cacheBytes = { throw LibraryException.Io("d") }
        vm.state // create it
        advanceUntilIdle()
        assertNull(vm.state.account)
        assertEquals(Message(R.string.app_error_unreachable), vm.state.accountFailure)
        assertNull(vm.state.cacheBytes)
    }

    @Test
    fun clearingTheCacheSaysHowMuchItFreed() = runTest(main.dispatcher) {
        library.cacheBytes = { 2048u }
        vm.state // create it
        advanceUntilIdle()
        library.clearCache = {
            library.cacheBytes = { 0u }
            2048u
        }
        vm.clearCache()
        advanceUntilIdle()
        assertEquals(SettingsEvent.CacheCleared(2048), vm.eventFlow.first())
        assertEquals(1, cacheCleared)
        assertEquals(0L, vm.state.cacheBytes)
        assertFalse(vm.state.clearing)
    }

    @Test
    fun syncNowReadsTheStateAgainAndReportsAFailure() = runTest(main.dispatcher) {
        vm.state // create it
        advanceUntilIdle()
        library.calls.clear()
        library.syncNow = { throw LibraryException.RateLimited("d") }
        vm.syncNow()
        assertTrue(vm.state.syncing)
        advanceUntilIdle()
        assertFalse(vm.state.syncing)
        assertEquals(SettingsEvent.Notice(Message(R.string.app_error_rate_limited)), vm.eventFlow.first())
        assertEquals("syncNow", library.calls.first())
        assertTrue("syncState" in library.calls)
    }

    @Test
    fun signingOutStopsTheSyncAndLeaves() = runTest(main.dispatcher) {
        library.account = { alice }
        vm.state // create it
        advanceUntilIdle()
        vm.signOut()
        advanceUntilIdle()
        assertTrue("logout" in library.calls)
        assertEquals(1, signedOut)
        assertTrue(vm.state.signedOut)
        assertNull(vm.state.account)
    }

    @Test
    fun aSignOutEndsWithItsFollowUpsWhenTheScreenGoes() = runTest(main.dispatcher) {
        library.account = { alice }
        val gate = CompletableDeferred<Unit>()
        library.logout = { gate.await() }
        val store = ViewModelStore()
        val vm = ViewModelProvider.create(store, viewModelFactory { initializer { model() } })[SettingsViewModel::class]
        advanceUntilIdle()
        vm.signOut()
        advanceUntilIdle()
        assertTrue(vm.state.signingOut)
        // The screen goes (its view models are cleared) before the server answers.
        store.clear()
        gate.complete(Unit)
        advanceUntilIdle()
        assertTrue("logout" in library.calls)
        assertEquals(1, signedOut)
    }

    @Test
    fun anAccountOfTheTailnetCannotSignOut() = runTest(main.dispatcher) {
        library.account = { alice.copy(auth = AuthKind.TAILNET) }
        vm.state // create it
        advanceUntilIdle()
        assertEquals(AuthKind.TAILNET, vm.state.account?.auth)
        assertFalse(vm.state.canSignOut)
        vm.signOut()
        advanceUntilIdle()
        assertFalse("logout" in library.calls)
        assertEquals(0, signedOut)
        assertFalse(vm.state.signingOut)
    }

    @Test
    fun aFailedSignOutStays() = runTest(main.dispatcher) {
        library.account = { alice }
        library.logout = { throw LibraryException.Io("token file") }
        vm.state // create it
        advanceUntilIdle()
        vm.signOut()
        advanceUntilIdle()
        assertEquals(SettingsEvent.Notice(Message(R.string.app_error_storage)), vm.eventFlow.first())
        assertEquals(0, signedOut)
        assertFalse(vm.state.signedOut)
        assertFalse(vm.state.signingOut)
        assertEquals(alice, vm.state.account)
    }

    @Test
    fun keyboardsAreListedByWhatIsShown() {
        val entries = listOf(
            KeyboardEntry("org.b.keyboard", "Zebra keys"),
            KeyboardEntry("juloo.keyboard2", null),
            KeyboardEntry("org.a.keyboard", "apple board"),
        )
        assertEquals(
            listOf("apple board", "juloo.keyboard2", "Zebra keys"),
            sortedEntries(entries).map { it.title },
        )
    }
}
