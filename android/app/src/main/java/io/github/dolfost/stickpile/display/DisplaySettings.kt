package io.github.dolfost.stickpile.display

import android.content.Context
import android.content.SharedPreferences
import androidx.core.content.edit
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Keeps the [Display] the app and the keyboard share, in the app's private
 * preferences. One per process ([io.github.dolfost.stickpile.StickpileApp.display]):
 * a change made in the keyboard shows in the app at once, and the other
 * way round, through [state].
 */
class DisplaySettings(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    private val mutable = MutableStateFlow(read(prefs))

    /** What the grids show now. */
    val state: StateFlow<Display> = mutable.asStateFlow()

    // Kept in a field: the preferences hold their listeners weakly.
    private val listener = SharedPreferences.OnSharedPreferenceChangeListener { p, _ -> mutable.value = read(p) }

    init {
        prefs.registerOnSharedPreferenceChangeListener(listener)
    }

    val current: Display get() = state.value

    fun setSafeMode(on: Boolean) = save(current.copy(safeMode = on))

    fun setShowInfo(on: Boolean) = save(current.copy(showInfo = on))

    fun setField(field: TileField, on: Boolean) = save(current.with(field, on))

    fun setTileSize(size: TileSize) = save(current.copy(tileSize = size))

    private fun save(display: Display) {
        mutable.value = display
        prefs.edit {
            putBoolean(SAFE_MODE, display.safeMode)
            putBoolean(SHOW_INFO, display.showInfo)
            putStringSet(HIDDEN_FIELDS, (TileField.entries - display.fields).map { it.name }.toSet())
            putString(TILE_SIZE, display.tileSize.name)
        }
    }

    private companion object {
        const val PREFS = "display"
        const val SAFE_MODE = "safe_mode"
        const val SHOW_INFO = "show_info"

        /** The fields turned off (so that a field added later starts on). */
        const val HIDDEN_FIELDS = "hidden_fields"
        const val TILE_SIZE = "tile_size"

        fun read(prefs: SharedPreferences): Display {
            val defaults = Display()
            val hidden = prefs.getStringSet(HIDDEN_FIELDS, emptySet()).orEmpty()
            return Display(
                safeMode = prefs.getBoolean(SAFE_MODE, defaults.safeMode),
                showInfo = prefs.getBoolean(SHOW_INFO, defaults.showInfo),
                fields = TileField.entries.filter { it.name !in hidden }.toSet(),
                tileSize = TileSize.entries.firstOrNull { it.name == prefs.getString(TILE_SIZE, null) }
                    ?: defaults.tileSize,
            )
        }
    }
}
