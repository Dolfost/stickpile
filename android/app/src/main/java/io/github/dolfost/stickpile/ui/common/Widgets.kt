package io.github.dolfost.stickpile.ui.common

import android.content.res.Resources
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import io.github.dolfost.stickpile.R

/** The message's text. */
@Composable
fun Message.text(): String = stringResource(id, *args.toTypedArray())

/** The message's text, outside composition (from `LocalResources`). */
fun Message.text(resources: Resources): String = resources.getString(id, *args.toTypedArray())

/** A spinner in the middle of the screen. */
@Composable
fun Loading(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        CircularProgressIndicator()
    }
}

/**
 * A screen that says what went wrong, with a way to try again and,
 * optionally, a second way out.
 */
@Composable
fun FailureScreen(
    title: String,
    message: Message,
    onRetry: (() -> Unit)?,
    modifier: Modifier = Modifier,
    otherLabel: String? = null,
    onOther: (() -> Unit)? = null,
) {
    Surface(modifier.fillMaxSize()) {
        Column(
            Modifier
                .safeDrawingPadding()
                .padding(24.dp)
                .fillMaxSize(),
            verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Icon(
                Icons.Filled.Warning,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.error,
                modifier = Modifier.size(48.dp),
            )
            Text(title, style = MaterialTheme.typography.titleLarge, textAlign = TextAlign.Center)
            Text(message.text(), textAlign = TextAlign.Center, modifier = Modifier.fillMaxWidth())
            if (onRetry != null) {
                Button(onClick = onRetry) { Text(stringResource(R.string.app_action_retry)) }
            }
            if (otherLabel != null && onOther != null) {
                TextButton(onClick = onOther) { Text(otherLabel) }
            }
        }
    }
}
