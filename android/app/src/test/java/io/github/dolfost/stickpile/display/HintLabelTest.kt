package io.github.dolfost.stickpile.display

import io.github.dolfost.stickpile.rust.HintKind
import io.github.dolfost.stickpile.rust.SearchHint
import org.junit.Assert.assertEquals
import org.junit.Test

class HintLabelTest {
    private fun label(label: String, kind: HintKind = HintKind.VALUE) =
        HintLabel.of(SearchHint(label, "e:x ", false, kind))

    @Test
    fun anEmojiOfferIsTheGlyphThenItsName() {
        assertEquals(HintLabel("💀", "skull"), label("💀 skull"))
        assertEquals(HintLabel("☠️", "skull and crossbones"), label("☠️ skull and crossbones"))
        assertEquals(HintLabel("👍🏽", "thumbs up"), label("👍🏽 thumbs up"))
    }

    @Test
    fun otherOffersAreTheirLabelAlone() {
        assertEquals(HintLabel("sarcastic", null), label("sarcastic"))
        assertEquals("an emoji without a name", HintLabel("🦨", null), label("🦨"))
        assertEquals(HintLabel("tag: (t:)", null), label("tag: (t:)", HintKind.KEY))
        assertEquals(HintLabel("tag:reaction", null), label("tag:reaction", HintKind.FILTER))
    }
}
