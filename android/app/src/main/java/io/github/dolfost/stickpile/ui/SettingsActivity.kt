package io.github.dolfost.stickpile.ui

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.lifecycle.viewmodel.compose.viewModel
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.motion.Motions
import io.github.dolfost.stickpile.provider.PreparedFiles
import io.github.dolfost.stickpile.rust.StickpileLibrary
import io.github.dolfost.stickpile.sync.SyncScheduler
import io.github.dolfost.stickpile.ui.common.AccountChanges
import io.github.dolfost.stickpile.ui.common.Doing
import io.github.dolfost.stickpile.ui.common.Failure
import io.github.dolfost.stickpile.ui.common.FailureLog
import io.github.dolfost.stickpile.ui.common.FailureScreen
import io.github.dolfost.stickpile.ui.common.Loading
import io.github.dolfost.stickpile.ui.common.messageFor
import io.github.dolfost.stickpile.ui.common.openLibrary
import io.github.dolfost.stickpile.ui.keyboard.KeyboardSetupScreen
import io.github.dolfost.stickpile.ui.library.Thumbnails
import io.github.dolfost.stickpile.ui.settings.LicencesScreen
import io.github.dolfost.stickpile.ui.settings.SettingsScreen
import io.github.dolfost.stickpile.ui.settings.SettingsViewModel
import io.github.dolfost.stickpile.ui.theme.StickpileTheme
import io.github.dolfost.stickpile.ui.theme.ThemedActivity

/**
 * The app's settings, also opened by the system as the input method's
 * settings (res/xml/method.xml): the settings list, the keyboard
 * walk-through and the licences.
 */
class SettingsActivity : ThemedActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContent {
            StickpileTheme {
                Surface(Modifier.fillMaxSize()) {
                    SettingsContent(
                        onFinish = ::finish,
                        onSignIn = { restartMain() },
                        onSignedOut = { restartMain() },
                        onSignInElsewhere = { restartMain(signInElsewhere = true) },
                    )
                }
            }
        }
    }
}

/**
 * The launcher afresh, in a new task: it asks again who is signed in, and
 * no screen of the account before stays behind; [signInElsewhere] takes
 * it to onboarding. This screen goes. Nothing when this screen is going
 * already: the launcher notices a sign-out by itself (AccountChanges).
 */
private fun ComponentActivity.restartMain(signInElsewhere: Boolean = false) {
    if (isFinishing || isDestroyed) return
    val intent = Intent(this, MainActivity::class.java)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
    if (signInElsewhere) intent.putExtra(MainActivity.EXTRA_SIGN_IN_ELSEWHERE, true)
    startActivity(intent)
    finish()
}

/** The pages of the settings activity. */
private enum class SettingsPage { MAIN, KEYBOARD_SETUP, LICENCES }

@Composable
private fun SettingsContent(
    onFinish: () -> Unit,
    onSignIn: () -> Unit,
    onSignedOut: () -> Unit,
    onSignInElsewhere: () -> Unit,
) {
    val context = LocalContext.current
    var attempt by rememberSaveable { mutableIntStateOf(0) }
    val opened by produceState<Result<StickpileLibrary>?>(null, attempt) {
        value = openLibrary(context).onFailure { FailureLog.android.failed(Doing.OPENING, it) }
    }
    val result = opened ?: return Loading()
    val library = result.getOrElse { e ->
        return FailureScreen(
            title = stringResource(R.string.app_broken_title),
            message = messageFor(Failure.of(e), Doing.OPENING),
            onRetry = { attempt++ },
        )
    }

    var page by rememberSaveable { mutableStateOf(SettingsPage.MAIN) }
    BackHandler(enabled = page != SettingsPage.MAIN) { page = SettingsPage.MAIN }
    val appContext = context.applicationContext
    val vm = viewModel {
        SettingsViewModel(
            library,
            background = StickpileApp.from(appContext).scope,
            onCacheCleared = {
                Thumbnails.clear()
                Motions.clear()
                // The files the tokens pointed to are gone.
                PreparedFiles.revokeAll()
            },
            // In the app's scope, also when this screen went half-way.
            onSignedOut = {
                SyncScheduler.cancel(appContext)
                Thumbnails.clear()
                Motions.clear()
                PreparedFiles.revokeAll()
                AccountChanges.changed()
            },
        )
    }
    when (page) {
        SettingsPage.MAIN -> SettingsScreen(
            vm = vm,
            onBack = onFinish,
            onSignIn = onSignIn,
            onSignedOut = onSignedOut,
            onSignInElsewhere = onSignInElsewhere,
            onKeyboardSetup = { page = SettingsPage.KEYBOARD_SETUP },
            onLicences = { page = SettingsPage.LICENCES },
        )
        SettingsPage.KEYBOARD_SETUP -> KeyboardSetupScreen(
            onDone = { page = SettingsPage.MAIN },
            onBack = { page = SettingsPage.MAIN },
        )
        SettingsPage.LICENCES -> LicencesScreen(onBack = { page = SettingsPage.MAIN })
    }
}
