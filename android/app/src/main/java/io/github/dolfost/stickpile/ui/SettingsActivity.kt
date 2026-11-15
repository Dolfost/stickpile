package io.github.dolfost.stickpile.ui

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.ui.Modifier
import io.github.dolfost.stickpile.ui.theme.StickpileTheme

/**
 * The app's settings, also opened by the system as the input method's
 * settings (res/xml/method.xml). Stub: an empty screen.
 */
class SettingsActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContent {
            StickpileTheme {
                Surface(Modifier.fillMaxSize()) {}
            }
        }
    }
}
