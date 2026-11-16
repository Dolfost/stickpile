package io.github.dolfost.stickpile.ui.library

import android.graphics.drawable.Drawable
import androidx.annotation.DrawableRes
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.Autoplay
import io.github.dolfost.stickpile.display.Columns
import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.display.Stat
import io.github.dolfost.stickpile.display.TileIcons
import io.github.dolfost.stickpile.ime.TileInfo as LineFit
import io.github.dolfost.stickpile.motion.LoopingVideoView
import io.github.dolfost.stickpile.motion.MotionPolicy
import io.github.dolfost.stickpile.motion.Motions
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.Motion
import io.github.dolfost.stickpile.rust.MotionKind
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface

/**
 * The lines over a thumbnail: white letters with a dark outline under them
 * ([Halo], as around the icons, [TileIcons]), readable on any picture
 * without a band behind them.
 */
private val LineStyle = TextStyle(color = Color.White, fontSize = 10.sp, lineHeight = 12.sp)
private val Halo = Color(0xB3000000)

/** The outline around the letters, both sides together. */
private val HALO_WIDTH = 2.5.dp

/**
 * The items as tiles, [Display.columns] in a row (more in a wider window,
 * [Columns.count]), with what [display] asks over each thumbnail, as many
 * lines as fit the tile. A tap shares an item, a long press opens its
 * details.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun ItemGrid(
    items: List<Item>,
    library: StickpileLibraryInterface,
    display: Display,
    preparing: String?,
    onShare: (Item) -> Unit,
    onDetails: (Item) -> Unit,
) {
    val haptics = LocalHapticFeedback.current
    val shareLabel = stringResource(R.string.library_share)
    val detailsLabel = stringResource(R.string.library_details_open)
    val configuration = LocalConfiguration.current
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val widthDp = maxWidth.value.toInt()
        val count = Columns.count(display.columns, widthDp, configuration.smallestScreenWidthDp)
        val gap = if (count > TIGHT_ABOVE) 4.dp else 8.dp
        val tileDp = ((widthDp - 16) / count).coerceAtLeast(1)
        val maxLines = LineFit.maxLines(tileDp, configuration.fontScale)
        LazyVerticalGrid(
            columns = GridCells.Fixed(count),
            contentPadding = PaddingValues(8.dp),
            horizontalArrangement = Arrangement.spacedBy(gap),
            verticalArrangement = Arrangement.spacedBy(gap),
            modifier = Modifier.fillMaxSize(),
        ) {
            // Keys must be unique: an item listed twice is drawn once.
            items(items.distinctBy { it.id }, key = { it.id }) { item ->
                val info = remember(item, display, maxLines) { TileInfo.of(item, display, maxLines) }
                Box(
                    Modifier
                        .aspectRatio(1f)
                        .clip(RoundedCornerShape(12.dp))
                        .background(MaterialTheme.colorScheme.surfaceVariant)
                        .combinedClickable(
                            onClickLabel = shareLabel,
                            onLongClickLabel = detailsLabel,
                            onLongClick = {
                                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                onDetails(item)
                            },
                            onClick = { onShare(item) },
                        )
                        .semantics { contentDescription = item.name },
                    contentAlignment = Alignment.Center,
                ) {
                    ItemPicture(library, item, display.autoplay, tileDp)
                    TileOverlays(info)
                    if (preparing == item.id) CircularProgressIndicator(Modifier.size(32.dp))
                }
            }
        }
    }
}

/** More columns than this: the tiles stand closer. */
private const val TIGHT_ABOVE = 4

/**
 * The kind and NSFW icons top left, the like top right, the lines at the
 * bottom; nothing has a background ([TileIcons], [HaloLine]).
 */
@Composable
private fun BoxScope.TileOverlays(info: TileInfo) {
    if (info.badge != null || info.nsfw) {
        Row(
            Modifier.align(Alignment.TopStart).padding(4.dp),
            horizontalArrangement = Arrangement.spacedBy(2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            info.badge?.let { TileIcon(TileIcons.badge(it), it) }
            if (info.nsfw) TileIcon(TileIcons.NSFW, stringResource(R.string.display_nsfw_badge))
        }
    }
    if (info.liked) {
        TileIcon(
            TileIcons.LIKED,
            stringResource(R.string.display_liked_by_you),
            Modifier.align(Alignment.TopEnd).padding(4.dp),
        )
    }
    if (info.hasBand) {
        Column(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .padding(horizontal = 5.dp, vertical = 3.dp),
        ) {
            info.name?.let { HaloLine(it, weight = FontWeight.SemiBold) }
            info.mood?.let { HaloLine(it) }
            info.tags?.let { HaloLine(it) }
            if (info.hasStatsLine) StatsLine(info.shape, info.stats)
        }
    }
}

/** An icon over the tile, at its own size, not tinted (it has its own outline). */
@Composable
private fun TileIcon(@DrawableRes icon: Int, description: String?, modifier: Modifier = Modifier) {
    Image(painterResource(icon), contentDescription = description, modifier = modifier)
}

/** A line of white text with a dark outline around its letters. */
@Composable
private fun HaloLine(text: String, modifier: Modifier = Modifier, weight: FontWeight? = null) {
    val halo = with(LocalDensity.current) { HALO_WIDTH.toPx() }
    val outline = remember(halo) { LineStyle.copy(color = Halo, drawStyle = Stroke(width = halo, join = StrokeJoin.Round)) }
    Box(modifier) {
        Text(text, style = outline, fontWeight = weight, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(text, style = LineStyle, fontWeight = weight, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** "PNG 512x512" (cut short first when the tile is narrow), then each number with its icon. */
@Composable
private fun StatsLine(shape: String?, stats: List<Stat>) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        shape?.let { HaloLine(it, Modifier.weight(1f, fill = false)) }
        stats.forEach { stat ->
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(1.dp)) {
                TileIcon(TileIcons.stat(stat.kind), null, Modifier.size(10.dp))
                HaloLine(stat.text)
            }
        }
    }
}

/**
 * The item's picture, fitted: its thumbnail, moving instead for an
 * animation or a video as [autoplay] lets it ([Motions]), decoded at most
 * [sideDp] (in pixels, at most [MAX_MOTION_PX]). The still shows until the
 * moving one is ready, and when it will not play.
 */
@Composable
internal fun ItemPicture(library: StickpileLibraryInterface, item: Item, autoplay: Autoplay, sideDp: Int) {
    val context = LocalContext.current
    val maxPx = with(LocalDensity.current) { sideDp.dp.roundToPx() }.coerceIn(1, MAX_MOTION_PX)
    val source = remember(item.kind, autoplay) { MotionPolicy.source(item.kind, autoplay, Motions.metered(context)) }
    val motion: Motion? by produceState<Motion?>(null, item.id, source) {
        value = source?.let { s -> Motions.motion(item.id, s) { id, download -> library.motion(id, download) } }
    }
    val m = motion
    when (m?.kind) {
        null -> Thumbnail(library, item.id)
        MotionKind.PICTURE -> MovingPicture(library, item.id, m, maxPx)
        MotionKind.VIDEO -> {
            Thumbnail(library, item.id)
            MovingVideo(item.id, m.path)
        }
    }
}

/** An animated GIF or WebP, looping; the still until it is decoded, and if it is not. */
@Composable
private fun MovingPicture(library: StickpileLibraryInterface, itemId: String, motion: Motion, maxPx: Int) {
    val drawable: Drawable? by produceState(Motions.cachedPicture(motion, maxPx), motion, maxPx) {
        if (value == null) value = Motions.picture(itemId, motion, maxPx)
    }
    val d = drawable
    if (d == null) {
        Thumbnail(library, itemId)
    } else {
        val painter = remember(d) { DrawablePainter(d) }
        Image(
            painter = painter,
            contentDescription = null,
            contentScale = ContentScale.Fit,
            modifier = Modifier.fillMaxSize().padding(PICTURE_PADDING),
        )
    }
}

/** A video over the still, without sound, looping while it is on screen. */
@Composable
private fun MovingVideo(itemId: String, path: String) {
    AndroidView(
        factory = { LoopingVideoView(it) },
        update = { view ->
            view.listener = object : LoopingVideoView.Listener {
                override fun failed() = Motions.failed(itemId)
            }
            view.play(path)
        },
        onReset = { it.play(null) },
        onRelease = { it.play(null) },
        modifier = Modifier.fillMaxSize().padding(PICTURE_PADDING),
    )
}

/** The longest side a moving picture is decoded at. */
private const val MAX_MOTION_PX = 768

private val PICTURE_PADDING = 6.dp

/** A pack's cover, fitted, once it is loaded; nothing for a pack without one. */
@Composable
internal fun PackCover(library: StickpileLibraryInterface, packId: String, modifier: Modifier = Modifier) {
    val resources = LocalResources.current
    val drawable: Drawable? by produceState(Thumbnails.cachedCover(packId), packId) {
        if (value == null) value = Thumbnails.loadCover(library, packId, resources)
    }
    Box(modifier) {
        drawable?.let { d ->
            val painter = remember(d) { DrawablePainter(d) }
            Image(
                painter = painter,
                contentDescription = null,
                contentScale = ContentScale.Fit,
                modifier = Modifier.fillMaxSize(),
            )
        }
    }
}

/** The item's thumbnail, fitted, once it is loaded (from memory at once when it is there). */
@Composable
internal fun Thumbnail(library: StickpileLibraryInterface, itemId: String, modifier: Modifier = Modifier) {
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
            modifier = modifier.fillMaxSize().padding(PICTURE_PADDING),
        )
    }
}
