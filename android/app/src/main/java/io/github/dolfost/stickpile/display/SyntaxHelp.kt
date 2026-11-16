package io.github.dolfost.stickpile.display

import androidx.annotation.StringRes
import io.github.dolfost.stickpile.R

/**
 * The search syntax help, as the desktop shows it (`g/`, F2;
 * `stickpile_quick::vim::SYNTAX`), for the app and the keyboard: sections
 * of an example (typed as is, never translated) and what it does.
 */
object SyntaxHelp {
    data class Line(val example: String, @StringRes val text: Int)

    data class Section(@StringRes val title: Int, val lines: List<Line>)

    val SECTIONS: List<Section> = listOf(
        Section(
            R.string.display_syntax,
            listOf(
                Line("cat \"big chungus\"", R.string.display_syntax_words),
                Line("e:😂", R.string.display_syntax_emoji),
                Line("vibe:tired", R.string.display_syntax_vibe),
                Line("tag:reaction", R.string.display_syntax_tag),
                Line("set:cats", R.string.display_syntax_set),
                Line("by:alice", R.string.display_syntax_by),
                Line("v: t: s: b:", R.string.display_syntax_short),
                Line("type:gif", R.string.display_syntax_type),
                Line("is:mine, is:liked, is:public", R.string.display_syntax_is),
                Line("q>70, likes>=2, copies<5", R.string.display_syntax_numbers),
                Line("sort:likes", R.string.display_syntax_sort),
                Line("tag:a tag:b", R.string.display_syntax_either),
                Line("-tag:nsfw, -word", R.string.display_syntax_leave_out),
                Line("v:tired -t:nsfw", R.string.display_syntax_in_pack),
            ),
        ),
        Section(
            R.string.display_syntax_offers,
            listOf(
                Line("vibe: tag: set: by: e:", R.string.display_syntax_offers_values),
                Line("rea", R.string.display_syntax_offers_words),
                Line("vibe:sarcstic", R.string.display_syntax_offers_typos),
            ),
        ),
    )
}
