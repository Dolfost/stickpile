package io.github.dolfost.stickpile.provider

import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction
import java.nio.charset.StandardCharsets

/** What a provider URI asks for (docs/android.md, "Provider contract"). */
sealed interface ProviderRoute {
    /** `/search?q=&package=&limit=` */
    data class Search(val query: String, val editorPackage: String, val limit: Int) : ProviderRoute

    /** `/recent?limit=` */
    data class Recent(val limit: Int) : ProviderRoute

    /** `/favorites?limit=` */
    data class Favorites(val limit: Int) : ProviderRoute

    /** `/packs` */
    data object Packs : ProviderRoute

    /** `/packs/<pack id>/items` */
    data class PackItems(val packId: String) : ProviderRoute

    /** `/thumb/<item id>`, a file. */
    data class Thumbnail(val itemId: String) : ProviderRoute

    /** `/prepared/<token>/<file name>`, a file; the token is the caller's capability. */
    data class PreparedFile(val token: String, val fileName: String) : ProviderRoute
}

/**
 * Turns the parts of a provider URI into a [ProviderRoute], and builds the
 * URIs the provider hands out. Pure: the provider passes `Uri.getAuthority`,
 * `Uri.getEncodedPath` and `Uri.getEncodedQuery`, still percent-encoded, so
 * that every decision about the path is made here, once.
 *
 * Strict on purpose: anything that is not exactly one of the contract's
 * paths is no route (null), including empty or extra segments, `.` and
 * `..` (encoded or not), segments that decode to a `/`, `\`, NUL or other
 * control character, and malformed percent escapes.
 */
object ProviderRoutes {
    /** The provider's authority; the manifest declares the same. */
    const val AUTHORITY = "io.github.dolfost.stickpile.stickers"

    /** Rows of a list when the caller gives no `limit` (or a bad one). */
    const val DEFAULT_LIMIT = 50

    /** The most rows of a list, whatever the caller asks. */
    const val MAX_LIMIT = 200

    /** Hex characters of a token: 128 bits. */
    const val TOKEN_LENGTH = 32

    /** Longest item or pack id accepted. Server ids are 26 characters. */
    private const val MAX_ID_LENGTH = 64

    private const val CONTENT = "content://$AUTHORITY"

    fun parse(authority: String?, encodedPath: String?, encodedQuery: String?): ProviderRoute? {
        if (authority != AUTHORITY || encodedPath == null || !encodedPath.startsWith("/")) return null
        val segments = encodedPath.substring(1).split('/').map { raw ->
            val segment = percentDecode(raw, plusIsSpace = false) ?: return null
            if (!isSafeSegment(segment)) return null
            segment
        }
        val query by lazy { parseQuery(encodedQuery) }
        return when (segments.size) {
            1 -> when (segments[0]) {
                "search" -> query?.let {
                    ProviderRoute.Search(it["q"].orEmpty(), it["package"].orEmpty(), limit(it["limit"]))
                }
                "recent" -> query?.let { ProviderRoute.Recent(limit(it["limit"])) }
                "favorites" -> query?.let { ProviderRoute.Favorites(limit(it["limit"])) }
                "packs" -> ProviderRoute.Packs
                else -> null
            }
            2 -> if (segments[0] == "thumb" && isId(segments[1])) ProviderRoute.Thumbnail(segments[1]) else null
            3 -> when {
                segments[0] == "packs" && isId(segments[1]) && segments[2] == "items" ->
                    ProviderRoute.PackItems(segments[1])
                segments[0] == "prepared" && isToken(segments[1]) ->
                    ProviderRoute.PreparedFile(segments[1], segments[2])
                else -> null
            }
            else -> null
        }
    }

    /** `content://<authority>/thumb/<item id>` */
    fun thumbnailUri(itemId: String): String = "$CONTENT/thumb/${encodeSegment(itemId)}"

    /** `content://<authority>/prepared/<token>/<file name>` */
    fun preparedUri(token: String, fileName: String): String =
        "$CONTENT/prepared/$token/${encodeSegment(fileName)}"

    /**
     * [name] made fit for the last segment of a `/prepared/` URI: separators
     * and control characters become `_`; nothing, `.` and `..` become `file`.
     */
    fun fileName(name: String): String {
        val clean = name.map { if (it == '/' || it == '\\' || Character.isISOControl(it)) '_' else it }.joinToString("")
        return if (clean.isEmpty() || clean == "." || clean == "..") "file" else clean
    }

    /** An id as the server makes them (letters, digits, `-` and `_`), not too long. */
    fun isId(s: String): Boolean =
        s.length in 1..MAX_ID_LENGTH && s.all { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it == '-' || it == '_' }

    /** A token as [PreparedTokens] makes them: 32 lowercase hex characters. */
    fun isToken(s: String): Boolean = s.length == TOKEN_LENGTH && s.all { it in '0'..'9' || it in 'a'..'f' }

    /** A `limit` parameter: absent or not a positive number gives the default; at most [MAX_LIMIT]. */
    fun limit(value: String?): Int {
        val n = value?.toIntOrNull() ?: return DEFAULT_LIMIT
        return if (n < 1) DEFAULT_LIMIT else minOf(n, MAX_LIMIT)
    }

    /**
     * Percent-encodes one path segment: everything but the unreserved
     * characters of RFC 3986 (letters, digits, `-._~`), as UTF-8.
     */
    fun encodeSegment(s: String): String = buildString {
        for (byte in s.toByteArray(StandardCharsets.UTF_8)) {
            val b = byte.toInt() and 0xff
            val c = b.toChar()
            if (c in 'a'..'z' || c in 'A'..'Z' || c in '0'..'9' || c == '-' || c == '.' || c == '_' || c == '~') {
                append(c)
            } else {
                append('%').append(HEX[b shr 4]).append(HEX[b and 0x0f])
            }
        }
    }

    /**
     * Decodes `%XX` escapes as UTF-8 (and `+` as a space in a query, as
     * `Uri.getQueryParameter` does); null for a malformed escape or bytes
     * that are not UTF-8.
     */
    fun percentDecode(s: String, plusIsSpace: Boolean): String? {
        if ('%' !in s && !(plusIsSpace && '+' in s)) return s
        val bytes = ByteArrayOutputStream(s.length)
        var i = 0
        while (i < s.length) {
            val c = s[i]
            when {
                c == '%' -> {
                    if (i + 2 >= s.length) return null
                    val high = Character.digit(s[i + 1], 16)
                    val low = Character.digit(s[i + 2], 16)
                    if (high < 0 || low < 0) return null
                    bytes.write(high * 16 + low)
                    i += 3
                }
                c == '+' && plusIsSpace -> {
                    bytes.write(' '.code)
                    i++
                }
                else -> {
                    // A run of literal characters, written as UTF-8.
                    var end = i + 1
                    while (end < s.length && s[end] != '%' && !(plusIsSpace && s[end] == '+')) end++
                    bytes.write(s.substring(i, end).toByteArray(StandardCharsets.UTF_8))
                    i = end
                }
            }
        }
        return try {
            StandardCharsets.UTF_8.newDecoder()
                .onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT)
                .decode(ByteBuffer.wrap(bytes.toByteArray()))
                .toString()
        } catch (_: CharacterCodingException) {
            null
        }
    }

    /** Not empty, not `.` or `..`, no separator and no control character. */
    private fun isSafeSegment(segment: String): Boolean =
        segment.isNotEmpty() && segment != "." && segment != ".." &&
            segment.none { it == '/' || it == '\\' || Character.isISOControl(it) }

    /** The query's parameters, the first of each name; null when one is malformed. */
    private fun parseQuery(encodedQuery: String?): Map<String, String>? {
        if (encodedQuery.isNullOrEmpty()) return emptyMap()
        val out = HashMap<String, String>()
        for (pair in encodedQuery.split('&')) {
            if (pair.isEmpty()) continue
            val eq = pair.indexOf('=')
            val name = percentDecode(if (eq < 0) pair else pair.substring(0, eq), plusIsSpace = true) ?: return null
            val value = if (eq < 0) "" else percentDecode(pair.substring(eq + 1), plusIsSpace = true) ?: return null
            out.putIfAbsent(name, value)
        }
        return out
    }

    private const val HEX = "0123456789ABCDEF"
}
