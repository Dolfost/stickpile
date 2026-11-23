package io.github.dolfost.stickpile

import android.app.Application
import android.content.Context
import android.os.SystemClock
import io.github.dolfost.stickpile.display.DisplaySettings
import io.github.dolfost.stickpile.ime.ThumbnailLoader
import io.github.dolfost.stickpile.motion.Motions
import io.github.dolfost.stickpile.offline.OfflineJobs
import io.github.dolfost.stickpile.offline.OfflineScheduler
import io.github.dolfost.stickpile.offline.OfflineSettings
import io.github.dolfost.stickpile.rust.LibraryDirs
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.StickpileLibrary
import io.github.dolfost.stickpile.sync.CacheChanges
import io.github.dolfost.stickpile.sync.NetworkWatch
import io.github.dolfost.stickpile.ui.library.Thumbnails
import java.io.File
import android.util.Log
import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * The application. It owns the one Rust [StickpileLibrary] of the process
 * (account, sync, offline cache, files), which the activities, the input
 * method, the provider and the sync worker all share, and a scope for work
 * that outlives a screen.
 *
 * Everything lives in `noBackupFilesDir`: never backed up or transferred
 * (the token is there), and not cleared by the system like `cacheDir`, so
 * the offline search keeps working, and the files kept offline stay. The
 * app's settings clear the files (those kept offline only when asked).
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
        ).also { lib ->
            // What is kept offline follows the app's settings from the start.
            val choices = offline.current
            lib.setOfflineOptions(choices.options)
            if (choices.paused) lib.pauseOffline()
            // A network that appears gets the server checked at once.
            NetworkWatch.start(this) { lib.checkServer() }
        }
    }

    private var backOnlineSync: Job? = null

    /**
     * The server answers again after it was away (the app or the keyboard
     * saw it, [io.github.dolfost.stickpile.sync.ServerWatch]): sync now, as
     * "Sync now" does, so what changed meanwhile comes in and the unsent
     * uses go out; one at a time, whoever saw it. Call on the main thread.
     */
    fun syncBackOnline() {
        if (backOnlineSync?.isActive == true) return
        backOnlineSync = scope.launch {
            try {
                cacheChanges.after(library.syncNow())
            } catch (e: LibraryException) {
                Log.i(TAG, "sync after the server came back failed: ${e.javaClass.simpleName}")
            }
        }
    }

    /** The options of keeping stickers offline, and whether the downloads are paused. */
    val offline: OfflineSettings by lazy { OfflineSettings(this) }

    /** The offline downloads' WorkManager job. */
    val offlineJobs: OfflineJobs by lazy { OfflineScheduler.jobs(this) }

    /** Run the offline downloads when the platform allows (after a keep or a sync), unless paused. */
    fun requestOfflineRun(restart: Boolean = false) {
        val choices = offline.current
        if (!choices.paused) offlineJobs.request(choices.options.mobileData, restart)
    }

    /**
     * For work that must finish even when the screen that started it goes.
     * An exception nobody caught (a Rust panic arrives as UniFFI's
     * `InternalException`) is logged instead of taking the process down,
     * which also hosts the keyboard and the provider.
     */
    val scope: CoroutineScope = CoroutineScope(
        SupervisorJob() + Dispatchers.Default + CoroutineExceptionHandler { _, e ->
            Log.e(TAG, "background work failed: ${e.javaClass.simpleName}")
        },
    )

    /**
     * The system is short of memory: the decoded pictures go, all of them
     * once nothing of the process is on screen ([MemoryTrim]).
     */
    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        val keep = MemoryTrim.keep(level)
        if (keep < 1f) {
            Thumbnails.trim(keep)
            ThumbnailLoader.trim(keep)
            Motions.trim(keep)
        }
    }

    /** What the grids show (safe mode, the tiles' information), shared by the app and the keyboard. */
    val display: DisplaySettings by lazy { DisplaySettings(this) }

    /**
     * The syncs that changed the offline cache, for the offers of the app
     * and the keyboard; each sync that went through runs the offline
     * downloads (new stickers of a kept pack, say).
     */
    val cacheChanges = CacheChanges(synced = { requestOfflineRun() })

    @Volatile
    private var likesReadAt = 0L

    /**
     * Read the account's favorites in the background (at most every few
     * minutes), so that the items' `liked` marks know them: the library
     * marks items from the favorites it last read. Returns the read, to
     * load the shown list again after it; null when one ran lately.
     * Failures are only logged; the marks then stay as they were.
     */
    fun refreshLikes(): Job? {
        val now = SystemClock.elapsedRealtime()
        if (likesReadAt != 0L && now - likesReadAt < LIKES_EVERY_MS) return null
        likesReadAt = now
        return scope.launch {
            try {
                library.favorites(1u)
            } catch (e: LibraryException) {
                likesReadAt = 0L
                Log.i(TAG, "favorites not read: ${e.javaClass.simpleName}")
            }
        }
    }

    companion object {
        private const val TAG = "stickpile"
        private const val LIKES_EVERY_MS = 5 * 60_000L

        /** The application object from any context. */
        fun from(context: Context): StickpileApp = context.applicationContext as StickpileApp
    }
}
