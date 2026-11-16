package io.github.dolfost.stickpile.ui.library

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.PrimaryTabRow
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
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
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.DisplaySettings
import io.github.dolfost.stickpile.provider.PreparedFiles
import io.github.dolfost.stickpile.rust.Pack
import io.github.dolfost.stickpile.rust.Prepared
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import io.github.dolfost.stickpile.ui.common.FailureLog
import io.github.dolfost.stickpile.ui.common.Loading
import io.github.dolfost.stickpile.ui.common.catching
import io.github.dolfost.stickpile.ui.common.text
import kotlinx.coroutines.launch

/**
 * The library: a search field with offers for what is typed, safe mode
 * and the tiles' info as toggles, a menu (what the tiles show, the search
 * syntax, sync, settings), the tabs (recent, liked, packs, most used) and
 * a grid of tiles that show what [settings] asks. A tap shares an item, a
 * long press opens its details; pulling down syncs.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LibraryScreen(
    vm: LibraryViewModel,
    library: StickpileLibraryInterface,
    settings: DisplaySettings,
    onOpenSettings: () -> Unit,
    onSignInAgain: () -> Unit,
) {
    val context = LocalContext.current
    val resources = LocalResources.current
    val state = vm.state
    val display by settings.state.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    var searchFocused by remember { mutableStateOf(false) }
    var sheet by rememberSaveable { mutableStateOf<LibrarySheet?>(null) }

    // A message that replaces the one shown.
    fun notify(text: String) {
        scope.launch {
            snackbar.currentSnackbarData?.dismiss()
            snackbar.showSnackbar(text)
        }
    }

    LaunchedEffect(vm) {
        vm.eventFlow.collect { event ->
            when (event) {
                is LibraryEvent.Share ->
                    if (!share(context, resources.getString(R.string.library_share_title), event.prepared)) {
                        launch { snackbar.showSnackbar(resources.getString(R.string.library_share_failed)) }
                    }
                is LibraryEvent.Notice -> launch { snackbar.showSnackbar(event.message.text(resources)) }
            }
        }
    }
    // Back from a chat (the picker keyboard) or the settings.
    LifecycleResumeEffect(vm) {
        vm.resumed()
        onPauseOrDispose {}
    }
    BackHandler(enabled = state.query.isNotEmpty()) { vm.setQuery("") }
    BackHandler(enabled = state.query.isEmpty() && state.pack != null) { vm.closePack() }

    Scaffold(
        contentWindowInsets = WindowInsets.safeDrawing,
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            Surface(color = MaterialTheme.colorScheme.surface) {
                Column(Modifier.statusBarsPadding()) {
                    SearchRow(
                        query = state.query,
                        display = display,
                        onQuery = vm::setQuery,
                        onFocusChange = { focused ->
                            if (focused && !searchFocused) vm.searchFocused()
                            searchFocused = focused
                        },
                        onSafeMode = { on ->
                            settings.setSafeMode(on)
                            notify(resources.getString(if (on) R.string.display_safe_on else R.string.display_safe_off))
                        },
                        onShowInfo = settings::setShowInfo,
                        onMenu = { entry ->
                            when (entry) {
                                LibraryMenu.DISPLAY -> sheet = LibrarySheet.DISPLAY
                                LibraryMenu.SYNTAX -> sheet = LibrarySheet.SYNTAX
                                LibraryMenu.SYNC -> vm.refresh()
                                LibraryMenu.SETTINGS -> onOpenSettings()
                            }
                        },
                    )
                    if ((searchFocused || state.query.isNotEmpty()) && state.hints.isNotEmpty()) {
                        HintRow(state.hints, vm::takeHint)
                    }
                    if (state.query.isBlank()) {
                        PrimaryTabRow(selectedTabIndex = state.tab.ordinal) {
                            LibraryTab.entries.forEach { tab ->
                                Tab(
                                    selected = tab == state.tab,
                                    onClick = { vm.selectTab(tab) },
                                    text = { Text(stringResource(tab.label), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                                )
                            }
                        }
                    }
                }
            }
        },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize()) {
            val pack = state.pack
            if (state.query.isBlank() && pack != null) PackHeader(pack, onBack = { vm.closePack() })
            val content = state.content
            val offline = (content as? LibraryContent.Items)?.offline ?: (content as? LibraryContent.Packs)?.offline ?: false
            if (offline) OfflineNotice()
            if (content is LibraryContent.Items && content.hidden > 0 && content.items.isNotEmpty()) {
                HiddenNotice(content.hidden)
            }
            PullToRefreshBox(
                isRefreshing = state.refreshing,
                onRefresh = vm::refresh,
                modifier = Modifier.fillMaxSize(),
            ) {
                when (content) {
                    LibraryContent.Loading -> Loading()
                    is LibraryContent.Items -> when {
                        content.items.isNotEmpty() ->
                            ItemGrid(content.items, library, display, state.preparing, vm::share, vm::showDetails)
                        content.hidden > 0 ->
                            Empty(pluralStringResource(R.plurals.display_hidden_by_safe_mode, content.hidden, content.hidden))
                        else -> Empty(stringResource(emptyText(state.request, display.safeMode)))
                    }
                    is LibraryContent.Packs ->
                        if (content.packs.isEmpty()) {
                            Empty(stringResource(R.string.library_empty_packs))
                        } else {
                            PackList(content.packs, library, vm::openPack)
                        }
                    is LibraryContent.Failed -> Failed(content, vm::reload, onSignInAgain)
                }
            }
        }
    }

    state.details?.let { details ->
        DetailsSheet(
            details,
            library,
            display.autoplay,
            onDismiss = vm::hideDetails,
            onShare = vm::share,
            onLike = vm::toggleLike,
            onOpen = { url -> if (!openPage(context, url)) notify(resources.getString(R.string.library_open_failed)) },
            onSearch = vm::searchNow,
        )
    }
    when (sheet) {
        LibrarySheet.DISPLAY -> DisplaySheet(display, settings, onDismiss = { sheet = null })
        LibrarySheet.SYNTAX -> SyntaxSheet(onExample = vm::searchNow, onDismiss = { sheet = null })
        null -> Unit
    }
}

/** A pack's cover in the list of packs. */
private val PACK_COVER = 48.dp

private val LibraryTab.label: Int
    get() = when (this) {
        LibraryTab.RECENT -> R.string.library_tab_recent
        LibraryTab.LIKED -> R.string.library_tab_liked
        LibraryTab.PACKS -> R.string.library_tab_packs
        LibraryTab.MOST_USED -> R.string.library_tab_most_used
    }

private fun emptyText(request: LibraryRequest, safeMode: Boolean): Int = when (request) {
    is LibraryRequest.Search -> if (safeMode) R.string.library_empty_search_safe else R.string.library_empty_search
    LibraryRequest.Recent -> R.string.library_empty_recent
    LibraryRequest.Favorites -> R.string.library_empty_liked
    LibraryRequest.MostUsed -> R.string.library_empty_most_used
    LibraryRequest.Packs -> R.string.library_empty_packs
    is LibraryRequest.PackItems -> R.string.library_empty_pack
}

@Composable
private fun PackHeader(pack: Pack, onBack: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(end = 16.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = onBack) {
            Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.library_all_packs))
        }
        Text(pack.name, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun OfflineNotice() {
    Surface(color = MaterialTheme.colorScheme.secondaryContainer, modifier = Modifier.fillMaxWidth()) {
        Text(
            stringResource(R.string.library_offline),
            style = MaterialTheme.typography.bodyMedium,
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
        )
    }
}

/** A slim line over the grid: safe mode hid some of the list. */
@Composable
private fun HiddenNotice(hidden: Int) {
    Text(
        pluralStringResource(R.plurals.display_hidden_by_safe_mode, hidden, hidden),
        style = MaterialTheme.typography.labelMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 4.dp),
    )
}

/** A message in the middle, scrollable so that pulling down still syncs. */
@Composable
private fun Empty(text: String, content: @Composable () -> Unit = {}) {
    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text(text, textAlign = TextAlign.Center, style = MaterialTheme.typography.bodyLarge)
        content()
    }
}

@Composable
private fun Failed(failed: LibraryContent.Failed, onRetry: () -> Unit, onSignInAgain: () -> Unit) {
    Empty(failed.message.text()) {
        OutlinedButton(onClick = onRetry) { Text(stringResource(R.string.app_action_retry)) }
        if (failed.signInAgain) {
            Button(onClick = onSignInAgain) { Text(stringResource(R.string.library_sign_in_again)) }
        }
    }
}

@Composable
private fun PackList(packs: List<Pack>, library: StickpileLibraryInterface, onOpen: (Pack) -> Unit) {
    LazyColumn(Modifier.fillMaxSize()) {
        items(packs.distinctBy { it.id }, key = { it.id }) { pack ->
            val count = pack.itemCount.toInt()
            ListItem(
                leadingContent = { PackCover(library, pack.id, Modifier.size(PACK_COVER)) },
                headlineContent = { Text(pack.name) },
                supportingContent = {
                    Text(stringResource(R.string.library_pack_details, pack.owner, pluralStringResource(R.plurals.library_items, count, count)))
                },
                modifier = Modifier.clickable { onOpen(pack) },
            )
        }
    }
}

/**
 * The system share sheet for a prepared file, readable through the
 * provider; false when it cannot open (no share sheet, refused, ...), which
 * the screen says. Only the failure's class is logged: the intent holds
 * the URI, and the URI the token.
 */
private fun share(context: Context, title: String, prepared: Prepared): Boolean = catching {
    val uri = PreparedFiles.uriFor(context, prepared)
    val send = Intent(Intent.ACTION_SEND).apply {
        type = prepared.mimeType
        putExtra(Intent.EXTRA_STREAM, uri)
        clipData = ClipData(prepared.displayName, arrayOf(prepared.mimeType), ClipData.Item(uri))
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }
    context.startActivity(Intent.createChooser(send, title))
}.onFailure { e -> Log.w(FailureLog.TAG, "no share sheet: ${e.javaClass.simpleName}") }.isSuccess

/** An item's web page in the browser; false when no app opens it. */
private fun openPage(context: Context, url: String): Boolean = catching {
    context.startActivity(Intent(Intent.ACTION_VIEW, url.toUri()))
}.onFailure { e -> Log.w(FailureLog.TAG, "no browser: ${e.javaClass.simpleName}") }.isSuccess
