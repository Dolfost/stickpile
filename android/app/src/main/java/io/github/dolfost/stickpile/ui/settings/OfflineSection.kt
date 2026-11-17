package io.github.dolfost.stickpile.ui.settings

import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.offline.OfflineProgress
import io.github.dolfost.stickpile.offline.OfflineShown
import io.github.dolfost.stickpile.offline.OfflineSizes
import io.github.dolfost.stickpile.rust.OfflineOptions
import io.github.dolfost.stickpile.ui.common.KeepDialog
import io.github.dolfost.stickpile.ui.common.ReleaseDialog
import io.github.dolfost.stickpile.ui.common.sizeText
import kotlin.math.roundToInt

/**
 * "Available offline" (docs/offline.md): the library switch (its estimate
 * and a confirmation first), where the downloads stand with pause, resume
 * and stop, the automatic rules, what packs, the library and the rules
 * download (per kind, the largest file), the space for kept files, mobile
 * data, and the confirmed "Remove kept files". [options] and [paused] are
 * the app's ([io.github.dolfost.stickpile.offline.OfflineSettings]).
 */
@Composable
internal fun OfflineSection(vm: SettingsViewModel, options: OfflineOptions, paused: Boolean) {
    val state = vm.state
    var confirmRelease by rememberSaveable { mutableStateOf(false) }
    var confirmRemove by rememberSaveable { mutableStateOf(false) }
    val set = { o: OfflineOptions -> vm.setOfflineOptions(o) }

    SectionHeader(stringResource(R.string.offline_title))
    Paragraph(stringResource(R.string.offline_text))
    SwitchItem(
        stringResource(R.string.offline_library),
        stringResource(if (state.libraryKept) R.string.offline_library_on else R.string.offline_library_off),
        state.libraryKept,
    ) { on -> if (on) vm.askKeepLibrary() else confirmRelease = true }

    Downloads(vm, options, paused)

    SubHeader(stringResource(R.string.offline_auto))
    SwitchItem(
        stringResource(R.string.offline_auto_thumbnails),
        stringResource(R.string.offline_auto_thumbnails_text),
        options.autoThumbnails,
    ) { set(options.copy(autoThumbnails = it)) }
    SwitchItem(
        stringResource(R.string.offline_auto_used),
        stringResource(R.string.offline_auto_used_count, options.autoUsedCount.toInt()),
        options.autoUsed,
    ) { set(options.copy(autoUsed = it)) }
    if (options.autoUsed) {
        StepSlider(
            OfflineSizes.USED_COUNT.map { it.toULong() },
            options.autoUsedCount.toULong(),
            labels = OfflineSizes.USED_COUNT.map { it.toString() },
        ) { set(options.copy(autoUsedCount = it.toUInt())) }
    }
    SwitchItem(stringResource(R.string.offline_auto_liked), null, options.autoLiked) {
        set(options.copy(autoLiked = it))
    }
    SwitchItem(stringResource(R.string.offline_auto_own_packs), null, options.autoOwnPacks) {
        set(options.copy(autoOwnPacks = it))
    }

    SubHeader(stringResource(R.string.offline_kinds))
    Paragraph(stringResource(R.string.offline_kinds_text))
    SwitchItem(stringResource(R.string.offline_stills), null, options.stills) { set(options.copy(stills = it)) }
    SwitchItem(stringResource(R.string.offline_animations), null, options.animations) {
        set(options.copy(animations = it))
    }
    SwitchItem(stringResource(R.string.offline_videos), null, options.videos) { set(options.copy(videos = it)) }
    SwitchItem(
        stringResource(R.string.offline_sticker_versions),
        stringResource(R.string.offline_sticker_versions_text),
        options.stickerRenditions,
    ) { set(options.copy(stickerRenditions = it)) }
    val noLimit = stringResource(R.string.offline_no_limit)
    ListItem(
        headlineContent = { Text(stringResource(R.string.offline_max_file)) },
        trailingContent = { Text(megabytes(options.maxFileMb, noLimit), style = MaterialTheme.typography.titleMedium) },
    )
    StepSlider(OfflineSizes.MAX_FILE_MB, options.maxFileMb, OfflineSizes.MAX_FILE_MB.map { megabytes(it, noLimit) }) {
        set(options.copy(maxFileMb = it))
    }
    ListItem(
        headlineContent = { Text(stringResource(R.string.offline_budget)) },
        supportingContent = { Text(stringResource(R.string.offline_budget_text)) },
        trailingContent = { Text(megabytes(options.maxMb, noLimit), style = MaterialTheme.typography.titleMedium) },
    )
    StepSlider(OfflineSizes.BUDGET_MB, options.maxMb, OfflineSizes.BUDGET_MB.map { megabytes(it, noLimit) }) {
        set(options.copy(maxMb = it))
    }
    SwitchItem(
        stringResource(R.string.offline_mobile_data),
        stringResource(R.string.offline_mobile_data_text),
        options.mobileData,
    ) { set(options.copy(mobileData = it)) }

    Actions {
        OutlinedButton(
            onClick = { confirmRemove = true },
            enabled = !state.removing && (state.usage?.keptFiles ?: 0uL) > 0uL,
        ) { BusyLabel(stringResource(R.string.offline_remove_kept), state.removing) }
    }

    state.keepLibrary?.let { ask ->
        KeepDialog(ask, options.mobileData, onConfirm = vm::confirmKeepLibrary, onDismiss = vm::dismissKeepLibrary)
    }
    if (confirmRelease) {
        ReleaseDialog(
            name = null,
            onConfirm = {
                confirmRelease = false
                vm.releaseLibrary()
            },
            onDismiss = { confirmRelease = false },
        )
    }
    if (confirmRemove) {
        AlertDialog(
            onDismissRequest = { confirmRemove = false },
            title = { Text(stringResource(R.string.offline_remove_kept_title)) },
            text = { Text(stringResource(R.string.offline_remove_kept_text)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmRemove = false
                    vm.removeKeptFiles()
                }) { Text(stringResource(R.string.offline_remove_kept)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmRemove = false }) { Text(stringResource(R.string.app_action_cancel)) }
            },
        )
    }
}

/** Where the downloads stand ([OfflineProgress]), how much is here, and pause, resume, stop or start. */
@Composable
private fun Downloads(vm: SettingsViewModel, options: OfflineOptions, paused: Boolean) {
    val state = vm.state
    val status = state.status ?: return
    val plan = state.plan
    SubHeader(stringResource(R.string.offline_progress))
    val shown = OfflineProgress.shown(status.copy(paused = status.paused || paused), state.work, plan, options.mobileData)
    val text = when (shown) {
        OfflineShown.LIMIT_REACHED -> stringResource(shown.text, sizeText(plan?.budget ?: state.usage?.budget ?: 0uL))
        OfflineShown.PENDING -> {
            val t = plan?.totals
            stringResource(shown.text, sizeText(if (t == null) 0uL else t.bytes - t.presentBytes))
        }
        else -> stringResource(shown.text)
    }
    Paragraph(text, error = shown == OfflineShown.LIMIT_REACHED || shown == OfflineShown.DISK_FULL)
    OfflineProgress.counts(status, plan)?.let { c ->
        LinearProgressIndicator(
            progress = { c.fraction },
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 4.dp),
        )
        Paragraph(
            stringResource(
                R.string.offline_on_phone,
                c.filesDone.toInt(),
                c.filesTotal.toInt(),
                sizeText(c.bytesDone),
                sizeText(c.bytesTotal),
            ),
        )
    }
    val failed = if (status.running) status.failed else plan?.failed ?: 0uL
    if (failed > 0uL) Paragraph(stringResource(R.string.offline_failed, failed.toInt()))
    val skipped = plan?.skipped?.let { it.kindOff + it.tooLarge } ?: 0uL
    if (skipped > 0uL) Paragraph(stringResource(R.string.offline_skipped, skipped.toInt()))
    Actions {
        if (shown == OfflineShown.PAUSED) {
            Button(onClick = vm::resumeDownloads) { Text(stringResource(R.string.offline_resume)) }
        } else if (OfflineProgress.canPause(shown)) {
            OutlinedButton(onClick = vm::pauseDownloads) { Text(stringResource(R.string.offline_pause)) }
        }
        if (OfflineProgress.canStop(shown)) {
            OutlinedButton(onClick = vm::stopDownloads) { Text(stringResource(R.string.offline_cancel)) }
        }
        if (OfflineProgress.canStart(shown)) {
            Button(onClick = vm::downloadNow) { Text(stringResource(R.string.offline_download_now)) }
        }
    }
}

/** A smaller title inside a section. */
@Composable
private fun SubHeader(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 2.dp),
    )
}

/** "5 MB", or "No limit" for 0. */
@Composable
private fun megabytes(value: ULong, noLimit: String): String =
    if (value == 0uL) noLimit else stringResource(R.string.offline_size_mb, value.toInt())

/**
 * A slider over [steps], at the step nearest [value]; [onChange] gets the
 * step's value when it is let go. [labels] (parallel to [steps]) say it to
 * accessibility services.
 */
@Composable
private fun StepSlider(steps: List<ULong>, value: ULong, labels: List<String>, onChange: (ULong) -> Unit) {
    var index by remember(value) { mutableIntStateOf(OfflineSizes.index(steps, value)) }
    Slider(
        value = index.toFloat(),
        onValueChange = { index = it.roundToInt().coerceIn(0, steps.size - 1) },
        onValueChangeFinished = { onChange(steps[index]) },
        valueRange = 0f..(steps.size - 1).toFloat(),
        steps = (steps.size - 2).coerceAtLeast(0),
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp)
            .semantics { stateDescription = labels[index] },
    )
}
