package io.github.dolfost.stickpile.provider

import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.LibraryErrorKind
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.MediaKind
import io.github.dolfost.stickpile.rust.Pack
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test

class ProviderRowsTest {
    private val item = Item(
        id = "abcdefghijklmnopqrstuvwxyz",
        name = "grumpy cat",
        setId = "set",
        setName = "Cats",
        emojis = listOf("😾", "❤️"),
        keywords = listOf("cat"),
        vibe = null,
        tags = emptyList(),
        kind = MediaKind.ANIMATION,
        width = 512u,
        height = 384u,
        likes = 3uL,
        copies = 9uL,
    )

    @Test
    fun itemRowsFollowTheContractsColumns() {
        assertArrayEquals(
            arrayOf("_id", "item_id", "name", "set_name", "emojis", "kind", "width", "height", "thumbnail_uri"),
            ProviderRows.ITEM_COLUMNS,
        )
        val row = ProviderRows.item(4, item)
        assertEquals(ProviderRows.ITEM_COLUMNS.size, row.size)
        assertArrayEquals(
            arrayOf<Any?>(
                4L, item.id, "grumpy cat", "Cats", "😾❤️", "animation", 512L, 384L,
                "content://io.github.dolfost.stickpile.stickers/thumb/abcdefghijklmnopqrstuvwxyz",
            ),
            row,
        )
    }

    @Test
    fun kindsHaveTheContractsNames() {
        assertEquals("still", ProviderRows.kindName(MediaKind.STILL))
        assertEquals("animation", ProviderRows.kindName(MediaKind.ANIMATION))
        assertEquals("video", ProviderRows.kindName(MediaKind.VIDEO))
    }

    @Test
    fun packRowsFollowTheContractsColumns() {
        assertArrayEquals(arrayOf("_id", "pack_id", "name", "owner", "item_count"), ProviderRows.PACK_COLUMNS)
        val pack = Pack(id = "p1", slug = "cats", name = "Cats", owner = "olha", itemCount = 12u, mine = false)
        assertArrayEquals(arrayOf<Any?>(0L, "p1", "Cats", "olha", 12L), ProviderRows.pack(0, pack))
    }

    @Test
    fun preparedFilesAnswerTheOpenableColumnsAskedFor() {
        val (all, allValues) = ProviderRows.openable(null, "a.png", 42L)
        assertArrayEquals(arrayOf("_display_name", "_size"), all)
        assertArrayEquals(arrayOf<Any?>("a.png", 42L), allValues)

        val (some, someValues) = ProviderRows.openable(arrayOf("_size", "_data", "_display_name", "_size"), "a.png", 42L)
        assertArrayEquals(arrayOf("_size", "_display_name"), some)
        assertArrayEquals(arrayOf<Any?>(42L, "a.png"), someValues)
    }

    @Test
    fun libraryErrorsMapToTheContractsCodes() {
        val cases = mapOf(
            LibraryException.NothingFits("d") to "nothing_fits",
            LibraryException.NotFound("d") to "not_found",
            LibraryException.NotSignedIn("d") to "not_signed_in",
            LibraryException.Unauthorized("d") to "not_signed_in",
            LibraryException.Unreachable("d") to "unreachable",
            LibraryException.RateLimited("d") to "failed",
            LibraryException.InvalidInput("variant", "d") to "failed",
            LibraryException.Server(500u, "d") to "failed",
            LibraryException.Io("d") to "failed",
            LibraryException.Other("d") to "failed",
        )
        assertEquals(LibraryErrorKind.entries.toSet(), cases.keys.map { it.kind }.toSet())
        cases.forEach { (e, code) -> assertEquals(e.toString(), code, ProviderErrors.code(e.kind)) }
    }
}
