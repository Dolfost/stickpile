package io.github.dolfost.stickpile.ime

/**
 * The size to decode a picture at, so that a huge file (a hostile server's
 * "thumbnail", a shared photo) is never decoded at its full size: the
 * picker's thumbnails ([ThumbnailLoader]) and the share screen's previews
 * use it. Pure.
 */
object DecodeSize {
    /** The longest side of a thumbnail in the picker, whatever the cell. */
    const val MAX_THUMBNAIL_PX = 512

    /**
     * [width] by [height] scaled down (never up) to at most [maxPx] on the
     * longer side, keeping the aspect ratio (no side below 1); null when a
     * side is not positive.
     */
    fun fit(width: Int, height: Int, maxPx: Int): Pair<Int, Int>? {
        if (width <= 0 || height <= 0) return null
        val longer = maxOf(width, height)
        if (longer <= maxPx) return width to height
        // In whole numbers, so that the longer side is exactly maxPx.
        fun scaled(side: Int) = maxOf(1, (side.toLong() * maxPx / longer).toInt())
        return scaled(width) to scaled(height)
    }

    /**
     * The `BitmapFactory.Options.inSampleSize` for [width] by [height]: the
     * largest power of two that keeps the longer side at [maxPx] or more,
     * so the decoded picture is less than twice [maxPx] on its longer side
     * (then scaled down to [fit]); 1 for a size that is not positive.
     */
    fun sampleSize(width: Int, height: Int, maxPx: Int): Int {
        val longer = maxOf(width, height)
        if (longer <= 0 || maxPx <= 0) return 1
        var sample = 1
        while (longer / (sample * 2) >= maxPx) sample *= 2
        return sample
    }

    /** The longest side to decode a picker thumbnail at: no larger than its cell, at most [MAX_THUMBNAIL_PX]. */
    fun thumbnailPx(cellPx: Int): Int = cellPx.coerceIn(1, MAX_THUMBNAIL_PX)

    /** The most pixels decoded whole where nothing smaller can be asked for (a 4K frame, 33 MB). */
    const val MAX_WHOLE_PIXELS = 3840L * 2160L

    /**
     * Whether a picture of [width] by [height] may be decoded at its full
     * size (a video frame on API 26, which cannot be asked for smaller):
     * both known, positive, and at most [MAX_WHOLE_PIXELS].
     */
    fun wholeFrameFits(width: Int?, height: Int?): Boolean =
        width != null && height != null && width > 0 && height > 0 && width.toLong() * height <= MAX_WHOLE_PIXELS
}
