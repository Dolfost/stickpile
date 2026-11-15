package io.github.dolfost.stickpile.ui.common

import java.net.URI
import java.net.URISyntaxException

/**
 * The host of a server address, to show where a sign-in goes: [url] as
 * the library normalised it (scheme, host, maybe a port and a path). An
 * internationalised host arrives as punycode (`xn--...`) and is shown as
 * it is, so that a look-alike name cannot pass for another. A user part
 * (`user@`) is never shown as the host. Null when [url] has no host.
 */
fun hostOf(url: String): String? {
    val uri = try {
        URI(url.trim())
    } catch (e: URISyntaxException) {
        return null
    }
    uri.host?.let { if (it.isNotEmpty()) return it }
    // java.net.URI has no host for a name it does not take as a server
    // name (one with an underscore, say): the authority without the user
    // part and the port.
    val authority = uri.rawAuthority?.substringAfterLast('@') ?: return null
    val host = if (authority.startsWith("[")) {
        authority.substringBefore(']') + "]"
    } else {
        authority.substringBefore(':')
    }
    return host.ifEmpty { null }
}

/** Whether [url] is plain `http://`: nothing sent to it is encrypted. */
fun isUnencrypted(url: String): Boolean = url.trim().startsWith("http://", ignoreCase = true)
