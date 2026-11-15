package io.github.dolfost.stickpile.share

/**
 * Names for shared files (pure, no Android): the name of a file's temporary
 * copy, and the item name the share screen suggests.
 *
 * The copy is named after the shared file because `StickpileLibrary.upload`
 * with no item name lets the server name the item after the file name
 * (its stem: `stickpile_setfmt::display_stem`).
 */
object FileNames {
    /**
     * At most this many UTF-8 bytes in a copy's file name. ext4 and f2fs
     * allow 255; the rest is headroom for whatever the server does with it.
     */
    const val MAX_FILE_NAME_BYTES = 120

    /** The server's limit for an item name, in characters (`clean_name`). */
    const val MAX_ITEM_NAME_CHARS = 200

    /** The stem of a copy whose shared file has no usable name. */
    const val FALLBACK_STEM = "shared"

    /** An extension is 1 to 10 ASCII letters or digits after the last dot. */
    private const val MAX_EXTENSION_CHARS = 10

    /**
     * The extension for a MIME type of the formats the server takes (the
     * types `stickpile_core::client::mime_from_name` maps back), or null.
     */
    fun extensionFor(mimeType: String?): String? =
        when (mimeType?.substringBefore(';')?.trim()?.lowercase()) {
            "image/png" -> "png"
            "image/jpeg", "image/jpg" -> "jpg"
            "image/gif" -> "gif"
            "image/webp" -> "webp"
            "video/mp4" -> "mp4"
            "video/webm" -> "webm"
            else -> null
        }

    /**
     * The file name for the copy of a shared file that its provider calls
     * [displayName] (`OpenableColumns.DISPLAY_NAME`, may be null) and types
     * [mimeType]: the last part of the name only (no path separators),
     * without control or bidirectional formatting characters, without
     * leading dots (never `.` or `..`, never hidden), at most
     * [MAX_FILE_NAME_BYTES] bytes with the extension kept, and with the
     * MIME type's extension added when the name has none. Never empty:
     * [FALLBACK_STEM] stands in for a name with nothing left.
     */
    fun forCopy(displayName: String?, mimeType: String?): String {
        val clean = lastPart(displayName.orEmpty())
            .filterNot(::isUnwanted)
            .trim()
            .trimStart('.', ' ')
            .trimEnd('.', ' ')
        val (stem, ownExtension) = split(clean)
        val extension = ownExtension ?: extensionFor(mimeType)
        val suffix = extension?.let { ".$it" }.orEmpty()
        val room = MAX_FILE_NAME_BYTES - suffix.utf8Size()
        val cut = truncateUtf8(stem, room).trimEnd('.', ' ')
        return cut.ifEmpty { FALLBACK_STEM } + suffix
    }

    /**
     * The item name the share screen suggests for a shared file called
     * [displayName]: its last part without the extension, without control
     * or bidirectional formatting characters, runs of white space as one
     * space, at most [MAX_ITEM_NAME_CHARS] characters. Empty when nothing
     * is left (then the server names the item after the copy's file name).
     */
    fun defaultItemName(displayName: String?): String {
        val clean = lastPart(displayName.orEmpty())
            .filterNot(::isUnwanted)
            .replace(WHITE_SPACE, " ")
            .trim()
        val stem = split(clean).first.trim()
        return truncateChars(stem, MAX_ITEM_NAME_CHARS).trim()
    }

    private val WHITE_SPACE = Regex("\\s+")

    /** What follows the last `/` or `\`. */
    private fun lastPart(name: String): String = name.substring(name.lastIndexOfAny(charArrayOf('/', '\\')) + 1)

    /**
     * The name without and with its extension: the part after the last dot
     * when the dot is not the first character and the part looks like an
     * extension ("Mr. Bean" has none).
     */
    private fun split(name: String): Pair<String, String?> {
        val dot = name.lastIndexOf('.')
        if (dot <= 0) return name to null
        val extension = name.substring(dot + 1)
        val looksLikeOne = extension.length in 1..MAX_EXTENSION_CHARS &&
            extension.all { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' }
        return if (looksLikeOne) name.substring(0, dot) to extension.lowercase() else name to null
    }

    /**
     * Control characters (the server refuses them in names) and the
     * bidirectional formatting characters, which can make a name read as
     * something else (a name ending in `.exe` shown as one ending in `.jpg`).
     */
    private fun isUnwanted(c: Char): Boolean =
        Character.isISOControl(c) || c in '\u202A'..'\u202E' || c in '\u2066'..'\u2069' ||
            c == '\u200E' || c == '\u200F' || c == '\u061C'

    private fun String.utf8Size(): Int = toByteArray(Charsets.UTF_8).size

    /** The longest prefix of [s] of at most [maxBytes] UTF-8 bytes, cut between code points. */
    private fun truncateUtf8(s: String, maxBytes: Int): String {
        if (maxBytes <= 0) return ""
        var bytes = 0
        var end = 0
        while (end < s.length) {
            val cp = s.codePointAt(end)
            val size = String(Character.toChars(cp)).utf8Size()
            if (bytes + size > maxBytes) break
            bytes += size
            end += Character.charCount(cp)
        }
        return s.substring(0, end)
    }

    /** The first [max] code points of [s]. */
    private fun truncateChars(s: String, max: Int): String {
        if (s.codePointCount(0, s.length) <= max) return s
        return s.substring(0, s.offsetByCodePoints(0, max))
    }
}
