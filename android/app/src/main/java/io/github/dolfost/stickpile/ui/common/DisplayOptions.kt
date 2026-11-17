package io.github.dolfost.stickpile.ui.common

import androidx.annotation.StringRes
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemColors
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.Autoplay
import io.github.dolfost.stickpile.display.Columns
import io.github.dolfost.stickpile.display.Display
import io.github.dolfost.stickpile.display.DisplaySettings
import io.github.dolfost.stickpile.display.TileField
import kotlin.math.roundToInt

/**
 * What the sticker tiles show, as switches, for the library's sheet and
 * the settings: safe mode when [withSafeMode], all the info at once, each
 * field (dimmed while the info is off), the stickers in a row, and when
 * GIFs and videos play. A change is saved in [settings] at once, which the app and
 * the keyboard both follow.
 */
@Composable
fun DisplayOptions(display: Display, settings: DisplaySettings, withSafeMode: Boolean = false) {
    if (withSafeMode) {
        OptionSwitch(
            stringResource(R.string.display_safe_mode),
            stringResource(R.string.display_safe_mode_text),
            display.safeMode,
            onChange = settings::setSafeMode,
        )
    }
    OptionSwitch(
        stringResource(R.string.display_show_info),
        stringResource(R.string.display_show_info_text),
        display.showInfo,
        onChange = settings::setShowInfo,
    )
    TileField.entries.forEach { field ->
        OptionSwitch(stringResource(field.label), null, field in display.fields, dimmed = !display.showInfo) {
            settings.setField(field, it)
        }
    }
    ColumnsSlider(display.columns, settings::setColumns)
    ListItem(
        headlineContent = { Text(stringResource(R.string.display_autoplay)) },
        supportingContent = { Text(stringResource(R.string.display_autoplay_text)) },
        colors = Transparent,
    )
    SingleChoiceSegmentedButtonRow(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 4.dp),
    ) {
        Autoplay.entries.forEachIndexed { i, autoplay ->
            SegmentedButton(
                selected = autoplay == display.autoplay,
                onClick = { settings.setAutoplay(autoplay) },
                shape = SegmentedButtonDefaults.itemShape(i, Autoplay.entries.size),
                label = { Text(stringResource(autoplay.label)) },
            )
        }
    }
}

/**
 * Stickers in a row ([Columns]), as a slider: the grids follow it while it
 * moves, and the value is saved when it is let go.
 */
@Composable
private fun ColumnsSlider(columns: Int, onChange: (Int) -> Unit) {
    var value by remember(columns) { mutableIntStateOf(columns) }
    ListItem(
        headlineContent = { Text(stringResource(R.string.display_columns)) },
        supportingContent = { Text(stringResource(R.string.display_columns_text)) },
        trailingContent = { Text(value.toString(), style = MaterialTheme.typography.titleMedium) },
        colors = Transparent,
    )
    Slider(
        value = value.toFloat(),
        onValueChange = { value = Columns.clamp(it.roundToInt()) },
        onValueChangeFinished = { onChange(value) },
        valueRange = Columns.MIN.toFloat()..Columns.MAX.toFloat(),
        steps = Columns.MAX - Columns.MIN - 1,
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp)
            .semantics { stateDescription = value.toString() },
    )
}

/** On any background: a sheet's or a screen's. */
private val Transparent: ListItemColors
    @Composable get() = ListItemDefaults.colors(containerColor = Color.Transparent)

/** A switch with its title (and explanation); the whole row toggles it. */
@Composable
private fun OptionSwitch(title: String, text: String?, checked: Boolean, dimmed: Boolean = false, onChange: (Boolean) -> Unit) {
    ListItem(
        headlineContent = { Text(title) },
        supportingContent = text?.let { { Text(it) } },
        trailingContent = { Switch(checked = checked, onCheckedChange = null) },
        colors = Transparent,
        modifier = Modifier
            .toggleable(value = checked, role = Role.Switch, onValueChange = onChange)
            .alpha(if (dimmed) DIMMED else 1f),
    )
}

private const val DIMMED = 0.5f

@get:StringRes
val TileField.label: Int
    get() = when (this) {
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

@get:StringRes
val Autoplay.label: Int
    get() = when (this) {
        Autoplay.ALWAYS -> R.string.display_autoplay_always
        Autoplay.WIFI -> R.string.display_autoplay_wifi
        Autoplay.NEVER -> R.string.display_autoplay_never
    }
