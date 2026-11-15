package io.github.dolfost.stickpile.ime

import android.content.res.Resources
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.ImageDecoder
import android.graphics.drawable.Animatable
import android.graphics.drawable.BitmapDrawable
import android.graphics.drawable.Drawable
import android.os.Build
import android.util.Log
import android.util.LruCache
import android.widget.ImageView
import androidx.core.graphics.drawable.toDrawable
import androidx.core.graphics.scale
import io.github.dolfost.stickpile.provider.kind
import io.github.dolfost.stickpile.rust.LibraryException
import java.io.File
import java.io.IOException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Thumbnails for the grid: the library's cached file (downloaded when
 * missing), decoded off the main thread, kept in memory in an [LruCache].
 * On API 28 and later [ImageDecoder] decodes them, so an animated WebP
 * moves; below, [BitmapFactory] gives its first frame.
 *
 * The file comes from the server and is not trusted: it is decoded at most
 * [cellPx] (and [DecodeSize.MAX_THUMBNAIL_PX]) on its longer side, and a
 * file that fails to decode, for whatever reason, shows nothing.
 *
 * @param cellPx the size of a grid cell, in pixels.
 */
internal class ThumbnailLoader(
    private val scope: CoroutineScope,
    private val library: ImeLibrary,
    private val resources: Resources,
    cellPx: Int,
) {
    private val maxPx = DecodeSize.thumbnailPx(cellPx)

    private val cache = object : LruCache<String, Drawable>(cacheBytes()) {
        override fun sizeOf(key: String, value: Drawable): Int = bytesOf(value)
    }

    /** Shows [itemId]'s thumbnail in [image]; the job to cancel when the view is reused, if any. */
    fun bind(itemId: String, image: ImageView): Job? {
        image.tag = itemId
        cache.get(itemId)?.let {
            show(image, it)
            return null
        }
        image.setImageDrawable(null)
        return scope.launch {
            val drawable = try {
                decode(library.thumbnail(itemId))
            } catch (e: LibraryException) {
                Log.d(TAG, "no thumbnail: ${e.kind}")
                null
            } ?: return@launch
            cache.put(itemId, drawable)
            if (image.tag == itemId) show(image, drawable)
        }
    }

    /** [image] no longer shows a thumbnail (its view was recycled). */
    fun unbind(image: ImageView) {
        (image.drawable as? Animatable)?.stop()
        image.tag = null
        image.setImageDrawable(null)
    }

    fun clear() {
        cache.evictAll()
    }

    private fun show(image: ImageView, drawable: Drawable) {
        image.setImageDrawable(drawable)
        (drawable as? Animatable)?.start()
    }

    private suspend fun decode(path: String): Drawable? = withContext(Dispatchers.IO) {
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                ImageDecoder.decodeDrawable(ImageDecoder.createSource(File(path))) { decoder, info, _ ->
                    val (w, h) = DecodeSize.fit(info.size.width, info.size.height, maxPx)
                        ?: throw IOException("no size")
                    decoder.setTargetSize(w, h)
                }
            } else {
                decodeFirstFrame(path)?.toDrawable(resources)
            }
        } catch (e: Exception) {
            // Any failure of a file from the server is no thumbnail, not a crash.
            Log.d(TAG, "thumbnail not decoded: ${e.javaClass.simpleName}")
            null
        }
    }

    /** Below API 28: sampled down while decoding, then scaled to fit. */
    private fun decodeFirstFrame(path: String): Bitmap? {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(path, bounds)
        DecodeSize.fit(bounds.outWidth, bounds.outHeight, maxPx) ?: return null
        val options = BitmapFactory.Options().apply {
            inSampleSize = DecodeSize.sampleSize(bounds.outWidth, bounds.outHeight, maxPx)
        }
        val bitmap = BitmapFactory.decodeFile(path, options) ?: return null
        val (w, h) = DecodeSize.fit(bitmap.width, bitmap.height, maxPx) ?: return null
        return if (w == bitmap.width && h == bitmap.height) bitmap else bitmap.scale(w, h)
    }

    private companion object {
        const val TAG = "stickpile.ime"

        /** An eighth of what the process may use, at most 32 MiB. */
        fun cacheBytes(): Int = minOf(Runtime.getRuntime().maxMemory() / 8, 32L * 1024 * 1024).toInt()

        fun bytesOf(drawable: Drawable): Int = when (drawable) {
            is BitmapDrawable -> drawable.bitmap?.allocationByteCount ?: 1
            // An animation keeps about a frame or two decoded.
            else -> maxOf(drawable.intrinsicWidth, 1) * maxOf(drawable.intrinsicHeight, 1) * 4 * 2
        }
    }
}
