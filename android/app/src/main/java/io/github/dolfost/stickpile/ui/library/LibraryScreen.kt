package io.github.dolfost.stickpile.ui.library

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Drawable
import android.util.Log
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.provider.PreparedFiles
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.MediaKind
import io.github.dolfost.stickpile.rust.Pack
import io.github.dolfost.stickpile.rust.Prepared
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import io.github.dolfost.stickpile.ui.common.FailureLog
import io.github.dolfost.stickpile.ui.common.Loading
import io.github.dolfost.stickpile.ui.common.text
import kotlinx.coroutines.launch

/**
 * The library: a search field, the tabs (recent, liked, packs, most used)
 * and a grid. A tap shares an item, a long press likes or unlikes it;
 * pulling down (or the refresh button) syncs.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LibraryScreen(
    vm: LibraryViewModel,
    library: StickpileLibraryInterface,
    onOpenSettings: () -> Unit,
    onSignInAgain: () -> Unit,
) {
    val context = LocalContext.current
    val resources = LocalResources.current
    val state = vm.state
    val snackbar = remember { SnackbarHostState() }

    LaunchedEffect(vm) {
        vm.eventFlow.collect { event ->
            when (event) {
                is LibraryEvent.Share -> share(context, resources.getString(R.string.library_share_title), event.prepared)
                is LibraryEvent.Notice -> launch { snackbar.showSnackbar(event.message.text(resources)) }
            }
        }
    }
    // Back from a chat (the picker keyboard) or the settings: the recent items may have changed.
    LifecycleResumeEffect(vm) {
        if (vm.state.request == LibraryRequest.Recent) vm.reload()
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
                    SearchRow(state.query, vm::setQuery, vm::refresh, onOpenSettings)
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
            PullToRefreshBox(
                isRefreshing = state.refreshing,
                onRefresh = vm::refresh,
                modifier = Modifier.fillMaxSize(),
            ) {
                when (content) {
                    LibraryContent.Loading -> Loading()
                    is LibraryContent.Items ->
                        if (content.items.isEmpty()) {
                            Empty(stringResource(emptyText(state.request)))
                        } else {
                            ItemGrid(content.items, library, state.preparing, vm::share, vm::toggleLike)
                        }
                    is LibraryContent.Packs ->
                        if (content.packs.isEmpty()) {
                            Empty(stringResource(R.string.library_empty_packs))
                        } else {
                            PackList(content.packs, vm::openPack)
                        }
                    is LibraryContent.Failed -> Failed(content, vm::reload, onSignInAgain)
                }
            }
        }
    }
}

private val LibraryTab.label: Int
    get() = when (this) {
        LibraryTab.RECENT -> R.string.library_tab_recent
        LibraryTab.LIKED -> R.string.library_tab_liked
        LibraryTab.PACKS -> R.string.library_tab_packs
        LibraryTab.MOST_USED -> R.string.library_tab_most_used
    }

private fun emptyText(request: LibraryRequest): Int = when (request) {
    is LibraryRequest.Search -> R.string.library_empty_search
    LibraryRequest.Recent -> R.string.library_empty_recent
    LibraryRequest.Favorites -> R.string.library_empty_liked
    LibraryRequest.MostUsed -> R.string.library_empty_most_used
    LibraryRequest.Packs -> R.string.library_empty_packs
    is LibraryRequest.PackItems -> R.string.library_empty_pack
}

@Composable
private fun SearchRow(query: String, onQuery: (String) -> Unit, onRefresh: () -> Unit, onOpenSettings: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(start = 12.dp, end = 4.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        OutlinedTextField(
            value = query,
            onValueChange = onQuery,
            placeholder = { Text(stringResource(R.string.library_search)) },
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
            modifier = Modifier.weight(1f),
        )
        IconButton(onClick = onRefresh) {
            Icon(Icons.Filled.Refresh, stringResource(R.string.library_sync))
        }
        IconButton(onClick = onOpenSettings) {
            Icon(Icons.Filled.Settings, stringResource(R.string.settings_title))
        }
    }
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
private fun PackList(packs: List<Pack>, onOpen: (Pack) -> Unit) {
    LazyColumn(Modifier.fillMaxSize()) {
        items(packs.distinctBy { it.id }, key = { it.id }) { pack ->
            val count = pack.itemCount.toInt()
            ListItem(
                headlineContent = { Text(pack.name) },
                supportingContent = {
                    Text(stringResource(R.string.library_pack_details, pack.owner, pluralStringResource(R.plurals.library_items, count, count)))
                },
                modifier = Modifier.clickable { onOpen(pack) },
            )
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ItemGrid(
    items: List<Item>,
    library: StickpileLibraryInterface,
    preparing: String?,
    onShare: (Item) -> Unit,
    onLike: (Item) -> Unit,
) {
    val haptics = LocalHapticFeedback.current
    val shareLabel = stringResource(R.string.library_share)
    val likeLabel = stringResource(R.string.library_like)
    LazyVerticalGrid(
        columns = GridCells.Adaptive(96.dp),
        contentPadding = PaddingValues(8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier.fillMaxSize(),
    ) {
        // Keys must be unique: an item listed twice is drawn once.
        items(items.distinctBy { it.id }, key = { it.id }) { item ->
            Box(
                Modifier
                    .aspectRatio(1f)
                    .clip(RoundedCornerShape(12.dp))
                    .background(MaterialTheme.colorScheme.surfaceVariant)
                    .combinedClickable(
                        onClickLabel = shareLabel,
                        onLongClickLabel = likeLabel,
                        onLongClick = {
                            haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                            onLike(item)
                        },
                        onClick = { onShare(item) },
                    )
                    .semantics { contentDescription = item.name },
                contentAlignment = Alignment.Center,
            ) {
                Thumbnail(library, item.id)
                if (item.kind == MediaKind.VIDEO) {
                    Icon(
                        Icons.Filled.PlayArrow,
                        contentDescription = null,
                        tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.align(Alignment.BottomEnd).padding(4.dp).size(20.dp),
                    )
                }
                if (preparing == item.id) CircularProgressIndicator(Modifier.size(32.dp))
            }
        }
    }
}

@Composable
private fun Thumbnail(library: StickpileLibraryInterface, itemId: String) {
    val resources = LocalResources.current
    val drawable: Drawable? by produceState(Thumbnails.cached(itemId), itemId) {
        if (value == null) value = Thumbnails.load(library, itemId, resources)
    }
    drawable?.let { d ->
        val painter = remember(d) { DrawablePainter(d) }
        Image(
            painter = painter,
            contentDescription = null,
            contentScale = ContentScale.Fit,
            modifier = Modifier.fillMaxSize().padding(6.dp),
        )
    }
}

/** The system share sheet for a prepared file, readable through the provider. */
private fun share(context: Context, title: String, prepared: Prepared) {
    val uri = PreparedFiles.uriFor(context, prepared)
    val send = Intent(Intent.ACTION_SEND).apply {
        type = prepared.mimeType
        putExtra(Intent.EXTRA_STREAM, uri)
        clipData = ClipData(prepared.displayName, arrayOf(prepared.mimeType), ClipData.Item(uri))
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }
    try {
        context.startActivity(Intent.createChooser(send, title))
    } catch (e: ActivityNotFoundException) {
        Log.w(FailureLog.TAG, "no share sheet: $e")
    }
}
