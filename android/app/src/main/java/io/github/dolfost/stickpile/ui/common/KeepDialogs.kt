package io.github.dolfost.stickpile.ui.common

import android.text.format.Formatter
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.offline.KeepAsk
import io.github.dolfost.stickpile.rust.OfflineEstimate

/** A size as the phone writes it ("12 MB"). */
@Composable
fun sizeText(bytes: ULong): String = Formatter.formatShortFileSize(LocalContext.current, bytes.toLong())

/**
 * Keep a pack or the whole library offline? What it takes first (the
 * library's estimate): stickers, files and their size, how much is here
 * already, what the options leave out, and whether it fits the space for
 * kept files. Keep is enabled once the estimate is there.
 */
@Composable
fun KeepDialog(ask: KeepAsk, mobileData: Boolean, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    val title = ask.name?.let { stringResource(R.string.offline_keep_title, it) }
        ?: stringResource(R.string.offline_keep_library_title)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            val estimate = ask.estimate
            if (estimate == null) {
                Text(stringResource(R.string.offline_estimate_loading))
            } else {
                EstimateText(estimate, mobileData)
            }
        },
        confirmButton = {
            TextButton(onClick = onConfirm, enabled = ask.estimate != null) { Text(stringResource(R.string.offline_keep)) }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.app_action_cancel)) }
        },
    )
}

@Composable
private fun EstimateText(e: OfflineEstimate, mobileData: Boolean) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(stringResource(R.string.offline_estimate_size, e.items.toInt(), e.totals.files.toInt(), sizeText(e.totals.bytes)))
        if (e.totals.presentBytes > 0u) {
            Text(stringResource(R.string.offline_estimate_present, sizeText(e.totals.presentBytes)))
        }
        if (e.skipped.kindOff > 0u) Text(stringResource(R.string.offline_estimate_kind_off, e.skipped.kindOff.toInt()))
        if (e.skipped.tooLarge > 0u) Text(stringResource(R.string.offline_estimate_too_large, e.skipped.tooLarge.toInt()))
        val budget = e.budget
        if (!e.fits && budget != null) {
            Text(
                stringResource(R.string.offline_estimate_over, sizeText(e.keptBytesAfter), sizeText(budget)),
                color = MaterialTheme.colorScheme.error,
            )
        } else {
            Text(stringResource(R.string.offline_estimate_after, sizeText(e.keptBytesAfter)))
        }
        Text(
            stringResource(if (mobileData) R.string.offline_estimate_any_network else R.string.offline_estimate_wifi),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/** Stop keeping a pack ([name]) or the library (null) offline? Its files stay until the space is needed. */
@Composable
fun ReleaseDialog(name: String?, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    val title = name?.let { stringResource(R.string.offline_release_title, it) }
        ?: stringResource(R.string.offline_release_library_title)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = { Text(stringResource(R.string.offline_release_text)) },
        confirmButton = {
            TextButton(onClick = onConfirm) { Text(stringResource(R.string.offline_release)) }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.app_action_cancel)) }
        },
    )
}
