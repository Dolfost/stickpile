package io.github.dolfost.stickpile.share

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SharedUrisTest {
    private val own = "io.github.dolfost.stickpile.stickers"

    @Test
    fun otherAppsContentIsRead() {
        assertTrue(SharedUris.accepts("content", "media"))
        assertTrue(SharedUris.accepts("content", "com.android.providers.media.documents"))
        assertTrue(SharedUris.accepts("content", "org.telegram.messenger.provider"))
        // A name that only starts or ends like the app's provider is another's.
        assertTrue(SharedUris.accepts("content", "$own.other"))
        assertTrue(SharedUris.accepts("content", "evil.$own"))
    }

    @Test
    fun theAppsOwnProviderIsNotRead() {
        assertFalse(SharedUris.accepts("content", own))
        assertFalse(SharedUris.accepts("content", own.uppercase()))
        // With the user in front, which the system strips before calling the provider.
        assertFalse(SharedUris.accepts("content", "0@$own"))
        assertFalse(SharedUris.accepts("content", "10@$own"))
        assertFalse(SharedUris.accepts("content", "$own:8080"))
    }

    @Test
    fun otherSchemesAreNotRead() {
        assertFalse(SharedUris.accepts("file", null))
        assertFalse(SharedUris.accepts("file", ""))
        assertFalse(SharedUris.accepts("http", "example.org"))
        assertFalse(SharedUris.accepts(null, "media"))
    }

    @Test
    fun noAuthorityIsNotTheAppsProvider() {
        assertFalse(SharedUris.isOwnProvider(null))
        assertFalse(SharedUris.isOwnProvider(""))
    }
}
