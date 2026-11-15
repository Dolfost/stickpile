package io.github.dolfost.stickpile.ime

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Intent
import android.inputmethodservice.InputMethodService
import android.net.Uri
import android.os.Build
import android.util.Log
import android.view.ContextThemeWrapper
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import androidx.core.view.inputmethod.EditorInfoCompat
import androidx.core.view.inputmethod.InputConnectionCompat
import androidx.core.view.inputmethod.InputContentInfoCompat
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.provider.PreparedFiles
import io.github.dolfost.stickpile.provider.kind
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.Prepared
import io.github.dolfost.stickpile.ui.MainActivity
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The picker keyboard (docs/android-plan.md 4.7): an input method with no
 * typing keys that inserts stickers. Its view is [PickerView]; this class
 * owns the input connection, the insert ([InsertFlow]: `commitContent`,
 * else `ACTION_SEND` to the editor's app, else the clipboard) and the
 * switch back to the previous keyboard.
 *
 * It uses the app's one library in process ([ImeLibrary], never the
 * provider) and never waits for it on the main thread: everything runs in
 * [scope], which lives as long as the service.
 */
class PickerInputMethodService : InputMethodService() {
    private val scope: CoroutineScope = MainScope()
    private val app: StickpileApp get() = StickpileApp.from(this)
    private val library by lazy { ImeLibrary(app) }
    private val settings by lazy { ImeSettings(this) }

    private var picker: PickerView? = null
    private var insertJob: Job? = null

    /** The draft searched for in this field, until it is deleted. */
    private var draft: Draft? = null

    /** What the view's buttons and long presses do. */
    private val actions = object : PickerView.Actions {
        override fun pick(item: Item, sticker: Boolean, fromDraft: Boolean) = startInsert(item, sticker, fromDraft)
        override fun toggleLike(item: Item) = toggleLikeOf(item)
        override fun share(item: Item) = shareItem(item)
        override fun backToKeyboard() = switchBack()
        override fun openApp() = openTheApp()
    }

    override fun onCreateInputView(): View {
        picker?.dispose()
        val view = PickerView(ContextThemeWrapper(this, R.style.Theme_Stickpile_Views), scope, library, actions)
        picker = view
        return view.root
    }

    /** Never the full-screen extract mode (landscape): there is no text to edit here. */
    override fun onEvaluateFullscreenMode(): Boolean = false

    override fun onStartInputView(info: EditorInfo, restarting: Boolean) {
        super.onStartInputView(info, restarting)
        library.editorPackage = info.packageName.orEmpty()
        val read = readDraft(info)
        draft = InputRestart.draft(restarting, read, draft)
        picker?.start(read?.query, restarting)
        syncInBackground()
    }

    override fun onFinishInputView(finishingInput: Boolean) {
        // The connection the insert would use is going away.
        insertJob?.cancel()
        picker?.stop()
        super.onFinishInputView(finishingInput)
    }

    override fun onDestroy() {
        picker?.dispose()
        picker = null
        scope.cancel()
        super.onDestroy()
    }

    // -- what the view asks for ---------------------------------------------------

    private fun startInsert(item: Item, sticker: Boolean, fromDraft: Boolean) {
        if (insertJob?.isActive == true) return
        val editor = currentInputEditorInfo ?: return
        val editorPackage = editor.packageName.orEmpty()
        val fieldTypes = EditorInfoCompat.getContentMimeTypes(editor).toList()
        insertJob = scope.launch {
            picker?.setBusy(true)
            try {
                val outcome = insert(item, sticker, fromDraft, editor, editorPackage, fieldTypes)
                finish(item, editorPackage, outcome)
            } finally {
                picker?.setBusy(false)
            }
        }
    }

    private fun toggleLikeOf(item: Item) {
        scope.launch {
            try {
                picker?.likeChanged(item, library.toggleLike(item.id))
            } catch (e: LibraryException) {
                picker?.showError(e.kind)
            }
        }
    }

    private fun shareItem(item: Item) {
        val editorPackage = currentInputEditorInfo?.packageName.orEmpty()
        scope.launch {
            val prepared = try {
                library.prepare(item.id, editorPackage, InsertFlow.SHARE_TYPES, sticker = false)
            } catch (e: LibraryException) {
                picker?.showError(e.kind)
                return@launch
            }
            val uri = PreparedFiles.uriFor(this@PickerInputMethodService, prepared)
            val chooser = Intent.createChooser(sendIntent(prepared, uri), getString(R.string.ime_share_title))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            try {
                startActivity(chooser)
            } catch (_: ActivityNotFoundException) {
                picker?.showStatus(R.string.ime_error_no_app)
            }
        }
    }

    private fun openTheApp() {
        startActivity(Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    // -- inserting ---------------------------------------------------------------

    /** Runs [InsertFlow] to its end. */
    private suspend fun insert(
        item: Item,
        sticker: Boolean,
        fromDraft: Boolean,
        editor: EditorInfo,
        editorPackage: String,
        fieldTypes: List<String>,
    ): InsertOutcome<Prepared> {
        var step: InsertStep<Prepared> = InsertFlow.start(fieldTypes)
        while (true) {
            step = when (val s = step) {
                is InsertStep.PrepareForField ->
                    InsertFlow.afterFieldPrepared(prepare(item, editorPackage, s.mimeTypes, sticker))
                is InsertStep.Commit -> InsertFlow.afterCommit(s.prepared, commit(s.prepared, editor, fromDraft))
                InsertStep.PrepareForSharing -> InsertFlow.afterSharePrepared(
                    prepare(item, editorPackage, InsertFlow.SHARE_TYPES, sticker),
                    canSend = InsertFlow.canSendTo(editorPackage, packageName),
                )
                is InsertStep.Send -> InsertFlow.afterSend(s.prepared, send(s.prepared, editorPackage))
                is InsertStep.Copy -> InsertFlow.afterCopy(s.prepared, copy(s.prepared))
                is InsertStep.Done -> return s.outcome
            }
        }
    }

    private suspend fun prepare(
        item: Item,
        editorPackage: String,
        mimeTypes: List<String>,
        sticker: Boolean,
    ): PrepareResult<Prepared> = try {
        PrepareResult.Ready(library.prepare(item.id, editorPackage, mimeTypes, sticker))
    } catch (e: LibraryException) {
        PrepareResult.Error(e.kind)
    }

    /**
     * `commitContent` with the prepared file's URI; only when the field took
     * it, the draft it was found by is deleted. A field that refuses keeps
     * its text, and the flow goes on to sharing.
     */
    private fun commit(prepared: Prepared, editor: EditorInfo, fromDraft: Boolean): Boolean {
        val ic = currentInputConnection ?: return false
        val uri = PreparedFiles.uriFor(this, prepared)
        // Not while text is being composed.
        ic.finishComposingText()
        val content = InputContentInfoCompat(uri, ClipDescription(prepared.displayName, arrayOf(prepared.mimeType)), null)
        val committed = InputConnectionCompat.commitContent(
            ic,
            editor,
            content,
            InputConnectionCompat.INPUT_CONTENT_GRANT_READ_URI_PERMISSION,
            null,
        )
        if (committed && fromDraft && settings.deleteDraft) {
            ic.beginBatchEdit()
            try {
                deleteDraft(ic)
            } finally {
                ic.endBatchEdit()
            }
        }
        return committed
    }

    /**
     * Deletes the draft, if the field still holds just that (an editor that
     * put the picture into the text changed it: then nothing is deleted).
     */
    private fun deleteDraft(ic: InputConnection) {
        val expected = draft ?: return
        val now = DraftQuery.read(
            ic.getTextBeforeCursor(DraftQuery.READ_LENGTH, 0),
            ic.getSelectedText(0),
            ic.getTextAfterCursor(DraftQuery.READ_LENGTH, 0),
        )
        if (now != expected) return
        ic.deleteSurroundingText(now.before, now.after)
        if (now.selected > 0) ic.commitText("", 1)
        draft = null
    }

    /** `ACTION_SEND` to the editor's app; false when it takes none. */
    private fun send(prepared: Prepared, editorPackage: String): Boolean {
        val intent = sendIntent(prepared, PreparedFiles.uriFor(this, prepared))
            .setPackage(editorPackage)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return try {
            startActivity(intent)
            true
        } catch (_: ActivityNotFoundException) {
            false
        } catch (e: SecurityException) {
            Log.w(TAG, "send refused: ${e.javaClass.simpleName}")
            false
        }
    }

    /** The URI on the clipboard; false when that did not work. */
    private fun copy(prepared: Prepared): Boolean {
        val clipboard = getSystemService(ClipboardManager::class.java) ?: return false
        return try {
            val uri = PreparedFiles.uriFor(this, prepared)
            clipboard.setPrimaryClip(ClipData.newUri(contentResolver, prepared.displayName, uri))
            true
        } catch (e: RuntimeException) {
            Log.w(TAG, "copy failed: ${e.javaClass.simpleName}")
            false
        }
    }

    private fun finish(item: Item, editorPackage: String, outcome: InsertOutcome<Prepared>) {
        InsertFlow.used(outcome)?.let { recordUse(item.id, it.variant, editorPackage) }
        when (outcome) {
            is InsertOutcome.Committed, is InsertOutcome.Sent -> if (!settings.stayOpen) switchBack()
            is InsertOutcome.Copied -> picker?.showStatus(R.string.ime_copied)
            is InsertOutcome.Failed -> picker?.showError(outcome.kind)
        }
    }

    /**
     * Records the use in the app's scope: it must not stop when the picker
     * closes, which it does right after.
     */
    private fun recordUse(itemId: String, variant: String, editorPackage: String) {
        val library = library
        app.scope.launch {
            try {
                library.recordUse(itemId, variant, editorPackage)
            } catch (e: LibraryException) {
                Log.w(TAG, "use not recorded: ${e.kind}")
                withContext(Dispatchers.Main) { picker?.showError(e.kind) }
            }
        }
    }

    // -- helpers -----------------------------------------------------------------

    private fun sendIntent(prepared: Prepared, uri: Uri): Intent = Intent(Intent.ACTION_SEND).apply {
        type = prepared.mimeType
        putExtra(Intent.EXTRA_STREAM, uri)
        clipData = ClipData.newRawUri(prepared.displayName, uri)
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }

    /** The draft in the field, when searching it is on and the field is not a password. */
    private fun readDraft(info: EditorInfo): Draft? {
        if (!settings.searchDraft || DraftQuery.isSensitive(info.inputType)) return null
        val ic = currentInputConnection ?: return null
        return DraftQuery.read(
            ic.getTextBeforeCursor(DraftQuery.READ_LENGTH, 0),
            ic.getSelectedText(0),
            ic.getTextAfterCursor(DraftQuery.READ_LENGTH, 0),
        )
    }

    /** Back to the keyboard used before; the system's keyboard list when there is none. */
    private fun switchBack() {
        val switched = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            switchToPreviousInputMethod()
        } else {
            switchToLastInputMethodBefore28()
        }
        if (!switched) inputMethodManager().showInputMethodPicker()
    }

    /** Below API 28: the InputMethodManager call, with the window's token. */
    @Suppress("DEPRECATION")
    private fun switchToLastInputMethodBefore28(): Boolean {
        val token = window?.window?.attributes?.token ?: return false
        return inputMethodManager().switchToLastInputMethod(token)
    }

    private fun inputMethodManager(): InputMethodManager = getSystemService(InputMethodManager::class.java)

    /** A sync when one is due; failures are the sync state's, only logged here. */
    private fun syncInBackground() {
        val library = library
        app.scope.launch {
            try {
                library.syncIfDue()
            } catch (e: LibraryException) {
                Log.i(TAG, "sync failed: ${e.kind}")
            }
        }
    }

    private companion object {
        const val TAG = "stickpile.ime"
    }
}
