package io.github.dolfost.stickpile.share

import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.ImageDecoder
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.os.Build
import android.provider.OpenableColumns
import android.util.Log
import androidx.core.content.IntentCompat
import androidx.core.graphics.scale
import java.io.File
import java.io.FileNotFoundException
import java.io.IOException
import java.util.UUID
import kotlinx.coroutines.CancellationException

/**
 * One stream of a share: its URI and what its provider says about it
 * ([size]: `OpenableColumns.SIZE` in bytes, null when it gives none).
 */
data class SharedStream(val uri: Uri, val displayName: String?, val mimeType: String?, val size: Long? = null) {
    val isVideo: Boolean get() = mimeType?.startsWith("video/") == true

    /** Its provider says it is larger than the share target copies ([ShareLimits]). */
    val tooLarge: Boolean get() = ShareLimits.tooLarge(size)

    /**
     * Its provider types it as something else than a picture or a video
     * (an unknown type, or just "bytes", is tried: the server looks at the
     * content).
     */
    val notMedia: Boolean
        get() = mimeType != null && mimeType != "*/*" && mimeType != OCTET_STREAM &&
            !mimeType.startsWith("image/") && !mimeType.startsWith("video/")

    companion object {
        const val OCTET_STREAM = "application/octet-stream"
    }
}

/**
 * The Android side of a share: the streams of the intent, their copies in
 * `cacheDir/share/<session>/<file id>/<file name>`, and previews. Blocking:
 * call from a background thread.
 */
object ShareFiles {
    private const val TAG = "stickpile.share"
    private const val DIR = "share"

    /** Sessions of this process whose copies are in use. */
    private val live = mutableSetOf<String>()

    /**
     * The content URIs of an `ACTION_SEND` (`EXTRA_STREAM`) or
     * `ACTION_SEND_MULTIPLE` (a list of them) intent, else of its clip
     * data; empty for any other intent and for a share with text only.
     * Other schemes (`file:` above all, which could name this app's own
     * files) and this app's own provider are left out ([SharedUris]).
     */
    fun streams(intent: Intent): List<Uri> {
        val extra = when (intent.action) {
            Intent.ACTION_SEND ->
                listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
            Intent.ACTION_SEND_MULTIPLE ->
                IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            else -> return emptyList()
        }
        val uris = extra.ifEmpty {
            val clip = intent.clipData
            if (clip == null) emptyList() else (0 until clip.itemCount).mapNotNull { clip.getItemAt(it).uri }
        }
        return uris.filter { SharedUris.accepts(it.scheme, it.authority) }.distinct()
    }

    /**
     * What the provider says about [uri]: its display name
     * (`OpenableColumns.DISPLAY_NAME`), size (`OpenableColumns.SIZE`) and
     * type, with the intent's type when the provider gives none or just
     * "bytes".
     */
    fun describe(context: Context, uri: Uri, intentType: String?): SharedStream {
        val resolver = context.contentResolver
        val columns = arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        val (name, size) = try {
            resolver.query(uri, columns, null, null, null)?.use { c ->
                if (!c.moveToFirst()) return@use null
                val nameColumn = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                val sizeColumn = c.getColumnIndex(OpenableColumns.SIZE)
                val name = if (nameColumn >= 0 && !c.isNull(nameColumn)) c.getString(nameColumn) else null
                val size = if (sizeColumn >= 0 && !c.isNull(sizeColumn)) c.getLong(sizeColumn) else null
                name to size
            } ?: (null to null)
        } catch (e: Exception) {
            Log.w(TAG, "no name or size for a shared file: ${e.javaClass.simpleName}")
            null to null
        }
        val provided = try {
            resolver.getType(uri)
        } catch (e: Exception) {
            null
        }
        val type = provided?.takeUnless { it == SharedStream.OCTET_STREAM }
            ?: intentType?.takeUnless { it.endsWith("/*") }
            ?: provided
        return SharedStream(uri, name?.takeIf { it.isNotBlank() }, type, size?.takeIf { it >= 0 })
    }

    /** Start a session: its copies are safe from [deleteStale] until [close]. */
    fun open(): String = UUID.randomUUID().toString().also { synchronized(live) { live += it } }

    /** Delete the copies of [session] and end it. */
    fun close(context: Context, session: String) {
        File(root(context), session).deleteRecursively()
        synchronized(live) { live -= session }
    }

    /** Delete the copies of sessions that are over (left behind when the process died). */
    fun deleteStale(context: Context) {
        val stale = root(context).listFiles().orEmpty().filter { synchronized(live) { it.name !in live } }
        stale.forEach { it.deleteRecursively() }
    }

    /**
     * Copy [uri] to `<session>/<id>/<fileName>` with the read access the
     * sender granted, and return the copy. Throws [CopyFailed] with the
     * reason, [ReadProblem.TOO_LARGE] as soon as the stream passes
     * [ShareLimits.MAX_FILE_BYTES]; a partial copy is deleted. Stops
     * between blocks when the calling coroutine is cancelled.
     */
    suspend fun copy(context: Context, session: String, id: Int, uri: Uri, fileName: String): File {
        val dir = File(File(root(context), session), id.toString())
        val target = File(dir, fileName)
        try {
            if (!dir.isDirectory && !dir.mkdirs()) throw IOException("cannot make $dir")
            val input = context.contentResolver.openInputStream(uri) ?: throw FileNotFoundException("no stream")
            input.use { from ->
                target.outputStream().use { to -> ShareLimits.copy(from, to) }
            }
            return target
        } catch (e: Exception) {
            dir.deleteRecursively()
            val problem = when (e) {
                is TooLargeException -> ReadProblem.TOO_LARGE
                is SecurityException -> ReadProblem.NO_ACCESS
                is FileNotFoundException -> ReadProblem.GONE
                is CancellationException -> throw e
                else -> ReadProblem.FAILED
            }
            Log.w(TAG, "copying a shared file failed: ${e.javaClass.simpleName}: ${e.message}")
            throw CopyFailed(problem)
        }
    }

    /** Delete the copy at [path] (with its directory). */
    fun deleteCopy(path: String) {
        File(path).parentFile?.deleteRecursively()
    }

    /**
     * A preview of the copy at [path], at most about [sizePx] on its longer
     * side: the picture, or a video's first frame. Null when it cannot be
     * decoded.
     */
    fun preview(path: String, isVideo: Boolean, sizePx: Int): Bitmap? = try {
        if (isVideo) videoFrame(path, sizePx) else picture(path, sizePx)
    } catch (e: Exception) {
        Log.i(TAG, "no preview: ${e.javaClass.simpleName}")
        null
    }

    private fun root(context: Context) = File(context.cacheDir, DIR)

    private fun sampleSize(width: Int, height: Int, sizePx: Int): Int {
        var sample = 1
        while (maxOf(width, height) / (sample * 2) >= sizePx) sample *= 2
        return sample
    }

    /** [width] by [height] scaled down (never up) to at most [sizePx] on the longer side. */
    private fun fit(width: Int, height: Int, sizePx: Int): Pair<Int, Int> {
        val scale = sizePx.toFloat() / maxOf(width, height)
        if (scale >= 1f) return width to height
        return maxOf(1, (width * scale).toInt()) to maxOf(1, (height * scale).toInt())
    }

    private fun picture(path: String, sizePx: Int): Bitmap? {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            // ImageDecoder also turns a photo the way its EXIF data says.
            return ImageDecoder.decodeBitmap(ImageDecoder.createSource(File(path))) { decoder, info, _ ->
                val (w, h) = fit(info.size.width, info.size.height, sizePx)
                decoder.setTargetSize(w, h)
                decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
            }
        }
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(path, bounds)
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
        val options = BitmapFactory.Options().apply { inSampleSize = sampleSize(bounds.outWidth, bounds.outHeight, sizePx) }
        val bitmap = BitmapFactory.decodeFile(path, options) ?: return null
        val (w, h) = fit(bitmap.width, bitmap.height, sizePx)
        return if (w == bitmap.width) bitmap else bitmap.scale(w, h)
    }

    private fun videoFrame(path: String, sizePx: Int): Bitmap? {
        val retriever = MediaMetadataRetriever()
        try {
            retriever.setDataSource(path)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
                return retriever.getScaledFrameAtTime(0, MediaMetadataRetriever.OPTION_CLOSEST_SYNC, sizePx, sizePx)
            }
            val frame = retriever.getFrameAtTime(0, MediaMetadataRetriever.OPTION_CLOSEST_SYNC) ?: return null
            val (w, h) = fit(frame.width, frame.height, sizePx)
            return if (w == frame.width) frame else frame.scale(w, h)
        } finally {
            retriever.release()
        }
    }
}

/** Copying a shared file failed, for [problem]. */
class CopyFailed(val problem: ReadProblem) : Exception(problem.name)
