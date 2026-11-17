package io.github.dolfost.stickpile.ui

import io.github.dolfost.stickpile.ui.common.ByteSize
import java.util.Locale
import org.junit.Assert.assertEquals
import org.junit.Test

/** Sizes count in 1024s, as the CLI writes them and the options (MiB) count (ui.common.ByteSize). */
class ByteSizeTest {
    private val en = listOf("B", "KiB", "MiB", "GiB", "TiB")
    private val uk = listOf("Б", "КіБ", "МіБ", "ГіБ", "ТіБ")

    @Test
    fun theUnitsAre1024Based() {
        assertEquals("0 B", ByteSize.format(0uL, en, Locale.US))
        assertEquals("1023 B", ByteSize.format(1023uL, en, Locale.US))
        assertEquals("1.0 KiB", ByteSize.format(1024uL, en, Locale.US))
        assertEquals("1.5 KiB", ByteSize.format(1536uL, en, Locale.US))
        assertEquals("1.0 MiB", ByteSize.format(1024uL * 1024uL, en, Locale.US))
        // The default budget, offline.max_mb = 1024: "1.0 GiB", not "1.1 GB".
        assertEquals("1.0 GiB", ByteSize.format(1024uL * 1024uL * 1024uL, en, Locale.US))
        assertEquals("2048.0 TiB", ByteSize.format(2048uL shl 40, en, Locale.US))
    }

    @Test
    fun ukrainianWritesItsUnitsAndDecimalComma() {
        assertEquals("1,5 МіБ", ByteSize.format(1536uL * 1024uL, uk, Locale.forLanguageTag("uk")))
        assertEquals("12 Б", ByteSize.format(12uL, uk, Locale.forLanguageTag("uk")))
    }
}
