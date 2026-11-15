package io.github.dolfost.stickpile.share

import android.content.Intent
import android.os.Bundle
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import io.github.dolfost.stickpile.ui.MainActivity
import io.github.dolfost.stickpile.ui.theme.StickpileTheme

/**
 * "Share -> stickpile" (ACTION_SEND and ACTION_SEND_MULTIPLE with images
 * and videos): uploads the shared files into one of the account's own sets
 * ([ShareScreen], [ShareViewModel]). The files are copied as soon as the
 * activity starts, while the sender's read grant holds.
 */
class ShareActivity : ComponentActivity() {
    private val model: ShareViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        val streams = try {
            ShareFiles.streams(intent)
        } catch (e: RuntimeException) {
            // A malformed intent (extras that do not unparcel): nothing shared.
            Log.w(TAG, "unreadable share intent: ${e.javaClass.simpleName}")
            emptyList()
        }
        model.start(streams, intent.type)
        setContent {
            StickpileTheme {
                ShareScreen(model, onOpenApp = ::openApp, onClose = ::finish)
            }
        }
    }

    override fun onResume() {
        super.onResume()
        model.onResume()
    }

    /** The app, in its own task, to sign in; this screen checks again when it is back. */
    private fun openApp() {
        startActivity(Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    private companion object {
        const val TAG = "stickpile.share"
    }
}
