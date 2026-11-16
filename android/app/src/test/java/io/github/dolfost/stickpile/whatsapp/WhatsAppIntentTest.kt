package io.github.dolfost.stickpile.whatsapp

import android.app.Activity
import org.junit.Assert.assertEquals
import org.junit.Test

/** WhatsApp's add intent and the sample app's choice of app (`AddStickerPackActivity`). */
class WhatsAppIntentTest {
    @Test
    fun theIntentCarriesTheReadmesActionAndExtras() {
        val request = AddRequest(identifier = "p1-2", name = "Cats (2/2)")
        assertEquals("com.whatsapp.intent.action.ENABLE_STICKER_PACK", request.action)
        assertEquals(
            mapOf(
                "sticker_pack_id" to "p1-2",
                "sticker_pack_authority" to "io.github.dolfost.stickpile.whatsapp",
                "sticker_pack_name" to "Cats (2/2)",
            ),
            request.extras,
        )
        assertEquals(null, request.packageName)
        assertEquals(
            "com.whatsapp.intent.action.ENABLE_STICKER_PACK sticker_pack_id=p1-2, " +
                "sticker_pack_authority=io.github.dolfost.stickpile.whatsapp, sticker_pack_name=Cats (2/2) -> com.whatsapp.w4b",
            request.copy(packageName = "com.whatsapp.w4b").toString(),
        )
    }

    @Test
    fun theAppIsChosenAsTheSampleChoosesIt() {
        fun d(consumer: Boolean, business: Boolean, inConsumer: Boolean = false, inBusiness: Boolean = false) =
            decide(WhatsAppPresence(consumer, business, inConsumer, inBusiness))
        assertEquals(AddDecision.NotInstalled, d(consumer = false, business = false))
        // One app: aimed at it (the other counts as having the pack), or added already.
        assertEquals(AddDecision.Only("com.whatsapp"), d(consumer = true, business = false))
        assertEquals(AddDecision.AlreadyAdded, d(consumer = true, business = false, inConsumer = true))
        assertEquals(AddDecision.Only("com.whatsapp.w4b"), d(consumer = false, business = true))
        assertEquals(AddDecision.AlreadyAdded, d(consumer = false, business = true, inBusiness = true))
        // Both: the one that lacks it, or a chooser when both do.
        assertEquals(AddDecision.Chooser, d(consumer = true, business = true))
        assertEquals(AddDecision.Only("com.whatsapp.w4b"), d(consumer = true, business = true, inConsumer = true))
        assertEquals(AddDecision.Only("com.whatsapp"), d(consumer = true, business = true, inBusiness = true))
        assertEquals(AddDecision.AlreadyAdded, d(consumer = true, business = true, inConsumer = true, inBusiness = true))
    }

    @Test
    fun theDialogsResultIsReadAsTheSampleReadsIt() {
        assertEquals(PartStatus.ADDED, statusAfter(Activity.RESULT_OK, null))
        assertEquals(PartStatus.CANCELLED, statusAfter(Activity.RESULT_CANCELED, null))
        assertEquals(PartStatus.REFUSED, statusAfter(Activity.RESULT_CANCELED, "sticker pack sticker count should be between 3 to 30"))
    }
}
