package io.github.dolfost.stickpile.ui.theme

import android.content.Context
import android.content.res.Configuration
import android.os.Bundle
import android.view.ContextThemeWrapper
import androidx.activity.ComponentActivity
import androidx.lifecycle.lifecycleScope
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.display.AppTheme
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

/**
 * An activity in the [AppTheme] chosen in the settings. Its resources are
 * night or not as the theme says, so the window, the system bars
 * (enableEdgeToEdge) and [StickpileTheme] agree; a change of the theme
 * makes it again.
 */
abstract class ThemedActivity : ComponentActivity() {
    private var shown = AppTheme.SYSTEM

    override fun attachBaseContext(newBase: Context) {
        super.attachBaseContext(newBase)
        shown = StickpileApp.from(newBase).display.current.theme
        AppThemes.override(shown)?.let(::applyOverrideConfiguration)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        lifecycleScope.launch {
            StickpileApp.from(this@ThemedActivity).display.state.first { it.theme != shown }
            recreate()
        }
    }
}

/** What an [AppTheme] changes in a context. */
object AppThemes {
    /**
     * The configuration to lay over a context's own so that it is night or
     * not as [theme] says; null for [AppTheme.SYSTEM], which keeps the
     * phone's. Only the night bits are set: everything else stays the
     * phone's (the font scale is unset, as 0).
     */
    fun override(theme: AppTheme): Configuration? {
        val night = theme.night ?: return null
        return Configuration().apply {
            fontScale = 0f
            uiMode = if (night) Configuration.UI_MODE_NIGHT_YES else Configuration.UI_MODE_NIGHT_NO
        }
    }

    /** The keyboard's context: Theme.Stickpile.Views in [chosen], on pure black for OLED. */
    fun views(base: Context, chosen: AppTheme): Context =
        ContextThemeWrapper(base, R.style.Theme_Stickpile_Views).apply {
            override(chosen)?.let(::applyOverrideConfiguration)
            if (chosen == AppTheme.OLED) theme.applyStyle(R.style.ThemeOverlay_Stickpile_Oled, true)
        }
}
