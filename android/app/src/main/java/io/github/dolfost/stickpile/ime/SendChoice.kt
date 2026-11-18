package io.github.dolfost.stickpile.ime

import androidx.annotation.DrawableRes
import androidx.annotation.StringRes
import io.github.dolfost.stickpile.R

/**
 * What a tap on a sticker in the picker sends, and the other way its
 * details offer, as "Prefer stickers" ([ImeSettings.preferStickers]) says.
 * Each is the sticker flag of the library's `prepare`, which decides by
 * the field's app (`target_for_package`): with the flag, Telegram and its
 * forks get the sticker version, and where there is none or the field
 * does not take it, the usual file; any other app gets what it gets
 * without the flag. So a tap with the setting on sends what "Send as a
 * sticker" sends, and nothing changes outside Telegram.
 */
internal object SendChoice {
    /** The sticker flag of a tap, and of Send in the details. */
    fun tap(preferStickers: Boolean): Boolean = preferStickers

    /** The sticker flag of the details' other button: the choice a tap does not make. */
    fun other(preferStickers: Boolean): Boolean = !tap(preferStickers)

    /** What the other button says: "Send as sticker", or "Send as a picture" when a tap sends stickers. */
    @StringRes
    fun otherLabel(preferStickers: Boolean): Int =
        if (other(preferStickers)) R.string.ime_menu_send_sticker else R.string.ime_menu_send_picture

    /** The other button's icon, as [otherLabel]. */
    @DrawableRes
    fun otherIcon(preferStickers: Boolean): Int =
        if (other(preferStickers)) R.drawable.ic_sticker else R.drawable.ic_picture
}
