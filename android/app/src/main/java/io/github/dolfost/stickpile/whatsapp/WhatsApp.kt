package io.github.dolfost.stickpile.whatsapp

import android.app.Activity

/**
 * WhatsApp's side of "Add to WhatsApp", as its sample sticker app does it
 * (github.com/WhatsApp/stickers, `Android/README.md` and
 * `Android/app/src/main/java/com/example/samplestickerapp/`, read
 * 2026-11-17): the intent that asks WhatsApp to add a pack, its extras,
 * the two apps, and the provider that tells whether a pack is added.
 * Pure, so that the JVM tests need no Android.
 */
object WhatsApp {
    /** WhatsApp (`WhitelistCheck.CONSUMER_WHATSAPP_PACKAGE_NAME`). */
    const val CONSUMER = "com.whatsapp"

    /** WhatsApp Business (`WhitelistCheck.SMB_WHATSAPP_PACKAGE_NAME`). */
    const val BUSINESS = "com.whatsapp.w4b"

    /** The two, as `<queries>` in the manifest names them. */
    val APPS = listOf(CONSUMER, BUSINESS)

    /** The intent WhatsApp answers with its "Add sticker pack" dialog (README, "Intent"). */
    const val ACTION_ENABLE_STICKER_PACK = "com.whatsapp.intent.action.ENABLE_STICKER_PACK"
    const val EXTRA_STICKER_PACK_ID = "sticker_pack_id"
    const val EXTRA_STICKER_PACK_AUTHORITY = "sticker_pack_authority"
    const val EXTRA_STICKER_PACK_NAME = "sticker_pack_name"

    /** What WhatsApp puts into a cancelled result when it refused the pack (`AddStickerPackActivity`). */
    const val EXTRA_VALIDATION_ERROR = "validation_error"

    /**
     * The provider of each app that says whether a pack is added: its
     * authority is the app's package plus this (`WhitelistCheck`).
     */
    const val WHITELIST_PROVIDER = ".provider.sticker_whitelist_check"
    const val WHITELIST_PATH = "is_whitelisted"
    const val WHITELIST_AUTHORITY_PARAM = "authority"
    const val WHITELIST_IDENTIFIER_PARAM = "identifier"

    /** Its one column: 1 when the pack is added, 0 when not. */
    const val WHITELIST_RESULT = "result"
}

/** Which WhatsApp apps are on the phone, and whether a part is added to each. */
data class WhatsAppPresence(
    val consumerInstalled: Boolean,
    val businessInstalled: Boolean,
    val inConsumer: Boolean = false,
    val inBusiness: Boolean = false,
)

/** What to do to add a part, as `AddStickerPackActivity.addStickerPackToWhatsApp` decides. */
sealed interface AddDecision {
    /** Neither WhatsApp nor WhatsApp Business is installed. */
    data object NotInstalled : AddDecision

    /** It is added to every installed app already. */
    data object AlreadyAdded : AddDecision

    /** Both apps are installed and neither has it: a chooser offers both. */
    data object Chooser : AddDecision

    /** It is in one of the two: ask the other ([packageName]). */
    data class Only(val packageName: String) : AddDecision
}

/**
 * The sample app's choice: an app that is not installed counts as having
 * the pack (`WhitelistCheck.isWhitelistedFromProvider`), so a pack in the
 * one installed app is added.
 */
fun decide(presence: WhatsAppPresence): AddDecision {
    if (!presence.consumerInstalled && !presence.businessInstalled) return AddDecision.NotInstalled
    val inConsumer = !presence.consumerInstalled || presence.inConsumer
    val inBusiness = !presence.businessInstalled || presence.inBusiness
    return when {
        inConsumer && inBusiness -> AddDecision.AlreadyAdded
        !inConsumer && !inBusiness -> AddDecision.Chooser
        !inConsumer -> AddDecision.Only(WhatsApp.CONSUMER)
        else -> AddDecision.Only(WhatsApp.BUSINESS)
    }
}

/**
 * The intent asking WhatsApp to add the part [identifier] named [name]
 * from this app's provider ([authority]): its action and extras, and the
 * app it goes to (null: the chooser).
 */
data class AddRequest(
    val identifier: String,
    val name: String,
    val authority: String = WhatsAppRoutes.AUTHORITY,
    val packageName: String? = null,
) {
    val action: String get() = WhatsApp.ACTION_ENABLE_STICKER_PACK

    /** The three extras of the README's intent, all strings. */
    val extras: Map<String, String>
        get() = linkedMapOf(
            WhatsApp.EXTRA_STICKER_PACK_ID to identifier,
            WhatsApp.EXTRA_STICKER_PACK_AUTHORITY to authority,
            WhatsApp.EXTRA_STICKER_PACK_NAME to name,
        )

    /** For the log: no secret in it. */
    override fun toString(): String =
        "$action ${extras.entries.joinToString { "${it.key}=${it.value}" }} -> ${packageName ?: "chooser"}"
}

/** What became of a part in this app's sheet. */
enum class PartStatus {
    /** Not asked for yet (or not known). */
    NOT_ADDED,

    /** WhatsApp's dialog is open for it. */
    ADDING,

    /** WhatsApp answered that it added it. */
    ADDED,

    /** Every installed WhatsApp has it already. */
    ALREADY_ADDED,

    /** The dialog was closed without adding it (or WhatsApp is too old). */
    CANCELLED,

    /** WhatsApp checked it and refused it (its validation error is logged). */
    REFUSED,
}

/**
 * The part's status after WhatsApp's dialog returned [resultCode]: OK is
 * added; a cancel with a `validation_error` is a refusal, any other
 * cancel is the user's (or an old WhatsApp's), as the sample app reads it
 * (`AddStickerPackActivity.onActivityResult`).
 */
fun statusAfter(resultCode: Int, validationError: String?): PartStatus = when {
    resultCode == Activity.RESULT_OK -> PartStatus.ADDED
    validationError != null -> PartStatus.REFUSED
    else -> PartStatus.CANCELLED
}
