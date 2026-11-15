package io.github.dolfost.stickpile.ime

import android.annotation.SuppressLint
import android.content.res.ColorStateList
import android.graphics.Color
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.Space
import android.widget.TextView
import androidx.annotation.DrawableRes
import androidx.annotation.StringRes
import androidx.annotation.StyleRes
import com.google.android.material.color.MaterialColors
import io.github.dolfost.stickpile.R

/**
 * The picker's own keys for its search (the rows of [SearchKeyboard]),
 * built as classic Views in [container]. What they type goes to
 * [Listener.type] and only ever into the picker's search query, never into
 * the editor's field; nothing typed is logged.
 *
 * Every key taps the keyboard haptic; backspace repeats while held.
 */
internal class SearchKeys(
    private val container: LinearLayout,
    layout: KeyLayout,
    private val listener: Listener,
) {
    interface Listener {
        fun type(text: String)
        fun backspace()
        fun done()

        /** The globe key chose [layout]: to remember. */
        fun layoutChanged(layout: KeyLayout)
    }

    var layout: KeyLayout = layout
        private set

    private var symbols = false
    private val context = container.context
    private val rowHeight = context.resources.getDimensionPixelSize(R.dimen.ime_key_row_height)
    private val gapPx = (KEY_GAP_DP * context.resources.displayMetrics.density).toInt()
    private val iconTint = ColorStateList.valueOf(
        MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSecondaryContainer, Color.BLACK),
    )

    private var backspaceKey: View? = null

    /** Set once backspace repeats while held, so that letting go does not delete once more. */
    private var repeated = false
    private val repeat = object : Runnable {
        override fun run() {
            val key = backspaceKey ?: return
            repeated = true
            tap(key)
            listener.backspace()
            key.postDelayed(this, REPEAT_MS)
        }
    }

    /** Shows the keys of the current page (letters of [layout], or the symbols). */
    fun build() {
        stopRepeat()
        backspaceKey = null
        container.removeAllViews()
        for (row in SearchKeyboard.rows(layout, symbols)) {
            val line = LinearLayout(context)
            line.orientation = LinearLayout.HORIZONTAL
            for (spec in row) line.addView(view(spec.key), params(spec.weight))
            container.addView(line, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, rowHeight))
        }
    }

    /** The keys are going away: a held backspace stops. */
    fun stopRepeat() {
        backspaceKey?.removeCallbacks(repeat)
    }

    private fun view(key: Key): View = when (key) {
        is Key.Text -> text(R.style.Widget_Stickpile_Ime_Key, key.text) { listener.type(key.text) }
        Key.Space -> text(R.style.Widget_Stickpile_Ime_Key_Space, context.getString(layout.label)) {
            listener.type(" ")
        }.described(R.string.ime_key_space)
        Key.Symbols -> {
            val label = if (symbols) layout.letters else R.string.ime_key_symbols_label
            text(R.style.Widget_Stickpile_Ime_Key_Action, context.getString(label)) {
                symbols = !symbols
                build()
            }.described(if (symbols) R.string.ime_key_letters else R.string.ime_key_symbols)
        }
        Key.NextLayout -> icon(R.drawable.ic_language, R.string.ime_key_language) {
            layout = layout.next()
            symbols = false
            build()
            listener.layoutChanged(layout)
        }
        Key.Backspace -> icon(R.drawable.ic_backspace, R.string.ime_key_backspace, null).also {
            holdToRepeat(it)
            backspaceKey = it
        }
        Key.Done -> text(R.style.Widget_Stickpile_Ime_Key_Action, context.getString(R.string.ime_key_done)) {
            listener.done()
        }
        Key.Gap -> Space(context)
    }

    private fun text(@StyleRes style: Int, label: String, onClick: () -> Unit): TextView =
        TextView(context, null, 0, style).apply {
            text = label
            clickable(this, onClick)
        }

    private fun icon(@DrawableRes icon: Int, @StringRes description: Int, onClick: (() -> Unit)?): ImageView =
        ImageView(context, null, 0, R.style.Widget_Stickpile_Ime_KeyIcon).apply {
            setImageResource(icon)
            imageTintList = iconTint
            contentDescription = context.getString(description)
            if (onClick != null) clickable(this, onClick) else isClickable = true
        }

    private fun clickable(view: View, onClick: () -> Unit) {
        view.isClickable = true
        view.isFocusable = true
        view.setOnClickListener {
            tap(it)
            onClick()
        }
    }

    private fun View.described(@StringRes description: Int): View = apply {
        contentDescription = context.getString(description)
    }

    /**
     * Backspace deletes on a tap (or an accessibility click) and, while
     * held, again and again. The touch listener only starts and stops the
     * repeat and lets the view handle the touch, so the view still clicks.
     */
    @SuppressLint("ClickableViewAccessibility")
    private fun holdToRepeat(key: View) {
        key.isFocusable = true
        key.setOnTouchListener { v, event ->
            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    repeated = false
                    v.postDelayed(repeat, REPEAT_START_MS)
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> v.removeCallbacks(repeat)
            }
            false
        }
        key.setOnClickListener {
            if (repeated) {
                repeated = false
            } else {
                tap(it)
                listener.backspace()
            }
        }
    }

    private fun tap(view: View) {
        view.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP)
    }

    private fun params(weight: Float) =
        LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.MATCH_PARENT, weight).apply {
            setMargins(gapPx, gapPx, gapPx, gapPx)
        }

    private companion object {
        const val KEY_GAP_DP = 2
        const val REPEAT_START_MS = 400L
        const val REPEAT_MS = 50L
    }
}
