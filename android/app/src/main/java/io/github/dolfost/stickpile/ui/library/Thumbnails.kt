package io.github.dolfost.stickpile.ui.library

import android.content.res.Resources
import android.graphics.BitmapFactory
import android.graphics.ImageDecoder
import android.graphics.drawable.Animatable
import android.graphics.drawable.BitmapDrawable
import android.graphics.drawable.Drawable
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.LruCache
import androidx.compose.runtime.RememberObserver
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.painter.Painter
import androidx.core.graphics.drawable.toDrawable
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import io.github.dolfost.stickpile.ui.common.Doing
import io.github.dolfost.stickpile.ui.common.FailureLog
import io.github.dolfost.stickpile.ui.common.catching
import java.io.File
import kotlin.math.roundToInt
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Decoded thumbnails, kept in memory for the whole process and shared by
 * every grid. The files come from the library (`thumbnail`, WebP of at
 * most 256 px); on Android 9 and later `ImageDecoder` decodes them, which
 * gives an animated drawable for an animated WebP or GIF; before,
 * `BitmapFactory` (the first frame).
 */
object Thumbnails {
    private val memory = object : LruCache<String, Drawable>(memoryBudget()) {
        override fun sizeOf(key: String, value: Drawable): Int = bytesOf(value)
    }

    /** The decoded thumbnail if it is in memory. */
    fun cached(itemId: String): Drawable? = memory.get(itemId)

    /** The decoded cover of a pack if it is in memory. */
    fun cachedCover(packId: String): Drawable? = memory.get(COVER_KEY + packId)

    /**
     * The item's thumbnail: from memory, else fetched by the library and
     * decoded off the main thread. Null when it cannot be had (offline and
     * not cached, or not decodable); the reason goes to the log.
     */
    suspend fun load(library: StickpileLibraryInterface, itemId: String, resources: Resources): Drawable? =
        load(itemId, resources) { library.thumbnail(itemId) }

    /** A pack's cover, as [load]; also null for a pack without one. */
    suspend fun loadCover(library: StickpileLibraryInterface, packId: String, resources: Resources): Drawable? =
        load(COVER_KEY + packId, resources) { library.packCover(packId) }

    private suspend fun load(key: String, resources: Resources, file: suspend () -> String?): Drawable? {
        memory.get(key)?.let { return it }
        val path = catching { file() }.getOrElse { e ->
            FailureLog.android.failed(Doing.LOADING, e)
            return null
        } ?: return null
        val drawable = withContext(Dispatchers.IO) { catching { decode(path, resources) }.getOrNull() } ?: return null
        memory.put(key, drawable)
        return drawable
    }

    /** Before a pack's id, the key of its cover (item ids never start so). */
    private const val COVER_KEY = "pack:"

    /** Forget every decoded thumbnail (the cache was cleared). */
    fun clear() {
        memory.evictAll()
    }

    /** Keep [share] of the decoded thumbnails, the least used go ([io.github.dolfost.stickpile.MemoryTrim]). */
    fun trim(share: Float) {
        memory.trimToSize((memory.maxSize() * share).toInt())
    }

    private fun decode(path: String, resources: Resources): Drawable? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            ImageDecoder.decodeDrawable(ImageDecoder.createSource(File(path))) { decoder, _, _ ->
                // Software bitmaps draw on any canvas; the thumbnails are small.
                decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
            }
        } else {
            BitmapFactory.decodeFile(path)?.toDrawable(resources)
        }

    private fun bytesOf(d: Drawable): Int = when (d) {
        is BitmapDrawable -> d.bitmap?.allocationByteCount ?: 1
        // An animation holds about two frames.
        else -> (d.intrinsicWidth.coerceAtLeast(1) * d.intrinsicHeight.coerceAtLeast(1) * 4 * 2)
    }.coerceAtLeast(1)

    private fun memoryBudget(): Int = (Runtime.getRuntime().maxMemory() / 8).coerceAtMost(Int.MAX_VALUE.toLong()).toInt()
}

/**
 * Draws a [Drawable] in Compose, animating it when it is [Animatable]
 * (an animated WebP or GIF from `ImageDecoder`): the drawable's callback
 * redraws the painter, and its scheduled frames run on the main thread.
 */
class DrawablePainter(private val drawable: Drawable) : Painter(), RememberObserver {
    private var frame by mutableIntStateOf(0)

    private val callback = object : Drawable.Callback {
        override fun invalidateDrawable(who: Drawable) {
            frame++
        }

        override fun scheduleDrawable(who: Drawable, what: Runnable, `when`: Long) {
            mainHandler.postAtTime(what, `when`)
        }

        override fun unscheduleDrawable(who: Drawable, what: Runnable) {
            mainHandler.removeCallbacks(what)
        }
    }

    override val intrinsicSize: Size
        get() = if (drawable.intrinsicWidth > 0 && drawable.intrinsicHeight > 0) {
            Size(drawable.intrinsicWidth.toFloat(), drawable.intrinsicHeight.toFloat())
        } else {
            Size.Unspecified
        }

    override fun DrawScope.onDraw() {
        frame // read, so that a new frame redraws
        drawIntoCanvas { canvas ->
            drawable.setBounds(0, 0, size.width.roundToInt(), size.height.roundToInt())
            drawable.draw(canvas.nativeCanvas)
        }
    }

    override fun onRemembered() {
        drawable.callback = callback
        drawable.setVisible(true, true)
        if (drawable is Animatable) drawable.start()
    }

    override fun onForgotten() = stop()

    override fun onAbandoned() = stop()

    private fun stop() {
        if (drawable is Animatable) drawable.stop()
        drawable.setVisible(false, false)
        if (drawable.callback === callback) drawable.callback = null
    }

    private companion object {
        val mainHandler = Handler(Looper.getMainLooper())
    }
}
