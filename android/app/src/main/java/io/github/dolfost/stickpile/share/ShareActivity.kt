package io.github.dolfost.stickpile.share

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.ui.Modifier
import io.github.dolfost.stickpile.ui.theme.StickpileTheme

/**
 * "Share -> stickpile" (ACTION_SEND and ACTION_SEND_MULTIPLE with images
 * and videos): uploads the files into a set. Stub: an empty screen that
 * reads nothing of what was shared.
 */
class ShareActivity : ComponentActivity() {
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
