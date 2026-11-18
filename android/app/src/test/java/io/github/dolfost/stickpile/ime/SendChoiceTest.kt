package io.github.dolfost.stickpile.ime

import io.github.dolfost.stickpile.R
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * What a tap and the details' buttons send with "Prefer stickers" off and
 * on (ime.SendChoice). Which file the sticker flag picks for which app
 * (Telegram and its forks the sticker version, other apps as without it,
 * the usual file where there is no sticker version) is the core's, tested
 * in Rust: `the_sticker_flag_changes_only_what_telegram_and_its_forks_get`.
 */
class SendChoiceTest {
    @Test
    fun offATapSendsThePictureAsBefore() {
        assertFalse(SendChoice.tap(preferStickers = false))
        assertTrue(SendChoice.other(preferStickers = false))
        assertEquals(R.string.ime_menu_send_sticker, SendChoice.otherLabel(preferStickers = false))
        assertEquals(R.drawable.ic_sticker, SendChoice.otherIcon(preferStickers = false))
    }

    @Test
    fun onATapSendsAsAStickerAndTheDetailsOfferThePicture() {
        assertTrue(SendChoice.tap(preferStickers = true))
        assertFalse(SendChoice.other(preferStickers = true))
        assertEquals(R.string.ime_menu_send_picture, SendChoice.otherLabel(preferStickers = true))
        assertEquals(R.drawable.ic_picture, SendChoice.otherIcon(preferStickers = true))
    }

    @Test
    fun theDetailsAlwaysOfferTheOtherChoice() {
        for (prefer in listOf(false, true)) {
            assertNotEquals("prefer=$prefer", SendChoice.tap(prefer), SendChoice.other(prefer))
        }
        assertNotEquals(SendChoice.otherLabel(false), SendChoice.otherLabel(true))
        assertNotEquals(SendChoice.otherIcon(false), SendChoice.otherIcon(true))
    }
}
