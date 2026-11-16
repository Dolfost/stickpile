package io.github.dolfost.stickpile.ui

import io.github.dolfost.stickpile.rust.Account
import io.github.dolfost.stickpile.rust.AuthKind
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.ItemList
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.MediaFormat
import io.github.dolfost.stickpile.rust.MediaKind
import io.github.dolfost.stickpile.rust.Motion
import io.github.dolfost.stickpile.rust.Pack
import io.github.dolfost.stickpile.rust.PackList
import io.github.dolfost.stickpile.rust.PairingLink
import io.github.dolfost.stickpile.rust.Prepared
import io.github.dolfost.stickpile.rust.SearchHint
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import io.github.dolfost.stickpile.rust.Suggestions
import io.github.dolfost.stickpile.rust.SyncState
import io.github.dolfost.stickpile.rust.Target
import io.github.dolfost.stickpile.rust.UploadFile
import io.github.dolfost.stickpile.rust.WhatsappPacks
import io.github.dolfost.stickpile.rust.WhatsappPart
import io.github.dolfost.stickpile.rust.WhatsappProgress
import io.github.dolfost.stickpile.rust.WhatsappSticker

/**
 * A [StickpileLibraryInterface] for the view model tests: every call is
 * recorded in [calls] (secrets included: test data only) and answered by
 * the matching handler, which a test replaces. A handler left alone
 * fails the test when it is called, except for the harmless reads.
 */
class FakeLibrary : StickpileLibraryInterface {
    val calls = mutableListOf<String>()

    var account: suspend () -> Account? = { null }
    var setServerUrl: suspend (String) -> Unit = { }
    var loginTailnet: suspend () -> Account? = { unexpected("loginTailnet") }
    var loginPassword: suspend (String, String, String?) -> Account = { _, _, _ -> unexpected("loginPassword") }
    var loginPairing: suspend (String, String) -> Account = { _, _ -> unexpected("loginPairing") }
    var logout: suspend () -> Unit = { }
    var search: suspend (String) -> ItemList = { ItemList(emptyList(), false) }
    var recent: (UInt) -> List<Item> = { emptyList() }
    var favorites: suspend () -> ItemList = { ItemList(emptyList(), false) }
    var mostUsed: suspend () -> ItemList = { ItemList(emptyList(), false) }
    var packs: suspend () -> PackList = { PackList(emptyList(), false) }
    var packItems: suspend (String) -> ItemList = { ItemList(emptyList(), false) }

    /** The pack's id and the query. */
    var packSearch: suspend (String, String) -> ItemList = { _, _ -> ItemList(emptyList(), false) }
    var prepare: suspend (String) -> Prepared = { unexpected("prepare") }
    var toggleLike: suspend (String) -> Boolean = { unexpected("toggleLike") }
    var parsePairingLink: (String) -> PairingLink? = { null }
    var normalizePairingCode: (String) -> String? = { null }
    var syncNow: suspend () -> SyncState = { syncState }
    var syncIfDue: suspend () -> SyncState = { syncState }
    var syncState = SyncState(lastSync = null, lastError = null, pendingUsage = 0u, due = false)
    var pendingUsage: UInt = 0u
    var cacheBytes: () -> ULong = { 0u }
    var clearCache: () -> ULong = { 0u }
    var serverUrl = "http://localhost:8080"

    /** The pack's id. */
    var whatsappPacks: suspend (String) -> WhatsappPacks = { unexpected("whatsappPacks") }

    /** Polled, so not recorded in [calls]. */
    var whatsappProgress: (String) -> WhatsappProgress? = { null }
    var whatsappUnpacked: () -> List<WhatsappPart> = { emptyList() }

    private fun unexpected(name: String): Nothing = throw AssertionError("unexpected call: $name")

    override suspend fun account(): Account? {
        calls += "account"
        return account.invoke()
    }

    override fun cacheBytes(): ULong {
        calls += "cacheBytes"
        return cacheBytes.invoke()
    }

    override fun clearCache(): ULong {
        calls += "clearCache"
        return clearCache.invoke()
    }

    override suspend fun favorites(limit: UInt): ItemList {
        calls += "favorites"
        return favorites.invoke()
    }

    override suspend fun loginPairing(serverUrl: String, code: String): Account {
        calls += "loginPairing $serverUrl $code"
        return loginPairing.invoke(serverUrl, code)
    }

    override suspend fun loginPassword(username: String, password: String, totp: String?): Account {
        calls += "loginPassword $username $password $totp"
        return loginPassword.invoke(username, password, totp)
    }

    override suspend fun loginTailnet(): Account? {
        calls += "loginTailnet"
        return loginTailnet.invoke()
    }

    override suspend fun logout() {
        calls += "logout"
        logout.invoke()
    }

    override suspend fun mostUsed(limit: UInt): ItemList {
        calls += "mostUsed"
        return mostUsed.invoke()
    }

    override fun normalizePairingCode(input: String): String? {
        calls += "normalizePairingCode $input"
        return normalizePairingCode.invoke(input)
    }

    override suspend fun packItems(packId: String): ItemList {
        calls += "packItems $packId"
        return packItems.invoke(packId)
    }

    override suspend fun packSearch(packId: String, query: String): ItemList {
        calls += "packSearch $packId $query"
        return packSearch.invoke(packId, query)
    }

    override suspend fun packs(): PackList {
        calls += "packs"
        return packs.invoke()
    }

    override suspend fun ownPacks(): PackList {
        calls += "ownPacks"
        return packs.invoke()
    }

    override fun parsePairingLink(text: String): PairingLink? {
        calls += "parsePairingLink"
        return parsePairingLink.invoke(text)
    }

    override fun pendingUsage(): UInt {
        calls += "pendingUsage"
        return pendingUsage
    }

    override suspend fun prepare(
        itemId: String,
        editorPackage: String,
        acceptedMimeTypes: List<String>,
        sticker: Boolean,
    ): Prepared {
        calls += "prepare $itemId '$editorPackage' $acceptedMimeTypes $sticker"
        return prepare.invoke(itemId)
    }

    override fun recent(limit: UInt): List<Item> {
        calls += "recent"
        return recent.invoke(limit)
    }

    override fun recordUse(itemId: String, variant: String, editorPackage: String) {
        unexpected("recordUse")
    }

    override suspend fun search(query: String, target: Target, limit: UInt): ItemList {
        calls += "search $query $target"
        return search.invoke(query)
    }

    override fun serverUrl(): String {
        calls += "serverUrl"
        return serverUrl
    }

    override suspend fun setServerUrl(url: String) {
        calls += "setServerUrl $url"
        setServerUrl.invoke(url)
    }

    override fun suggestions(query: String): Suggestions? = unexpected("suggestions")

    /** What [hints] answers; calls are recorded. */
    var hintsFor: (String) -> List<SearchHint> = { emptyList() }

    override fun hints(query: String): List<SearchHint> {
        calls += "hints $query"
        return hintsFor(query)
    }

    override fun itemUrl(itemId: String): String? = "https://s.example/items/$itemId"

    override suspend fun syncIfDue(): SyncState {
        calls += "syncIfDue"
        return syncIfDue.invoke()
    }

    override suspend fun syncNow(): SyncState {
        calls += "syncNow"
        return syncNow.invoke()
    }

    override fun syncState(): SyncState {
        calls += "syncState"
        return syncState
    }

    override suspend fun thumbnail(itemId: String): String = unexpected("thumbnail")
    override suspend fun packCover(packId: String): String? = null

    override suspend fun motion(itemId: String, download: Boolean): Motion? = unexpected("motion")

    override suspend fun toggleLike(itemId: String): Boolean {
        calls += "toggleLike $itemId"
        return toggleLike.invoke(itemId)
    }

    override suspend fun upload(setId: String, file: UploadFile): Item = unexpected("upload")

    override suspend fun whatsappPacks(packId: String): WhatsappPacks {
        calls += "whatsappPacks $packId"
        return whatsappPacks.invoke(packId)
    }

    override fun whatsappProgress(packId: String): WhatsappProgress? = whatsappProgress.invoke(packId)

    override fun whatsappUnpacked(): List<WhatsappPart> {
        calls += "whatsappUnpacked"
        return whatsappUnpacked.invoke()
    }
}

/** Part [number] of the pack [packId], made ready for WhatsApp, with [stickers] stickers. */
fun whatsappPart(packId: String, number: Int = 1, stickers: Int = 3, animated: Boolean = false) = WhatsappPart(
    packId = packId,
    number = number.toUInt(),
    identifier = "$packId-$number",
    name = "Pack $packId",
    publisher = "Alice",
    animated = animated,
    version = "00ff00ff00ff00ff",
    dir = "/data/whatsapp/$packId/$number-00ff00ff00ff00ff",
    trayFile = "cover.png",
    stickers = (1..stickers).map { WhatsappSticker(file = "%02d.webp".format(it), emojis = listOf("\uD83D\uDE42")) },
)

val alice = Account(username = "alice", displayName = null, serverUrl = "https://s.example", auth = AuthKind.TOKEN, isAdmin = false)

fun item(
    id: String,
    name: String = "item $id",
    tags: List<String> = emptyList(),
    emojis: List<String> = emptyList(),
    liked: Boolean = false,
) = Item(
    id = id,
    name = name,
    setId = "set",
    setName = "Set",
    owner = "alice",
    emojis = emojis,
    keywords = emptyList(),
    vibe = null,
    tags = tags,
    kind = MediaKind.STILL,
    format = MediaFormat.PNG,
    width = 0u,
    height = 0u,
    quality = null,
    likes = 0u,
    copies = 0u,
    nsfw = tags.any { it.trim().equals("nsfw", ignoreCase = true) },
    liked = liked,
)

fun pack(id: String) = Pack(id = id, slug = id, name = "Pack $id", owner = "alice", itemCount = 3u, mine = true)

fun prepared(name: String = "cat.png") = Prepared(
    path = "/data/cache/$name",
    mimeType = "image/png",
    displayName = name,
    target = Target.GENERIC,
    variant = "original",
    sticker = false,
)

fun unauthorized() = LibraryException.Unauthorized("401")

fun unreachable() = LibraryException.Unreachable("offline")

fun invalid(field: String) = LibraryException.InvalidInput(field, "bad $field")
