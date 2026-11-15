package io.github.dolfost.stickpile.ui.library

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.display.DisplaySettings
import io.github.dolfost.stickpile.display.SyntaxHelp
import io.github.dolfost.stickpile.ui.common.DisplayOptions
import kotlinx.coroutines.launch

/** The sheets the library's menu opens. */
internal enum class LibrarySheet { DISPLAY, SYNTAX }

/** What the tiles show, from the menu: the same switches as in the settings. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun DisplaySheet(display: Display, settings: DisplaySettings, onDismiss: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.verticalScroll(rememberScrollState()).padding(bottom = 16.dp)) {
            SheetTitle(stringResource(R.string.display_info))
            DisplayOptions(display, settings)
        }
    }
}

/**
 * The search syntax, as the desktop's help shows it: each example in
 * monospace, and what it does. A tap on an example searches for it.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
internal fun SyntaxSheet(onExample: (String) -> Unit, onDismiss: () -> Unit) {
    val sheet = rememberModalBottomSheetState()
    val scope = rememberCoroutineScope()
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheet) {
        LazyColumn(contentPadding = PaddingValues(bottom = 16.dp)) {
            item {
                SheetTitle(stringResource(R.string.display_syntax))
                Text(
                    stringResource(R.string.library_syntax_tap),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 16.dp),
                )
            }
            SyntaxHelp.SECTIONS.forEach { section ->
                item {
                    Text(
                        stringResource(section.title),
                        style = MaterialTheme.typography.titleSmall,
                        color = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 4.dp),
                    )
                }
                items(section.lines) { line ->
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .padding(horizontal = 16.dp, vertical = 6.dp),
                        verticalArrangement = Arrangement.spacedBy(2.dp),
                    ) {
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            SearchTerms.examples(line.example).forEach { example ->
                                Text(
                                    example,
                                    fontFamily = FontFamily.Monospace,
                                    style = MaterialTheme.typography.bodyMedium,
                                    color = MaterialTheme.colorScheme.primary,
                                    modifier = Modifier
                                        .clip(RoundedCornerShape(6.dp))
                                        .background(MaterialTheme.colorScheme.surfaceVariant)
                                        .clickable {
                                            onExample(SearchTerms.typed(example))
                                            scope.launch { sheet.hide() }.invokeOnCompletion { onDismiss() }
                                        }
                                        .padding(horizontal = 6.dp, vertical = 2.dp),
                                )
                            }
                        }
                        Text(
                            stringResource(line.text),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun SheetTitle(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.titleLarge,
        modifier = Modifier.padding(start = 16.dp, end = 16.dp, bottom = 8.dp),
    )
}
