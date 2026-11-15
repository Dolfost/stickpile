package io.github.dolfost.stickpile.ime

import android.content.Context
import androidx.core.content.edit

/**
 * The picker keyboard's settings (docs/android-plan.md 4.7), shown on the
 * app's settings screen and read by [PickerInputMethodService]. Stored in
 * the app's private preferences.
 */
class ImeSettings(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** Search for the draft already in the text field (one line, at most 60 characters). */
    var searchDraft: Boolean
        get() = prefs.getBoolean(SEARCH_DRAFT, true)
        set(value) = prefs.edit { putBoolean(SEARCH_DRAFT, value) }

    /** Delete that draft when a result found from it is inserted. */
    var deleteDraft: Boolean
        get() = prefs.getBoolean(DELETE_DRAFT, true)
        set(value) = prefs.edit { putBoolean(DELETE_DRAFT, value) }

    /** Stay open after inserting, instead of switching back to the previous keyboard. */
    var stayOpen: Boolean
        get() = prefs.getBoolean(STAY_OPEN, false)
        set(value) = prefs.edit { putBoolean(STAY_OPEN, value) }

    /** The letters of the picker's search keys ([KeyLayout.tag]); null until one is chosen. */
    var searchLayout: String?
        get() = prefs.getString(SEARCH_LAYOUT, null)
        set(value) = prefs.edit { putString(SEARCH_LAYOUT, value) }

    private companion object {
        const val PREFS = "ime_settings"
        const val SEARCH_DRAFT = "search_draft"
        const val DELETE_DRAFT = "delete_draft"
        const val STAY_OPEN = "stay_open"
        const val SEARCH_LAYOUT = "search_layout"
    }
}
