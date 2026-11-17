package io.github.dolfost.stickpile.ui.settings

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.offline.KeepAsk
import io.github.dolfost.stickpile.offline.MemoryOfflineStore
import io.github.dolfost.stickpile.offline.OfflineChoices
import io.github.dolfost.stickpile.offline.OfflineJobs
import io.github.dolfost.stickpile.offline.OfflineStore
import io.github.dolfost.stickpile.offline.OfflineWork
import io.github.dolfost.stickpile.rust.Account
import io.github.dolfost.stickpile.rust.AuthKind
import io.github.dolfost.stickpile.rust.OfflineEstimate
import io.github.dolfost.stickpile.rust.OfflineOptions
import io.github.dolfost.stickpile.rust.OfflinePlan
import io.github.dolfost.stickpile.rust.OfflineScope
import io.github.dolfost.stickpile.rust.OfflineStatus
import io.github.dolfost.stickpile.rust.OfflineUsage
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import io.github.dolfost.stickpile.rust.SyncState
import io.github.dolfost.stickpile.ui.common.Doing
import io.github.dolfost.stickpile.ui.common.Failure
import io.github.dolfost.stickpile.ui.common.FailureLog
import io.github.dolfost.stickpile.ui.common.Message
import io.github.dolfost.stickpile.ui.common.catching
import io.github.dolfost.stickpile.ui.common.messageFor
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** What the settings screen shows of the library. */
data class SettingsState(
    /** Asking who is signed in. */
    val checking: Boolean = true,
    val account: Account? = null,
    /** Why the account is not known (offline with nothing cached, ...). */
    val accountFailure: Message? = null,
    val serverUrl: String = "",
    val sync: SyncState? = null,
    val pendingUsage: Long = 0,
    /** The files on the phone, kept offline and other; null until read, or when it cannot be read. */
    val usage: OfflineUsage? = null,
    val syncing: Boolean = false,
    val clearing: Boolean = false,
    val signingOut: Boolean = false,
    /** Signed out: the screen goes back to onboarding. */
    val signedOut: Boolean = false,
    /** What is kept offline and how much of it is here; null until read. */
    val plan: OfflinePlan? = null,
    /** The whole library is kept offline. */
    val libraryKept: Boolean = false,
    /** The offline downloads, as the library last said; null until read. */
    val status: OfflineStatus? = null,
    /** The downloads' WorkManager job. */
    val work: OfflineWork = OfflineWork.NONE,
    /** Asking before the library is kept: what it would take (the estimate null while it is counted). */
    val keepLibrary: KeepAsk? = null,
    val removing: Boolean = false,
) {
    /**
     * Signed in with a token, which signing out revokes. An account signed
     * in by its tailnet identity has none: the server knows the phone by
     * its tailnet address, and signing out would change nothing.
     */
    val canSignOut: Boolean get() = account?.auth == AuthKind.TOKEN
}


/** What the settings screen does once. */
sealed interface SettingsEvent {
    data class Notice(val message: Message) : SettingsEvent

    /** The cache was cleared, [bytes] freed; [keptFiles] kept offline stay. */
    data class CacheCleared(val bytes: Long, val keptFiles: Long = 0) : SettingsEvent

    /** The kept files were removed, [bytes] freed. */
    data class KeptRemoved(val bytes: Long) : SettingsEvent
}

/**
 * The library side of the settings: the account, the sync, the cache,
 * keeping offline, and signing out. The non-suspend library calls run on
 * [io]. [onCacheCleared], [onSignedOut] and [onSynced] are what the rest
 * of the app must hear of (drop the decoded thumbnails and the prepared
 * files' tokens; stop the background work; offer from the synced cache).
 * Signing out runs in [background], the app's scope, so that it ends with
 * its follow-ups even when the screen goes. The offline options are kept
 * in [offline] and given to the library, and the downloads run as [jobs]
 * (WorkManager) allows; the library decides what they download.
 */
class SettingsViewModel(
    private val library: StickpileLibraryInterface,
    private val background: CoroutineScope,
    private val io: CoroutineDispatcher = Dispatchers.IO,
    private val log: FailureLog = FailureLog.android,
    private val onCacheCleared: () -> Unit = {},
    private val onSignedOut: () -> Unit = {},
    private val onSynced: (SyncState) -> Unit = {},
    private val offline: OfflineStore = MemoryOfflineStore(),
    private val jobs: OfflineJobs = OfflineJobs.NONE,
) : ViewModel() {

    var state: SettingsState by mutableStateOf(SettingsState())
        private set

    private val events = Channel<SettingsEvent>(Channel.BUFFERED)
    val eventFlow: Flow<SettingsEvent> = events.receiveAsFlow()

    init {
        refreshLocal()
        checkAccount()
        followDownloads()
        viewModelScope.launch { jobs.work.collect { state = state.copy(work = it) } }
    }

    /** The offline options in use and the pause (the app's). */
    val offlineChoices: StateFlow<OfflineChoices> get() = offline.state

    /** Read again what is local (sync state, waiting uses, the files, what is kept). */
    fun refreshLocal() {
        viewModelScope.launch {
            val local = withContext(io) {
                Local(
                    serverUrl = library.serverUrl(),
                    sync = library.syncState(),
                    pendingUsage = library.pendingUsage().toLong(),
                    usage = catching { library.offlineUsage() }.onFailure { log.failed(Doing.LOADING, it) }.getOrNull(),
                )
            }
            state = state.copy(
                serverUrl = local.serverUrl,
                sync = local.sync,
                pendingUsage = local.pendingUsage,
                usage = local.usage,
            )
            refreshOffline()
        }
    }

    private data class Local(val serverUrl: String, val sync: SyncState, val pendingUsage: Long, val usage: OfflineUsage?)

    /** What is kept and how much of it is here. */
    private suspend fun refreshOffline() {
        val keeps = withContext(io) { catching { library.offlineKeeps() }.getOrNull() }
        val plan = catching { library.offlinePlan() }.onFailure { log.failed(Doing.LOADING, it) }.getOrNull()
        val usage = withContext(io) { catching { library.offlineUsage() }.getOrNull() }
        state = state.copy(
            libraryKept = keeps?.contains(OfflineScope.Library) ?: state.libraryKept,
            plan = plan ?: state.plan,
            usage = usage ?: state.usage,
        )
    }

    /**
     * Follow the downloads' status while the screen lives: each change the
     * library reports, at most every [STATUS_EVERY_MS]; what is here is read
     * again when a run ends.
     */
    private fun followDownloads() {
        viewModelScope.launch {
            var seen: OfflineStatus? = null
            while (true) {
                val now = catching { library.offlineStatusChange(seen) }.getOrElse {
                    log.failed(Doing.LOADING, it)
                    delay(STATUS_RETRY_MS)
                    continue
                }
                val ended = seen?.running == true && !now.running
                seen = now
                state = state.copy(status = now)
                if (ended) refreshOffline()
                delay(STATUS_EVERY_MS)
            }
        }
    }

    private fun checkAccount() {
        viewModelScope.launch {
            val result = catching { library.account() }
            result.exceptionOrNull()?.let { log.failed(Doing.CHECKING_ACCOUNT, it) }
            state = state.copy(
                checking = false,
                account = result.getOrNull(),
                accountFailure = result.exceptionOrNull()?.let { messageFor(Failure.of(it), Doing.CHECKING_ACCOUNT) },
            )
        }
    }

    fun syncNow() {
        if (state.syncing) return
        state = state.copy(syncing = true)
        viewModelScope.launch {
            catching { library.syncNow() }.onSuccess(onSynced).onFailure { notice(Doing.SYNCING, it) }
            state = state.copy(syncing = false)
            refreshLocal()
        }
    }

    /** "Clear cache": the files not kept offline go; it says how many kept ones stay. */
    fun clearCache() {
        if (state.clearing) return
        state = state.copy(clearing = true)
        viewModelScope.launch {
            withContext(io) { catching { library.clearCache() } }.fold(
                onSuccess = { cleared ->
                    onCacheCleared()
                    events.send(SettingsEvent.CacheCleared(cleared.freedBytes.toLong(), cleared.keptFiles.toLong()))
                },
                onFailure = { notice(Doing.CLEARING_CACHE, it) },
            )
            state = state.copy(clearing = false)
            refreshLocal()
        }
    }

    // -- keeping offline ------------------------------------------------------------

    /** New options (the screen's switches and sliders): kept, given to the library, and a run with them. */
    fun setOfflineOptions(options: OfflineOptions) {
        if (options == offline.current.options) return
        offline.setOptions(options)
        viewModelScope.launch {
            withContext(io) { library.setOfflineOptions(options) }
            if (!offline.current.paused) jobs.request(options.mobileData, restart = true)
            refreshOffline()
        }
    }

    /** Keep the whole library? Its estimate first ([SettingsState.keepLibrary]). */
    fun askKeepLibrary() {
        if (state.keepLibrary != null) return
        state = state.copy(keepLibrary = KeepAsk(OfflineScope.Library))
        viewModelScope.launch {
            catching { library.offlineEstimate(OfflineScope.Library) }.fold(
                onSuccess = { e -> if (state.keepLibrary != null) state = state.copy(keepLibrary = KeepAsk(OfflineScope.Library, estimate = e)) },
                onFailure = {
                    state = state.copy(keepLibrary = null)
                    notice(Doing.KEEPING_OFFLINE, it)
                },
            )
        }
    }

    fun dismissKeepLibrary() {
        state = state.copy(keepLibrary = null)
    }

    /** The library is kept (asked first); the downloads start when the platform allows. */
    fun confirmKeepLibrary() {
        if (state.keepLibrary == null) return
        state = state.copy(keepLibrary = null)
        viewModelScope.launch {
            catching { library.keepOffline(OfflineScope.Library) }.fold(
                onSuccess = {
                    requestRun()
                    events.send(SettingsEvent.Notice(Message(R.string.offline_kept_library_notice)))
                },
                onFailure = { notice(Doing.KEEPING_OFFLINE, it) },
            )
            refreshOffline()
        }
    }

    /** The library is no longer kept (asked first); its files stay until the space is needed. */
    fun releaseLibrary() {
        viewModelScope.launch {
            catching { library.releaseOffline(OfflineScope.Library) }.fold(
                onSuccess = {
                    events.send(SettingsEvent.Notice(Message(R.string.offline_released_library_notice)))
                },
                onFailure = { notice(Doing.KEEPING_OFFLINE, it) },
            )
            refreshOffline()
        }
    }

    /** Hold the downloads, also after a restart: the run going on stops, the job goes. */
    fun pauseDownloads() {
        offline.setPaused(true)
        viewModelScope.launch {
            withContext(io) { library.pauseOffline() }
            jobs.cancel()
        }
    }

    /** Let the downloads go on, when the platform allows. */
    fun resumeDownloads() {
        offline.setPaused(false)
        viewModelScope.launch {
            withContext(io) { library.resumeOffline() }
            jobs.request(offline.current.options.mobileData, restart = true)
        }
    }

    /** End the run going on; the next sync or keep starts another. */
    fun stopDownloads() {
        viewModelScope.launch {
            withContext(io) { library.cancelOffline() }
            jobs.cancel()
        }
    }

    /** Download what is missing now (when the platform allows). */
    fun downloadNow() {
        if (offline.current.paused) return
        jobs.request(offline.current.options.mobileData, restart = true)
    }

    /** The confirmed "Remove kept files": every kept file goes, nothing stays kept. */
    fun removeKeptFiles() {
        if (state.removing) return
        state = state.copy(removing = true)
        viewModelScope.launch {
            withContext(io) { catching { library.removeKeptFiles() } }.fold(
                onSuccess = { removed ->
                    onCacheCleared()
                    events.send(SettingsEvent.KeptRemoved(removed.freedBytes.toLong()))
                },
                onFailure = { notice(Doing.CLEARING_CACHE, it) },
            )
            state = state.copy(removing = false)
            refreshLocal()
        }
    }

    private fun requestRun() {
        val choices = offline.current
        if (!choices.paused) jobs.request(choices.options.mobileData)
    }

    /**
     * Sign out (the screen asked first); then [onSignedOut] (the background
     * work stops, ...). Both run in [background], on the main thread: leaving
     * the screen half-way does not stop them, and the screen hears of the
     * end only if it is still there. Only an account with a token signs out
     * ([SettingsState.canSignOut]).
     */
    fun signOut() {
        if (state.signingOut || !state.canSignOut) return
        state = state.copy(signingOut = true)
        background.launch(Dispatchers.Main.immediate) {
            catching { library.logout() }.fold(
                onSuccess = {
                    onSignedOut()
                    state = state.copy(signingOut = false, signedOut = true, account = null)
                },
                onFailure = {
                    notice(Doing.SIGNING_OUT, it)
                    state = state.copy(signingOut = false)
                },
            )
        }
    }

    private suspend fun notice(doing: Doing, e: Throwable) {
        log.failed(doing, e)
        events.send(SettingsEvent.Notice(messageFor(Failure.of(e), doing)))
    }

    companion object {
        /** The fastest the progress is redrawn. */
        const val STATUS_EVERY_MS = 300L

        /** After a failed read of the progress. */
        const val STATUS_RETRY_MS = 5_000L
    }
}
