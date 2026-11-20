package io.github.dolfost.stickpile.motion

import android.content.Context
import android.graphics.ImageDecoder
import android.graphics.drawable.AnimatedImageDrawable
import android.graphics.drawable.Drawable
import android.net.ConnectivityManager
import android.os.Build
import android.os.SystemClock
import android.util.Log
import android.util.LruCache
import io.github.dolfost.stickpile.ime.DecodeSize
import io.github.dolfost.stickpile.provider.kind
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.Motion
import io.github.dolfost.stickpile.rust.MotionKind
import java.io.File
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext

/**
 * The moving renditions of the items, for the grids and the details of
 * the app and the keyboard alike: the file to play (the library's
 * `motion`), and the animated drawables of the pictures, decoded off the
 * main thread and kept in memory for the whole process. Videos play in a
 * [LoopingVideoView].
 *
 * A file that will not decode or play is not tried again until the cache
 * is cleared ([clear]): its item shows its still.
 */
object Motions {
    private const val TAG = "stickpile.motion"

    /**
     * How long a tile waits before it downloads: a fling past it downloads
     * nothing (a download, once started, runs to its end).
     */
    const val DOWNLOAD_DELAY_MS = 400L

    /** Downloads at once, so that a grid full of GIFs does not start them all together. */
    private val downloads = Semaphore(3)

    private val failed: MutableSet<String> = ConcurrentHashMap.newKeySet()

    private val pictures = object : LruCache<String, Drawable>(memoryBudget()) {
        override fun sizeOf(key: String, value: Drawable): Int = bytesOf(value)
    }

    /**
     * The file to play for [itemId] from [source]: the cached one at once,
     * else, for [MotionPolicy.Source.ANY], downloaded after
     * [DOWNLOAD_DELAY_MS]. Null when the item shows its still: nothing to
     * play, not cached for [MotionPolicy.Source.CACHED], a failure (only
     * logged) or a picture before API 28. [fetch] is the library's
     * `motion(itemId, download)`.
     */
    suspend fun motion(itemId: String, source: MotionPolicy.Source, fetch: suspend (String, Boolean) -> Motion?): Motion? {
        if (itemId in failed) return null
        val motion = ask { fetch(itemId, false) } ?: run {
            if (source != MotionPolicy.Source.ANY) return null
            delay(DOWNLOAD_DELAY_MS)
            downloads.withPermit { ask { fetch(itemId, true) } }
        }
        return motion?.takeIf { MotionPolicy.playable(it) }
    }

    /** The decoded picture of [motion] at [maxPx] if it is in memory. */
    fun cachedPicture(motion: Motion, maxPx: Int): Drawable? = pictures.get(key(motion, maxPx))

    /**
     * The animated drawable of [motion], a picture, at most [maxPx] on its
     * longer side, looping for ever: from memory, else decoded off the main
     * thread. Null when it does not decode; [itemId] then shows its still.
     */
    suspend fun picture(itemId: String, motion: Motion, maxPx: Int): Drawable? {
        if (motion.kind != MotionKind.PICTURE || Build.VERSION.SDK_INT < Build.VERSION_CODES.P) return null
        val key = key(motion, maxPx)
        pictures.get(key)?.let { return it }
        val drawable = withContext(Dispatchers.IO) { decode(motion.path, maxPx) }
        if (drawable == null) {
            failed.add(itemId)
            return null
        }
        pictures.put(key, drawable)
        return drawable
    }

    /** [itemId]'s file would not play: it shows its still from now on. */
    fun failed(itemId: String) {
        failed.add(itemId)
    }

    /** Forget every decoded picture and failure (the cache was cleared). */
    fun clear() {
        pictures.evictAll()
        failed.clear()
    }

    /** Keep [share] of the decoded pictures, the least used go ([io.github.dolfost.stickpile.MemoryTrim]). */
    fun trim(share: Float) {
        pictures.trimToSize((pictures.maxSize() * share).toInt())
    }

    @Volatile
    private var meteredAt = 0L

    @Volatile
    private var meteredNow = true

    /**
     * Whether the network in use costs by the byte (mobile data, a
     * hotspot); unknown counts as yes. Asked of the system at most every
     * few seconds: every tile wants to know.
     */
    fun metered(context: Context): Boolean {
        val now = SystemClock.elapsedRealtime()
        if (meteredAt == 0L || now - meteredAt > METERED_FOR_MS) {
            meteredNow = context.getSystemService(ConnectivityManager::class.java)?.isActiveNetworkMetered ?: true
            meteredAt = now
        }
        return meteredNow
    }

    private const val METERED_FOR_MS = 5_000L

    private suspend fun ask(call: suspend () -> Motion?): Motion? = try {
        withContext(Dispatchers.IO) { call() }
    } catch (e: CancellationException) {
        throw e
    } catch (e: LibraryException) {
        Log.d(TAG, "nothing to play: ${e.kind}")
        null
    } catch (e: Exception) {
        // A Rust panic arrives as UniFFI's InternalException: no motion, not a crash.
        Log.w(TAG, "motion failed: ${e.javaClass.simpleName}")
        null
    }

    private fun decode(path: String, maxPx: Int): Drawable? {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) return null
        return try {
            val drawable = ImageDecoder.decodeDrawable(ImageDecoder.createSource(File(path))) { decoder, info, _ ->
                val (w, h) = DecodeSize.fit(info.size.width, info.size.height, maxPx) ?: throw IOException("no size")
                decoder.setTargetSize(w, h)
            }
            (drawable as? AnimatedImageDrawable)?.repeatCount = AnimatedImageDrawable.REPEAT_INFINITE
            drawable
        } catch (e: Exception) {
            // Any failure of a file from the server is no motion, not a crash.
            Log.d(TAG, "not decoded: ${e.javaClass.simpleName}")
            null
        }
    }

    private fun key(motion: Motion, maxPx: Int) = "${motion.path}@$maxPx"

    /** An animation holds about two frames. */
    private fun bytesOf(d: Drawable): Int =
        (d.intrinsicWidth.coerceAtLeast(1).toLong() * d.intrinsicHeight.coerceAtLeast(1) * 4 * 2)
            .coerceAtMost(Int.MAX_VALUE.toLong()).toInt()

    /** A sixteenth of what the process may use, at most 24 MiB. */
    private fun memoryBudget(): Int = minOf(Runtime.getRuntime().maxMemory() / 16, 24L * 1024 * 1024).toInt()
}
