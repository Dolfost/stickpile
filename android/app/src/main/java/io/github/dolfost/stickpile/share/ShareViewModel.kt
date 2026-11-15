package io.github.dolfost.stickpile.share

import android.app.Application
import android.content.Context
import android.net.Uri
import android.util.Log
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.core.content.edit
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.Pack
import io.github.dolfost.stickpile.rust.UploadFile
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** What the share screen can do, before the files. */
sealed interface Setup {
    /** Reading the account and its sets. */
    data object Loading : Setup

    /** The intent carried no picture or video. */
    data object NothingShared : Setup

    /** No account: sign in in the app first. */
    data object SignedOut : Setup

    /** The account owns no set (that the library lists) on [serverUrl]. */
    data class NoOwnSets(val serverUrl: String) : Setup

    /** The account or the sets could not be read. */
    data class Failed(val failure: UploadFailure) : Setup

    /** The account's own [sets]; [offline]: from the offline cache. */
    data class Ready(val sets: List<Pack>, val offline: Boolean) : Setup
}

/** Everything the share screen shows. */
data class ShareState(
    val setup: Setup = Setup.Loading,
    /** The set to upload into. */
    val setId: String? = null,
    val batch: ShareBatch = ShareBatch(),
) {
    val set: Pack? get() = (setup as? Setup.Ready)?.sets?.firstOrNull { it.id == setId }
}

/**
 * The share screen's state and work. It outlives the activity across
 * rotation; its work runs in [viewModelScope] (not the activity's), with
 * every library call and file operation off the main thread. When the
 * screen goes for good, [onCleared] stops the work and deletes the copies.
 */
class ShareViewModel(application: Application) : AndroidViewModel(application) {
    private val app = getApplication<StickpileApp>()
    private val session = ShareFiles.open()
    private var started = false
    private var setupJob: Job? = null
    private var copyJob: Job? = null
    private var uploadJob: Job? = null

    /** The screen's state; changed on the main thread only. */
    var state by mutableStateOf(ShareState())
        private set

    /** The previews made so far, by file id. */
    val previews: Map<Int, ImageBitmap> get() = previewMap
    private val previewMap = mutableStateMapOf<Int, ImageBitmap>()

    /** Take the shared [streams] (from the intent of type [intentType]); only the first call counts. */
    fun start(streams: List<Uri>, intentType: String?) {
        if (started) return
        started = true
        viewModelScope.launch(Dispatchers.IO) { ShareFiles.deleteStale(app) }
        if (streams.isEmpty()) {
            state = state.copy(setup = Setup.NothingShared)
            return
        }
        copyJob = viewModelScope.launch { copyAll(streams, intentType) }
        loadSetup()
    }

    /** The screen is back: read the account and sets again if that may have changed outside. */
    fun onResume() {
        if (state.setup is Setup.SignedOut || state.setup is Setup.NoOwnSets) loadSetup()
    }

    /** Read the account and its sets (again). */
    fun loadSetup() {
        if (state.setup is Setup.NothingShared) return
        setupJob?.cancel()
        state = state.copy(setup = Setup.Loading)
        setupJob = viewModelScope.launch {
            val (setup, last) = withContext(Dispatchers.IO) { readSetup() to lastSet() }
            val sets = (setup as? Setup.Ready)?.sets.orEmpty()
            val setId = state.setId?.takeIf { id -> sets.any { it.id == id } }
                ?: last?.takeIf { id -> sets.any { it.id == id } }
                ?: sets.firstOrNull()?.id
            state = state.copy(setup = setup, setId = setId)
        }
    }

    /** The account's own sets, or why there are none to pick. Off the main thread: the first use opens the library. */
    private suspend fun readSetup(): Setup = try {
        val library = app.library
        if (library.account() == null) {
            Setup.SignedOut
        } else {
            // Own sets, empty ones included (packs() leaves those out).
            val list = library.ownPacks()
            if (list.packs.isEmpty()) Setup.NoOwnSets(library.serverUrl()) else Setup.Ready(list.packs, list.offline)
        }
    } catch (e: LibraryException) {
        Log.w(TAG, "reading the account or sets failed: ${e.javaClass.simpleName}: ${e.message}")
        when (e) {
            is LibraryException.NotSignedIn, is LibraryException.Unauthorized -> Setup.SignedOut
            else -> Setup.Failed(UploadFailure.of(e))
        }
    }

    fun selectSet(id: String) {
        if (!state.batch.running) state = state.copy(setId = id)
    }

    fun editName(id: Int, text: String) = changeBatch { it.edit(id, name = text) }
    fun editKeywords(id: Int, text: String) = changeBatch { it.edit(id, keywords = text) }
    fun editEmojis(id: Int, text: String) = changeBatch { it.edit(id, emojis = text) }
    fun keywordsToAll(fromId: Int) = changeBatch { it.keywordsToAll(fromId) }
    fun emojisToAll(fromId: Int) = changeBatch { it.emojisToAll(fromId) }

    /** Leave file [id] out of the share. */
    fun remove(id: Int) {
        val file = state.batch.file(id) ?: return
        val after = state.batch.remove(id)
        if (after.file(id) != null) return
        state = state.copy(batch = after)
        previewMap.remove(id)
        file.path?.let { path -> viewModelScope.launch(Dispatchers.IO) { ShareFiles.deleteCopy(path) } }
    }

    /** Upload the editable files ([ids] only, when given) into the chosen set, one at a time. */
    fun upload(ids: Collection<Int>? = null) {
        val setId = state.setId ?: return
        if (uploadJob?.isActive == true || state.batch.copying) return
        val batch = state.batch.startRun(ids)
        if (batch == state.batch) return
        state = state.copy(batch = batch)
        uploadJob = viewModelScope.launch {
            withContext(Dispatchers.IO) { rememberSet(setId) }
            while (true) {
                val file = state.batch.next() ?: break
                changeBatch { it.uploading(file.id) }
                val request = file.toUpload()
                if (request == null) {
                    changeBatch { it.failed(file.id, UploadFailure(LibraryErrorKind.IO)) }
                    continue
                }
                uploadOne(setId, file.id, request)
            }
        }
    }

    private suspend fun uploadOne(setId: String, id: Int, request: UploadFile) {
        try {
            val item = withContext(Dispatchers.IO) { app.library.upload(setId, request) }
            changeBatch { it.uploaded(id, item.name) }
            withContext(Dispatchers.IO) { ShareFiles.deleteCopy(request.path) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: LibraryException) {
            Log.w(TAG, "upload failed: ${e.javaClass.simpleName}: ${e.message}")
            changeBatch { it.failed(id, UploadFailure.of(e)) }
        } catch (e: Exception) {
            Log.w(TAG, "upload failed: ${e.javaClass.simpleName}: ${e.message}")
            changeBatch { it.failed(id, UploadFailure.OTHER) }
        }
    }

    /**
     * Stop the running upload: the file being sent may still arrive (the
     * library finishes a write it started), the queued ones are not sent.
     */
    fun stop() {
        uploadJob?.cancel()
    }

    override fun onCleared() {
        val jobs = listOfNotNull(uploadJob, copyJob, setupJob)
        app.scope.launch(Dispatchers.IO) {
            jobs.forEach { it.cancelAndJoin() }
            ShareFiles.close(app, session)
        }
    }

    private inline fun changeBatch(f: (ShareBatch) -> ShareBatch) {
        state = state.copy(batch = f(state.batch))
    }

    /** Look at every stream, then copy them one at a time, each with its preview. */
    private suspend fun copyAll(uris: List<Uri>, intentType: String?) {
        val streams = withContext(Dispatchers.IO) { uris.map { ShareFiles.describe(app, it, intentType) } }
        val files = streams.mapIndexed { id, s ->
            SharedFile(
                id = id,
                fileName = FileNames.forCopy(s.displayName, s.mimeType),
                isVideo = s.isVideo,
                name = FileNames.defaultItemName(s.displayName),
                status = if (s.notMedia) FileStatus.Unreadable(ReadProblem.NOT_MEDIA) else FileStatus.Copying,
            )
        }
        state = state.copy(batch = ShareBatch(files))
        for (file in files) {
            if (file.status != FileStatus.Copying) continue
            val stream = streams[file.id]
            try {
                val copy = withContext(Dispatchers.IO) { ShareFiles.copy(app, session, file.id, stream.uri, file.fileName) }
                if (state.batch.file(file.id) == null) {
                    // Removed while it was being copied.
                    withContext(Dispatchers.IO) { ShareFiles.deleteCopy(copy.path) }
                    continue
                }
                changeBatch { it.copied(file.id, copy.path) }
                val preview = withContext(Dispatchers.Default) {
                    ShareFiles.preview(copy.path, file.isVideo, PREVIEW_PX)?.asImageBitmap()
                }
                if (preview != null && state.batch.file(file.id) != null) previewMap[file.id] = preview
            } catch (e: CopyFailed) {
                changeBatch { it.unreadable(file.id, e.problem) }
            }
        }
    }

    private fun prefs() = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    private fun lastSet(): String? = prefs().getString(KEY_LAST_SET, null)

    private fun rememberSet(id: String) = prefs().edit { putString(KEY_LAST_SET, id) }

    private companion object {
        const val TAG = "stickpile.share"
        const val PREFS = "share"
        const val KEY_LAST_SET = "last_set"

        /** Previews are decoded to about this size (pixels, longer side). */
        const val PREVIEW_PX = 256
    }
}
