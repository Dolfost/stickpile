package io.github.dolfost.stickpile.ui

import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.offline.KeepAsk
import io.github.dolfost.stickpile.offline.MemoryOfflineStore
import io.github.dolfost.stickpile.offline.OfflineJobs
import io.github.dolfost.stickpile.offline.OfflineStore
import io.github.dolfost.stickpile.offline.OfflineWork
import io.github.dolfost.stickpile.rust.AuthKind
import io.github.dolfost.stickpile.rust.CacheCleared
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.OfflineOptions
import io.github.dolfost.stickpile.rust.OfflineScope
import io.github.dolfost.stickpile.rust.OfflineState
import io.github.dolfost.stickpile.rust.OfflineUsage
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
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/** The settings: account, sync, cache, keeping offline and signing out (ui.settings). */
@OptIn(ExperimentalCoroutinesApi::class)
class SettingsViewModelTest {
    @get:Rule
    val main = MainDispatcherRule()

    private val library = FakeLibrary()
    private var cacheCleared = 0
    private var signedOut = 0
    private val synced = mutableListOf<SyncState>()

    /** The app's scope: it outlives the view model. */
    private val background = CoroutineScope(SupervisorJob() + main.dispatcher)

    private fun model(jobs: OfflineJobs = FakeJobs(), store: OfflineStore = MemoryOfflineStore()) = SettingsViewModel(
        library,
        background = background,
        io = main.dispatcher,
        log = { _, _ -> },
        onCacheCleared = { cacheCleared++ },
        onSignedOut = { signedOut++ },
        onSynced = { synced += it },
        offline = store,
        jobs = jobs,
    )

    /** The downloads' job: what was asked of it. */
    private class FakeJobs : OfflineJobs {
        val calls = mutableListOf<String>()

        override fun request(mobileData: Boolean, restart: Boolean) {
            calls += "request ${if (mobileData) "any" else "wifi"}" + if (restart) " restart" else ""
        }

        override fun cancel() {
            calls += "cancel"
        }

        override val work: Flow<OfflineWork> = flowOf(OfflineWork.NONE)
    }

    private val vm by lazy { model() }

    @Test
    fun showsTheAccountTheSyncAndTheCache() = runTest(main.dispatcher) {
        library.account = { alice }
        library.serverUrl = "https://s.example"
        library.syncState =
            SyncState(lastSync = 1_700_000_000, lastError = LibraryErrorKind.UNREACHABLE, pendingUsage = 2u, due = false, cacheChanged = false)
        library.pendingUsage = 3u
        library.usage = { OfflineUsage(keptFiles = 2u, keptBytes = 900u, otherFiles = 5u, otherBytes = 12_345u, budget = null) }
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
        assertEquals(12_345uL, s.usage?.otherBytes)
        assertEquals(900uL, s.usage?.keptBytes)
    }

    @Test
    fun anUnknownAccountSaysWhy() = runTest(main.dispatcher) {
        library.account = { throw LibraryException.Unreachable("d") }
        library.usage = { throw LibraryException.Io("d") }
        vm.state // create it
        advanceUntilIdle()
        assertNull(vm.state.account)
        assertEquals(Message(R.string.app_error_unreachable), vm.state.accountFailure)
        assertNull(vm.state.usage)
    }

    @Test
    fun clearingTheCacheSaysHowMuchItFreedAndThatKeptFilesStay() = runTest(main.dispatcher) {
        library.usage = { OfflineUsage(keptFiles = 3u, keptBytes = 300u, otherFiles = 2u, otherBytes = 2048u, budget = null) }
        vm.state // create it
        advanceUntilIdle()
        library.clearCache = {
            library.usage = { OfflineUsage(keptFiles = 3u, keptBytes = 300u, otherFiles = 0u, otherBytes = 0u, budget = null) }
            CacheCleared(removedFiles = 2u, freedBytes = 2048u, keptFiles = 3u, keptBytes = 300u)
        }
        vm.clearCache()
        advanceUntilIdle()
        assertEquals(SettingsEvent.CacheCleared(2048, keptFiles = 3), vm.eventFlow.first())
        assertEquals(1, cacheCleared)
        assertEquals(0uL, vm.state.usage?.otherBytes)
        assertEquals(300uL, vm.state.usage?.keptBytes)
        assertFalse(vm.state.clearing)
    }

    @Test
    fun keepingTheLibraryShowsItsEstimateFirstThenRunsTheDownloads() = runTest(main.dispatcher) {
        val jobs = FakeJobs()
        val vm = model(jobs = jobs)
        advanceUntilIdle()
        assertFalse(vm.state.libraryKept)
        vm.askKeepLibrary()
        assertEquals(KeepAsk(OfflineScope.Library), vm.state.keepLibrary)
        advanceUntilIdle()
        assertEquals(sampleEstimate(), vm.state.keepLibrary?.estimate)
        assertTrue("nothing kept before the confirmation", library.keeps.isEmpty())

        vm.confirmKeepLibrary()
        advanceUntilIdle()
        assertEquals(listOf<OfflineScope>(OfflineScope.Library), library.keeps)
        assertNull(vm.state.keepLibrary)
        assertTrue(vm.state.libraryKept)
        assertEquals(listOf("request wifi"), jobs.calls)
        assertEquals(SettingsEvent.Notice(Message(R.string.offline_kept_library_notice)), vm.eventFlow.first())

        vm.releaseLibrary()
        advanceUntilIdle()
        assertTrue(library.keeps.isEmpty())
        assertFalse(vm.state.libraryKept)
    }

    @Test
    fun aDismissedEstimateKeepsNothing() = runTest(main.dispatcher) {
        vm.state // create it
        advanceUntilIdle()
        vm.askKeepLibrary()
        advanceUntilIdle()
        vm.dismissKeepLibrary()
        vm.confirmKeepLibrary()
        advanceUntilIdle()
        assertTrue(library.keeps.isEmpty())
    }

    @Test
    fun newOptionsAreKeptGivenToTheLibraryAndRestartTheDownloads() = runTest(main.dispatcher) {
        val jobs = FakeJobs()
        val store = MemoryOfflineStore()
        val vm = model(jobs = jobs, store = store)
        advanceUntilIdle()
        val options = OfflineOptions(videos = true, mobileData = true)
        vm.setOfflineOptions(options)
        advanceUntilIdle()
        assertEquals(options, store.current.options)
        assertEquals(options, library.options)
        assertEquals(listOf("request any restart"), jobs.calls)
        // The same again changes nothing.
        vm.setOfflineOptions(options)
        advanceUntilIdle()
        assertEquals(1, jobs.calls.size)
    }

    @Test
    fun pauseHoldsTheDownloadsAcrossRestartsAndResumeStartsThem() = runTest(main.dispatcher) {
        val jobs = FakeJobs()
        val store = MemoryOfflineStore()
        val vm = model(jobs = jobs, store = store)
        advanceUntilIdle()
        vm.pauseDownloads()
        advanceUntilIdle()
        assertTrue(store.current.paused)
        assertTrue("pauseOffline" in library.calls)
        assertEquals(listOf("cancel"), jobs.calls)
        assertTrue("the status follows", vm.state.status?.paused == true)
        // Paused: new options start nothing.
        vm.setOfflineOptions(OfflineOptions(videos = true))
        vm.downloadNow()
        advanceUntilIdle()
        assertEquals(listOf("cancel"), jobs.calls)

        vm.resumeDownloads()
        advanceUntilIdle()
        assertFalse(store.current.paused)
        assertTrue("resumeOffline" in library.calls)
        assertEquals(listOf("cancel", "request wifi restart"), jobs.calls)

        vm.stopDownloads()
        advanceUntilIdle()
        assertTrue("cancelOffline" in library.calls)
        assertEquals("cancel", jobs.calls.last())
    }

    @Test
    fun removingTheKeptFilesSaysWhatItFreed() = runTest(main.dispatcher) {
        library.removeKept = { CacheCleared(removedFiles = 4u, freedBytes = 4096u, keptFiles = 0u, keptBytes = 0u) }
        vm.state // create it
        advanceUntilIdle()
        vm.removeKeptFiles()
        advanceUntilIdle()
        assertTrue("removeKeptFiles" in library.calls)
        assertEquals(SettingsEvent.KeptRemoved(4096), vm.eventFlow.first())
        assertEquals(1, cacheCleared)
        assertFalse(vm.state.removing)
    }

    @Test
    fun theStatusFollowsTheLibraryAndAnEndedRunReadsThePlanAgain() = runTest(main.dispatcher) {
        library.status = sampleStatus(OfflineState.DOWNLOADING, running = true, filesDone = 1u, filesTotal = 4u)
        vm.state // create it
        advanceUntilIdle()
        assertEquals(library.status, vm.state.status)
        library.plan = { samplePlan(totals(files = 4u, presentFiles = 4u)) }
        library.status = sampleStatus(OfflineState.IDLE, running = false, filesDone = 4u, filesTotal = 4u)
        advanceTimeBy(SettingsViewModel.STATUS_EVERY_MS + 1)
        advanceUntilIdle()
        assertEquals(library.status, vm.state.status)
        assertEquals(4uL, vm.state.plan?.totals?.presentFiles)
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
        assertEquals(emptyList<SyncState>(), synced)
    }

    @Test
    fun aSyncThatWentThroughIsHandedOn() = runTest(main.dispatcher) {
        vm.state // create it
        advanceUntilIdle()
        val done = library.syncState.copy(lastSync = 1_700_000_000, cacheChanged = true)
        library.syncNow = { done }
        vm.syncNow()
        advanceUntilIdle()
        assertEquals(listOf(done), synced)
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
