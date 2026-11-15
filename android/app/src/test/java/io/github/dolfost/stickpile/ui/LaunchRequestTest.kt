package io.github.dolfost.stickpile.ui

import android.content.Intent
import io.github.dolfost.stickpile.ui.main.LaunchRequest
import io.github.dolfost.stickpile.ui.main.launchRequest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** What an intent opening the launcher asks for (ui.main.LaunchRequest). */
class LaunchRequestTest {
    private val link = "stickpile://pair?url=https://s.example&code=7KQ2M9XA4FTR"

    @Test
    fun aLinkIsOpened() {
        assertEquals(LaunchRequest.OpenLink(link), launchRequest(link, flags = 0))
        assertEquals(LaunchRequest.OpenLink(link), launchRequest(link, flags = Intent.FLAG_ACTIVITY_NEW_TASK))
        assertNull(launchRequest(null, flags = 0))
    }

    @Test
    fun theFirstIntentAgainFromRecentsAsksForNothing() {
        assertNull(launchRequest(link, flags = Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY))
    }
}
