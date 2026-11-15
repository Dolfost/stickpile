package io.github.dolfost.stickpile.share

import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.UploadFile
import io.github.dolfost.stickpile.sync.kind

/** Why a shared file could not be copied for upload. */
enum class ReadProblem {
    /** Its type is neither a picture nor a video. */
    NOT_MEDIA,

    /** The sender did not let stickpile read it (no grant). */
    NO_ACCESS,

    /** It is not there any more. */
    GONE,

    /** Reading or copying it failed otherwise (storage full, say). */
    FAILED,
}

/** Why an upload failed: the library error's kind, and the HTTP status for `SERVER`. */
data class UploadFailure(val kind: LibraryErrorKind, val status: Int? = null) {
    /**
     * The files after this one would fail the same way (signed out, the
     * set gone, the server unreachable or refusing requests), so the run
     * stops instead of trying each of them.
     */
    val stopsRun: Boolean get() = kind in RUN_STOPPERS

    companion object {
        private val RUN_STOPPERS = setOf(
            LibraryErrorKind.NOT_SIGNED_IN,
            LibraryErrorKind.UNAUTHORIZED,
            LibraryErrorKind.NOT_FOUND,
            LibraryErrorKind.UNREACHABLE,
            LibraryErrorKind.RATE_LIMITED,
        )

        /** The failure a library call threw. */
        fun of(e: LibraryException): UploadFailure =
            UploadFailure(e.kind(), (e as? LibraryException.Server)?.status?.toInt())

        /** A failure that is not a library error. */
        val OTHER = UploadFailure(LibraryErrorKind.OTHER)
    }
}

/** Where one shared file stands. */
sealed interface FileStatus {
    /** Being copied to a temporary file. */
    data object Copying : FileStatus

    /** Could not be copied; it is left out. */
    data class Unreadable(val problem: ReadProblem) : FileStatus

    /** Copied; can be edited and uploaded. */
    data object Ready : FileStatus

    /** Waiting for its turn in the running upload. */
    data object Queued : FileStatus

    /** Being uploaded. */
    data object Uploading : FileStatus

    /** In the set now, as [itemName]. */
    data class Uploaded(val itemName: String) : FileStatus

    /** The upload failed; can be edited and uploaded again. */
    data class Failed(val failure: UploadFailure) : FileStatus
}

/**
 * One shared file: its temporary copy, what the user typed for it, and
 * where it stands.
 */
data class SharedFile(
    /** Its position in the share, a stable key. */
    val id: Int,
    /** The copy's file name ([FileNames.forCopy]). */
    val fileName: String,
    val isVideo: Boolean,
    /** The copy, once made. */
    val path: String? = null,
    /** The name field. Blank: the server names the item after [fileName]. */
    val name: String = "",
    /** The keywords field, as typed. */
    val keywords: String = "",
    /** The emojis field, as typed. */
    val emojis: String = "",
    val status: FileStatus = FileStatus.Copying,
) {
    /** The user may change its fields and upload it. */
    val editable: Boolean get() = status == FileStatus.Ready || status is FileStatus.Failed

    /** What the progress line calls it: the name, else the file name. */
    val label: String get() = name.trim().ifEmpty { fileName }

    /** The library's upload request for it; null while there is no copy. */
    fun toUpload(): UploadFile? = path?.let {
        UploadFile(
            path = it,
            name = name.trim().ifEmpty { null },
            keywords = TagFields.keywords(keywords),
            emojis = TagFields.emojis(emojis),
        )
    }
}

/** "File [position] of [total]: [name]", while an upload runs. */
data class UploadProgress(val position: Int, val total: Int, val name: String)

/** How the share went, once an upload has run. */
data class UploadSummary(val uploaded: Int, val failed: Int, val unreadable: Int, val total: Int) {
    /** Everything that could be read is in the set. */
    val complete: Boolean get() = failed == 0 && uploaded > 0 && uploaded + unreadable == total
}

/**
 * The files of one share and the upload runs over them (pure, no
 * Android, immutable: every change returns a new batch). A run uploads
 * the files that were ready or had failed when it started, one at a time
 * in share order.
 */
data class ShareBatch(
    val files: List<SharedFile> = emptyList(),
    /** The ids of the files in the current or last run, in upload order. */
    val run: List<Int> = emptyList(),
) {
    /** A run is going: some file of it is queued or uploading. */
    val running: Boolean
        get() = files.any { it.id in run && (it.status == FileStatus.Queued || it.status == FileStatus.Uploading) }

    /** Some file is still being copied. */
    val copying: Boolean get() = files.any { it.status == FileStatus.Copying }

    /** The files a new run would upload. */
    val uploadable: List<SharedFile> get() = files.filter { it.editable }

    /** Every uploadable file failed before (the next run is a retry). */
    val onlyRetries: Boolean get() = uploadable.isNotEmpty() && uploadable.all { it.status is FileStatus.Failed }

    fun file(id: Int): SharedFile? = files.firstOrNull { it.id == id }

    /** The progress of the running upload; null when none runs. */
    val progress: UploadProgress?
        get() {
            if (!running) return null
            val inRun = run.mapNotNull(::file)
            val done = inRun.count { it.status is FileStatus.Uploaded || it.status is FileStatus.Failed }
            val current = inRun.firstOrNull { it.status == FileStatus.Uploading }
                ?: inRun.first { it.status == FileStatus.Queued }
            return UploadProgress(position = done + 1, total = inRun.size, name = current.label)
        }

    /** The outcome over all files, after a run ended; null before one and while one runs. */
    val summary: UploadSummary?
        get() = if (run.isEmpty() || running) {
            null
        } else {
            UploadSummary(
                uploaded = files.count { it.status is FileStatus.Uploaded },
                failed = files.count { it.status is FileStatus.Failed },
                unreadable = files.count { it.status is FileStatus.Unreadable },
                total = files.size,
            )
        }

    private fun change(id: Int, f: (SharedFile) -> SharedFile): ShareBatch =
        copy(files = files.map { if (it.id == id) f(it) else it })

    /** The file [id] was copied to [path]. */
    fun copied(id: Int, path: String): ShareBatch =
        change(id) { if (it.status == FileStatus.Copying) it.copy(path = path, status = FileStatus.Ready) else it }

    /** The file [id] could not be copied. */
    fun unreadable(id: Int, problem: ReadProblem): ShareBatch =
        change(id) { if (it.status == FileStatus.Copying) it.copy(status = FileStatus.Unreadable(problem)) else it }

    /** The user typed in the fields of file [id] (null: that field unchanged); only while it is [SharedFile.editable]. */
    fun edit(id: Int, name: String? = null, keywords: String? = null, emojis: String? = null): ShareBatch =
        change(id) {
            if (!it.editable) {
                it
            } else {
                it.copy(name = name ?: it.name, keywords = keywords ?: it.keywords, emojis = emojis ?: it.emojis)
            }
        }

    /** Give every other editable file the keywords of file [fromId]. */
    fun keywordsToAll(fromId: Int): ShareBatch {
        val text = file(fromId)?.keywords ?: return this
        return copy(files = files.map { if (it.editable) it.copy(keywords = text) else it })
    }

    /** Give every other editable file the emojis of file [fromId]. */
    fun emojisToAll(fromId: Int): ShareBatch {
        val text = file(fromId)?.emojis ?: return this
        return copy(files = files.map { if (it.editable) it.copy(emojis = text) else it })
    }

    /** Leave file [id] out; not while it is queued or uploading. */
    fun remove(id: Int): ShareBatch {
        val f = file(id) ?: return this
        if (f.status == FileStatus.Queued || f.status == FileStatus.Uploading) return this
        return copy(files = files - f, run = run - id)
    }

    /**
     * Start a run over the editable files ([ids] only, when given): they
     * are queued. Nothing changes while a run is going or when no file
     * would be uploaded.
     */
    fun startRun(ids: Collection<Int>? = null): ShareBatch {
        if (running) return this
        val chosen = files.filter { it.editable && (ids == null || it.id in ids) }.map { it.id }
        if (chosen.isEmpty()) return this
        return copy(
            files = files.map { if (it.id in chosen) it.copy(status = FileStatus.Queued) else it },
            run = chosen,
        )
    }

    /** The file to upload next, in run order; null when the run is over. */
    fun next(): SharedFile? = run.firstNotNullOfOrNull { id -> file(id)?.takeIf { it.status == FileStatus.Queued } }

    /** File [id] is being uploaded. */
    fun uploading(id: Int): ShareBatch =
        change(id) { if (it.status == FileStatus.Queued) it.copy(status = FileStatus.Uploading) else it }

    /** File [id] is in the set now, as [itemName]; its copy is no longer needed. */
    fun uploaded(id: Int, itemName: String): ShareBatch =
        change(id) { it.copy(status = FileStatus.Uploaded(itemName), path = null) }

    /**
     * Uploading file [id] failed. When the [failure] [stops the run][UploadFailure.stopsRun],
     * the files still queued fail with it too.
     */
    fun failed(id: Int, failure: UploadFailure): ShareBatch {
        val failed = FileStatus.Failed(failure)
        return copy(
            files = files.map {
                when {
                    it.id == id -> it.copy(status = failed)
                    failure.stopsRun && it.id in run && it.status == FileStatus.Queued -> it.copy(status = failed)
                    else -> it
                }
            },
        )
    }
}
