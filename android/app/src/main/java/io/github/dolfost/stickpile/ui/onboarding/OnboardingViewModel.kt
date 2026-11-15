package io.github.dolfost.stickpile.ui.onboarding

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.Account
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.PairingLink
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import io.github.dolfost.stickpile.ui.common.Doing
import io.github.dolfost.stickpile.ui.common.Failure
import io.github.dolfost.stickpile.ui.common.FailureLog
import io.github.dolfost.stickpile.ui.common.Message
import io.github.dolfost.stickpile.ui.common.catching
import io.github.dolfost.stickpile.ui.common.messageFor
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** The onboarding screens, most prominent first (docs/android-plan.md 4.6). */
enum class OnboardingStep {
    /** Scan a QR code, or pick one of the other ways. */
    WELCOME,

    /** A pairing link the app was opened with: pair with its server? */
    CONFIRM_LINK,

    /** Server address and pairing code, typed. */
    CODE,

    /** Server address, before signing in with Tailscale or a password. */
    SERVER,

    /** Tailscale, or username, password and (when asked) a two-factor code. */
    SIGN_IN,
}

/**
 * Everything the onboarding screens show. The fields hold what the user
 * typed; [codeDisplay] is the library's display form of [code] (null
 * while it is not a whole code). [toString] leaves the secrets out.
 */
data class OnboardingState(
    val step: OnboardingStep = OnboardingStep.WELCOME,
    val link: PairingLink? = null,
    val serverUrl: String = "",
    val code: String = "",
    val codeDisplay: String? = null,
    val username: String = "",
    val password: String = "",
    val totp: String = "",
    /** The server asked for a two-factor code. */
    val needsTotp: Boolean = false,
    /** A call is running: the inputs wait. */
    val busy: Boolean = false,
    val error: Message? = null,
    /** Not an error: what to do next (enter the two-factor code). */
    val notice: Message? = null,
    /** Signed in: onboarding is done. */
    val account: Account? = null,
) {
    val canPair: Boolean get() = !busy && serverUrl.isNotBlank() && codeDisplay != null
    val canContinue: Boolean get() = !busy && serverUrl.isNotBlank()
    val canSignIn: Boolean
        get() = !busy && username.isNotBlank() && password.isNotEmpty() && (!needsTotp || totp.isNotBlank())

    override fun toString(): String = "OnboardingState(step=$step, busy=$busy, needsTotp=$needsTotp, error=$error)"
}

/**
 * Onboarding (docs/android-plan.md 4.6): pair by a scanned QR code or a
 * typed code, or give the server's address and sign in with Tailscale or
 * a password. The library decides everything (what a code or link is,
 * which server, what failed); this keeps what the user typed and which
 * screen is up. The non-suspend library calls run on [io].
 */
class OnboardingViewModel(
    private val library: StickpileLibraryInterface,
    private val io: CoroutineDispatcher = Dispatchers.IO,
    private val log: FailureLog = FailureLog.android,
) : ViewModel() {

    var state: OnboardingState by mutableStateOf(OnboardingState())
        private set

    private var normalizing: Job? = null
    private var openedLink: String? = null

    /** Open the way to pair by a typed code. */
    fun showCode() = go(OnboardingStep.CODE)

    /** Open the way to sign in on a server by its address. */
    fun showServer() = go(OnboardingStep.SERVER)

    /** Back one screen; false when there is nowhere to go back to (welcome). */
    fun back(): Boolean {
        if (state.busy) return true
        val to = when (state.step) {
            OnboardingStep.WELCOME -> return false
            OnboardingStep.SIGN_IN -> OnboardingStep.SERVER
            else -> OnboardingStep.WELCOME
        }
        state = state.copy(
            step = to,
            link = if (to == OnboardingStep.WELCOME) null else state.link,
            error = null,
            notice = null,
        )
        return true
    }

    fun setServerUrl(text: String) {
        state = state.copy(serverUrl = text, error = null)
    }

    fun setUsername(text: String) {
        state = state.copy(username = text, error = null)
    }

    fun setPassword(text: String) {
        state = state.copy(password = text, error = null)
    }

    fun setTotp(text: String) {
        state = state.copy(totp = text, error = null)
    }

    /** The typed code, and its display form as the library reads it. */
    fun setCode(text: String) {
        state = state.copy(code = text, codeDisplay = null, error = null)
        normalizing?.cancel()
        normalizing = viewModelScope.launch {
            val display = withContext(io) { library.normalizePairingCode(text) }
            if (state.code == text) state = state.copy(codeDisplay = display)
        }
    }

    /**
     * The app was opened with [text] as its data (`stickpile://pair?...`):
     * ask before pairing with the server it names, since any web page can
     * open such a link. The same link twice (a rotation) is shown once.
     */
    fun openLink(text: String) {
        if (text == openedLink) return
        openedLink = text
        viewModelScope.launch {
            val link = withContext(io) { library.parsePairingLink(text) }
            state = if (link == null) {
                state.copy(step = OnboardingStep.WELCOME, link = null, error = Message(R.string.app_error_not_a_pairing_link))
            } else {
                state.copy(step = OnboardingStep.CONFIRM_LINK, link = link, error = null, notice = null)
            }
        }
    }

    /** Pair with the link [openLink] showed. */
    fun confirmLink() {
        val link = state.link ?: return
        pair(link.serverUrl, link.code)
    }

    /** What the QR scanner read; null when the scan was cancelled. */
    fun scanned(contents: String?) {
        if (contents == null || state.busy) return
        viewModelScope.launch {
            val link = withContext(io) { library.parsePairingLink(contents) }
            if (link == null) {
                state = state.copy(error = Message(R.string.app_error_not_a_pairing_qr))
            } else {
                pair(link.serverUrl, link.code)
            }
        }
    }

    /** The camera permission was refused: typing the code is the way left. */
    fun cameraRefused() {
        state = state.copy(error = Message(R.string.onboarding_camera_refused))
    }

    /** Pair with the typed address and code. */
    fun pairWithCode() {
        if (!state.canPair) return
        pair(state.serverUrl, state.code)
    }

    /** Save the typed address, then offer the ways to sign in there. */
    fun continueToSignIn() {
        if (!state.canContinue) return
        val url = state.serverUrl
        run(Doing.SETTING_SERVER, { library.setServerUrl(url) }) {
            state = state.copy(step = OnboardingStep.SIGN_IN, needsTotp = false, totp = "")
        }
    }

    /** Sign in by this device's tailnet identity. */
    fun signInWithTailscale() {
        if (state.busy) return
        run(Doing.TAILNET_LOGIN, { library.loginTailnet() }) { account ->
            if (account == null) {
                state = state.copy(error = Message(R.string.onboarding_tailnet_unknown))
            } else {
                state = state.copy(account = account)
            }
        }
    }

    /** Sign in with the username and password (and the two-factor code once asked for). */
    fun signInWithPassword() {
        if (!state.canSignIn) return
        val s = state
        val totp = if (s.needsTotp) s.totp else null
        val doing = if (s.needsTotp) Doing.PASSWORD_AND_CODE_LOGIN else Doing.PASSWORD_LOGIN
        run(doing, { library.loginPassword(s.username, s.password, totp) }) { account ->
            state = state.copy(account = account, password = "", totp = "")
        }
    }

    private fun pair(serverUrl: String, code: String) {
        run(Doing.PAIRING, { library.loginPairing(serverUrl, code) }) { account ->
            state = state.copy(account = account, code = "", codeDisplay = null)
        }
    }

    /**
     * Run [call] with the inputs waiting; on success [then], on failure
     * the message for it. A login that needs a two-factor code is not an
     * error: the code field appears.
     */
    private fun <T> run(doing: Doing, call: suspend () -> T, then: (T) -> Unit) {
        state = state.copy(busy = true, error = null)
        viewModelScope.launch {
            val result = catching { call() }
            state = state.copy(busy = false)
            result.fold(
                onSuccess = {
                    state = state.copy(error = null, notice = null)
                    then(it)
                },
                onFailure = { e ->
                    log.failed(doing, e)
                    val failure = Failure.of(e)
                    if (failure.kind == LibraryErrorKind.INVALID_INPUT && failure.field == "totp") {
                        state = state.copy(needsTotp = true, notice = Message(R.string.onboarding_totp_notice))
                    } else {
                        state = state.copy(error = messageFor(failure, doing))
                    }
                },
            )
        }
    }

    private fun go(step: OnboardingStep) {
        state = state.copy(step = step, error = null, notice = null)
    }
}
