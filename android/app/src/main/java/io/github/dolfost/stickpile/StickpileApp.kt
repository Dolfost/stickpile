package io.github.dolfost.stickpile

import android.app.Application
import android.content.Context
import io.github.dolfost.stickpile.rust.LibraryDirs
import io.github.dolfost.stickpile.rust.StickpileLibrary
import java.io.File
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob

/**
 * The application. It owns the one Rust [StickpileLibrary] of the process
 * (account, sync, offline cache, files), which the activities, the input
 * method, the provider and the sync worker all share, and a scope for work
 * that outlives a screen.
 *
 * Everything lives in `noBackupFilesDir`: never backed up or transferred
 * (the token is there), and not cleared by the system like `cacheDir`, so
 * the offline search keeps working. The app's settings clear the files.
 */
class StickpileApp : Application() {

    /**
     * The library, created on first use (the native library loads then;
     * opening it reads the config and opens the cache, so the first use is
     * better off the main thread). Throws [io.github.dolfost.stickpile.rust.LibraryException]
     * when the config file cannot be read as a config.
     */
    val library: StickpileLibrary by lazy {
        StickpileLibrary(
            LibraryDirs(
                configFile = File(noBackupFilesDir, "config.toml").path,
                cacheDir = File(noBackupFilesDir, "cache").path,
                tokenFile = File(noBackupFilesDir, "token").path,
            ),
        )
    }

    /** For work that must finish even when the screen that started it goes. */
    val scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    companion object {
        /** The application object from any context. */
        fun from(context: Context): StickpileApp = context.applicationContext as StickpileApp
    }
}
