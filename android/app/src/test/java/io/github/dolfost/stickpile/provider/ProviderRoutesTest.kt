package io.github.dolfost.stickpile.provider

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ProviderRoutesTest {
    private val auth = ProviderRoutes.AUTHORITY
    private val token = "0123456789abcdef0123456789abcdef"
    private val id = "abcdefghijklmnopqrstuvwxyz"

    private fun parse(path: String?, query: String? = null) = ProviderRoutes.parse(auth, path, query)

    @Test
    fun searchTakesTheQueryThePackageAndTheLimit() {
        assertEquals(
            ProviderRoute.Search("grumpy cat", "org.telegram.messenger", 20),
            parse("/search", "q=grumpy+cat&package=org.telegram.messenger&limit=20"),
        )
        assertEquals(ProviderRoute.Search("a b", "", ProviderRoutes.DEFAULT_LIMIT), parse("/search", "q=a%20b"))
        assertEquals(ProviderRoute.Search("", "", ProviderRoutes.DEFAULT_LIMIT), parse("/search"))
        // UTF-8, and a literal + encoded.
        assertEquals(ProviderRoute.Search("❤ 1+1", "", 50), parse("/search", "q=%E2%9D%A4+1%2B1"))
        // The first of a repeated parameter wins, as with Uri.getQueryParameter.
        assertEquals(ProviderRoute.Search("one", "", 50), parse("/search", "q=one&q=two"))
        // Unknown parameters are ignored.
        assertEquals(ProviderRoute.Search("x", "", 50), parse("/search", "q=x&sort=new&&flag"))
    }

    @Test
    fun limitsDefaultAndAreCapped() {
        assertEquals(ProviderRoute.Recent(ProviderRoutes.DEFAULT_LIMIT), parse("/recent"))
        assertEquals(ProviderRoute.Recent(7), parse("/recent", "limit=7"))
        assertEquals(ProviderRoute.Recent(ProviderRoutes.MAX_LIMIT), parse("/recent", "limit=100000"))
        assertEquals(ProviderRoute.Recent(ProviderRoutes.DEFAULT_LIMIT), parse("/recent", "limit=0"))
        assertEquals(ProviderRoute.Recent(ProviderRoutes.DEFAULT_LIMIT), parse("/recent", "limit=-3"))
        assertEquals(ProviderRoute.Recent(ProviderRoutes.DEFAULT_LIMIT), parse("/recent", "limit=ten"))
        assertEquals(ProviderRoute.Recent(ProviderRoutes.DEFAULT_LIMIT), parse("/recent", "limit=99999999999"))
        assertEquals(ProviderRoute.Favorites(3), parse("/favorites", "limit=3"))
    }

    @Test
    fun packsAndTheirItems() {
        assertEquals(ProviderRoute.Packs, parse("/packs"))
        assertEquals(ProviderRoute.PackItems(id), parse("/packs/$id/items"))
        assertEquals(ProviderRoute.PackItems("Old_id-1"), parse("/packs/Old_id-1/items"))
    }

    @Test
    fun files() {
        assertEquals(ProviderRoute.Thumbnail(id), parse("/thumb/$id"))
        assertEquals(
            ProviderRoute.PreparedFile(token, "grumpy cat.png"),
            parse("/prepared/$token/grumpy%20cat.png"),
        )
        // Decoded once only: a double-encoded dot is a literal name.
        assertEquals(ProviderRoute.PreparedFile(token, "%2e%2e"), parse("/prepared/$token/%252e%252e"))
        // Query strings do not matter for files.
        assertEquals(ProviderRoute.Thumbnail(id), parse("/thumb/$id", "x=1"))
    }

    @Test
    fun unknownPathsAndExtraSegmentsAreNoRoute() {
        listOf(
            "", "/", "//", "/search/", "/search/x", "/recent/", "//recent", "/recent//", "/Recent",
            "/favorites/1", "/packs/", "/packs/$id", "/packs/$id/items/", "/packs/$id/items/x",
            "/packs//items", "/thumb", "/thumb/", "/thumb/$id/", "/thumb/$id/x", "/prepared",
            "/prepared/$token", "/prepared/$token/", "/prepared/$token/a/b", "/prepared//a.png",
            "/other", "search", "/info",
        ).forEach { assertNull(it, parse(it)) }
        assertNull(parse(null))
        assertNull(ProviderRoutes.parse("io.github.dolfost.stickpile.other", "/recent", null))
        assertNull(ProviderRoutes.parse(null, "/recent", null))
    }

    @Test
    fun traversalIsRejectedEncodedOrNot() {
        listOf(
            "/thumb/..", "/thumb/.", "/thumb/%2e%2e", "/thumb/%2E%2E", "/thumb/.%2e",
            "/thumb/a%2Fb", "/thumb/a%2fb", "/thumb/a%5Cb", "/thumb/%2e%2e%2f%2e%2e%2fetc",
            "/thumb/../../etc/passwd", "/thumb/a/../b",
            "/prepared/$token/..", "/prepared/$token/.", "/prepared/$token/%2e%2e",
            "/prepared/$token/..%2F..%2Fdatabases%2Fcache.db", "/prepared/$token/a%2Fb.png",
            "/prepared/$token/a%5Cb.png", "/prepared/$token/a%00b.png", "/prepared/$token/a%0Ab.png",
            "/packs/..%2F..%2F/items", "/packs/%2e%2e/items", "/packs/../items",
            "/prepared/../thumb/$id",
        ).forEach { assertNull(it, parse(it)) }
    }

    @Test
    fun idsAndTokensMustLookRight() {
        listOf(
            "/thumb/a%20b", "/thumb/a.b", "/thumb/%252e", "/thumb/" + "a".repeat(65),
            "/packs/a%20b/items", "/packs/a:b/items",
            "/prepared/0123456789ABCDEF0123456789ABCDEF/a.png",
            "/prepared/0123456789abcdef0123456789abcde/a.png",
            "/prepared/0123456789abcdef0123456789abcdef0/a.png",
            "/prepared/0123456789abcdef0123456789abcdeg/a.png",
        ).forEach { assertNull(it, parse(it)) }
        assertEquals(ProviderRoute.Thumbnail("a".repeat(64)), parse("/thumb/" + "a".repeat(64)))
    }

    @Test
    fun malformedEscapesAreNoRoute() {
        listOf("/thumb/%zz", "/thumb/%2", "/thumb/%", "/prepared/$token/a%c3.png", "/prepared/$token/%ff")
            .forEach { assertNull(it, parse(it)) }
        assertNull(parse("/search", "q=%zz"))
        assertNull(parse("/search", "q=%e2%9d"))
        assertNull(parse("/recent", "li%mit=3"))
    }

    @Test
    fun segmentsAreEncodedAndDecodedBack() {
        assertEquals("grumpy%20cat.png", ProviderRoutes.encodeSegment("grumpy cat.png"))
        assertEquals("a%2Fb%25c%3F%23d", ProviderRoutes.encodeSegment("a/b%c?#d"))
        assertEquals("AZaz09-._~", ProviderRoutes.encodeSegment("AZaz09-._~"))
        listOf("grumpy cat.png", "кіт 😺.webp", "100% done+.gif", "a&b=c.mp4").forEach {
            assertEquals(it, ProviderRoutes.percentDecode(ProviderRoutes.encodeSegment(it), plusIsSpace = false))
        }
        assertEquals("a+b", ProviderRoutes.percentDecode("a+b", plusIsSpace = false))
        assertEquals("a b", ProviderRoutes.percentDecode("a+b", plusIsSpace = true))
    }

    @Test
    fun handedOutUrisParseBack() {
        val name = "кіт #1 100%.png"
        val uri = ProviderRoutes.preparedUri(token, name)
        assertEquals(
            "content://io.github.dolfost.stickpile.stickers/prepared/$token/" +
                "%D0%BA%D1%96%D1%82%20%231%20100%25.png",
            uri,
        )
        val path = uri.removePrefix("content://$auth")
        assertEquals(ProviderRoute.PreparedFile(token, name), parse(path))

        val thumb = ProviderRoutes.thumbnailUri(id)
        assertEquals("content://io.github.dolfost.stickpile.stickers/thumb/$id", thumb)
        assertEquals(ProviderRoute.Thumbnail(id), parse(thumb.removePrefix("content://$auth")))
    }

    @Test
    fun fileNamesAreMadeSafeForTheUri() {
        assertEquals("grumpy cat.png", ProviderRoutes.fileName("grumpy cat.png"))
        assertEquals("a_b_c.png", ProviderRoutes.fileName("a/b\\c.png"))
        assertEquals("a_b.png", ProviderRoutes.fileName("a\nb.png"))
        assertEquals("file", ProviderRoutes.fileName(""))
        assertEquals("file", ProviderRoutes.fileName("."))
        assertEquals("file", ProviderRoutes.fileName(".."))
        assertEquals("...png", ProviderRoutes.fileName("...png"))
        // Whatever the name, the URI made from it parses back to it.
        listOf("a/b", "..", "x\u0000y", "\t", "%2e%2e").forEach {
            val name = ProviderRoutes.fileName(it)
            val path = ProviderRoutes.preparedUri(token, name).removePrefix("content://$auth")
            assertEquals(it, ProviderRoute.PreparedFile(token, name), parse(path))
        }
    }
}
