package io.github.dolfost.stickpile.offline

import android.content.Context
import android.util.Log
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters
import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.rust.LibraryException

/**
 * The offline downloads (WorkManager, default initializer; scheduled by
 * [OfflineScheduler], like the background sync's
 * [io.github.dolfost.stickpile.sync.SyncWorker]). One run is
 * `StickpileLibrary.offlineRun`: the library decides what to download and
 * in which order; this only lets it run when the platform allows (the
 * job's constraints) and not while paused. WorkManager stopping the job
 * cancels the coroutine, which stops the run cleanly. A run that ended
 * offline, at the budget or with full storage waits for the next request
 * (the next sync, keep or change of the options); a request that came
 * during a run makes it run again ([OfflineScheduler.goesOn]).
 */
class OfflineWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val app = StickpileApp.from(applicationContext)
        if (app.offline.current.paused) return Result.success()
        var runs = 0
        try {
            val library = app.library
            do {
                OfflineScheduler.takeRequest()
                val outcome = library.offlineRun()
                runs++
                Log.i(TAG, "offline run: ${outcome.end}, ${outcome.status.filesDone}/${outcome.status.filesTotal} files")
            } while (OfflineScheduler.goesOn(outcome.end, OfflineScheduler.takeRequest(), runs))
        } catch (e: LibraryException) {
            Log.w(TAG, "offline run failed: ${e.javaClass.simpleName}: ${e.message}")
        }
        return Result.success()
    }

    private companion object {
        const val TAG = "stickpile.offline"
    }
}
