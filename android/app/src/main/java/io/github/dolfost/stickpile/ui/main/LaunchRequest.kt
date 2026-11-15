package io.github.dolfost.stickpile.ui.main

import android.content.Intent

/** What an intent that opens the launcher asks for, besides opening it. */
sealed interface LaunchRequest {
    /** Pair with this link (`stickpile://pair?...`), after asking. */
    data class OpenLink(val link: String) : LaunchRequest
}

/**
 * What the launcher's intent asks for: [data] (a pairing link); null for
 * nothing. An intent relaunched from Recents ([flags] has
 * `FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY`) is the task's first intent again,
 * whose pairing code is used or expired: it asks for nothing.
 */
fun launchRequest(data: String?, flags: Int): LaunchRequest? = when {
    flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0 -> null
    data != null -> LaunchRequest.OpenLink(data)
    else -> null
}
