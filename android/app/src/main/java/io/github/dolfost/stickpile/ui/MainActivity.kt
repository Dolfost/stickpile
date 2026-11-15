package io.github.dolfost.stickpile.ui

import android.content.Context
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.provider.PreparedFiles
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import io.github.dolfost.stickpile.sync.SyncScheduler
import io.github.dolfost.stickpile.ui.common.AccountChanges
import io.github.dolfost.stickpile.ui.common.Doing
import io.github.dolfost.stickpile.ui.common.FailureLog
import io.github.dolfost.stickpile.ui.common.FailureScreen
import io.github.dolfost.stickpile.ui.common.Loading
import io.github.dolfost.stickpile.ui.common.catching
import io.github.dolfost.stickpile.ui.common.messageFor
import io.github.dolfost.stickpile.ui.common.openLibrary
import io.github.dolfost.stickpile.ui.keyboard.KeyboardSetupScreen
import io.github.dolfost.stickpile.ui.library.LibraryScreen
import io.github.dolfost.stickpile.ui.library.LibraryViewModel
import io.github.dolfost.stickpile.ui.library.Thumbnails
import io.github.dolfost.stickpile.ui.main.LaunchRequest
import io.github.dolfost.stickpile.ui.main.MainRoute
import io.github.dolfost.stickpile.ui.main.MainViewModel
import io.github.dolfost.stickpile.ui.main.launchRequest
import io.github.dolfost.stickpile.ui.onboarding.OnboardingScreen
import io.github.dolfost.stickpile.ui.onboarding.OnboardingViewModel
import io.github.dolfost.stickpile.ui.theme.StickpileTheme
import kotlinx.coroutines.launch

/**
 * The launcher: onboarding when nobody is signed in, else the library.
 * Opened with a pairing link as its data (`stickpile://pair?...`), it goes
 * to pairing with that link.
 */
class MainActivity : ComponentActivity() {

    private val vm: MainViewModel by viewModels {
        viewModelFactory {
            initializer {
                val context = applicationContext
                MainViewModel(
                    open = { openLibrary(context) },
                    signedIn = { library -> startSync(context, library) },
                    accountChanged = {
                        // Another account or server: what was handed out
                        // for the last one goes.
                        PreparedFiles.revokeAll()
                        Thumbnails.clear()
                    },
                    accountVersion = AccountChanges.version.value,
                )
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        // A restored activity has seen its intent already.
        if (savedInstanceState == null) consume(intent)
        addOnNewIntentListener(::consume)
        vm.start()
        // Back from the settings (or anywhere the account changed): ask
        // again, also when a sign-out ended after its screen went.
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.RESUMED) {
                AccountChanges.version.collect(vm::onAccountChanges)
            }
        }
        setContent {
            StickpileTheme {
                Surface(Modifier.fillMaxSize()) {
                    MainContent(vm, onOpenSettings = { startActivity(Intent(this, SettingsActivity::class.java)) })
                }
            }
        }
    }

    /**
     * Hand what [intent] asks for to the view model, once: the intent then
     * loses its data (a used pairing code is not offered again), and an
     * intent relaunched from Recents asks for nothing ([launchRequest]).
     */
    private fun consume(intent: Intent) {
        when (val request = launchRequest(intent.dataString, intent.flags)) {
            is LaunchRequest.OpenLink -> vm.openLink(request.link)
            null -> Unit
        }
        if (intent.data != null) setIntent(Intent(intent).apply { data = null })
    }

    companion object {
        /**
         * Signed in: make sure the background sync is scheduled, and sync
         * now if one is due. A failure goes to the log and the sync state.
         */
        fun startSync(context: Context, library: StickpileLibraryInterface) {
            SyncScheduler.ensureScheduled(context)
            StickpileApp.from(context).scope.launch {
                catching { library.syncIfDue() }.onFailure { FailureLog.android.failed(Doing.SYNCING, it) }
            }
        }
    }
}

@Composable
private fun MainContent(vm: MainViewModel, onOpenSettings: () -> Unit) {
    val library = vm.library
    when (val route = vm.route) {
        MainRoute.Starting -> Loading()
        is MainRoute.Broken -> FailureScreen(
            title = stringResource(R.string.app_broken_title),
            message = messageFor(route.failure, Doing.OPENING),
            onRetry = vm::check,
        )
        is MainRoute.AccountUnknown -> FailureScreen(
            title = stringResource(R.string.main_account_unknown_title),
            message = messageFor(route.failure, Doing.CHECKING_ACCOUNT),
            onRetry = vm::check,
            otherLabel = stringResource(R.string.main_sign_in_anew),
            onOther = vm::signInAnew,
        )
        is MainRoute.Onboarding -> {
            if (library == null) return Loading()
            val onboarding = viewModel(key = "onboarding-${vm.session}") { OnboardingViewModel(library) }
            OnboardingScreen(
                vm = onboarding,
                link = route.link,
                canCancel = route.canCancel,
                onSignedIn = vm::onSignedIn,
                onCancel = vm::onOnboardingCancelled,
            )
        }
        MainRoute.KeyboardSetup -> KeyboardSetupScreen(onDone = vm::onKeyboardSetupDone, onBack = null, signedInAs = vm.signedInAs)
        MainRoute.Library -> {
            if (library == null) return Loading()
            val screen = viewModel(key = "library-${vm.session}") { LibraryViewModel(library) }
            LibraryScreen(
                vm = screen,
                library = library,
                onOpenSettings = onOpenSettings,
                onSignInAgain = vm::signInAnew,
            )
        }
    }
}
