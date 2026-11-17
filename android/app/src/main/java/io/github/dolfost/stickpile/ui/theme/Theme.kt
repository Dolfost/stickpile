package io.github.dolfost.stickpile.ui.theme

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.platform.LocalContext
import io.github.dolfost.stickpile.StickpileApp
import io.github.dolfost.stickpile.display.AppTheme

/**
 * The Material 3 theme of every Compose screen: light or dark as the
 * activity's configuration says ([ThemedActivity] sets it from the
 * [AppTheme] chosen in the settings), in the wallpaper's colours (dynamic
 * colour) on Android 12 and later, in Material's baseline colours before.
 * [oled] puts the dark colours on pure black.
 */
@Composable
fun StickpileTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    oled: Boolean = chosenTheme() == AppTheme.OLED,
    dynamicColor: Boolean = true,
    content: @Composable () -> Unit,
) {
    val colorScheme = when {
        dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> {
            val context = LocalContext.current
            if (darkTheme) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        }
        darkTheme -> darkColorScheme()
        else -> lightColorScheme()
    }
    MaterialTheme(colorScheme = if (darkTheme && oled) colorScheme.onBlack() else colorScheme, content = content)
}

/** The theme chosen in the settings; [AppTheme.SYSTEM] outside the app (previews). */
@Composable
private fun chosenTheme(): AppTheme {
    val app = LocalContext.current.applicationContext as? StickpileApp ?: return AppTheme.SYSTEM
    val display by app.display.state.collectAsState()
    return display.theme
}

/**
 * A dark scheme on pure black, for OLED screens: the background and the
 * surfaces black, the containers (bars, cards, sheets) darker than in the
 * dark theme but still apart from the black.
 */
internal fun ColorScheme.onBlack(): ColorScheme = copy(
    background = Color.Black,
    surface = Color.Black,
    surfaceDim = Color.Black,
    surfaceContainerLowest = Color.Black,
    surfaceContainerLow = surfaceContainerLow.darker(),
    surfaceContainer = surfaceContainer.darker(),
    surfaceContainerHigh = surfaceContainerHigh.darker(),
    surfaceContainerHighest = surfaceContainerHighest.darker(),
)

private fun Color.darker(): Color = lerp(this, Color.Black, OLED_DARKER)

/** How far [onBlack] takes a container towards black. */
private const val OLED_DARKER = 0.4f
