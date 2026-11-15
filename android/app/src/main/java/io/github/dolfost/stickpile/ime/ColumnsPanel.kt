package io.github.dolfost.stickpile.ime

import android.view.View
import android.widget.TextView
import androidx.core.view.isVisible
import com.google.android.material.slider.Slider
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.Columns
import io.github.dolfost.stickpile.display.DisplaySettings
import java.text.NumberFormat

/**
 * Stickers in a row, opened from the picker's menu (`ime_columns.xml`): a
 * strip over the top of the grid with a slider from [Columns.MIN] to
 * [Columns.MAX]. Each step goes to the shared [settings] at once, so the
 * grid under it (and the app's) follows while the slider moves.
 */
internal class ColumnsPanel(
    private val view: View,
    private val settings: DisplaySettings,
) {
    private val value: TextView = view.findViewById(R.id.ime_columns_value)
    private val slider: Slider = view.findViewById(R.id.ime_columns_slider)
    private val count = NumberFormat.getIntegerInstance()

    init {
        slider.valueFrom = Columns.MIN.toFloat()
        slider.valueTo = Columns.MAX.toFloat()
        slider.stepSize = 1f
        slider.addOnChangeListener { _, v, fromUser ->
            val columns = Columns.clamp(v.toInt())
            value.text = count.format(columns.toLong())
            if (fromUser && columns != settings.current.columns) settings.setColumns(columns)
        }
        view.findViewById<View>(R.id.ime_columns_close).setOnClickListener { close() }
    }

    val shown: Boolean get() = view.isVisible

    fun show() {
        val columns = settings.current.columns
        slider.value = columns.toFloat()
        value.text = count.format(columns.toLong())
        view.isVisible = true
    }

    fun close() {
        view.isVisible = false
    }
}
