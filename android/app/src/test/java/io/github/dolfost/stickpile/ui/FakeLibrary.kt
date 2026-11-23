package io.github.dolfost.stickpile.ui

import io.github.dolfost.stickpile.rust.Account
import io.github.dolfost.stickpile.rust.CacheCleared
import io.github.dolfost.stickpile.rust.KeepReason
import io.github.dolfost.stickpile.rust.OfflineEstimate
import io.github.dolfost.stickpile.rust.OfflineOptions
import io.github.dolfost.stickpile.rust.OfflineOutcome
import io.github.dolfost.stickpile.rust.OfflinePlan
import io.github.dolfost.stickpile.rust.OfflineScope
import io.github.dolfost.stickpile.rust.OfflineSkips
import io.github.dolfost.stickpile.rust.OfflineState
import io.github.dolfost.stickpile.rust.OfflineStatus
import io.github.dolfost.stickpile.rust.OfflineTotals
import io.github.dolfost.stickpile.rust.OfflineUsage
import io.github.dolfost.stickpile.rust.AuthKind
import io.github.dolfost.stickpile.rust.Item
import io.github.dolfost.stickpile.rust.ItemList
import io.github.dolfost.stickpile.rust.LibraryException
import io.github.dolfost.stickpile.rust.MediaFormat
import io.github.dolfost.stickpile.rust.MediaKind
import io.github.dolfost.stickpile.rust.Motion
import io.github.dolfost.stickpile.rust.Pack
import io.github.dolfost.stickpile.rust.PackList
import io.github.dolfost.stickpile.rust.NoHandle
import io.github.dolfost.stickpile.rust.PairingLink
import io.github.dolfost.stickpile.rust.PendingSearch
import io.github.dolfost.stickpile.rust.Prepared
import io.github.dolfost.stickpile.rust.SearchAnswer
import io.github.dolfost.stickpile.rust.SearchHint
import io.github.dolfost.stickpile.rust.ServerReach
import io.github.dolfost.stickpile.rust.StickpileLibraryInterface
import io.github.dolfost.stickpile.rust.Suggestions
import io.github.dolfost.stickpile.rust.SyncState
import io.github.dolfost.stickpile.rust.Target
import io.github.dolfost.stickpile.rust.UploadFile
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first

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
    var syncState = SyncState(lastSync = null, lastError = null, pendingUsage = 0u, due = false, cacheChanged = false)
    var pendingUsage: UInt = 0u
    var clearCache: () -> CacheCleared = { CacheCleared(0u, 0u, 0u, 0u) }
    var serverUrl = "http://localhost:8080"

    // Keeping offline: a small model of what the library keeps.
    var options = OfflineOptions()
    val keeps = mutableListOf<OfflineScope>()
    var reasons: (String) -> List<KeepReason> = { id ->
        if (OfflineScope.Sticker(id) in keeps) listOf(KeepReason.STICKER) else emptyList()
    }
    var estimate: suspend (OfflineScope) -> OfflineEstimate = { sampleEstimate() }
    var plan: suspend () -> OfflinePlan = { samplePlan() }
    var usage: () -> OfflineUsage = { OfflineUsage(0u, 0u, 0u, 0u, 1024uL * 1024u * 1024u) }
    var removeKept: () -> CacheCleared = { CacheCleared(0u, 0u, 0u, 0u) }

    /** The ids `availableOffline` says yes to. */
    var available: Set<String> = emptySet()
    var unreachable = false

    /** The `availableOffline` calls, as "'<package>' <types>" (not in [calls]: every list asks). */
    val marksAsked = mutableListOf<String>()
    private val statusFlow = MutableStateFlow(sampleStatus())
    var status: OfflineStatus
        get() = statusFlow.value
        set(value) {
            statusFlow.value = value
        }
    var keepOffline: suspend (OfflineScope) -> Unit = { keeps += it }
    var releaseOffline: suspend (OfflineScope) -> Unit = { keeps -= it }

    private fun unexpected(name: String): Nothing = throw AssertionError("unexpected call: $name")

    override suspend fun account(): Account? {
        calls += "account"
        return account.invoke()
    }

    override fun clearCache(): CacheCleared {
        calls += "clearCache"
        return clearCache.invoke()
    }

    override fun offlineOptions(): OfflineOptions = options

    override fun setOfflineOptions(options: OfflineOptions) {
        calls += "setOfflineOptions"
        this.options = options
    }

    override suspend fun keepOffline(scope: OfflineScope) {
        calls += "keepOffline $scope"
        keepOffline.invoke(scope)
    }

    override suspend fun releaseOffline(scope: OfflineScope) {
        calls += "releaseOffline $scope"
        releaseOffline.invoke(scope)
    }

    override fun offlineKeeps(): List<OfflineScope> = keeps.toList()

    override fun offlineReasons(itemId: String): List<KeepReason> = reasons(itemId)

    override suspend fun offlineEstimate(scope: OfflineScope): OfflineEstimate {
        calls += "offlineEstimate $scope"
        return estimate.invoke(scope)
    }

    override suspend fun offlinePlan(): OfflinePlan = plan.invoke()

    override fun offlineUsage(): OfflineUsage = usage.invoke()

    override fun removeKeptFiles(): CacheCleared {
        calls += "removeKeptFiles"
        return removeKept.invoke()
    }

    override fun availableOffline(
        itemIds: List<String>,
        editorPackage: String,
        acceptedMimeTypes: List<String>,
        sticker: Boolean,
    ): List<Boolean> {
        marksAsked += "'$editorPackage' $acceptedMimeTypes"
        return itemIds.map { it in available }
    }

    override fun serverUnreachable(): Boolean = unreachable

    override suspend fun offlineRun(): OfflineOutcome = unexpected("offlineRun")

    override fun offlineStatus(): OfflineStatus = status

    /** Answers at once when the status differs from [seen], else when it does. */
    override suspend fun offlineStatusChange(seen: OfflineStatus?): OfflineStatus {
        return statusFlow.first { it != seen }
    }

    override fun pauseOffline() {
        calls += "pauseOffline"
        status = status.copy(paused = true)
    }

    override fun resumeOffline() {
        calls += "resumeOffline"
        status = status.copy(paused = false)
    }

    override fun cancelOffline() {
        calls += "cancelOffline"
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

    /**
     * The server's results to come for a search ([searchSteps]); null:
     * none, [search] answered as the server would.
     */
    var pendingFor: (String) -> PendingSearch? = { null }

    /** Answers with [search] first and [pendingFor] after it; recorded as `search`. */
    override suspend fun searchSteps(query: String, target: Target, limit: UInt): SearchAnswer {
        calls += "search $query $target"
        return SearchAnswer(search.invoke(query), pendingFor(query))
    }

    /** What the library knows of the server; a test moves it. */
    val reach = MutableStateFlow<ServerReach>(ServerReach.Unknown)

    /** The `checkServer` calls (not in [calls]: every resume checks). */
    var checks = 0

    override fun serverReach(): ServerReach = reach.value

    override suspend fun serverReachChange(seen: ServerReach): ServerReach = reach.first { it != seen }

    override fun checkServer() {
        checks++
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
}

/**
 * The server's answer to a search whose offline results came first:
 * [answer] gives it (null: the server could not be reached). [closed] once
 * the view model let it go.
 */
class FakePending(private val answer: suspend () -> ItemList?) : PendingSearch(NoHandle) {
    var closed = false

    override suspend fun serverResults(): ItemList? = answer()

    override fun close() {
        closed = true
    }
}

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

fun totals(files: ULong = 0u, bytes: ULong = 0u, presentFiles: ULong = 0u, presentBytes: ULong = 0u) =
    OfflineTotals(files, bytes, presentFiles, presentBytes)

val noSkips = OfflineSkips(0u, 0u, 0u, 0u)

fun sampleEstimate(totals: OfflineTotals = totals(10u, 1_000u), items: ULong = 4u, fits: Boolean = true) = OfflineEstimate(
    totals = totals,
    items = items,
    skipped = noSkips,
    keptBytesAfter = totals.bytes,
    budget = 1024uL * 1024u * 1024u,
    fits = fits,
)

fun samplePlan(totals: OfflineTotals = totals(), items: ULong = 0u) = OfflinePlan(
    totals = totals,
    items = items,
    skipped = noSkips,
    failed = 0u,
    budget = 1024uL * 1024u * 1024u,
    overBudget = false,
    parts = emptyList(),
)

fun sampleStatus(
    state: OfflineState = OfflineState.IDLE,
    running: Boolean = false,
    paused: Boolean = false,
    filesDone: ULong = 0u,
    filesTotal: ULong = 0u,
) = OfflineStatus(
    state = state,
    filesDone = filesDone,
    filesTotal = filesTotal,
    bytesDone = filesDone * 100u,
    bytesTotal = filesTotal * 100u,
    skipped = 0u,
    failed = 0u,
    parts = emptyList(),
    message = null,
    running = running,
    paused = paused,
)
