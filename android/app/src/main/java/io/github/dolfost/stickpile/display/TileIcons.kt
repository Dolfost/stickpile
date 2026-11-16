package io.github.dolfost.stickpile.display

import androidx.annotation.DrawableRes
import io.github.dolfost.stickpile.R

/**
 * The icons drawn over a tile, in the app and in the keyboard. Each has
 * its own dark outline (res/drawable/ic_tile_*), so it shows on any
 * sticker without a band or a box behind it; they are not tinted.
 */
object TileIcons {
    /** The icon of a kind badge ([ItemLabels.badge]): "GIF", "VIDEO", or another animation. */
    @DrawableRes
    fun badge(label: String): Int = when (label) {
        GIF -> R.drawable.ic_tile_gif
        VIDEO -> R.drawable.ic_tile_video
        else -> R.drawable.ic_tile_animation
    }

    /** The icon of a number in the stats line ([ItemLabels.stats]). */
    @DrawableRes
    fun stat(field: TileField): Int = when (field) {
        TileField.LIKES -> R.drawable.ic_tile_likes
        TileField.QUALITY -> R.drawable.ic_tile_quality
        else -> R.drawable.ic_tile_copies
    }

    @DrawableRes
    val NSFW: Int = R.drawable.ic_tile_nsfw

    /** The account liked the item. */
    @DrawableRes
    val LIKED: Int = R.drawable.ic_tile_heart

    private const val GIF = "GIF"
    private const val VIDEO = "VIDEO"
}
