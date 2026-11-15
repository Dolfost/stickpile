package io.github.dolfost.stickpile.ui.settings

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.ui.common.FailureLog

/** A library in the app, and its licence (an SPDX expression). */
data class Licence(@StringRes val name: Int, @StringRes val licence: Int)

/** The libraries of the version catalog (android/gradle/libs.versions.toml) and the Kotlin standard library. */
val LIBRARIES: List<Licence> = listOf(
    Licence(R.string.licence_lib_compose, R.string.licence_apache),
    Licence(R.string.licence_lib_androidx, R.string.licence_apache),
    Licence(R.string.licence_lib_material, R.string.licence_apache),
    Licence(R.string.licence_lib_work, R.string.licence_apache),
    Licence(R.string.licence_lib_coroutines, R.string.licence_apache),
    Licence(R.string.licence_lib_kotlin, R.string.licence_apache),
    Licence(R.string.licence_lib_jna, R.string.licence_jna),
    Licence(R.string.licence_lib_zxing, R.string.licence_apache),
    Licence(R.string.licence_lib_zxing_embedded, R.string.licence_apache),
)

/** The app's licence, the libraries' licences, and where the Rust crates are listed. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LicencesScreen(onBack: () -> Unit) {
    val context = LocalContext.current
    Scaffold(
        contentWindowInsets = WindowInsets.safeDrawing,
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.licences_title)) },
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
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(R.string.licences_app), style = MaterialTheme.typography.bodyLarge)
                val source = stringResource(R.string.licences_source_url)
                TextButton(onClick = { openLink(context, source) }) { Text(source) }
            }
            SectionHeader(stringResource(R.string.licences_libraries))
            LIBRARIES.forEach { lib ->
                ListItem(
                    headlineContent = { Text(stringResource(lib.name)) },
                    supportingContent = { Text(stringResource(lib.licence)) },
                )
            }
            Text(
                stringResource(R.string.licences_rust),
                style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.padding(16.dp),
            )
        }
    }
}

private fun openLink(context: Context, url: String) {
    try {
        context.startActivity(Intent(Intent.ACTION_VIEW, url.toUri()))
    } catch (e: ActivityNotFoundException) {
        Log.w(FailureLog.TAG, "no browser: $e")
    }
}
