package io.github.dolfost.stickpile.whatsapp

import android.util.Log
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.Pack
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import io.github.dolfost.stickpile.rust.WhatsappPart
import io.github.dolfost.stickpile.rust.WhatsappStep
import io.github.dolfost.stickpile.ui.common.Doing
import io.github.dolfost.stickpile.ui.common.Failure
import io.github.dolfost.stickpile.ui.common.FailureLog
import io.github.dolfost.stickpile.ui.common.Message
import io.github.dolfost.stickpile.ui.common.catching
import io.github.dolfost.stickpile.ui.common.messageFor
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** A part of the pack in the sheet, and what became of it. */
data class PartRow(val part: WhatsappPart, val status: PartStatus = PartStatus.NOT_ADDED)

/** What the "Add to WhatsApp" sheet shows. */
sealed interface WhatsAppState {
    /** No sheet. */
    data object Closed : WhatsAppState

    /**
     * The library is at [step]: the server builds the packs, or part
     * [part] of [parts] downloads (both 0 while building).
     */
    data class Working(val pack: Pack, val step: WhatsappStep, val part: Int = 0, val parts: Int = 0) : WhatsAppState

    /**
     * The parts are on the phone, each added on its own. [offline]: the
     * server could not be reached and these were made before. [skipped]:
     * items the server could not make into stickers. [notice]: what the
     * last tap on Add led to, when it is not a part's status (WhatsApp is
     * not installed).
     */
    data class Ready(
        val pack: Pack,
        val parts: List<PartRow>,
        val offline: Boolean = false,
        val skipped: Int = 0,
        val notice: Message? = null,
    ) : WhatsAppState

    data class Failed(val pack: Pack, val message: Message) : WhatsAppState
}

/** What the screen does once: open WhatsApp's dialog for a part. */
sealed interface WhatsAppEvent {
    data class Launch(val request: AddRequest) : WhatsAppEvent
}

/**
 * "Add to WhatsApp" for a pack (docs/android.md, "Add to WhatsApp"): the
 * library makes it ready ([StickpileLibraryInterface.whatsappPacks],
 * watched through `whatsappProgress`), and each part is then added on its
 * own with WhatsApp's dialog, as WhatsApp asks of sticker apps ("each pack
 * must have its own affordance to add it to WhatsApp (do not try to
 * create 'add all packs' operations)", its README). A pack of one part
 * opens the dialog straight away: the tap on "Add to WhatsApp" was for it.
 * [apps] tells which WhatsApp apps are installed and what they have
 * (called on [io]); [trace] hears each request, to show in the log.
 */
class WhatsAppViewModel(
    private val library: StickpileLibraryInterface,
    private val apps: WhatsAppApps,
    private val io: CoroutineDispatcher = Dispatchers.IO,
    private val log: FailureLog = FailureLog.android,
    private val trace: (String) -> Unit = { Log.i(FailureLog.TAG, it) },
) : ViewModel() {

    var state: WhatsAppState by mutableStateOf(WhatsAppState.Closed)
        private set

    private val events = Channel<WhatsAppEvent>(Channel.BUFFERED)

    /** The dialogs to open, each once. */
    val eventFlow: Flow<WhatsAppEvent> = events.receiveAsFlow()

    private var work: Job? = null

    /** Make [pack] ready for WhatsApp, showing how far it is; again for the same pack, a retry. */
    fun open(pack: Pack) {
        work?.cancel()
        state = WhatsAppState.Working(pack, WhatsappStep.BUILDING)
        work = viewModelScope.launch {
            val watching = launch { watch(pack) }
            val result = catching { library.whatsappPacks(pack.id) }
            watching.cancel()
            result.fold(
                onSuccess = { packs ->
                    val rows = withContext(io) {
                        packs.parts.map { part ->
                            val added = decide(apps.presence(part.identifier)) == AddDecision.AlreadyAdded
                            PartRow(part, if (added) PartStatus.ALREADY_ADDED else PartStatus.NOT_ADDED)
                        }
                    }
                    state = WhatsAppState.Ready(pack, rows, packs.offline, packs.skipped.toInt())
                    rows.singleOrNull()?.takeIf { it.status == PartStatus.NOT_ADDED }?.let { add(it.part.identifier) }
                },
                onFailure = { e ->
                    log.failed(Doing.WHATSAPP, e)
                    state = WhatsAppState.Failed(pack, whatsAppMessage(Failure.of(e)))
                },
            )
        }
    }

    /** Try [open]'s pack again. */
    fun retry() {
        val pack = when (val s = state) {
            is WhatsAppState.Failed -> s.pack
            is WhatsAppState.Ready -> s.pack
            else -> return
        }
        open(pack)
    }

    /**
     * Ask WhatsApp to add the part [identifier]: its dialog, aimed at the
     * app that lacks it (a chooser when both do); nothing when every
     * installed app has it, a notice when none is installed. One dialog at
     * a time.
     */
    fun add(identifier: String) {
        val ready = state as? WhatsAppState.Ready ?: return
        if (ready.parts.any { it.status == PartStatus.ADDING }) return
        val row = ready.parts.firstOrNull { it.part.identifier == identifier } ?: return
        setStatus(identifier, PartStatus.ADDING, notice = null)
        viewModelScope.launch {
            val decision = withContext(io) { decide(apps.presence(identifier)) }
            val request = AddRequest(identifier, row.part.name)
            when (decision) {
                AddDecision.NotInstalled -> {
                    trace("WhatsApp is not installed; would ask it: $request")
                    setStatus(identifier, row.status, notice = Message(R.string.whatsapp_not_installed))
                }
                AddDecision.AlreadyAdded -> setStatus(identifier, PartStatus.ALREADY_ADDED, notice = null)
                AddDecision.Chooser -> launchDialog(request)
                is AddDecision.Only -> launchDialog(request.copy(packageName = decision.packageName))
            }
        }
    }

    /** WhatsApp's dialog for the part being added returned [resultCode]. */
    fun added(resultCode: Int, validationError: String?) {
        val ready = state as? WhatsAppState.Ready ?: return
        val row = ready.parts.firstOrNull { it.status == PartStatus.ADDING } ?: return
        val status = statusAfter(resultCode, validationError)
        if (status == PartStatus.REFUSED) trace("WhatsApp refused ${row.part.identifier}: $validationError")
        setStatus(row.part.identifier, status, notice = null)
    }

    /** No app took the intent (WhatsApp went meanwhile). */
    fun notStarted() {
        val ready = state as? WhatsAppState.Ready ?: return
        val row = ready.parts.firstOrNull { it.status == PartStatus.ADDING } ?: return
        setStatus(row.part.identifier, PartStatus.NOT_ADDED, notice = Message(R.string.whatsapp_not_installed))
    }

    /** The sheet went: a build under way is no longer waited for (the server goes on with it). */
    fun close() {
        work?.cancel()
        state = WhatsAppState.Closed
    }

    private suspend fun launchDialog(request: AddRequest) {
        trace("asking WhatsApp: $request")
        events.send(WhatsAppEvent.Launch(request))
    }

    /** Follow the library's progress while [pack] is made ready. */
    private suspend fun watch(pack: Pack) {
        while (true) {
            delay(PROGRESS_EVERY_MS)
            val progress = withContext(io) { catching { library.whatsappProgress(pack.id) }.getOrNull() } ?: continue
            if (state is WhatsAppState.Working) {
                state = WhatsAppState.Working(pack, progress.step, progress.part.toInt(), progress.parts.toInt())
            }
        }
    }

    private fun setStatus(identifier: String, status: PartStatus, notice: Message?) {
        val ready = state as? WhatsAppState.Ready ?: return
        state = ready.copy(
            parts = ready.parts.map { if (it.part.identifier == identifier) it.copy(status = status) else it },
            notice = notice,
        )
    }

    companion object {
        /** How often the progress is read while the library works. */
        const val PROGRESS_EVERY_MS = 300L
    }
}

/**
 * The text for a failed [StickpileLibraryInterface.whatsappPacks]: its own
 * for a pack that is gone, a server too old to make WhatsApp packs (404 on
 * their routes), a pack with too few stickers, no server and nothing made
 * before, and a build that failed; the usual ones otherwise.
 */
fun whatsAppMessage(failure: Failure): Message = when {
    failure.kind == LibraryErrorKind.NOT_FOUND -> Message(R.string.whatsapp_error_pack_gone)
    failure.kind == LibraryErrorKind.SERVER && failure.status == 404 -> Message(R.string.whatsapp_error_server_too_old)
    failure.kind == LibraryErrorKind.INVALID_INPUT && failure.field == "pack" -> Message(R.string.whatsapp_error_too_small)
    failure.kind == LibraryErrorKind.UNREACHABLE -> Message(R.string.whatsapp_error_offline)
    failure.kind == LibraryErrorKind.OTHER -> Message(R.string.whatsapp_error_failed)
    else -> messageFor(failure, Doing.WHATSAPP)
}
