package io.github.dolfost.stickpile.whatsapp

import io.github.dolfost.stickpile.rust.WhatsappSticker
import io.github.dolfost.stickpile.ui.whatsappPart
import java.io.File
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The WhatsApp provider's contract as WhatsApp's sample sticker app has
 * it (`StickerContentProvider.java`): its paths, types, columns and rows,
 * which files it serves, and who may read.
 */
class WhatsAppProviderTest {
    private val a = "content://io.github.dolfost.stickpile.whatsapp"
    private val authority = WhatsAppRoutes.AUTHORITY

    private fun parse(path: String) = WhatsAppRoutes.parse(authority, path)

    @Test
    fun theSamplesPathsAreRoutes() {
        assertEquals(WhatsAppRoute.Metadata, parse("/metadata"))
        assertEquals(WhatsAppRoute.Pack("p1-1"), parse("/metadata/p1-1"))
        assertEquals(WhatsAppRoute.Stickers("p1-1"), parse("/stickers/p1-1"))
        assertEquals(WhatsAppRoute.Asset("p1-1", "01.webp"), parse("/stickers_asset/p1-1/01.webp"))
        assertEquals(WhatsAppRoute.Asset("p1-2", "cover.png"), parse("/stickers_asset/p1-2/cover.png"))
    }

    @Test
    fun anythingElseIsNoRoute() {
        for (path in listOf(
            "", "/", "metadata", "/metadata/", "/metadata/a/b", "/stickers", "/stickers/", "/stickers_asset/p1-1",
            "/stickers_asset/p1-1/", "/stickers_asset/p1-1/a/b", "/stickers_asset/../01.webp",
            "/stickers_asset/p1-1/..", "/stickers_asset/p1-1/.hidden", "/stickers_asset/p1-1/%2e%2e",
            "/stickers_asset/p1-1/a%2Fb", "/stickers_asset/p1-1/a b", "/stickers_asset/p1-1/a\\b", "/thumb/x", "//metadata",
        )) {
            assertNull(path, parse(path))
        }
        assertNull(WhatsAppRoutes.parse("io.github.dolfost.stickpile.stickers", "/metadata"))
        assertNull(WhatsAppRoutes.parse(null, "/metadata"))
        assertNull(WhatsAppRoutes.parse(authority, null))
    }

    @Test
    fun typesAreTheSamples() {
        assertEquals(
            "vnd.android.cursor.dir/vnd.io.github.dolfost.stickpile.whatsapp.metadata",
            WhatsAppRoutes.type(WhatsAppRoute.Metadata),
        )
        assertEquals(
            "vnd.android.cursor.item/vnd.io.github.dolfost.stickpile.whatsapp.metadata",
            WhatsAppRoutes.type(WhatsAppRoute.Pack("p1-1")),
        )
        assertEquals(
            "vnd.android.cursor.dir/vnd.io.github.dolfost.stickpile.whatsapp.stickers",
            WhatsAppRoutes.type(WhatsAppRoute.Stickers("p1-1")),
        )
        assertEquals("image/png", WhatsAppRoutes.type(WhatsAppRoute.Asset("p1-1", "cover.png")))
        assertEquals("image/webp", WhatsAppRoutes.type(WhatsAppRoute.Asset("p1-1", "01.webp")))
        assertEquals("$a/stickers_asset/p1-1/01.webp", WhatsAppRoutes.assetUri("p1-1", "01.webp"))
    }

    @Test
    fun packRowsHaveTheColumnsWhatsAppReads() {
        assertArrayEquals(
            arrayOf(
                "sticker_pack_identifier", "sticker_pack_name", "sticker_pack_publisher", "sticker_pack_icon",
                "android_play_store_link", "ios_app_download_link", "sticker_pack_publisher_email",
                "sticker_pack_publisher_website", "sticker_pack_privacy_policy_website",
                "sticker_pack_license_agreement_website", "image_data_version", "whatsapp_will_not_cache_stickers",
                "animated_sticker_pack",
            ),
            WhatsAppRows.PACK_COLUMNS,
        )
        val row = WhatsAppRows.pack(whatsappPart("p1", number = 2))
        assertEquals(WhatsAppRows.PACK_COLUMNS.size, row.size)
        assertArrayEquals(
            arrayOf<Any?>("p1-2", "Pack p1", "Alice", "cover.png", "", "", "", "", "", "", "00ff00ff00ff00ff", 0, 0),
            row,
        )
        assertEquals(1, WhatsAppRows.pack(whatsappPart("p1", animated = true)).last())
    }

    @Test
    fun stickerRowsJoinTheEmojiWithCommas() {
        assertArrayEquals(
            arrayOf("sticker_file_name", "sticker_emoji", "sticker_accessibility_text"),
            WhatsAppRows.STICKER_COLUMNS,
        )
        val sticker = WhatsappSticker(file = "07.webp", emojis = listOf("😾", "❤️", "x"))
        assertArrayEquals(arrayOf<Any?>("07.webp", "😾,❤️,x", null), WhatsAppRows.sticker(sticker))
    }

    @Test
    fun onlyAPartsOwnFilesAreServed() {
        val parts = listOf(whatsappPart("p1", 1), whatsappPart("p1", 2, stickers = 4))
        assertEquals(parts[1], WhatsAppRows.find(parts, "p1-2"))
        assertNull(WhatsAppRows.find(parts, "p1-3"))
        assertEquals(File(parts[1].dir, "04.webp"), WhatsAppRows.file(parts, "p1-2", "04.webp"))
        assertEquals(File(parts[0].dir, "cover.png"), WhatsAppRows.file(parts, "p1-1", "cover.png"))
        assertNull("part 1 has three", WhatsAppRows.file(parts, "p1-1", "04.webp"))
        assertNull(WhatsAppRows.file(parts, "p1-1", "index.json"))
        assertNull(WhatsAppRows.file(parts, "p9-1", "01.webp"))
    }

    @Test
    fun onlyWhatsAppTheAppAndRootMayRead() {
        val own = 10_123
        assertTrue(WhatsAppAccess.allowed(own, own, null))
        assertTrue(WhatsAppAccess.allowed(0, own, null))
        assertTrue(WhatsAppAccess.allowed(10_200, own, arrayOf("com.whatsapp")))
        assertTrue(WhatsAppAccess.allowed(10_201, own, arrayOf("com.whatsapp.w4b")))
        assertFalse(WhatsAppAccess.allowed(10_202, own, arrayOf("com.example.keyboard")))
        assertFalse(WhatsAppAccess.allowed(10_203, own, null))
        assertFalse(WhatsAppAccess.allowed(2000, own, arrayOf("com.android.shell")))
    }
}
