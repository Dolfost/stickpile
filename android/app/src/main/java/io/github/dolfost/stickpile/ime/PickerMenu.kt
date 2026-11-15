package io.github.dolfost.stickpile.ime

import android.view.Menu
import android.view.View
import android.widget.PopupMenu
import androidx.annotation.StringRes
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.DisplaySettings
import io.github.dolfost.stickpile.display.TileField
import io.github.dolfost.stickpile.display.TileSize

/**
 * The picker's menu (the button at the end of the search bar): what the
 * tiles show (all of it, and each field), the tile size, the search
 * syntax help, and the app. The changes go to the shared [DisplaySettings],
 * so the app shows them too.
 */
internal object PickerMenu {
    private const val SHOW_INFO = 1
    private const val SYNTAX = 2
    private const val OPEN_APP = 3
    private const val FIELDS = 100
    private const val SIZES = 200
    private const val GROUP_FIELDS = 1
    private const val GROUP_SIZES = 2

    fun show(anchor: View, settings: DisplaySettings, onSyntax: () -> Unit, onOpenApp: () -> Unit) {
        val display = settings.current
        val popup = PopupMenu(anchor.context, anchor)
        val menu = popup.menu
        menu.add(Menu.NONE, SHOW_INFO, 0, R.string.display_show_info).isCheckable = true
        menu.findItem(SHOW_INFO).isChecked = display.showInfo
        for (field in TileField.entries) {
            menu.add(GROUP_FIELDS, FIELDS + field.ordinal, 1 + field.ordinal, label(field)).apply {
                isCheckable = true
                isChecked = field in display.fields
                isEnabled = display.showInfo
            }
        }
        val sizes = menu.addSubMenu(Menu.NONE, Menu.NONE, 1 + TileField.entries.size, R.string.display_tile_size)
        for (size in TileSize.entries) sizes.add(GROUP_SIZES, SIZES + size.ordinal, size.ordinal, label(size))
        sizes.setGroupCheckable(GROUP_SIZES, true, true)
        sizes.findItem(SIZES + display.tileSize.ordinal).isChecked = true
        menu.add(Menu.NONE, SYNTAX, 2 + TileField.entries.size, R.string.display_syntax)
        menu.add(Menu.NONE, OPEN_APP, 3 + TileField.entries.size, R.string.ime_open_app)
        popup.setOnMenuItemClickListener { item ->
            val id = item.itemId
            when {
                id == SHOW_INFO -> settings.setShowInfo(!settings.current.showInfo)
                id == SYNTAX -> onSyntax()
                id == OPEN_APP -> onOpenApp()
                id - FIELDS in TileField.entries.indices -> {
                    val field = TileField.entries[id - FIELDS]
                    settings.setField(field, field !in settings.current.fields)
                }
                id - SIZES in TileSize.entries.indices -> settings.setTileSize(TileSize.entries[id - SIZES])
                else -> return@setOnMenuItemClickListener false
            }
            true
        }
        popup.show()
    }

    @StringRes
    fun label(field: TileField): Int = when (field) {
        TileField.BADGES -> R.string.display_field_badges
        TileField.LIKED -> R.string.display_field_liked
        TileField.NAME -> R.string.display_field_name
        TileField.EMOJIS -> R.string.display_field_emojis
        TileField.VIBE -> R.string.display_field_vibe
        TileField.TAGS -> R.string.display_field_tags
        TileField.SIZE -> R.string.display_field_size
        TileField.COPIES -> R.string.display_field_copies
        TileField.LIKES -> R.string.display_field_likes
        TileField.QUALITY -> R.string.display_field_quality
    }

    @StringRes
    fun label(size: TileSize): Int = when (size) {
        TileSize.SMALL -> R.string.display_tile_small
        TileSize.MEDIUM -> R.string.display_tile_medium
        TileSize.LARGE -> R.string.display_tile_large
    }
}
