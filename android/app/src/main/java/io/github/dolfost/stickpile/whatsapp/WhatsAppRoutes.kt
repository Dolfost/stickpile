package io.github.dolfost.stickpile.whatsapp

import io.github.dolfost.stickpile.rust.WhatsappPart
import io.github.dolfost.stickpile.rust.WhatsappSticker
import java.io.File

/** What a URI of the WhatsApp provider asks for. */
sealed interface WhatsAppRoute {
    /** `/metadata`: every pack. */
    data object Metadata : WhatsAppRoute

    /** `/metadata/<identifier>`: one pack (no row when there is none). */
    data class Pack(val identifier: String) : WhatsAppRoute

    /** `/stickers/<identifier>`: a pack's stickers. */
    data class Stickers(val identifier: String) : WhatsAppRoute

    /** `/stickers_asset/<identifier>/<file name>`: a sticker or the tray icon. */
    data class Asset(val identifier: String, val fileName: String) : WhatsAppRoute
}

/**
 * The paths and types of the provider WhatsApp reads the packs from, as
 * its sample app serves them (`StickerContentProvider.java`; README,
 * "ContentProvider"). Pure: the provider passes `Uri.getAuthority` and
 * `Uri.getEncodedPath`.
 *
 * Strict: a segment is letters, digits, `.`, `-` and `_`, not starting
 * with a `.`, which is all the library's identifiers and file names use
 * (so a percent escape, `..` or an empty segment is no route).
 */
object WhatsAppRoutes {
    /** The provider's authority (the manifest declares the same): it starts with the package, as the sample checks. */
    const val AUTHORITY = "io.github.dolfost.stickpile.whatsapp"

    const val METADATA = "metadata"
    const val STICKERS = "stickers"
    const val STICKERS_ASSET = "stickers_asset"

    fun parse(authority: String?, encodedPath: String?): WhatsAppRoute? {
        if (authority != AUTHORITY || encodedPath == null || !encodedPath.startsWith("/")) return null
        val segments = encodedPath.substring(1).split('/')
        if (!segments.all(::isSegment)) return null
        return when {
            segments == listOf(METADATA) -> WhatsAppRoute.Metadata
            segments.size == 2 && segments[0] == METADATA -> WhatsAppRoute.Pack(segments[1])
            segments.size == 2 && segments[0] == STICKERS -> WhatsAppRoute.Stickers(segments[1])
            segments.size == 3 && segments[0] == STICKERS_ASSET -> WhatsAppRoute.Asset(segments[1], segments[2])
            else -> null
        }
    }

    /**
     * The type of what [route] answers, as the sample gives it: the
     * cursors' `vnd.android.cursor` types, `image/png` for the tray icon
     * and `image/webp` for a sticker (told apart by the name: the library
     * keeps PNG tray icons and WebP stickers only). It depends on the path
     * alone, so it tells nothing about what is unpacked.
     */
    fun type(route: WhatsAppRoute): String = when (route) {
        WhatsAppRoute.Metadata -> "vnd.android.cursor.dir/vnd.$AUTHORITY.$METADATA"
        is WhatsAppRoute.Pack -> "vnd.android.cursor.item/vnd.$AUTHORITY.$METADATA"
        is WhatsAppRoute.Stickers -> "vnd.android.cursor.dir/vnd.$AUTHORITY.$STICKERS"
        is WhatsAppRoute.Asset -> if (route.fileName.endsWith(".png")) "image/png" else "image/webp"
    }

    /** `content://<authority>/stickers_asset/<identifier>/<file name>` (for the log and the tests). */
    fun assetUri(identifier: String, fileName: String) = "content://$AUTHORITY/$STICKERS_ASSET/$identifier/$fileName"

    private fun isSegment(s: String): Boolean =
        s.isNotEmpty() && !s.startsWith('.') &&
            s.all { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it == '.' || it == '-' || it == '_' }
}

/**
 * The columns and rows of the WhatsApp provider's cursors, with the names
 * WhatsApp reads (the sample's `StickerContentProvider`: "Do not change
 * the strings listed below, as these are used by WhatsApp"), in the
 * sample's order.
 */
object WhatsAppRows {
    val PACK_COLUMNS = arrayOf(
        "sticker_pack_identifier",
        "sticker_pack_name",
        "sticker_pack_publisher",
        "sticker_pack_icon",
        "android_play_store_link",
        "ios_app_download_link",
        "sticker_pack_publisher_email",
        "sticker_pack_publisher_website",
        "sticker_pack_privacy_policy_website",
        "sticker_pack_license_agreement_website",
        "image_data_version",
        "whatsapp_will_not_cache_stickers",
        "animated_sticker_pack",
    )

    val STICKER_COLUMNS = arrayOf("sticker_file_name", "sticker_emoji", "sticker_accessibility_text")

    /**
     * A part as a pack row: the links, the e-mail and the websites empty
     * (as the sample's `contents.json` has them), WhatsApp may cache the
     * stickers (0; it does anyway since 2.25.9.78, says the README), and 1
     * for an animated pack.
     */
    fun pack(part: WhatsappPart): Array<Any?> = arrayOf(
        part.identifier,
        part.name,
        part.publisher,
        part.trayFile,
        "",
        "",
        "",
        "",
        "",
        "",
        part.version,
        0,
        if (part.animated) 1 else 0,
    )

    /** A sticker row: its emoji joined with `,` as the sample does; no accessibility text. */
    fun sticker(sticker: WhatsappSticker): Array<Any?> = arrayOf(sticker.file, sticker.emojis.joinToString(","), null)

    /** The parts [identifier] names among [parts]: none or one. */
    fun find(parts: List<WhatsappPart>, identifier: String): WhatsappPart? = parts.firstOrNull { it.identifier == identifier }

    /**
     * The file [fileName] of the part [identifier], when it is the part's
     * tray icon or one of its stickers (as the sample checks the name
     * against the pack's list); null for anything else.
     */
    fun file(parts: List<WhatsappPart>, identifier: String, fileName: String): File? {
        val part = find(parts, identifier) ?: return null
        val known = fileName == part.trayFile || part.stickers.any { it.file == fileName }
        return if (known) File(part.dir, fileName) else null
    }
}
