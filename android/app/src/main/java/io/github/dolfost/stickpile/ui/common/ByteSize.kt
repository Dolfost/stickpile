package io.github.dolfost.stickpile.ui.common

import android.content.res.Resources
import io.github.dolfost.stickpile.R
import java.util.Locale

/**
 * Sizes in 1024-based units, as the CLI writes them and the offline
 * options count (MiB): one convention everywhere, not the phone's
 * 1000-based one (pure).
 */
object ByteSize {
    /**
     * [bytes] in [units] (the language's B, KiB, MiB, GiB, TiB,
     * `R.array.size_units`): whole bytes below 1 KiB, else one decimal as
     * [locale] writes it ("1.5 MiB", "1,5 МіБ").
     */
    fun format(bytes: ULong, units: List<String>, locale: Locale): String {
        var value = bytes.toDouble()
        var unit = 0
        while (value >= 1024.0 && unit + 1 < units.size) {
            value /= 1024.0
            unit++
        }
        return if (unit == 0) "$bytes ${units[0]}" else String.format(locale, "%.1f %s", value, units[unit])
    }
}

/** [bytes] in the app's language ([ByteSize]). */
fun Resources.byteSize(bytes: ULong): String =
    ByteSize.format(bytes, getStringArray(R.array.size_units).toList(), configuration.locales[0])
