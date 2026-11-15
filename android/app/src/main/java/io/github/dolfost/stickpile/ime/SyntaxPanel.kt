package io.github.dolfost.stickpile.ime

import android.view.LayoutInflater
import android.view.View
import android.widget.LinearLayout
import android.widget.TextView
import androidx.core.view.isEmpty
import androidx.core.view.isVisible
import io.github.dolfost.stickpile.R
import io.github.dolfost.stickpile.display.SyntaxHelp

/**
 * The search syntax help over the picker's grid, opened from the menu
 * (`ime_syntax.xml`): the sections of [SyntaxHelp], scrolling, with a
 * close button. A tap on a line puts its example in the search
 * ([PickerSearch.exampleQuery]) and closes the panel.
 */
internal class SyntaxPanel(
    private val view: View,
    private val onExample: (query: String) -> Unit,
) {
    private val lines: LinearLayout = view.findViewById(R.id.ime_syntax_lines)

    init {
        view.findViewById<View>(R.id.ime_syntax_close).setOnClickListener { close() }
    }

    fun show() {
        if (lines.isEmpty()) fill()
        view.isVisible = true
    }

    fun close() {
        view.isVisible = false
    }

    private fun fill() {
        val inflater = LayoutInflater.from(view.context)
        for (section in SyntaxHelp.SECTIONS) {
            val title = inflater.inflate(R.layout.ime_syntax_section, lines, false) as TextView
            title.setText(section.title)
            lines.addView(title)
            for (line in section.lines) {
                val row = inflater.inflate(R.layout.ime_syntax_line, lines, false)
                row.findViewById<TextView>(R.id.ime_syntax_example).text = line.example
                row.findViewById<TextView>(R.id.ime_syntax_text).setText(line.text)
                row.setOnClickListener {
                    close()
                    onExample(PickerSearch.exampleQuery(line.example))
                }
                lines.addView(row)
            }
        }
    }
}
