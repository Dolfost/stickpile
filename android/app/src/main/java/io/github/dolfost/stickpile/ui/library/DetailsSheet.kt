package io.github.dolfost.stickpile.ui.library

import androidx.annotation.StringRes
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SuggestionChip
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.Autoplay
import io.github.dolfost.stickpile.display.ItemLabels
import io.github.dolfost.stickpile.offline.KeepAction
import io.github.dolfost.stickpile.offline.OfflineLabels
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import kotlinx.coroutines.launch

/**
 * An item's details, from a long press: a larger picture (moving, as
 * [autoplay] lets it), the name, the
 * pack and its owner, the numbers, the vibe, emojis, tags and keywords,
 * and the words in the picture (line breaks kept). A tap on a vibe, emoji
 * or tag searches for it ([onSearch]
 * gets the query). Share, Like or Unlike, and Open in browser when the
 * item has a web page; Keep offline, or Don't keep offline for a sticker kept
 * by itself, and why it is kept.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
internal fun DetailsSheet(
    details: Details,
    library: StickpileLibraryInterface,
    autoplay: Autoplay,
    onDismiss: () -> Unit,
    onShare: (Item) -> Unit,
    onLike: (Item) -> Unit,
    onOpen: (String) -> Unit,
    onSearch: (String) -> Unit,
    onKeep: (Item) -> Unit,
) {
    val item = details.item
    val sheet = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val scope = rememberCoroutineScope()
    // Do it now, and let the sheet slide away before it goes.
    val closing = { action: () -> Unit ->
        action()
        scope.launch { sheet.hide() }.invokeOnCompletion { onDismiss() }
        Unit
    }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheet) {
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(start = 16.dp, end = 16.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                Box(
                    Modifier
                        .size(DETAILS_PICTURE_DP.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(MaterialTheme.colorScheme.surfaceVariant),
                    contentAlignment = Alignment.Center,
                ) { ItemPicture(library, item, autoplay, DETAILS_PICTURE_DP) }
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(item.name, style = MaterialTheme.typography.titleMedium)
                    Text(
                        stringResource(R.string.display_pack_by, item.setName, item.owner),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Text(facts(item), style = MaterialTheme.typography.bodySmall)
                    if (item.liked) {
                        Text(
                            stringResource(R.string.display_liked_by_you),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.primary,
                        )
                    }
                    val reasons = details.reasons.orEmpty()
                    if (reasons.isNotEmpty()) {
                        val why = OfflineLabels.reasons(reasons).map { stringResource(it) }.joinToString(", ")
                        Text(
                            stringResource(R.string.offline_reasons, why),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.primary,
                        )
                    }
                }
            }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { closing { onShare(item) } }) {
                    ButtonLabel(R.drawable.ic_share, stringResource(R.string.library_share))
                }
                OutlinedButton(onClick = { onLike(item) }) {
                    if (item.liked) {
                        ButtonLabel(R.drawable.ic_heart, stringResource(R.string.library_unlike))
                    } else {
                        ButtonLabel(R.drawable.ic_heart_outline, stringResource(R.string.library_like))
                    }
                }
                details.reasons?.let { reasons ->
                    val action = OfflineLabels.stickerAction(reasons)
                    OutlinedButton(onClick = { onKeep(item) }) {
                        val icon = if (action == KeepAction.KEEP) R.drawable.ic_offline else R.drawable.ic_offline_done
                        ButtonLabel(icon, stringResource(OfflineLabels.label(action)))
                    }
                }
                details.url?.let { url ->
                    OutlinedButton(onClick = { closing { onOpen(url) } }) {
                        ButtonLabel(R.drawable.ic_open, stringResource(R.string.library_open_in_browser))
                    }
                }
            }
            val search = { query: String -> closing { onSearch(query) } }
            ChipSection(R.string.display_field_vibe, listOfNotNull(ItemLabels.vibe(item)), { it }, { SearchTerms.filter("vibe", it) }, search)
            ChipSection(R.string.display_field_emojis, cleaned(item.emojis), { it }, { SearchTerms.filter("e", it) }, search)
            ChipSection(R.string.display_field_tags, cleaned(item.tags), { "#$it" }, { SearchTerms.filter("tag", it) }, search)
            val keywords = cleaned(item.keywords)
            if (keywords.isNotEmpty()) {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    SectionTitle(R.string.library_details_keywords)
                    Text(keywords.joinToString(", "), style = MaterialTheme.typography.bodyMedium)
                }
            }
            // The words in the picture as they are, line breaks kept.
            val text = item.text.trim()
            if (text.isNotEmpty()) {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    SectionTitle(R.string.display_field_text)
                    Text(text, style = MaterialTheme.typography.bodyMedium)
                }
            }
        }
    }
}

/** "PNG 512x512, copied 3 times, 2 likes, quality 80", as the desktop's footer (no likes: left out). */
@Composable
private fun facts(item: Item): String {
    val copies = item.copies.toCount()
    val likes = item.likes.toCount()
    return listOfNotNull(
        ItemLabels.shape(item),
        if (copies == 0) stringResource(R.string.display_never_copied) else pluralStringResource(R.plurals.display_copied, copies, copies),
        if (likes == 0) null else pluralStringResource(R.plurals.display_likes, likes, likes),
        item.quality?.let { stringResource(R.string.display_quality, it.toInt()) },
    ).joinToString(", ")
}

private fun ULong.toCount(): Int = coerceAtMost(Int.MAX_VALUE.toULong()).toInt()

private fun cleaned(words: List<String>): List<String> = words.map { it.trim() }.filter { it.isNotEmpty() }.distinct()

/** A title over [values] as chips; a tap searches for [query] of the value. Nothing for no values. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ChipSection(
    @StringRes title: Int,
    values: List<String>,
    label: (String) -> String,
    query: (String) -> String,
    onSearch: (String) -> Unit,
) {
    if (values.isEmpty()) return
    Column {
        SectionTitle(title)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            values.forEach { value ->
                SuggestionChip(onClick = { onSearch(query(value)) }, label = { Text(label(value)) })
            }
        }
    }
}

@Composable
private fun SectionTitle(@StringRes text: Int) {
    Text(
        stringResource(text),
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun ButtonLabel(icon: Int, text: String) {
    Icon(painterResource(icon), contentDescription = null, modifier = Modifier.size(18.dp))
    Spacer(Modifier.size(8.dp))
    Text(text)
}

/** The side of the details' picture. */
private const val DETAILS_PICTURE_DP = 144
