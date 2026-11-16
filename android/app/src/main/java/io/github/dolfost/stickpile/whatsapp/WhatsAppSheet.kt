package io.github.dolfost.stickpile.whatsapp

import android.content.ActivityNotFoundException
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.WhatsappStep
import io.github.dolfost.stickpile.ui.common.text

/**
 * The "Add to WhatsApp" sheet of [vm], and the launcher of WhatsApp's
 * dialog. Kept in the composition all the time (the sheet shows only
 * while [vm] is not closed), so that the dialog's result comes back to
 * it, also after the activity was recreated.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun WhatsAppHost(vm: WhatsAppViewModel) {
    val resources = LocalResources.current
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        vm.added(result.resultCode, result.data?.getStringExtra(WhatsApp.EXTRA_VALIDATION_ERROR))
    }
    LaunchedEffect(vm) {
        vm.eventFlow.collect { event ->
            when (event) {
                is WhatsAppEvent.Launch -> try {
                    launcher.launch(event.request.intent(resources.getString(R.string.whatsapp_add)))
                } catch (_: ActivityNotFoundException) {
                    vm.notStarted()
                }
            }
        }
    }
    val state = vm.state
    if (state == WhatsAppState.Closed) return
    ModalBottomSheet(onDismissRequest = vm::close) {
        Column(
            Modifier
                .verticalScroll(rememberScrollState())
                .padding(start = 16.dp, end = 16.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(stringResource(R.string.whatsapp_add), style = MaterialTheme.typography.titleLarge)
            when (state) {
                is WhatsAppState.Working -> {
                    Text(state.pack.name, style = MaterialTheme.typography.titleMedium)
                    Working(state)
                }
                is WhatsAppState.Failed -> {
                    Text(state.pack.name, style = MaterialTheme.typography.titleMedium)
                    Text(state.message.text(), style = MaterialTheme.typography.bodyLarge)
                    OutlinedButton(onClick = vm::retry) { Text(stringResource(R.string.app_action_retry)) }
                }
                is WhatsAppState.Ready -> Ready(state, onAdd = vm::add)
                WhatsAppState.Closed -> Unit
            }
        }
    }
}

@Composable
private fun Working(state: WhatsAppState.Working) {
    if (state.step == WhatsappStep.DOWNLOADING && state.parts > 0) {
        Text(stringResource(R.string.whatsapp_downloading, state.part, state.parts))
        LinearProgressIndicator(
            progress = { (state.part - 1).coerceAtLeast(0).toFloat() / state.parts },
            modifier = Modifier.fillMaxWidth(),
        )
    } else {
        Text(stringResource(R.string.whatsapp_building))
        LinearProgressIndicator(Modifier.fillMaxWidth())
    }
}

@Composable
private fun Ready(state: WhatsAppState.Ready, onAdd: (String) -> Unit) {
    val note = MaterialTheme.typography.bodyMedium
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    if (state.offline) Text(stringResource(R.string.whatsapp_offline), style = note, color = muted)
    if (state.parts.size > 1) {
        Text(pluralStringResource(R.plurals.whatsapp_parts, state.parts.size, state.parts.size), style = note, color = muted)
    }
    if (state.skipped > 0) {
        Text(pluralStringResource(R.plurals.whatsapp_skipped, state.skipped, state.skipped), style = note, color = muted)
    }
    state.notice?.let { Text(it.text(), style = note, color = MaterialTheme.colorScheme.error) }
    val busy = state.parts.any { it.status == PartStatus.ADDING }
    state.parts.forEach { row ->
        val count = row.part.stickers.size
        val stickers = pluralStringResource(R.plurals.whatsapp_stickers, count, count)
        val details = if (row.part.animated) stringResource(R.string.whatsapp_animated, stickers) else stickers
        ListItem(
            headlineContent = { Text(row.part.name, maxLines = 2, overflow = TextOverflow.Ellipsis) },
            supportingContent = {
                Column {
                    Text(details)
                    statusText(row.status)?.let { Text(stringResource(it), color = MaterialTheme.colorScheme.primary) }
                }
            },
            trailingContent = {
                when (row.status) {
                    PartStatus.ADDING -> CircularProgressIndicator(Modifier.size(24.dp))
                    PartStatus.ADDED, PartStatus.ALREADY_ADDED -> Unit
                    else -> Button(onClick = { onAdd(row.part.identifier) }, enabled = !busy) {
                        Text(stringResource(R.string.whatsapp_add_part))
                    }
                }
            },
        )
    }
}

private fun statusText(status: PartStatus): Int? = when (status) {
    PartStatus.NOT_ADDED -> null
    PartStatus.ADDING -> R.string.whatsapp_status_adding
    PartStatus.ADDED -> R.string.whatsapp_status_added
    PartStatus.ALREADY_ADDED -> R.string.whatsapp_status_already_added
    PartStatus.CANCELLED -> R.string.whatsapp_status_cancelled
    PartStatus.REFUSED -> R.string.whatsapp_status_refused
}
