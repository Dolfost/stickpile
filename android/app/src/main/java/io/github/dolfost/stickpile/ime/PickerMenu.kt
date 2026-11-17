package io.github.dolfost.stickpile.ime

import android.view.Menu
import android.view.View
import android.widget.PopupMenu
import androidx.annotation.StringRes
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.DisplaySettings
import io.github.dolfost.stickpile.display.TileField

/**
 * The picker's menu (the button at the end of the search bar): what the
 * tiles show (all of it, and each field), the stickers in a row (a
 * slider, [ColumnsPanel]), the search syntax help, and the app. The changes go to the shared [DisplaySettings],
 * so the app shows them too.
 */
internal object PickerMenu {
    private const val SHOW_INFO = 1
    private const val SYNTAX = 2
    private const val OPEN_APP = 3
    private const val COLUMNS = 4
    private const val FIELDS = 100
    private const val GROUP_FIELDS = 1

    fun show(
        anchor: View,
        settings: DisplaySettings,
        onColumns: () -> Unit,
        onSyntax: () -> Unit,
        onOpenApp: () -> Unit,
    ) {
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
        val columns = anchor.context.getString(R.string.display_columns_menu, display.columns)
        menu.add(Menu.NONE, COLUMNS, 1 + TileField.entries.size, columns)
        menu.add(Menu.NONE, SYNTAX, 2 + TileField.entries.size, R.string.display_syntax)
        menu.add(Menu.NONE, OPEN_APP, 3 + TileField.entries.size, R.string.ime_open_app)
        popup.setOnMenuItemClickListener { item ->
            val id = item.itemId
            when {
                id == SHOW_INFO -> settings.setShowInfo(!settings.current.showInfo)
                id == SYNTAX -> onSyntax()
                id == OPEN_APP -> onOpenApp()
                id == COLUMNS -> onColumns()
                id - FIELDS in TileField.entries.indices -> {
                    val field = TileField.entries[id - FIELDS]
                    settings.setField(field, field !in settings.current.fields)
                }
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
        TileField.TEXT -> R.string.display_field_text
        TileField.SIZE -> R.string.display_field_size
        TileField.COPIES -> R.string.display_field_copies
        TileField.LIKES -> R.string.display_field_likes
        TileField.QUALITY -> R.string.display_field_quality
    }
}
