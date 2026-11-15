package io.github.dolfost.stickpile.ui.library

import android.graphics.drawable.Drawable
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
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
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.ContentScale
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
import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.display.Stat
import io.github.dolfost.stickpile.display.TileField
import io.github.dolfost.stickpile.motion.LoopingVideoView
import io.github.dolfost.stickpile.motion.MotionPolicy
import io.github.dolfost.stickpile.motion.Motions
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.Motion
import io.github.dolfost.stickpile.rust.MotionKind
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface

/** The dark band and badges over a thumbnail: light text stays readable on any picture. */
private val Scrim = Color(0x99000000)
private val OnScrim = Color.White
private val LikedRed = Color(0xFFFF5A6E)
private val BandStyle = TextStyle(color = OnScrim, fontSize = 10.sp, lineHeight = 12.sp)

/**
 * The items as tiles at least [Display.tileSize] wide, with what [display]
 * asks over each thumbnail. A tap shares an item, a long press opens its
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
    LazyVerticalGrid(
        columns = GridCells.Adaptive(display.tileSize.minDp.dp),
        contentPadding = PaddingValues(8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier.fillMaxSize(),
    ) {
        // Keys must be unique: an item listed twice is drawn once.
        items(items.distinctBy { it.id }, key = { it.id }) { item ->
            val info = remember(item, display) { TileInfo.of(item, display) }
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
                ItemPicture(library, item, display.autoplay, display.tileSize.minDp * 2)
                TileOverlays(info)
                if (preparing == item.id) CircularProgressIndicator(Modifier.size(32.dp))
            }
        }
    }
}

/** The badges top left, the like top right, the band of texts at the bottom. */
@Composable
private fun BoxScope.TileOverlays(info: TileInfo) {
    if (info.badge != null || info.nsfw) {
        Row(Modifier.align(Alignment.TopStart).padding(4.dp), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
            info.badge?.let { Pill(it, Scrim, OnScrim) }
            if (info.nsfw) {
                Pill(stringResource(R.string.display_nsfw_badge), MaterialTheme.colorScheme.error, MaterialTheme.colorScheme.onError)
            }
        }
    }
    if (info.liked) {
        Icon(
            painterResource(R.drawable.ic_heart),
            contentDescription = stringResource(R.string.display_liked_by_you),
            tint = LikedRed,
            modifier = Modifier
                .align(Alignment.TopEnd)
                .padding(4.dp)
                .background(Scrim, CircleShape)
                .padding(3.dp)
                .size(12.dp),
        )
    }
    if (info.hasBand) {
        Column(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .background(Scrim)
                .padding(horizontal = 4.dp, vertical = 2.dp),
        ) {
            info.name?.let { BandLine(it, weight = FontWeight.SemiBold) }
            info.mood?.let { BandLine(it) }
            info.tags?.let { BandLine(it) }
            if (info.hasStatsLine) StatsLine(info.shape, info.stats)
        }
    }
}

@Composable
private fun Pill(text: String, background: Color, color: Color) {
    Text(
        text,
        color = color,
        fontSize = 9.sp,
        lineHeight = 11.sp,
        fontWeight = FontWeight.Bold,
        maxLines = 1,
        modifier = Modifier
            .background(background, RoundedCornerShape(4.dp))
            .padding(horizontal = 3.dp),
    )
}

@Composable
private fun BandLine(text: String, modifier: Modifier = Modifier, weight: FontWeight? = null) {
    Text(text, style = BandStyle, fontWeight = weight, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = modifier)
}

/** "PNG 512x512" (cut short first when the tile is narrow), then each number with its icon. */
@Composable
private fun StatsLine(shape: String?, stats: List<Stat>) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        shape?.let { BandLine(it, Modifier.weight(1f, fill = false)) }
        stats.forEach { stat ->
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(1.dp)) {
                Icon(painterResource(stat.kind.icon), contentDescription = null, tint = OnScrim, modifier = Modifier.size(9.dp))
                BandLine(stat.text)
            }
        }
    }
}

private val TileField.icon: Int
    get() = when (this) {
        TileField.LIKES -> R.drawable.ic_heart
        TileField.QUALITY -> R.drawable.ic_quality
        else -> R.drawable.ic_copies
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
