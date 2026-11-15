package io.github.dolfost.stickpile.ui.library

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconToggleButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.rust.SearchHint

/** The entries of the library's menu, in their order. */
internal enum class LibraryMenu { DISPLAY, SYNTAX, SYNC, SETTINGS }

/**
 * The top row: the search field, safe mode and the tiles' info as
 * toggles, and the menu. The field shows [query]; when [query] is changed
 * from elsewhere (an offer taken), the cursor goes to its end.
 */
@Composable
internal fun SearchRow(
    query: String,
    display: Display,
    onQuery: (String) -> Unit,
    onFocusChange: (Boolean) -> Unit,
    onSafeMode: (Boolean) -> Unit,
    onShowInfo: (Boolean) -> Unit,
    onMenu: (LibraryMenu) -> Unit,
) {
    var field by remember { mutableStateOf(TextFieldValue(query, TextRange(query.length))) }
    val value = if (field.text == query) field else TextFieldValue(query, TextRange(query.length))
    val focus = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    Row(
        Modifier
            .fillMaxWidth()
            .padding(start = 12.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        OutlinedTextField(
            value = value,
            onValueChange = {
                field = it
                if (it.text != query) onQuery(it.text)
            },
            placeholder = { Text(stringResource(R.string.library_search), maxLines = 1, overflow = TextOverflow.Ellipsis) },
            leadingIcon = { Icon(Icons.Filled.Search, contentDescription = null) },
            trailingIcon = {
                if (query.isNotEmpty()) {
                    IconButton(onClick = { onQuery("") }) {
                        Icon(Icons.Filled.Clear, stringResource(R.string.library_search_clear))
                    }
                }
            },
            singleLine = true,
            shape = CircleShape,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            keyboardActions = KeyboardActions(onSearch = {
                keyboard?.hide()
                focus.clearFocus()
            }),
            modifier = Modifier
                .weight(1f)
                .onFocusChanged { onFocusChange(it.isFocused) },
        )
        IconToggleButton(checked = display.safeMode, onCheckedChange = onSafeMode) {
            Icon(
                painterResource(if (display.safeMode) R.drawable.ic_safe_on else R.drawable.ic_safe_off),
                stringResource(R.string.display_safe_mode),
            )
        }
        IconToggleButton(checked = display.showInfo, onCheckedChange = onShowInfo) {
            Icon(
                painterResource(if (display.showInfo) R.drawable.ic_info_on else R.drawable.ic_info_off),
                stringResource(R.string.display_show_info),
            )
        }
        Menu(onMenu)
    }
}

@Composable
private fun Menu(onMenu: (LibraryMenu) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }) {
            Icon(painterResource(R.drawable.ic_more), stringResource(R.string.library_more))
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            LibraryMenu.entries.forEach { entry ->
                DropdownMenuItem(
                    text = { Text(stringResource(entry.label)) },
                    leadingIcon = { MenuIcon(entry) },
                    onClick = {
                        open = false
                        onMenu(entry)
                    },
                )
            }
        }
    }
}

private val LibraryMenu.label: Int
    get() = when (this) {
        LibraryMenu.DISPLAY -> R.string.display_info
        LibraryMenu.SYNTAX -> R.string.display_syntax
        LibraryMenu.SYNC -> R.string.library_sync
        LibraryMenu.SETTINGS -> R.string.settings_title
    }

@Composable
private fun MenuIcon(entry: LibraryMenu) = when (entry) {
    LibraryMenu.DISPLAY -> Icon(painterResource(R.drawable.ic_info_on), contentDescription = null)
    LibraryMenu.SYNTAX -> Icon(painterResource(R.drawable.ic_help), contentDescription = null)
    LibraryMenu.SYNC -> Icon(Icons.Filled.Refresh, contentDescription = null)
    LibraryMenu.SETTINGS -> Icon(Icons.Filled.Settings, contentDescription = null)
}

/**
 * The offers for what is typed, in a row that scrolls; a tap takes one.
 * The one the search would take now ([SearchHint.chosen]) stands out.
 */
@Composable
internal fun HintRow(hints: List<SearchHint>, onHint: (SearchHint) -> Unit) {
    val list = rememberLazyListState()
    LaunchedEffect(hints) { list.scrollToItem(0) }
    LazyRow(
        state = list,
        contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        items(hints) { hint ->
            FilterChip(
                selected = hint.chosen,
                onClick = { onHint(hint) },
                label = { Text(hint.label, maxLines = 1) },
            )
        }
    }
}
