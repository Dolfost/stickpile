package io.github.dolfost.stickpile.offline

import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.display.TileField
import io.github.dolfost.stickpile.rust.KeepReason
import io.github.dolfost.stickpile.rust.OfflineScope
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The menus' keep and release, and the tiles' marks (offline.OfflineLabels, offline.OfflineMarks). */
class OfflineLabelsTest {
    @Test
    fun aStickerKeptByItselfOffersToStop() {
        assertEquals(KeepAction.KEEP, OfflineLabels.stickerAction(emptyList()))
        assertEquals(KeepAction.RELEASE, OfflineLabels.stickerAction(listOf(KeepReason.STICKER)))
        assertEquals(KeepAction.RELEASE, OfflineLabels.stickerAction(listOf(KeepReason.STICKER, KeepReason.PACK)))
    }

    @Test
    fun aStickerKeptWithItsPackOrARuleCanStillBeKeptByItself() {
        for (reason in KeepReason.entries - KeepReason.STICKER) {
            assertEquals("$reason", KeepAction.KEEP, OfflineLabels.stickerAction(listOf(reason)))
        }
    }

    @Test
    fun theMenusSayKeepOrDontKeep() {
        assertEquals(R.string.offline_keep, OfflineLabels.label(KeepAction.KEEP))
        assertEquals(R.string.offline_release, OfflineLabels.label(KeepAction.RELEASE))
        assertEquals(KeepAction.RELEASE, OfflineLabels.scopeAction(kept = true))
        assertEquals(KeepAction.KEEP, OfflineLabels.scopeAction(kept = false))
        val cats = OfflineScope.Pack("cats")
        assertTrue(OfflineLabels.kept(cats, listOf(OfflineScope.Library, cats)))
        assertFalse(OfflineLabels.kept(cats, listOf(OfflineScope.Library, OfflineScope.Pack("dogs"))))
    }

    @Test
    fun everyReasonHasItsWordsInOrder() {
        val words = OfflineLabels.reasons(KeepReason.entries + KeepReason.PACK)
        assertEquals(KeepReason.entries.size, words.size)
        assertEquals(KeepReason.entries.size, words.toSet().size)
        assertEquals(R.string.offline_reason_sticker, words.first())
        assertEquals(emptyList<Int>(), OfflineLabels.reasons(emptyList()))
    }

    @Test
    fun slidersShowAValueAtItsNearestStep() {
        assertEquals(0, OfflineSizes.index(OfflineSizes.MAX_FILE_MB, 0uL))
        assertEquals(3, OfflineSizes.index(OfflineSizes.MAX_FILE_MB, 5uL))
        assertEquals(3, OfflineSizes.index(OfflineSizes.MAX_FILE_MB, 6uL))
        assertEquals(OfflineSizes.BUDGET_MB.size - 1, OfflineSizes.index(OfflineSizes.BUDGET_MB, 0uL), "no limit last")
        assertEquals(2, OfflineSizes.index(OfflineSizes.BUDGET_MB, 1024uL), "the default")
        assertEquals(OfflineSizes.BUDGET_MB.size - 2, OfflineSizes.index(OfflineSizes.BUDGET_MB, 30_000uL))
        assertEquals(3, OfflineSizes.usedIndex(200u))
    }

    private fun assertEquals(expected: Int, actual: Int, message: String) = assertEquals(message, expected, actual)

    @Test
    fun withTheServerThereAMarkShowsWhatWorksWithoutIt() {
        val marks = OfflineMarks.of(listOf("a", "b"), listOf(true, false), unreachable = false)
        val display = Display()
        assertEquals(OfflineMark.AVAILABLE, marks.of("a", display))
        assertEquals(OfflineMark.NONE, marks.of("b", display))
        // A tile mark like the others: it can be turned off, and so can all the info.
        assertEquals(OfflineMark.NONE, marks.of("a", display.with(TileField.OFFLINE, false)))
        assertEquals(OfflineMark.NONE, marks.of("a", display.copy(showInfo = false)))
    }

    @Test
    fun withTheServerAwayWhatIsNotHereIsMarkedWhateverTheTilesShow() {
        val marks = OfflineMarks.of(listOf("a", "b"), listOf(true, false), unreachable = true)
        for (display in listOf(Display(), Display(showInfo = false), Display().with(TileField.OFFLINE, false))) {
            assertEquals(OfflineMark.NONE, marks.of("a", display))
            assertEquals(OfflineMark.UNAVAILABLE, marks.of("b", display))
        }
    }

    @Test
    fun theKeyboardMarksWhatWorksAsAStickerOnlyAvailable() {
        // Plain pick, "Send as a sticker": either one working offline will do.
        assertEquals(
            listOf(false, true, true, true),
            OfflineMarks.either(listOf(false, true, false, true), listOf(false, false, true, true)),
        )
    }

    @Test
    fun withPreferStickersOnTheKeyboardMarksWhatATapInserts() {
        // The library's answers for four stickers, without and with the
        // sticker flag. The core keeps the usual files after the sticker
        // ones, so what works without the flag works with it
        // (the_sticker_flag_only_puts_the_sticker_renditions_first).
        val plain = listOf(false, false, true, true)
        val asSticker = listOf(false, true, true, true)
        val off = OfflineMarks.either(plain, asSticker)
        // "Prefer stickers" on: a tap sends with the flag, the details' other button without.
        val on = OfflineMarks.either(asSticker, plain)
        assertEquals(off, on)
        assertEquals(asSticker, on)
    }

    @Test
    fun nothingIsMarkedUntilTheLibraryAnswered() {
        val unknown = OfflineMarks()
        assertEquals(OfflineMark.NONE, unknown.of("a", Display()))
        assertEquals(OfflineMark.NONE, unknown.copy(unreachable = true).of("a", Display()))
    }

    @Test
    fun theOfflineMarkStartsOnForEveryone() {
        assertTrue(TileField.OFFLINE in TileField.DEFAULTS)
        // Stored before the mark existed: some fields off, the mark on.
        val old = TileField.fromStored(TileField.Stored(hidden = setOf("TAGS"), shown = emptySet()))
        assertTrue(TileField.OFFLINE in old)
    }
}
