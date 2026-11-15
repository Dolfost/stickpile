package io.github.dolfost.stickpile

import android.app.Activity
import android.os.Bundle
import android.widget.TextView
import io.github.dolfost.stickpile.rust.clientName

/** Phase 0 stub: proves the Rust library loads and answers. */
class MainActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(TextView(this).apply { text = clientName() })
    }
}
