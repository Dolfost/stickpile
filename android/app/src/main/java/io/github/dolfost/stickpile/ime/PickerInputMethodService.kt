package io.github.dolfost.stickpile.ime

import android.inputmethodservice.InputMethodService

/**
 * The picker keyboard: an input method with no typing keys that inserts
 * stickers with InputConnection.commitContent. Its views are classic Views
 * themed with R.style.Theme_Stickpile_Views. Stub: no input view.
 */
class PickerInputMethodService : InputMethodService()
