package io.github.dolfost.stickpile.ui.keyboard

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.provider.Settings
import android.util.Log
import android.view.inputmethod.InputMethodManager
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.ime.PickerInputMethodService
import io.github.dolfost.stickpile.ui.common.FailureLog

/** The picker keyboard's service class, as the system lists input methods. */
val PICKER_SERVICE: String = PickerInputMethodService::class.java.name

/**
 * Whether the picker keyboard is among the enabled input methods, given
 * them as (package, service class) pairs.
 */
fun isPickerEnabled(enabled: List<Pair<String, String>>, packageName: String): Boolean =
    enabled.any { (pkg, service) -> pkg == packageName && service == PICKER_SERVICE }

/** Whether the user has turned the picker keyboard on in the system settings. */
fun isPickerEnabled(context: Context): Boolean {
    val imm = context.getSystemService(InputMethodManager::class.java) ?: return false
    return isPickerEnabled(imm.enabledInputMethodList.map { it.packageName to it.serviceName }, context.packageName)
}

/**
 * The walk-through that turns the picker keyboard on and shows how to
 * switch to it (docs/android-plan.md 4.8): after a sign-in, and from the
 * settings. [onBack] null: no back arrow (after a sign-in, where "skip"
 * is offered instead).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun KeyboardSetupScreen(onDone: () -> Unit, onBack: (() -> Unit)?) {
    val context = LocalContext.current
    var enabled by remember { mutableStateOf(isPickerEnabled(context)) }
    // Back from the system settings: look again.
    LifecycleResumeEffect(Unit) {
        enabled = isPickerEnabled(context)
        onPauseOrDispose {}
    }
    val imeName = stringResource(R.string.ime_name)

    Scaffold(
        contentWindowInsets = WindowInsets.safeDrawing,
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.keyboard_setup_title)) },
                navigationIcon = {
                    if (onBack != null) {
                        IconButton(onClick = onBack) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.app_action_back))
                        }
                    }
                },
            )
        },
    ) { padding ->
        Column(
            Modifier
                .padding(padding)
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 24.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text(stringResource(R.string.keyboard_setup_intro))

            Step(
                title = stringResource(R.string.keyboard_setup_enable_title),
                done = enabled,
            ) {
                Text(stringResource(R.string.keyboard_setup_enable_text, imeName))
                val open = { openInputMethodSettings(context) }
                if (enabled) {
                    OutlinedButton(onClick = open, modifier = Modifier.fillMaxWidth()) {
                        Text(stringResource(R.string.keyboard_setup_open_settings))
                    }
                } else {
                    Button(onClick = open, modifier = Modifier.fillMaxWidth()) {
                        Text(stringResource(R.string.keyboard_setup_open_settings))
                    }
                }
            }

            Step(title = stringResource(R.string.keyboard_setup_switch_title), done = false) {
                Text(stringResource(R.string.keyboard_setup_switch_text, imeName))
                if (!enabled) {
                    Text(
                        stringResource(R.string.keyboard_setup_switch_first),
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                OutlinedButton(
                    onClick = { showInputMethodPicker(context) },
                    enabled = enabled,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(stringResource(R.string.keyboard_setup_show_picker))
                }
            }

            if (enabled || onBack != null) {
                Button(onClick = onDone, modifier = Modifier.fillMaxWidth()) {
                    Text(stringResource(if (onBack != null) R.string.app_action_done else R.string.keyboard_setup_to_library))
                }
            } else {
                TextButton(onClick = onDone, modifier = Modifier.fillMaxWidth()) {
                    Text(stringResource(R.string.keyboard_setup_skip))
                }
            }
        }
    }
}

@Composable
private fun Step(title: String, done: Boolean, content: @Composable () -> Unit) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(title, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                if (done) {
                    Icon(
                        Icons.Filled.CheckCircle,
                        contentDescription = stringResource(R.string.keyboard_setup_enabled),
                        tint = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.size(24.dp),
                    )
                }
            }
            content()
        }
    }
}

private fun openInputMethodSettings(context: Context) {
    try {
        context.startActivity(Intent(Settings.ACTION_INPUT_METHOD_SETTINGS))
    } catch (e: ActivityNotFoundException) {
        Log.w(FailureLog.TAG, "no input method settings: $e")
    }
}

private fun showInputMethodPicker(context: Context) {
    context.getSystemService(InputMethodManager::class.java)?.showInputMethodPicker()
}
