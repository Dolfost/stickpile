package io.github.dolfost.stickpile.share

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowDropDown
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.Pack

/**
 * The share screen: the set to upload into, one card per shared file
 * (preview, name, keywords, emojis, status), and the upload with its
 * progress, or why nothing can be uploaded.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ShareScreen(model: ShareViewModel, onOpenApp: () -> Unit, onClose: () -> Unit) {
    val state = model.state
    val running = state.batch.running
    var askStop by rememberSaveable { mutableStateOf(false) }
    BackHandler(enabled = running) { askStop = true }
    val close = { if (running) askStop = true else onClose() }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.share_title)) },
                navigationIcon = {
                    IconButton(onClick = close) {
                        Icon(Icons.Filled.Close, contentDescription = stringResource(R.string.share_close))
                    }
                },
            )
        },
        bottomBar = {
            if (state.setup is Setup.Ready) BottomPane(state, model, onClose = onClose, onStop = { askStop = true })
        },
        contentWindowInsets = WindowInsets.safeDrawing,
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize()) {
            when (val setup = state.setup) {
                Setup.Loading -> Message(stringResource(R.string.share_loading)) { CircularProgressIndicator() }
                Setup.NothingShared -> Message(stringResource(R.string.share_nothing)) {
                    Button(onClick = onClose) { Text(stringResource(R.string.share_close)) }
                }
                Setup.SignedOut -> Message(stringResource(R.string.share_signed_out)) {
                    TextButton(onClick = onClose) { Text(stringResource(R.string.share_close)) }
                    Button(onClick = onOpenApp) { Text(stringResource(R.string.share_open_app)) }
                }
                is Setup.NoOwnSets -> Message(stringResource(R.string.share_no_sets, setup.serverUrl)) {
                    TextButton(onClick = onClose) { Text(stringResource(R.string.share_close)) }
                    Button(onClick = model::loadSetup) { Text(stringResource(R.string.share_check_again)) }
                }
                is Setup.Failed -> Message(setupFailureText(setup.failure)) {
                    TextButton(onClick = onClose) { Text(stringResource(R.string.share_close)) }
                    Button(onClick = model::loadSetup) { Text(stringResource(R.string.share_retry)) }
                }
                is Setup.Ready -> FileList(state, setup, model)
            }
        }
    }

    if (askStop) {
        AlertDialog(
            onDismissRequest = { askStop = false },
            title = { Text(stringResource(R.string.share_stop_title)) },
            text = { Text(stringResource(R.string.share_stop_text)) },
            confirmButton = {
                TextButton(onClick = {
                    askStop = false
                    model.stop()
                    onClose()
                }) { Text(stringResource(R.string.share_stop)) }
            },
            dismissButton = {
                TextButton(onClick = { askStop = false }) { Text(stringResource(R.string.share_keep_uploading)) }
            },
        )
    }
}

/** A message in the middle of the screen, with buttons below. */
@Composable
private fun Message(text: String, note: String? = null, actions: @Composable RowScope.() -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        // Selectable, so that a server address can be copied.
        SelectionContainer {
            Text(text, style = MaterialTheme.typography.bodyLarge, textAlign = TextAlign.Center)
        }
        if (note != null) {
            Text(
                note,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically, content = actions)
    }
}

@Composable
private fun FileList(state: ShareState, setup: Setup.Ready, model: ShareViewModel) {
    val batch = state.batch
    val several = batch.uploadable.size > 1
    LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item(key = "set") {
            SetPicker(setup.sets, state.setId, enabled = !batch.running, onSelect = model::selectSet)
        }
        if (setup.offline) {
            item(key = "offline") {
                Text(
                    stringResource(R.string.share_offline),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        items(batch.files, key = { it.id }) { file ->
            FileCard(
                file = file,
                preview = model.previews[file.id],
                several = several,
                running = batch.running,
                copying = batch.copying,
                model = model,
            )
        }
    }
}

@Composable
private fun SetPicker(sets: List<Pack>, setId: String?, enabled: Boolean, onSelect: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    val chosen = sets.firstOrNull { it.id == setId }
    Box {
        OutlinedCard(onClick = { open = true }, enabled = enabled, modifier = Modifier.fillMaxWidth()) {
            Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(
                        stringResource(R.string.share_set_label),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Text(
                        chosen?.name ?: stringResource(R.string.share_set_choose),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    if (chosen != null) ItemCount(chosen)
                }
                Icon(Icons.Filled.ArrowDropDown, contentDescription = stringResource(R.string.share_set_choose))
            }
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            sets.forEach { set ->
                DropdownMenuItem(
                    text = {
                        Column {
                            Text(set.name)
                            ItemCount(set)
                        }
                    },
                    onClick = {
                        open = false
                        onSelect(set.id)
                    },
                )
            }
        }
    }
}

@Composable
private fun ItemCount(set: Pack) {
    val n = set.itemCount.toInt()
    Text(
        pluralStringResource(R.plurals.share_set_items, n, n),
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun FileCard(
    file: SharedFile,
    preview: ImageBitmap?,
    several: Boolean,
    running: Boolean,
    copying: Boolean,
    model: ShareViewModel,
) {
    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Preview(file, preview)
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(
                        file.fileName,
                        style = MaterialTheme.typography.bodyMedium,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Status(file)
                }
                val removable = !running && file.status !is FileStatus.Uploaded
                if (removable) {
                    IconButton(onClick = { model.remove(file.id) }) {
                        Icon(Icons.Filled.Close, contentDescription = stringResource(R.string.share_remove))
                    }
                }
            }
            if (file.editable) {
                OutlinedTextField(
                    value = file.name,
                    onValueChange = { model.editName(file.id, it) },
                    label = { Text(stringResource(R.string.share_name_label)) },
                    placeholder = { Text(stringResource(R.string.share_name_placeholder)) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = file.keywords,
                    onValueChange = { model.editKeywords(file.id, it) },
                    label = { Text(stringResource(R.string.share_keywords_label)) },
                    supportingText = { Text(stringResource(R.string.share_keywords_hint)) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                if (several) ApplyToAll { model.keywordsToAll(file.id) }
                OutlinedTextField(
                    value = file.emojis,
                    onValueChange = { model.editEmojis(file.id, it) },
                    label = { Text(stringResource(R.string.share_emojis_label)) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                if (several) ApplyToAll { model.emojisToAll(file.id) }
            }
            if (file.status is FileStatus.Failed) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                    TextButton(onClick = { model.upload(listOf(file.id)) }, enabled = !running && !copying) {
                        Icon(Icons.Filled.Refresh, contentDescription = null)
                        Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.share_retry))
                    }
                }
            }
        }
    }
}

@Composable
private fun ApplyToAll(onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
        TextButton(onClick = onClick) { Text(stringResource(R.string.share_apply_to_all)) }
    }
}

@Composable
private fun Preview(file: SharedFile, preview: ImageBitmap?) {
    Box(
        Modifier
            .size(72.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant),
        contentAlignment = Alignment.Center,
    ) {
        if (preview != null) {
            Image(preview, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        }
        if (file.isVideo) {
            Icon(
                Icons.Filled.PlayArrow,
                contentDescription = stringResource(R.string.share_video),
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier
                    .size(32.dp)
                    .clip(RoundedCornerShape(16.dp))
                    .background(MaterialTheme.colorScheme.surfaceVariant),
            )
        }
    }
}

@Composable
private fun Status(file: SharedFile) {
    val small = MaterialTheme.typography.bodySmall
    val error = MaterialTheme.colorScheme.error
    val quiet = MaterialTheme.colorScheme.onSurfaceVariant
    when (val status = file.status) {
        FileStatus.Ready -> Unit
        FileStatus.Copying -> Text(stringResource(R.string.share_status_copying), style = small, color = quiet)
        FileStatus.Queued -> Text(stringResource(R.string.share_status_queued), style = small, color = quiet)
        FileStatus.Uploading -> Row(verticalAlignment = Alignment.CenterVertically) {
            CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
            Spacer(Modifier.width(8.dp))
            Text(stringResource(R.string.share_status_uploading), style = small, color = quiet)
        }
        is FileStatus.Uploaded -> Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Filled.CheckCircle, contentDescription = null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(16.dp))
            Spacer(Modifier.width(8.dp))
            Text(stringResource(R.string.share_status_uploaded, status.itemName), style = small)
        }
        is FileStatus.Failed -> Row(verticalAlignment = Alignment.Top) {
            Icon(Icons.Filled.Warning, contentDescription = null, tint = error, modifier = Modifier.size(16.dp))
            Spacer(Modifier.width(8.dp))
            Text(failureText(status.failure), style = small, color = error)
        }
        is FileStatus.Unreadable -> Text(readProblemText(status.problem), style = small, color = error)
    }
}

/** The bar at the bottom: the upload button, the progress while it runs, the summary after. */
@Composable
private fun BottomPane(state: ShareState, model: ShareViewModel, onClose: () -> Unit, onStop: () -> Unit) {
    val progress = state.batch.progress
    Surface(tonalElevation = 3.dp) {
        Column(
            Modifier
                .fillMaxWidth()
                .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Bottom + WindowInsetsSides.Horizontal))
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (progress != null) {
                Progress(progress, onStop)
            } else {
                Idle(state, model, onClose)
            }
        }
    }
}

@Composable
private fun Progress(progress: UploadProgress, onStop: () -> Unit) {
    Text(
        stringResource(R.string.share_progress, progress.position, progress.total),
        style = MaterialTheme.typography.titleSmall,
    )
    Text(progress.name, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
    LinearProgressIndicator(
        progress = { (progress.position - 1).toFloat() / progress.total },
        modifier = Modifier.fillMaxWidth(),
    )
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
        TextButton(onClick = onStop) { Text(stringResource(R.string.share_stop)) }
    }
}

/** No upload runs: the summary of the last one, and the buttons. */
@Composable
private fun Idle(state: ShareState, model: ShareViewModel, onClose: () -> Unit) {
    val batch = state.batch
    val summary = batch.summary
    if (summary != null) {
        val text = if (summary.complete) {
            pluralStringResource(R.plurals.share_summary_done, summary.uploaded, summary.uploaded, state.set?.name.orEmpty())
        } else {
            stringResource(R.string.share_summary_partial, summary.uploaded, summary.total - summary.unreadable)
        }
        Text(text, style = MaterialTheme.typography.titleSmall)
    }
    Row(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (summary?.complete == true) {
            Button(onClick = onClose) { Text(stringResource(R.string.share_done)) }
        } else {
            TextButton(onClick = onClose) { Text(stringResource(R.string.share_close)) }
            val n = batch.uploadable.size
            Button(onClick = { model.upload() }, enabled = n > 0 && state.setId != null && !batch.copying) {
                Text(
                    if (batch.onlyRetries) {
                        pluralStringResource(R.plurals.share_retry_failed, n, n)
                    } else {
                        pluralStringResource(R.plurals.share_upload, n, n)
                    },
                )
            }
        }
    }
}

@Composable
private fun failureText(failure: UploadFailure): String = when (failure.kind) {
    LibraryErrorKind.NOT_SIGNED_IN -> stringResource(R.string.share_error_not_signed_in)
    LibraryErrorKind.UNAUTHORIZED -> stringResource(R.string.share_error_unauthorized)
    LibraryErrorKind.UNREACHABLE -> stringResource(R.string.share_error_unreachable)
    LibraryErrorKind.NOT_FOUND -> stringResource(R.string.share_error_not_found)
    LibraryErrorKind.RATE_LIMITED -> stringResource(R.string.share_error_rate_limited)
    LibraryErrorKind.INVALID_INPUT -> stringResource(R.string.share_error_invalid_input)
    LibraryErrorKind.SERVER -> stringResource(R.string.share_error_server, failure.status ?: 0)
    LibraryErrorKind.IO -> stringResource(R.string.share_error_io)
    LibraryErrorKind.NOTHING_FITS, LibraryErrorKind.OTHER -> stringResource(R.string.share_error_other)
}

/** Why the account or its sets could not be read. */
@Composable
private fun setupFailureText(failure: UploadFailure): String = when (failure.kind) {
    LibraryErrorKind.UNREACHABLE, LibraryErrorKind.RATE_LIMITED, LibraryErrorKind.SERVER -> failureText(failure)
    else -> stringResource(R.string.share_error_setup)
}

@Composable
private fun readProblemText(problem: ReadProblem): String = when (problem) {
    ReadProblem.NOT_MEDIA -> stringResource(R.string.share_unreadable_not_media)
    ReadProblem.NO_ACCESS -> stringResource(R.string.share_unreadable_no_access)
    ReadProblem.GONE -> stringResource(R.string.share_unreadable_gone)
    ReadProblem.FAILED -> stringResource(R.string.share_unreadable_failed)
}
