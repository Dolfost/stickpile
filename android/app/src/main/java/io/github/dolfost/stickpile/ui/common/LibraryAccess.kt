package io.github.dolfost.stickpile.ui.common

import android.content.Context
import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.rust.StickpileLibrary
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The app's one library, opened off the main thread: the first use loads
 * the native library, reads the config and opens the cache, and throws
 * when the config cannot be read (the next call tries again).
 */
suspend fun openLibrary(context: Context): Result<StickpileLibrary> {
    val app = StickpileApp.from(context)
    return withContext(Dispatchers.IO) { catching { app.library } }
}
