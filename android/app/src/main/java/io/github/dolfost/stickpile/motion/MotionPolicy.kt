package io.github.dolfost.stickpile.motion

import android.os.Build
import io.github.dolfost.stickpile.display.Autoplay
import io.github.dolfost.stickpile.rust.MediaKind
import io.github.dolfost.stickpile.rust.Motion
import io.github.dolfost.stickpile.rust.MotionKind

/** Whether a moving item plays, and where its file may come from. Pure. */
object MotionPolicy {
    /** Where the file to play may come from. */
    enum class Source {
        /** The cache, else downloaded. */
        ANY,

        /** The cache only: nothing is downloaded. */
        CACHED,
    }

    /**
     * How an item of [kind] plays with [autoplay] while the network is
     * [metered]; null when it shows its still. An animation plays as a
     * picture, which `ImageDecoder` animates from API 28 ([sdk]) only.
     */
    fun source(kind: MediaKind, autoplay: Autoplay, metered: Boolean, sdk: Int = Build.VERSION.SDK_INT): Source? = when {
        kind == MediaKind.STILL -> null
        kind == MediaKind.ANIMATION && sdk < Build.VERSION_CODES.P -> null
        autoplay == Autoplay.NEVER -> null
        autoplay == Autoplay.WIFI && metered -> Source.CACHED
        else -> Source.ANY
    }

    /** Whether [motion] can play here: a picture needs API 28. */
    fun playable(motion: Motion, sdk: Int = Build.VERSION.SDK_INT): Boolean =
        motion.kind == MotionKind.VIDEO || sdk >= Build.VERSION_CODES.P
}
