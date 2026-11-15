package io.github.dolfost.stickpile.ui.onboarding

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.autofill.ContentType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentType
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.ui.common.Message
import io.github.dolfost.stickpile.ui.common.isUnencrypted
import io.github.dolfost.stickpile.ui.common.text

/**
 * Onboarding: scan the server's QR code, or type the code, or sign in with
 * the server's address (Tailscale, or a password). [link] is a pairing link
 * the app was opened with; [canCancel] offers a way back to the library
 * (someone is signed in already).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun OnboardingScreen(
    vm: OnboardingViewModel,
    link: String?,
    canCancel: Boolean,
    onSignedIn: () -> Unit,
    onCancel: () -> Unit,
) {
    val state = vm.state
    LaunchedEffect(link) { if (link != null) vm.openLink(link) }
    LaunchedEffect(state.account) { if (state.account != null) onSignedIn() }
    BackHandler(enabled = state.step != OnboardingStep.WELCOME || canCancel) {
        if (!vm.back() && canCancel) onCancel()
    }

    val title = when (state.step) {
        OnboardingStep.WELCOME -> null
        OnboardingStep.CONFIRM_LINK -> R.string.onboarding_link_title
        OnboardingStep.CODE -> R.string.onboarding_code_title
        OnboardingStep.SERVER -> R.string.onboarding_server_title
        OnboardingStep.SIGN_IN -> R.string.onboarding_sign_in_title
    }
    Scaffold(
        contentWindowInsets = WindowInsets.safeDrawing,
        topBar = {
            if (title != null) {
                TopAppBar(
                    title = { Text(stringResource(title)) },
                    navigationIcon = {
                        IconButton(onClick = { vm.back() }, enabled = !state.busy) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.app_action_back))
                        }
                    },
                )
            }
        },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize()) {
            if (state.busy || state.preparing) LinearProgressIndicator(Modifier.fillMaxWidth())
            Column(
                Modifier
                    .fillMaxSize()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 24.dp, vertical = 16.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                when (state.step) {
                    OnboardingStep.WELCOME -> Welcome(vm, canCancel, onCancel)
                    OnboardingStep.CONFIRM_LINK -> ConfirmLink(vm)
                    OnboardingStep.CODE -> PairByCode(vm)
                    OnboardingStep.SERVER -> ServerAddress(vm, signedIn = canCancel)
                    OnboardingStep.SIGN_IN -> SignIn(vm)
                }
                Messages(state.notice, state.error)
            }
        }
    }
}

@Composable
private fun ColumnScope.Welcome(vm: OnboardingViewModel, canCancel: Boolean, onCancel: () -> Unit) {
    val context = LocalContext.current
    val prompt = stringResource(R.string.onboarding_scan_prompt)
    val scan = rememberLauncherForActivityResult(ScanContract()) { result -> vm.scanned(result.contents) }
    val options = {
        ScanOptions()
            .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
            .setPrompt(prompt)
            .setBeepEnabled(false)
            .setOrientationLocked(false)
    }
    // The camera is asked for here, when the user wants to scan, and nowhere else.
    val askCamera = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) scan.launch(options()) else vm.cameraRefused()
    }
    val startScan = {
        val granted = ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
            PackageManager.PERMISSION_GRANTED
        if (granted) scan.launch(options()) else askCamera.launch(Manifest.permission.CAMERA)
    }

    Spacer(Modifier.height(32.dp))
    Text(
        stringResource(R.string.onboarding_welcome_title),
        style = MaterialTheme.typography.headlineMedium,
    )
    Text(stringResource(R.string.onboarding_welcome_text))
    Spacer(Modifier.height(8.dp))
    Text(stringResource(R.string.onboarding_scan_text))
    Button(onClick = startScan, enabled = !vm.state.busy, modifier = Modifier.fillMaxWidth()) {
        Text(stringResource(R.string.onboarding_scan))
    }
    OutlinedButton(onClick = vm::showCode, enabled = !vm.state.busy, modifier = Modifier.fillMaxWidth()) {
        Text(stringResource(R.string.onboarding_enter_code))
    }
    TextButton(onClick = vm::showServer, enabled = !vm.state.busy, modifier = Modifier.fillMaxWidth()) {
        Text(stringResource(R.string.onboarding_use_address))
    }
    if (canCancel) {
        TextButton(onClick = onCancel, enabled = !vm.state.busy, modifier = Modifier.fillMaxWidth()) {
            Text(stringResource(R.string.onboarding_back_to_library))
        }
    }
}

/**
 * Pair with a link's server? Its host big (punycode as it came, so that a
 * look-alike name shows as what it is), the whole address small, and what
 * pairing replaces. The Pair button waits a moment after the link changed.
 */
@Composable
private fun ConfirmLink(vm: OnboardingViewModel) {
    val state = vm.state
    val link = state.link ?: return
    Text(stringResource(R.string.onboarding_link_text))
    Column {
        Text(
            state.linkHost ?: link.serverUrl,
            style = MaterialTheme.typography.headlineSmall,
            fontWeight = FontWeight.Bold,
        )
        Text(
            link.serverUrl,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
    if (isUnencrypted(link.serverUrl)) NotEncrypted()
    state.replaces?.let { replaced ->
        val username = replaced.username
        Text(
            if (username != null) {
                stringResource(R.string.onboarding_replaces, username, replaced.host)
            } else {
                stringResource(R.string.onboarding_replaces_unknown, replaced.host)
            },
            fontWeight = FontWeight.Bold,
        )
    }
    Text(stringResource(R.string.onboarding_link_warning), style = MaterialTheme.typography.bodyMedium)
    BusyButton(stringResource(R.string.onboarding_pair), enabled = state.canConfirm, busy = state.busy, onClick = vm::confirmLink)
    TextButton(onClick = { vm.back() }, enabled = !state.busy, modifier = Modifier.fillMaxWidth()) {
        Text(stringResource(R.string.app_action_cancel))
    }
}

/** One line: the address is plain http, nothing sent to it is encrypted. */
@Composable
private fun NotEncrypted() {
    Text(
        stringResource(R.string.onboarding_not_encrypted),
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.error,
    )
}

@Composable
private fun PairByCode(vm: OnboardingViewModel) {
    val state = vm.state
    Text(stringResource(R.string.onboarding_code_text))
    ServerField(state.serverUrl, vm::setServerUrl, enabled = !state.busy, imeAction = ImeAction.Next)
    if (isUnencrypted(state.serverUrl)) NotEncrypted()
    OutlinedTextField(
        value = state.code,
        onValueChange = vm::setCode,
        label = { Text(stringResource(R.string.onboarding_code_label)) },
        supportingText = {
            val display = state.codeDisplay
            Text(
                if (display != null) {
                    stringResource(R.string.onboarding_code_display, display)
                } else {
                    stringResource(R.string.onboarding_code_hint)
                },
            )
        },
        singleLine = true,
        enabled = !state.busy,
        keyboardOptions = KeyboardOptions(
            capitalization = KeyboardCapitalization.Characters,
            autoCorrectEnabled = false,
            keyboardType = KeyboardType.Ascii,
            imeAction = ImeAction.Go,
        ),
        keyboardActions = KeyboardActions(onGo = { vm.pairWithCode() }),
        modifier = Modifier.fillMaxWidth(),
    )
    BusyButton(stringResource(R.string.onboarding_pair), enabled = state.canPair, busy = state.busy, onClick = vm::pairWithCode)
}

@Composable
private fun ServerAddress(vm: OnboardingViewModel, signedIn: Boolean) {
    val state = vm.state
    Text(stringResource(R.string.onboarding_server_text))
    // Saving another address signs out before the new sign-in (the library's set_server_url).
    if (signedIn) Text(stringResource(R.string.onboarding_server_signs_out), style = MaterialTheme.typography.bodyMedium)
    ServerField(
        state.serverUrl,
        vm::setServerUrl,
        enabled = !state.busy,
        imeAction = ImeAction.Go,
        onGo = vm::continueToSignIn,
    )
    if (isUnencrypted(state.serverUrl)) NotEncrypted()
    BusyButton(
        stringResource(R.string.app_action_continue),
        enabled = state.canContinue,
        busy = state.busy,
        onClick = vm::continueToSignIn,
    )
}

@Composable
private fun SignIn(vm: OnboardingViewModel) {
    val state = vm.state
    Text(stringResource(R.string.onboarding_sign_in_server, state.serverUrl.trim()))
    // The password would go in the clear.
    if (isUnencrypted(state.serverUrl)) NotEncrypted()
    Text(stringResource(R.string.onboarding_tailnet_text))
    FilledTonalButton(
        onClick = vm::signInWithTailscale,
        enabled = !state.busy,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Text(stringResource(R.string.onboarding_tailnet))
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        HorizontalDivider(Modifier.weight(1f))
        Text(
            stringResource(R.string.onboarding_or),
            modifier = Modifier.padding(horizontal = 12.dp),
            style = MaterialTheme.typography.labelLarge,
        )
        HorizontalDivider(Modifier.weight(1f))
    }
    OutlinedTextField(
        value = state.username,
        onValueChange = vm::setUsername,
        label = { Text(stringResource(R.string.onboarding_username)) },
        singleLine = true,
        enabled = !state.busy,
        keyboardOptions = KeyboardOptions(
            capitalization = KeyboardCapitalization.None,
            autoCorrectEnabled = false,
            imeAction = ImeAction.Next,
        ),
        modifier = Modifier
            .fillMaxWidth()
            .semantics { contentType = ContentType.Username },
    )
    OutlinedTextField(
        value = state.password,
        onValueChange = vm::setPassword,
        label = { Text(stringResource(R.string.onboarding_password)) },
        singleLine = true,
        enabled = !state.busy,
        visualTransformation = PasswordVisualTransformation(),
        keyboardOptions = KeyboardOptions(
            autoCorrectEnabled = false,
            keyboardType = KeyboardType.Password,
            imeAction = if (state.needsTotp) ImeAction.Next else ImeAction.Go,
        ),
        keyboardActions = KeyboardActions(onGo = { vm.signInWithPassword() }),
        modifier = Modifier
            .fillMaxWidth()
            .semantics { contentType = ContentType.Password },
    )
    if (state.needsTotp) {
        OutlinedTextField(
            value = state.totp,
            onValueChange = vm::setTotp,
            label = { Text(stringResource(R.string.onboarding_totp)) },
            singleLine = true,
            enabled = !state.busy,
            keyboardOptions = KeyboardOptions(
                autoCorrectEnabled = false,
                keyboardType = KeyboardType.NumberPassword,
                imeAction = ImeAction.Go,
            ),
            keyboardActions = KeyboardActions(onGo = { vm.signInWithPassword() }),
            modifier = Modifier
                .fillMaxWidth()
                .semantics { contentType = ContentType.SmsOtpCode },
        )
    }
    BusyButton(
        stringResource(R.string.onboarding_sign_in),
        enabled = state.canSignIn,
        busy = state.busy,
        onClick = vm::signInWithPassword,
    )
}

@Composable
private fun ServerField(
    value: String,
    onChange: (String) -> Unit,
    enabled: Boolean,
    imeAction: ImeAction,
    onGo: () -> Unit = {},
) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        label = { Text(stringResource(R.string.onboarding_server_label)) },
        placeholder = { Text(stringResource(R.string.onboarding_server_placeholder)) },
        singleLine = true,
        enabled = enabled,
        keyboardOptions = KeyboardOptions(
            capitalization = KeyboardCapitalization.None,
            autoCorrectEnabled = false,
            keyboardType = KeyboardType.Uri,
            imeAction = imeAction,
        ),
        keyboardActions = KeyboardActions(onGo = { onGo() }),
        modifier = Modifier.fillMaxWidth(),
    )
}

@Composable
private fun BusyButton(label: String, enabled: Boolean, busy: Boolean, onClick: () -> Unit) {
    Button(onClick = onClick, enabled = enabled && !busy, modifier = Modifier.fillMaxWidth()) {
        if (busy) {
            CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
            Spacer(Modifier.size(8.dp))
        }
        Text(label)
    }
}

@Composable
private fun Messages(notice: Message?, error: Message?) {
    if (notice != null) {
        Text(notice.text(), color = MaterialTheme.colorScheme.primary, textAlign = TextAlign.Start)
    }
    if (error != null) {
        Text(error.text(), color = MaterialTheme.colorScheme.error, textAlign = TextAlign.Start)
    }
}
