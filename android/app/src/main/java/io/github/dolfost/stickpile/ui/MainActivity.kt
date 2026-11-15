package io.github.dolfost.stickpile.ui

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.ui.Modifier
import io.github.dolfost.stickpile.rust.clientName
import io.github.dolfost.stickpile.ui.theme.StickpileTheme

/**
 * The launcher: onboarding, then the library. Stub: shows the Rust
 * library's client name, which proves the native library loads and answers.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContent {
            StickpileTheme {
                Surface(Modifier.fillMaxSize()) {
                    Text(clientName(), Modifier.safeDrawingPadding())
                }
            }
        }
    }
}
