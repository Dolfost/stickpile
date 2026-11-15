package io.github.dolfost.stickpile.share

import io.github.dolfost.stickpile.provider.ProviderRoutes

/** Which shared URIs the share target reads at all. Pure: [ShareFiles] passes the URI's parts. */
object SharedUris {
    /**
     * Whether a shared URI is read: a `content:` URI (not `file:`, which
     * could name this app's own files) of another app's provider. The app's
     * own provider is never read: a share would otherwise make the app read
     * its own stickers, or a prepared file, with its own rights.
     *
     * @param scheme `Uri.getScheme`.
     * @param authority `Uri.getAuthority` (decoded).
     */
    fun accepts(scheme: String?, authority: String?): Boolean =
        scheme == SCHEME_CONTENT && !isOwnProvider(authority)

    /**
     * Whether [authority] names this app's provider: also with a `<user>@`
     * in front (a content URI may carry the user, which the system strips
     * before it calls the provider) or a port after it, in any case. Too
     * much rather than too little: such a variant reaches no other app.
     */
    fun isOwnProvider(authority: String?): Boolean {
        if (authority == null) return false
        val host = authority.substringAfterLast('@').substringBefore(':')
        return host.equals(ProviderRoutes.AUTHORITY, ignoreCase = true)
    }

    private const val SCHEME_CONTENT = "content"
}
