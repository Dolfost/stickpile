package io.github.dolfost.stickpile.share

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FileNamesTest {
    private fun bytes(s: String) = s.toByteArray(Charsets.UTF_8).size

    @Test
    fun aPlainNameIsKept() {
        assertEquals("cat.png", FileNames.forCopy("cat.png", "image/png"))
        assertEquals("Кіт у капелюсі.jpg", FileNames.forCopy("Кіт у капелюсі.jpg", "image/jpeg"))
    }

    @Test
    fun theExtensionIsLowerCased() {
        assertEquals("IMG_0001.jpg", FileNames.forCopy("IMG_0001.JPG", "image/jpeg"))
    }

    @Test
    fun onlyTheLastPathPartIsUsed() {
        assertEquals("cat.png", FileNames.forCopy("/storage/emulated/0/DCIM/cat.png", null))
        assertEquals("cat.png", FileNames.forCopy("..\\..\\cat.png", null))
        assertEquals("cat.png", FileNames.forCopy("../../cat.png", null))
    }

    @Test
    fun neverADotNameOrAHiddenFile() {
        assertEquals(FileNames.FALLBACK_STEM, FileNames.forCopy(".", null))
        assertEquals(FileNames.FALLBACK_STEM, FileNames.forCopy("..", null))
        assertEquals("${FileNames.FALLBACK_STEM}.png", FileNames.forCopy("..", "image/png"))
        assertEquals("hidden.png", FileNames.forCopy(".hidden.png", null))
        assertEquals("cat", FileNames.forCopy("cat...", null))
    }

    @Test
    fun controlAndBidiCharactersGo() {
        assertEquals("catdog.png", FileNames.forCopy("cat\u0000\n\tdog.png", null))
        assertEquals("photogpj.exe", FileNames.forCopy("photo\u202Egpj.exe", null))
        assertEquals("a b.gif", FileNames.forCopy("a\u2066 b\u2069.gif", null))
    }

    @Test
    fun anEmptyOrMissingNameFallsBack() {
        assertEquals("shared.png", FileNames.forCopy(null, "image/png"))
        assertEquals("shared.mp4", FileNames.forCopy("", "video/mp4"))
        assertEquals("shared.webm", FileNames.forCopy("   ", "video/webm"))
        assertEquals("shared", FileNames.forCopy("\u0001\u0002", null))
        assertEquals("shared", FileNames.forCopy("/", "application/octet-stream"))
    }

    @Test
    fun aNameWithoutExtensionGetsTheTypes() {
        assertEquals("1000012345.jpg", FileNames.forCopy("1000012345", "image/jpeg"))
        assertEquals("cat.webp", FileNames.forCopy("cat", "image/webp; charset=binary"))
        assertEquals("cat", FileNames.forCopy("cat", "image/heic"))
        assertEquals("cat", FileNames.forCopy("cat", null))
        // A dot in a name is not an extension.
        assertEquals("Mr. Bean reacts.gif", FileNames.forCopy("Mr. Bean reacts", "image/gif"))
    }

    @Test
    fun aNamesOwnExtensionWins() {
        assertEquals("clip.mov", FileNames.forCopy("clip.mov", "video/mp4"))
    }

    @Test
    fun aVeryLongNameIsCutAndKeepsItsExtension() {
        val long = "a".repeat(300) + ".png"
        val name = FileNames.forCopy(long, null)
        assertEquals(FileNames.MAX_FILE_NAME_BYTES, bytes(name))
        assertTrue(name.endsWith("a.png"))

        // Cut between code points, never inside one.
        val wide = "\uD83D\uDE00".repeat(100) + ".gif"
        val cut = FileNames.forCopy(wide, null)
        assertTrue(bytes(cut) <= FileNames.MAX_FILE_NAME_BYTES)
        assertTrue(cut.endsWith(".gif"))
        assertEquals(cut, String(cut.toByteArray(Charsets.UTF_8), Charsets.UTF_8))
        assertEquals(0, cut.removeSuffix(".gif").length % 2)

        val cyrillic = "ж".repeat(200)
        val cutCyrillic = FileNames.forCopy(cyrillic, "image/png")
        assertTrue(bytes(cutCyrillic) <= FileNames.MAX_FILE_NAME_BYTES)
        assertTrue(cutCyrillic.endsWith("ж.png"))
    }

    @Test
    fun extensions() {
        assertEquals("jpg", FileNames.extensionFor("IMAGE/JPEG"))
        assertEquals("gif", FileNames.extensionFor("image/gif"))
        assertNull(FileNames.extensionFor("image/*"))
        assertNull(FileNames.extensionFor(null))
    }

    @Test
    fun theDefaultItemNameIsTheNameWithoutExtension() {
        assertEquals("cat", FileNames.defaultItemName("cat.png"))
        assertEquals("cat.tar", FileNames.defaultItemName("cat.tar.gz"))
        assertEquals("Mr. Bean reacts", FileNames.defaultItemName("Mr. Bean reacts"))
        assertEquals("2024-01-01 meme", FileNames.defaultItemName("2024-01-01 meme.jpg"))
        assertEquals("Кіт", FileNames.defaultItemName("Кіт.webp"))
        assertEquals(".hidden", FileNames.defaultItemName(".hidden"))
    }

    @Test
    fun theDefaultItemNameIsCleaned() {
        assertEquals("cat", FileNames.defaultItemName("/sdcard/Pictures/cat.png"))
        assertEquals("a cat", FileNames.defaultItemName("  a\u0000 \t cat .png"))
        assertEquals("ab", FileNames.defaultItemName("a\u202Eb.png"))
    }

    @Test
    fun theDefaultItemNameMayBeEmpty() {
        assertEquals("", FileNames.defaultItemName(null))
        assertEquals("", FileNames.defaultItemName(""))
        assertEquals("", FileNames.defaultItemName("dir/"))
        assertEquals("", FileNames.defaultItemName("\u0007"))
    }

    @Test
    fun theDefaultItemNameFitsTheServersLimit() {
        val name = FileNames.defaultItemName("b".repeat(500) + ".png")
        assertEquals(FileNames.MAX_ITEM_NAME_CHARS, name.length)
        val emoji = FileNames.defaultItemName("\uD83D\uDE00".repeat(300))
        assertEquals(FileNames.MAX_ITEM_NAME_CHARS, emoji.codePointCount(0, emoji.length))
        assertFalse(Character.isHighSurrogate(emoji.last()))
    }
}
