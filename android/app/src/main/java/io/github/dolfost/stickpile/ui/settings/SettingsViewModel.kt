package io.github.dolfost.stickpile.ui.settings

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.dolfost.stickpile.rust.Account
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
import kotlinx.coroutines.flow.Flow
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
    /** Null until read, or when it cannot be read. */
    val cacheBytes: Long? = null,
    val syncing: Boolean = false,
    val clearing: Boolean = false,
    val signingOut: Boolean = false,
    /** Signed out: the screen goes back to onboarding. */
    val signedOut: Boolean = false,
)

/** What the settings screen does once. */
sealed interface SettingsEvent {
    data class Notice(val message: Message) : SettingsEvent

    /** The cache was cleared, [bytes] freed. */
    data class CacheCleared(val bytes: Long) : SettingsEvent
}

/**
 * The library side of the settings: the account, the sync, the cache, and
 * signing out. The non-suspend library calls run on [io].
 * [onCacheCleared] and [onSignedOut] are what the rest of the app must
 * hear of (drop the decoded thumbnails and the prepared files' tokens;
 * stop the background sync). Signing out runs in [background], the app's
 * scope, so that it ends with its follow-ups even when the screen goes.
 */
class SettingsViewModel(
    private val library: StickpileLibraryInterface,
    private val background: CoroutineScope,
    private val io: CoroutineDispatcher = Dispatchers.IO,
    private val log: FailureLog = FailureLog.android,
    private val onCacheCleared: () -> Unit = {},
    private val onSignedOut: () -> Unit = {},
) : ViewModel() {

    var state: SettingsState by mutableStateOf(SettingsState())
        private set

    private val events = Channel<SettingsEvent>(Channel.BUFFERED)
    val eventFlow: Flow<SettingsEvent> = events.receiveAsFlow()

    init {
        refreshLocal()
        checkAccount()
    }

    /** Read again what is local (sync state, waiting uses, cache size). */
    fun refreshLocal() {
        viewModelScope.launch {
            val local = withContext(io) {
                Local(
                    serverUrl = library.serverUrl(),
                    sync = library.syncState(),
                    pendingUsage = library.pendingUsage().toLong(),
                    cacheBytes = catching { library.cacheBytes().toLong() }.onFailure { log.failed(Doing.LOADING, it) }.getOrNull(),
                )
            }
            state = state.copy(
                serverUrl = local.serverUrl,
                sync = local.sync,
                pendingUsage = local.pendingUsage,
                cacheBytes = local.cacheBytes,
            )
        }
    }

    private data class Local(val serverUrl: String, val sync: SyncState, val pendingUsage: Long, val cacheBytes: Long?)

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
            catching { library.syncNow() }.onFailure { notice(Doing.SYNCING, it) }
            state = state.copy(syncing = false)
            refreshLocal()
        }
    }

    fun clearCache() {
        if (state.clearing) return
        state = state.copy(clearing = true)
        viewModelScope.launch {
            withContext(io) { catching { library.clearCache().toLong() } }.fold(
                onSuccess = { freed ->
                    onCacheCleared()
                    events.send(SettingsEvent.CacheCleared(freed))
                },
                onFailure = { notice(Doing.CLEARING_CACHE, it) },
            )
            state = state.copy(clearing = false)
            refreshLocal()
        }
    }

    /**
     * Sign out (the screen asked first); then [onSignedOut] (the background
     * sync stops, ...). Both run in [background], on the main thread: leaving
     * the screen half-way does not stop them, and the screen hears of the
     * end only if it is still there.
     */
    fun signOut() {
        if (state.signingOut) return
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
}
