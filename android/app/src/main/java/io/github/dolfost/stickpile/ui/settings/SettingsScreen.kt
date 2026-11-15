package io.github.dolfost.stickpile.ui.settings

import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.ime.ImeSettings
import io.github.dolfost.stickpile.provider.KeyboardAllowlist
import io.github.dolfost.stickpile.rust.AuthKind
import io.github.dolfost.stickpile.rust.version
import io.github.dolfost.stickpile.ui.common.Doing
import io.github.dolfost.stickpile.ui.common.messageFor
import io.github.dolfost.stickpile.ui.common.text
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The settings: account, sync, cache, the picker keyboard, the keyboards
 * that may read the stickers, and the licences.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    vm: SettingsViewModel,
    onBack: () -> Unit,
    onSignIn: () -> Unit,
    onSignedOut: () -> Unit,
    onKeyboardSetup: () -> Unit,
    onLicences: () -> Unit,
) {
    val context = LocalContext.current
    val resources = LocalResources.current
    val state = vm.state
    val snackbar = remember { SnackbarHostState() }

    LaunchedEffect(vm) {
        vm.eventFlow.collect { event ->
            val text = when (event) {
                is SettingsEvent.Notice -> event.message.text(resources)
                is SettingsEvent.CacheCleared ->
                    resources.getString(R.string.settings_cache_cleared, Formatter.formatShortFileSize(context, event.bytes))
            }
            launch { snackbar.showSnackbar(text) }
        }
    }
    LaunchedEffect(state.signedOut) { if (state.signedOut) onSignedOut() }
    // Back from a chat or the keyboard: uses may have been recorded, a sync may have run.
    LifecycleResumeEffect(vm) {
        vm.refreshLocal()
        onPauseOrDispose {}
    }

    Scaffold(
        contentWindowInsets = WindowInsets.safeDrawing,
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.settings_title)) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.app_action_back))
                    }
                },
            )
        },
    ) { padding ->
        Column(
            Modifier
                .padding(padding)
                .fillMaxSize()
                .verticalScroll(rememberScrollState()),
        ) {
            AccountSection(vm, onSignIn)
            HorizontalDivider()
            SyncSection(vm)
            HorizontalDivider()
            CacheSection(vm)
            HorizontalDivider()
            KeyboardSection(onKeyboardSetup)
            HorizontalDivider()
            AllowlistSection()
            HorizontalDivider()
            AboutSection(onLicences)
        }
    }
}

@Composable
internal fun SectionHeader(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 20.dp, bottom = 4.dp),
    )
}

@Composable
private fun Paragraph(text: String, error: Boolean = false) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        color = if (error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
    )
}

@Composable
private fun Actions(content: @Composable () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) { content() }
}

@Composable
private fun BusyLabel(text: String, busy: Boolean) {
    if (busy) {
        CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
        Spacer(Modifier.size(8.dp))
    }
    Text(text)
}

@Composable
private fun AccountSection(vm: SettingsViewModel, onSignIn: () -> Unit) {
    val state = vm.state
    var confirm by rememberSaveable { mutableStateOf(false) }
    SectionHeader(stringResource(R.string.settings_account))
    val account = state.account
    when {
        state.checking -> Paragraph(stringResource(R.string.settings_account_checking))
        account != null -> {
            ListItem(
                headlineContent = { Text(account.displayName ?: account.username) },
                supportingContent = {
                    Text(stringResource(R.string.settings_account_username, account.username))
                },
            )
            ListItem(
                headlineContent = { Text(stringResource(R.string.settings_account_server)) },
                supportingContent = { Text(account.serverUrl) },
            )
            ListItem(
                headlineContent = { Text(stringResource(R.string.settings_account_how)) },
                supportingContent = {
                    Text(
                        stringResource(
                            when (account.auth) {
                                AuthKind.TOKEN -> R.string.settings_account_how_token
                                AuthKind.TAILNET -> R.string.settings_account_how_tailnet
                            },
                        ),
                    )
                },
            )
            Actions {
                OutlinedButton(onClick = { confirm = true }, enabled = !state.signingOut) {
                    BusyLabel(stringResource(R.string.settings_sign_out), state.signingOut)
                }
            }
        }
        else -> {
            state.accountFailure?.let { Paragraph(it.text(), error = true) }
                ?: Paragraph(stringResource(R.string.settings_account_signed_out))
            if (state.serverUrl.isNotEmpty()) {
                ListItem(
                    headlineContent = { Text(stringResource(R.string.settings_account_server)) },
                    supportingContent = { Text(state.serverUrl) },
                )
            }
            Actions {
                Button(onClick = onSignIn) { Text(stringResource(R.string.settings_sign_in)) }
            }
        }
    }
    if (confirm) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text(stringResource(R.string.settings_sign_out_title)) },
            text = { Text(stringResource(R.string.settings_sign_out_text)) },
            confirmButton = {
                TextButton(onClick = {
                    confirm = false
                    vm.signOut()
                }) { Text(stringResource(R.string.settings_sign_out)) }
            },
            dismissButton = {
                TextButton(onClick = { confirm = false }) { Text(stringResource(R.string.app_action_cancel)) }
            },
        )
    }
}

@Composable
private fun SyncSection(vm: SettingsViewModel) {
    val state = vm.state
    SectionHeader(stringResource(R.string.settings_sync))
    val sync = state.sync
    val last = sync?.lastSync
    Paragraph(
        if (last == null) {
            stringResource(R.string.settings_sync_never)
        } else {
            val ago = DateUtils.getRelativeTimeSpanString(
                last * 1000,
                System.currentTimeMillis(),
                DateUtils.MINUTE_IN_MILLIS,
            )
            stringResource(R.string.settings_sync_last, ago)
        },
    )
    sync?.lastError?.let { kind ->
        Paragraph(stringResource(R.string.settings_sync_failed, messageFor(kind, Doing.SYNCING).text()), error = true)
    }
    val pending = state.pendingUsage.toInt()
    Paragraph(
        if (pending == 0) {
            stringResource(R.string.settings_sync_nothing_pending)
        } else {
            pluralStringResource(R.plurals.settings_sync_pending, pending, pending)
        },
    )
    Actions {
        Button(onClick = vm::syncNow, enabled = !state.syncing) {
            BusyLabel(stringResource(R.string.settings_sync_now), state.syncing)
        }
    }
}

@Composable
private fun CacheSection(vm: SettingsViewModel) {
    val context = LocalContext.current
    val state = vm.state
    SectionHeader(stringResource(R.string.settings_cache))
    val bytes = state.cacheBytes
    Paragraph(
        if (bytes == null) {
            stringResource(R.string.settings_cache_unknown)
        } else {
            stringResource(R.string.settings_cache_size, Formatter.formatShortFileSize(context, bytes))
        },
    )
    Paragraph(stringResource(R.string.settings_cache_text))
    Actions {
        OutlinedButton(onClick = vm::clearCache, enabled = !state.clearing) {
            BusyLabel(stringResource(R.string.settings_cache_clear), state.clearing)
        }
    }
}

@Composable
private fun KeyboardSection(onKeyboardSetup: () -> Unit) {
    val context = LocalContext.current
    val settings = remember { ImeSettings(context) }
    // The preferences file is read on first use: off the main thread.
    val loaded by produceState<Triple<Boolean, Boolean, Boolean>?>(null) {
        value = withContext(Dispatchers.IO) { Triple(settings.searchDraft, settings.deleteDraft, settings.stayOpen) }
    }
    SectionHeader(stringResource(R.string.settings_keyboard))
    ListItem(
        headlineContent = { Text(stringResource(R.string.settings_keyboard_setup)) },
        supportingContent = { Text(stringResource(R.string.settings_keyboard_setup_text)) },
        trailingContent = { Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, contentDescription = null) },
        modifier = Modifier.clickable(onClick = onKeyboardSetup),
    )
    val values = loaded ?: return
    var searchDraft by remember { mutableStateOf(values.first) }
    var deleteDraft by remember { mutableStateOf(values.second) }
    var stayOpen by remember { mutableStateOf(values.third) }
    SwitchItem(
        stringResource(R.string.settings_keyboard_search_draft),
        stringResource(R.string.settings_keyboard_search_draft_text),
        searchDraft,
    ) {
        searchDraft = it
        settings.searchDraft = it
    }
    SwitchItem(
        stringResource(R.string.settings_keyboard_delete_draft),
        stringResource(R.string.settings_keyboard_delete_draft_text),
        deleteDraft,
    ) {
        deleteDraft = it
        settings.deleteDraft = it
    }
    SwitchItem(
        stringResource(R.string.settings_keyboard_stay_open),
        stringResource(R.string.settings_keyboard_stay_open_text),
        stayOpen,
    ) {
        stayOpen = it
        settings.stayOpen = it
    }
}

/** A switch with its title and explanation; the whole row toggles it. */
@Composable
private fun SwitchItem(title: String, text: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    ListItem(
        headlineContent = { Text(title) },
        supportingContent = { Text(text) },
        trailingContent = { Switch(checked = checked, onCheckedChange = null) },
        modifier = Modifier.toggleable(value = checked, role = Role.Switch, onValueChange = onChange),
    )
}

@Composable
private fun AllowlistSection() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var generation by remember { mutableIntStateOf(0) }
    val access by produceState<KeyboardAccess?>(null, generation) {
        value = readKeyboardAccess(context)
    }
    // A keyboard may have asked while the app was in the background.
    LifecycleResumeEffect(Unit) {
        generation++
        onPauseOrDispose {}
    }
    val change = { action: KeyboardAllowlist.() -> Unit ->
        scope.launch {
            changeKeyboardAccess(context, action)
            generation++
        }
        Unit
    }

    SectionHeader(stringResource(R.string.settings_allowlist))
    Paragraph(stringResource(R.string.settings_allowlist_text))
    val lists = access ?: return
    if (lists.approved.isEmpty() && lists.pending.isEmpty()) {
        Paragraph(stringResource(R.string.settings_allowlist_empty))
        return
    }
    lists.approved.forEach { entry ->
        ListItem(
            headlineContent = { Text(entry.title) },
            supportingContent = {
                Text(
                    if (entry.label != null) {
                        stringResource(R.string.settings_allowlist_allowed_package, entry.packageName)
                    } else {
                        stringResource(R.string.settings_allowlist_allowed)
                    },
                )
            },
            trailingContent = {
                TextButton(onClick = { change { revoke(entry.packageName) } }) {
                    Text(stringResource(R.string.settings_allowlist_revoke))
                }
            },
        )
    }
    lists.pending.forEach { entry ->
        ListItem(
            headlineContent = { Text(entry.title) },
            supportingContent = {
                Text(
                    if (entry.label != null) {
                        stringResource(R.string.settings_allowlist_asked_package, entry.packageName)
                    } else {
                        stringResource(R.string.settings_allowlist_asked)
                    },
                )
            },
        )
        Actions {
            Button(onClick = { change { approve(entry.packageName) } }) {
                Text(stringResource(R.string.settings_allowlist_allow))
            }
            TextButton(onClick = { change { dismiss(entry.packageName) } }) {
                Text(stringResource(R.string.settings_allowlist_dismiss))
            }
        }
    }
}

@Composable
private fun AboutSection(onLicences: () -> Unit) {
    val appVersion by produceState<String?>(null) { value = withContext(Dispatchers.IO) { version() } }
    SectionHeader(stringResource(R.string.settings_about))
    ListItem(
        headlineContent = { Text(stringResource(R.string.licences_title)) },
        trailingContent = { Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, contentDescription = null) },
        modifier = Modifier.clickable(onClick = onLicences),
    )
    appVersion?.let { Paragraph(stringResource(R.string.settings_version, it)) }
    Spacer(Modifier.size(16.dp))
}
