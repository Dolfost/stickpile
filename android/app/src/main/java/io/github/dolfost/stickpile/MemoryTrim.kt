package io.github.dolfost.stickpile

import android.content.ComponentCallbacks2

/**
 * How much of the pictures decoded in memory (the grids' thumbnails, the
 * keyboard's, the moving pictures) the process keeps when the system asks
 * it to trim its memory ([StickpileApp.onTrimMemory]).
 */
object MemoryTrim {
    /** The share of each cache to keep at [level]: 1 all of it, 0 none. */
    @Suppress("DEPRECATION") // the RUNNING_ levels: still sent before Android 14
    fun keep(level: Int): Float = when {
        // Nothing of the process is on screen, neither the app nor the keyboard.
        level >= ComponentCallbacks2.TRIM_MEMORY_BACKGROUND -> 0f
        // The app's screens went; the keyboard may still be open.
        level >= ComponentCallbacks2.TRIM_MEMORY_UI_HIDDEN -> 1f
        level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL -> 0f
        level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW -> 0.5f
        else -> 1f
    }
}
