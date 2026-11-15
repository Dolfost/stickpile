package io.github.dolfost.stickpile.motion

/**
 * A `TextureView` stretches the video over its whole size; these are the
 * scales that fit it instead, centred and whole, keeping its aspect ratio
 * (as `ImageView.ScaleType.FIT_CENTER` fits a picture). Pure.
 */
object VideoFit {
    /** The x and y scales of the view's transform; null while a size is unknown. */
    fun scale(viewWidth: Int, viewHeight: Int, videoWidth: Int, videoHeight: Int): Pair<Float, Float>? {
        if (viewWidth <= 0 || viewHeight <= 0 || videoWidth <= 0 || videoHeight <= 0) return null
        val fit = minOf(viewWidth.toFloat() / videoWidth, viewHeight.toFloat() / videoHeight)
        return (videoWidth * fit / viewWidth) to (videoHeight * fit / viewHeight)
    }
}
