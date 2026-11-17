package io.github.dolfost.stickpile.display

import io.github.dolfost.stickpile.rust.HintKind
import io.github.dolfost.stickpile.rust.SearchHint

/**
 * How an offer under a search field reads, in the app and the keyboard: an
 * emoji of `e:` comes with its English name ([SearchHint.label] is
 * `💀 skull`), drawn as the glyph at the chip's size and the [name] after
 * it in the secondary style; any other offer is its [text] alone. A value
 * offered for any other filter is one word, so the first space of a
 * value's label parts the emoji from its name.
 */
data class HintLabel(val text: String, val name: String?) {
    companion object {
        fun of(hint: SearchHint): HintLabel {
            val space = hint.label.indexOf(' ')
            if (hint.kind != HintKind.VALUE || space <= 0) return HintLabel(hint.label, null)
            return HintLabel(hint.label.substring(0, space), hint.label.substring(space + 1))
        }
    }
}
