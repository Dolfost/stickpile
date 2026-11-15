package io.github.dolfost.stickpile.ui

import io.github.dolfost.stickpile.ui.common.hostOf
import io.github.dolfost.stickpile.ui.common.isUnencrypted
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The host a sign-in goes to, as the screens show it (ui.common.ServerUrls). */
class ServerUrlsTest {
    @Test
    fun theHostWithoutPortPathOrUser() {
        assertEquals("s.example", hostOf("https://s.example"))
        assertEquals("s.example", hostOf("http://s.example:8080/stickpile"))
        assertEquals("s.example", hostOf("  https://s.example/  "))
        assertEquals("192.168.1.5", hostOf("http://192.168.1.5:8080"))
        assertEquals("[::1]", hostOf("http://[::1]:8080"))
        // A user part never passes for the host.
        assertEquals("s.example", hostOf("https://good.example@s.example"))
    }

    @Test
    fun anInternationalisedHostStaysPunycode() {
        assertEquals("xn--80ak6aa92e.com", hostOf("https://xn--80ak6aa92e.com/"))
    }

    @Test
    fun aNameJavaDoesNotTakeAsAServerName() {
        assertEquals("my_server", hostOf("http://my_server:8080"))
        assertEquals("my_server", hostOf("http://user@my_server"))
    }

    @Test
    fun noHostIsNull() {
        assertNull(hostOf("s.example"))
        assertNull(hostOf("not a url"))
        assertNull(hostOf(""))
    }

    @Test
    fun plainHttpIsNotEncrypted() {
        assertTrue(isUnencrypted("http://s.example"))
        assertTrue(isUnencrypted(" HTTP://s.example"))
        assertFalse(isUnencrypted("https://s.example"))
        assertFalse(isUnencrypted("s.example"))
        assertFalse(isUnencrypted(""))
    }
}
