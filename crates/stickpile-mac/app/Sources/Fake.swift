import AppKit

/// A stand-in for the Rust popup with made-up data (`--fake[=SCENE]`), to
/// look at the layouts without a driver or a server. Scenes: items, recent,
/// packs, pack, many, text (the text line on the tiles), me, login,
/// settings, help, error; and for keeping offline: offline (the marks with
/// the server unreachable), confirm (the question before keeping a pack),
/// offline-settings (the "On this device" block). A right click on a tile
/// opens a made-up menu (not in snapshots: the menu is a window of its own).
final class FakePopup: PopupControl {
    private weak var ui: PopupController?
    private let scene: String
    private var state: ViewState
    private var entries: [EntryView] = []
    private var packs: [PackView] = []
    private var files: [String] = []
    private var visible = false
    private var revision: UInt64 = 1
    private var sent: UInt64 = 0

    private static let tabs: [(TabId, String)] = [
        (.search, "Search"), (.recent, "Recent"), (.mostUsed, "Most used"), (.packs, "Packs"),
        (.favorites, "Favorites"), (.me, "Me"), (.settings, "Settings"),
    ]

    init(ui: PopupController, scene: String) {
        self.ui = ui
        self.scene = scene
        state = FakePopup.base()
    }

    // MARK: data

    private static func base() -> ViewState {
        ViewState(
            visible: false, error: nil, configPath: "~/Library/Application Support/stickpile/config.toml",
            language: "en",
            theme: ProcessInfo.processInfo.environment["STICKPILE_FAKE_THEME"] ?? "system",
            tabs: tabs.enumerated().map { TabLabel(tab: $1.0, label: $1.1, number: UInt8($0 + 1)) },
            tab: .search, view: .items, focus: .field, title: "Search", hasField: true, query: "cat",
            placeholder: "Search, or try e:... set:cats type:gif vibe:tired", count: "", back: false,
            entriesRevision: 0, entries: nil, packs: nil, selected: 0, columns: 6, thumbPx: 128,
            tiles: Tiles(favorites: true, emojis: true, tags: true, vibe: true, text: false), help: nil,
            helpHint: "? or Esc closes this", suggestions: nil,
            footer: Footer(mode: "INSERT", insert: true, pending: "", target: "Telegram",
                           targetHint: "auto: com.tdesktop.Telegram", sticker: .off, stickerLabel: "sticker: off",
                           status: "24 results", level: .info, safe: false, safeLabel: "safe: off",
                           details: "cat · Cats · GIF 320×240 · copied 12 times · ♥ 3 · quality 87",
                           detailsRow: true),
            me: MeView(loading: false, who: nil, detail: "", account: nil, form: nil, tailnetLogin: false,
                       busy: false, message: nil, retry: false, clearSecrets: false),
            settings: SettingsView(error: nil, note: "", path: "", warnings: [], groups: [], saveError: nil)
        )
    }

    private static let names = ["cat", "cat-wave", "doge", "facepalm", "party-parrot", "thumbs-up", "this-is-fine",
                                "shrug", "blob-hug", "sad-cat", "coffee", "ok-hand", "fire", "nyan", "hmm", "yes",
                                "no", "pikachu", "frog", "rocket", "sparkles", "heart-eyes", "skull", "wave"]
    private static let emojis = ["\u{1F431}", "\u{1F44B}", "\u{1F436}", "\u{1F926}", "\u{1F99C}", "\u{1F44D}",
                                 "\u{1F525}", "\u{1F937}", "\u{1F917}", "\u{1F63F}", "\u{2615}", "\u{1F44C}"]

    private func makeFiles() {
        let dir = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("stickpile-fake")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        for i in 0..<FakePopup.names.count {
            let url = dir.appendingPathComponent("\(i).png")
            if !FileManager.default.fileExists(atPath: url.path) {
                FakePopup.drawSticker(i).flatMap { try? $0.write(to: url) }
            }
            files.append(url.path)
        }
    }

    /// A made-up sticker: a coloured blob with an emoji, some not square.
    private static func drawSticker(_ i: Int) -> Data? {
        let w = i % 5 == 3 ? 320 : 256, h = i % 7 == 4 ? 340 : 256
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: w, pixelsHigh: h, bitsPerSample: 8,
                                         samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                         bytesPerRow: 0, bitsPerPixel: 0) else { return nil }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        let hue = CGFloat(i) / CGFloat(names.count)
        let r = NSRect(x: 0, y: 0, width: w, height: h).insetBy(dx: 14, dy: 14)
        NSColor(calibratedHue: hue, saturation: 0.55, brightness: 0.95, alpha: 1).setFill()
        let blob = i % 3 == 0 ? NSBezierPath(ovalIn: r) : NSBezierPath(roundedRect: r, xRadius: 48, yRadius: 48)
        blob.fill()
        NSColor.white.setStroke()
        blob.lineWidth = 10
        blob.stroke()
        let e = NSAttributedString(string: emojis[i % emojis.count], attributes: [.font: NSFont.systemFont(ofSize: 120)])
        let s = e.size()
        e.draw(at: NSPoint(x: (CGFloat(w) - s.width) / 2, y: (CGFloat(h) - s.height) / 2))
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .png, properties: [:])
    }

    /// `text`: the words in the picture on some tiles (the "text" scene).
    /// `unreachable`: the server is gone, the tiles not kept are dimmed
    /// (the "offline" scene); else some tiles are marked available.
    private func makeEntries(_ n: Int, text: Bool = false, unreachable: Bool = false) -> [EntryView] {
        (0..<n).map { i in
            let k = i % FakePopup.names.count
            let words = text && i % 3 != 2 ? FakePopup.texts[i % FakePopup.texts.count] : ""
            let kept = i % 3 == 0 || i == 1
            let offline: OfflineMark = kept ? .available : (unreachable ? .unavailable : OfflineMark.none)
            return EntryView(
                id: "item-\(i)", name: FakePopup.names[k],
                badge: i % 6 == 2 ? "GIF" : (i % 11 == 5 ? "WEBP" : nil),
                tooltip: "\(FakePopup.names[k])\nPack: Cats\n\(FakePopup.emojis[k % FakePopup.emojis.count]) cat, cute"
                    + (words.isEmpty ? "" : "\n\n" + words.replacingOccurrences(of: " / ", with: "\n")),
                incompatible: i % 9 == 7, liked: i % 4 == 1,
                emojis: i % 3 == 0 ? FakePopup.emojis[k % FakePopup.emojis.count] : "",
                tags: i % 5 == 0 ? "cat, cute, wave" : "",
                vibe: i % 4 == 2 ? "sarcastic" : (i % 7 == 3 ? "tired" : ""),
                text: words,
                offline: offline)
        }
    }

    /// Tile text lines as the driver sends them (" / " between lines).
    private static let texts = [
        "NO THOUGHTS / head empty", "me when the build is green", "КОЛИ В П'ЯТНИЦЮ / о 17:59",
    ]

    private func makePacks() -> [PackView] {
        let names = ["Cats", "Reactions", "Work memes", "Telegram favourites", "Animals", "Gifs", "Empty pack"]
        return names.enumerated().map { i, n in
            PackView(id: "pack-\(i)", coverKey: "cover-\(i)", name: n,
                     detail: "\(12 + i * 7) items - \(i % 2 == 0 ? "alice" : "bob")", kept: i == 0)
        }
    }

    /// The "On this device" block, as the driver sends it (`fraction`: a
    /// run in progress).
    private func offlineBlock(library: Bool, fraction: Double?, paused: Bool = false) -> OfflineView {
        let running = fraction != nil
        let state: String
        if paused {
            state = "Paused: 412 of 1,180 files, 41.0 MiB of 132.5 MiB."
        } else if running {
            state = "Downloading: 412 of 1,180 files, 41.0 MiB of 132.5 MiB."
        } else {
            state = "Up to date: all 1,180 kept files are on this device."
        }
        return OfflineView(
            title: "On this device", library: library, libraryLabel: "Keep everything offline",
            libraryHelp: "Every sticker you can see, now and later, within the limit of the Offline settings below. Asks first, with what it takes.",
            kept: "Kept offline: 412 files, 41.0 MiB of 1.0 GiB allowed.",
            other: "Other cached files: 37, 2.4 MiB (the cache size limit applies).",
            state: state, stateWarning: false, fraction: fraction ?? 1, canPause: running && !paused,
            canResume: paused, canCancel: running || paused,
            notes: "2 files could not be downloaded; they are tried again on the next run.\n14 stickers are left out: their kind is off in the Offline settings.",
            message: "Clearing leaves the kept files; removing them asks first.", messageError: false, canRemove: true,
            pauseLabel: "Pause", resumeLabel: "Resume", cancelLabel: "Cancel", cacheTitle: "Cached files",
            clearLabel: "Clear cache",
            clearHelp: "Delete the thumbnails and copied files that are not kept; kept files stay",
            removeLabel: "Remove kept files", removeHelp: "Delete every kept file and forget what to keep (asks first)")
    }

    private func offlineGroup() -> SettingsGroup {
        SettingsGroup(title: "Offline", rows: [
            SettingRow(key: "offline.auto.thumbnails", label: "Keep all thumbnails",
                       help: "After each sync, fetch every item's thumbnail so the grids are complete offline.",
                       kind: .bool, value: .bool(value: true), locked: false, resettable: false),
            SettingRow(key: "offline.videos", label: "Videos",
                       help: "Download videos when keeping a pack, the library or by an automatic rule.",
                       kind: .bool, value: .bool(value: false), locked: false, resettable: false),
            SettingRow(key: "offline.max_mb", label: "Offline limit", help: "MiB that kept files may take (0: no limit).",
                       kind: .int(min: 0, max: 1_000_000), value: .int(value: 1024), locked: false, resettable: false),
        ])
    }

    private static let keepPack = ConfirmView(
        title: "Keep Cats offline?",
        body: "39 stickers, 82 files, 14.2 MiB. 1.1 MiB of it is on this device already.\n\nKept files would take 55.2 MiB of the 1.0 GiB allowed.",
        accept: "Keep offline", cancel: "Cancel", destructive: false, hint: "Return: keep offline, Escape: cancel")

    private static let removeKept = ConfirmView(
        title: "Remove kept files?",
        body: "412 kept files (41.0 MiB) are deleted, and nothing stays marked to keep offline.\n\nThe automatic rules of the Offline settings download what they cover again; turn them off first to keep nothing.",
        accept: "Remove kept files", cancel: "Cancel", destructive: true, hint: "Return: remove, Escape: cancel")

    private func meSignedIn() -> MeView {
        MeView(loading: false, who: "Alice Example", detail: "alice - admin", account: MeAccount(
            serverUrl: "https://stickpile.example.ts.net", openServer: true, auth: "API token", tailnet: "alice@github",
            twoFactor: "On", stats: [
                Stat(value: "1,204", label: "copies"), Stat(value: "37", label: "this week"),
                Stat(value: "88", label: "likes given"), Stat(value: "152", label: "likes received"),
                Stat(value: "9", label: "packs"), Stat(value: "640", label: "items"),
                Stat(value: "412 MiB", label: "storage"), Stat(value: "0", label: "not uploaded"),
            ], logout: true, storeToken: false),
               form: nil, tailnetLogin: false, busy: false, message: nil, retry: false, clearSecrets: false)
    }

    private func meLogin() -> MeView {
        MeView(loading: false, who: nil, detail: "", account: nil,
               form: MeForm(description: "Server: https://stickpile.example.ts.net", totp: true), tailnetLogin: true,
               busy: false, message: Message(text: "Enter the code from your authenticator app", error: true),
               retry: false, clearSecrets: false)
    }

    private func settingsView() -> SettingsView {
        let popup = SettingsGroup(title: "Popup", rows: [
            SettingRow(key: "ui.columns", label: "Columns", help: "Thumbnails per row at the default window size",
                       kind: .int(min: 1, max: 64), value: .int(value: 6), locked: false, resettable: false),
            SettingRow(key: "ui.thumb_px", label: "Thumbnail size", help: "Cell size in points",
                       kind: .int(min: 48, max: 512), value: .int(value: 144), locked: false, resettable: true),
            SettingRow(key: "ui.modal", label: "Vim-style modes", help: "Insert and Normal mode",
                       kind: .bool, value: .bool(value: true), locked: false, resettable: false),
            SettingRow(key: "ui.start_tab", label: "Start tab", help: "The tab the popup opens on",
                       kind: .choice(options: [ChoiceOption(value: "last", label: "Last used"),
                                               ChoiceOption(value: "search", label: "Search"),
                                               ChoiceOption(value: "recent", label: "Recent")]),
                       value: .text(value: "last"), locked: false, resettable: false),
            SettingRow(key: "ui.show_tags", label: "Show tags", help: "An item's tags on its tile (zt). Set by home-manager.",
                       kind: .bool, value: .bool(value: false), locked: true, resettable: false),
        ])
        let server = SettingsGroup(title: "Server", rows: [
            SettingRow(key: "server_url", label: "Server URL", help: "Saved on Enter or when the field loses the focus",
                       kind: .text, value: .text(value: "https://stickpile.example.ts.net"), locked: false,
                       resettable: true),
            SettingRow(key: "timeout_secs", label: "Timeout", help: "Seconds before falling back to the cache",
                       kind: .int(min: 1, max: 120), value: .int(value: 5), locked: false, resettable: false),
        ])
        return SettingsView(error: nil, note: "Changes are saved to ~/Library/Application Support/stickpile/config.toml",
                            path: "/Users/alice/Library/Application Support/stickpile/config.toml",
                            warnings: ["settings.toml: unknown key ui.colums (skipped)"],
                            groups: [popup, offlineGroup(), server], saveError: nil,
                            offline: offlineBlock(library: false, fraction: nil), offlineBefore: 1)
    }

    private func helpSections() -> [HelpSection] {
        [
            HelpSection(title: "Everywhere", keys: [
                HelpKey(keys: "Tab, Shift+Tab", action: "next / previous tab"),
                HelpKey(keys: "Cmd+1 to Cmd+7", action: "go to that tab"),
                HelpKey(keys: "Cmd+T", action: "next target app"),
                HelpKey(keys: "Cmd+S", action: "toggle sticker mode"),
                HelpKey(keys: "F1", action: "show or hide this help"),
            ]),
            HelpSection(title: "Insert mode", keys: [
                HelpKey(keys: "typing", action: "search"),
                HelpKey(keys: "arrows", action: "move in the grid"),
                HelpKey(keys: "Return", action: "copy the selected item and hide"),
                HelpKey(keys: "Shift+Return", action: "copy with sticker mode flipped"),
                HelpKey(keys: "Escape", action: "Normal mode"),
            ]),
            HelpSection(title: "Normal mode", keys: [
                HelpKey(keys: "h j k l, arrows", action: "move left, down, up, right"),
                HelpKey(keys: "gg / G", action: "first / last item"),
                HelpKey(keys: "y, Return", action: "copy the selected item and hide"),
                HelpKey(keys: "f", action: "add to favorites, or remove"),
                HelpKey(keys: "/", action: "clear the text field and type"),
                HelpKey(keys: "q, Escape", action: "hide"),
            ]),
        ]
    }

    // MARK: scenes

    private func setScene(_ name: String) {
        var s = FakePopup.base()
        entries = makeEntries(name == "many" ? 600 : 36, text: name == "text", unreachable: name == "offline")
        packs = makePacks()
        revision += 1
        switch name {
        case "offline":
            s.selected = 4
            s.footer.status = "Copy failed: party-parrot is not on this device and the server is unreachable; keep it offline (K) to copy it without the server"
            s.footer.level = .error
            s.footer.details = "party-parrot · Cats · GIF 320×240 · copied 2 times"
        case "confirm":
            s.tab = .packs; s.view = .packs; s.title = "Packs"; s.query = ""; s.placeholder = "Filter packs by name"
            s.selected = 0; s.focus = .noField; s.footer.mode = "NORMAL"; s.footer.insert = false
            s.footer.status = ""; s.footer.details = "Cats · 39 items · by alice"
            s.confirm = FakePopup.keepPack
        case "offline-settings":
            s.tab = .settings; s.view = .settings; s.title = "Settings"; s.hasField = false; s.query = ""
            s.focus = .noField; s.footer.mode = "NORMAL"; s.footer.insert = false; s.footer.details = ""
            s.footer.status = "The whole library is kept offline"
            s.settings = SettingsView(error: nil, note: "Changes are saved to ~/Library/Application Support/stickpile/config.toml",
                                      path: "/Users/alice/Library/Application Support/stickpile/config.toml",
                                      warnings: [], groups: [offlineGroup()], saveError: nil,
                                      offline: offlineBlock(library: true, fraction: 0.31), offlineBefore: 0)
        case "recent":
            s.tab = .recent; s.title = "Recent"; s.hasField = false; s.query = ""; s.count = "36 items"
            s.focus = .noField; s.footer.mode = "NORMAL"; s.footer.insert = false; s.footer.pending = "3"
            s.footer.sticker = .on; s.footer.stickerLabel = "sticker: on"; s.selected = 8
            s.footer.safe = true; s.footer.safeLabel = "safe: on"
            s.footer.details = "sad-cat · Cats · WEBP 512×512 · copied 1 time"
        case "packs":
            s.tab = .packs; s.view = .packs; s.title = "Packs"; s.query = ""; s.placeholder = "Filter packs by name"
            s.selected = 1; s.footer.status = ""; s.footer.target = "Discord"; s.footer.targetHint = "default"
            s.footer.sticker = .unsupported; s.footer.details = "Cats · 39 items · by alice"
        case "pack":
            s.tab = .packs; s.title = "Packs / Cats"; s.back = true; s.query = ""
            s.placeholder = "Filter Cats by name, keyword or emoji"; s.selected = 3
            s.footer.status = "Searching..."; s.footer.level = .busy
        case "many":
            s.selected = 599; s.footer.status = "600 results"
        case "text":
            s.tiles.text = true; s.footer.status = "Text shown"
        case "me":
            s.tab = .me; s.view = .me; s.title = "Me"; s.hasField = false; s.query = ""; s.focus = .noField
            s.footer.mode = "NORMAL"; s.footer.insert = false; s.me = meSignedIn(); s.footer.status = ""
            s.footer.details = ""
        case "login":
            s.tab = .me; s.view = .me; s.title = "Me"; s.hasField = false; s.query = ""; s.focus = .keep
            s.me = meLogin(); s.footer.status = "Not logged in; log in on the Me tab (6)"; s.footer.level = .warning; s.footer.details = ""
        case "settings":
            s.tab = .settings; s.view = .settings; s.title = "Settings"; s.hasField = false; s.query = ""
            s.focus = .noField; s.footer.mode = "NORMAL"; s.footer.insert = false; s.settings = settingsView()
            s.footer.status = "Could not save: server_url must be http or https"; s.footer.level = .error; s.footer.details = ""
        case "help":
            s.help = helpSections(); s.focus = .noField; s.footer.mode = "NORMAL"; s.footer.insert = false
        case "error":
            s.error = "config.toml: TOML parse error at line 3, column 9: invalid string"
        default:
            break
        }
        state = s
    }

    // MARK: rendering

    private func render() {
        state.visible = visible
        state.entriesRevision = revision
        var s = state
        if sent != revision {
            s.entries = entries
            s.packs = packs
            sent = revision
        }
        ui?.render(view: s)
    }

    private func sendThumbnails() {
        guard let ui, !files.isEmpty else { return }
        for (i, e) in entries.enumerated() {
            ui.thumbnail(key: e.id, path: files[i % files.count])
        }
        // The last pack has no cover yet.
        for (i, p) in packs.enumerated().dropLast() {
            ui.thumbnail(key: p.coverKey, path: files[(i * 5 + 2) % files.count])
        }
    }

    private func count() -> Int {
        state.view == .packs ? packs.count : (state.view == .items ? entries.count : 0)
    }

    private func move(_ d: Int) {
        let n = count()
        guard n > 0 else { return }
        state.selected = UInt32(min(max(Int(state.selected ?? 0) + d, 0), n - 1))
    }

    // MARK: PopupControl

    func start() {
        makeFiles()
        setScene(scene)
        render()
        sendThumbnails()
    }

    func toggle(front: FrontApp?, popupActive: Bool) {
        if popupActive && visible {
            hide()
        } else {
            open(front: front, tab: nil)
        }
    }

    func open(front: FrontApp?, tab: TabId?) {
        visible = true
        if let front { state.footer.targetHint = "auto: \(front.bundleId)" }
        if let tab { showTab(tab: tab) }
        render()
        ui?.present()
    }

    func hide() {
        visible = false
        render()
        ui?.hide()
    }

    func hidden() {
        visible = false
        render()
    }

    func key(key: KeyInput) -> Bool {
        let ch = key.latin ?? key.characters
        let insert = state.footer.insert
        // A question takes the keys: Return or y yes, Escape, n or q no.
        if state.confirm != nil {
            if key.keyCode == 36 || ch == "y" {
                confirm(yes: true)
            } else if key.keyCode == 53 || ch == "n" || ch == "q" {
                confirm(yes: false)
            }
            return true
        }
        switch key.keyCode {
        case 53:
            if state.help != nil {
                state.help = nil
            } else if insert {
                state.footer.insert = false; state.footer.mode = "NORMAL"; state.focus = .noField
            } else {
                hide(); return true
            }
        case 123: move(-1)
        case 124: move(1)
        case 125: move(Int(state.columns))
        case 126: move(-Int(state.columns))
        case 36:
            if let i = state.selected { activate(index: i) }
            return true
        case 122: toggleHelp(); return true
        case 48:
            let i = FakePopup.tabs.firstIndex { $0.0 == state.tab } ?? 0
            showTab(tab: FakePopup.tabs[(i + (key.shift ? 6 : 1)) % 7].0)
            return true
        default:
            if insert && !key.command && !key.control { return false }
            switch ch {
            case "h": move(-1)
            case "l": move(1)
            case "j": move(Int(state.columns))
            case "k": move(-Int(state.columns))
            case "?": toggleHelp(); return true
            case "i", "/":
                state.footer.insert = true; state.footer.mode = "INSERT"; state.focus = state.hasField ? .field : .keep
            case "1", "2", "3", "4", "5", "6", "7":
                showTab(tab: FakePopup.tabs[Int(ch)! - 1].0); return true
            default: return !insert
            }
        }
        render()
        return true
    }

    func queryChanged(text: String) {
        state.query = text
        state.footer.status = text.isEmpty ? "" : "Searching..."
        let last = text.split(separator: " ", omittingEmptySubsequences: false).last.map(String.init) ?? ""
        if last.hasPrefix("vibe:") {
            state.suggestions = SuggestionView(key: "vibe:",
                                               words: ["sarcastic", "sad", "happy", "tired", "angry", "confused"],
                                               chosen: 0, labels: [])
        } else if last.hasPrefix("e:") {
            // Emojis with their names, as e:sk offers them.
            state.suggestions = SuggestionView(key: "e:",
                                               words: ["\u{1F480}", "\u{2620}\u{FE0F}", "\u{1FA7B}", "\u{1F9A8}"],
                                               chosen: nil,
                                               labels: ["skull", "skull and crossbones", "x-ray", "skunk"])
        } else {
            state.suggestions = nil
        }
        render()
    }

    func acceptSuggestion(index: UInt32) {
        guard let s = state.suggestions, Int(index) < s.words.count else { return }
        let head = state.query.split(separator: " ", omittingEmptySubsequences: false).dropLast().joined(separator: " ")
        state.query = (head.isEmpty ? "" : head + " ") + s.key + s.words[Int(index)] + " "
        state.suggestions = nil
        render()
    }

    func select(index: UInt32) {
        state.selected = index
        render()
    }

    func activate(index: UInt32) {
        guard state.view == .items, Int(index) < entries.count else { return }
        let path = files[Int(index) % files.count]
        let png = (try? Data(contentsOf: URL(fileURLWithPath: path))) ?? Data()
        let url = URL(fileURLWithPath: path).absoluteString.data(using: .utf8) ?? Data()
        let error = ui?.setClipboard(items: [PasteboardData(kind: "public.png", data: png),
                                             PasteboardData(kind: "public.file-url", data: url)])
        state.footer.status = error ?? "Copied \(entries[Int(index)].name) for \(state.footer.target)"
        state.footer.level = error == nil ? .info : .error
        hide()
    }

    func showTab(tab: TabId) {
        let wanted: String
        switch tab {
        case .search: wanted = "items"
        case .recent, .mostUsed, .favorites: wanted = "recent"
        case .packs: wanted = "packs"
        case .me: wanted = "me"
        case .settings: wanted = "settings"
        }
        let help = state.help
        setScene(wanted)
        state.help = help
        state.tab = tab
        if let label = FakePopup.tabs.first(where: { $0.0 == tab })?.1, !state.hasField { state.title = label }
        render()
        sendThumbnails()
    }

    func toggleHelp() {
        state.help = state.help == nil ? helpSections() : nil
        render()
    }

    /// The fake has one help page: the syntax button shows it too.
    func toggleSyntax() {
        toggleHelp()
    }

    func toggleSticker() {
        guard state.footer.sticker != .unsupported else { return }
        let on = state.footer.sticker != .on
        state.footer.sticker = on ? .on : .off
        state.footer.stickerLabel = on ? "sticker: on" : "sticker: off"
        render()
    }

    func toggleSafe() {
        state.footer.safe.toggle()
        state.footer.safeLabel = state.footer.safe ? "safe: on" : "safe: off"
        render()
    }

    func cycleTarget() {
        let targets = ["Telegram", "Discord", "Signal", "Generic"]
        let i = targets.firstIndex(of: state.footer.target) ?? 0
        state.footer.target = targets[(i + 1) % targets.count]
        state.footer.targetHint = "chosen"
        render()
    }

    func packBack() { showTab(tab: .packs) }

    func textFocus(focused: Bool) {
        state.footer.insert = focused
        state.footer.mode = focused ? "INSERT" : "NORMAL"
        state.focus = focused ? (state.hasField ? .field : .keep) : .noField
        render()
    }

    func gridSize(width: Double, height: Double) {
        let columns = max(1, Int(width) / (Int(state.thumbPx) + Int(cellExtra)))
        if columns != Int(state.columns) {
            state.columns = UInt32(columns)
            render()
        }
    }

    func windowSize() -> Size { Size(width: 888, height: 688) }

    func login(username: String, password: String, totp: String) {
        state.me = meSignedIn()
        state.me.clearSecrets = true
        render()
        state.me.clearSecrets = false
    }

    func loginTailnet() { login(username: "", password: "", totp: "") }

    func logout() {
        state.me = meLogin()
        state.me.message = nil
        render()
    }

    func retryProfile() {}
    func openServer() {}

    func setSetting(key: String, value: SettingValue) {
        for g in state.settings.groups.indices {
            for r in state.settings.groups[g].rows.indices where state.settings.groups[g].rows[r].key == key {
                state.settings.groups[g].rows[r].value = value
                state.settings.groups[g].rows[r].resettable = true
            }
        }
        state.footer.status = "Saved \(key)"
        state.footer.level = .info
        render()
    }

    func resetSetting(key: String) {
        for g in state.settings.groups.indices {
            for r in state.settings.groups[g].rows.indices where state.settings.groups[g].rows[r].key == key {
                state.settings.groups[g].rows[r].resettable = false
            }
        }
        render()
    }

    // MARK: keeping offline

    func openMenu(index: UInt32?) {
        guard state.view == .items || state.view == .packs else { return }
        let i = index ?? state.selected ?? 0
        state.selected = i
        if state.view == .packs {
            state.menu = MenuView(index: i, entries: [
                MenuEntryView(label: "Open the pack", keys: "o", action: .openPack),
                MenuEntryView(label: "Keep \(packs[Int(i) % packs.count].name) offline...", keys: "K", action: .keepPack),
            ])
        } else {
            let e = entries[Int(i) % entries.count]
            var list = [
                MenuEntryView(label: "Copy", keys: "y", action: .copy),
                MenuEntryView(label: "Copy as a sticker", keys: "Y", action: .copyFlipped),
                MenuEntryView(label: e.liked ? "Remove from favorites" : "Add to favorites", keys: "f", action: .favorite),
                MenuEntryView(label: "Open in the browser", keys: "gx", action: .browser),
            ]
            if e.offline == .available {
                list.append(MenuEntryView(label: "Stop keeping offline", keys: "K", action: .release))
                list.append(MenuEntryView(label: "Available offline", keys: "", action: nil))
            } else {
                list.append(MenuEntryView(label: "Kept offline: pack Cats", keys: "", action: nil))
                list.append(MenuEntryView(label: "Keep offline", keys: "K", action: .keep))
            }
            state.menu = MenuView(index: i, entries: list)
        }
        render()
    }

    func menuChoose(action: MenuChoice) {
        state.menu = nil
        switch action {
        case .keepPack: state.confirm = FakePopup.keepPack
        case .keep: state.footer.status = "\(entries[Int(state.selected ?? 0)].name) is kept offline"
        case .release: state.footer.status = "\(entries[Int(state.selected ?? 0)].name) is no longer kept offline"
        default: state.footer.status = "Chose \(action)"
        }
        state.footer.level = .info
        render()
    }

    func menuClosed() {
        state.menu = nil
        render()
    }

    func confirm(yes: Bool) {
        let was = state.confirm
        state.confirm = nil
        if yes, let was {
            state.footer.status = was.destructive ? "Removed 412 kept files (41.0 MiB)" : "Pack Cats is kept offline"
            state.footer.level = .info
        }
        render()
    }

    func offlineKeepLibrary(keep: Bool) {
        if keep {
            state.confirm = ConfirmView(
                title: "Keep the whole library offline?",
                body: "1,180 stickers, 2,410 files, 132.5 MiB. 41.0 MiB of it is on this device already.\n\nKept files would take 140.2 MiB of the 1.0 GiB allowed.",
                accept: "Keep offline", cancel: "Cancel", destructive: false, hint: "Return: keep offline, Escape: cancel")
        } else {
            state.settings.offline = offlineBlock(library: false, fraction: nil)
        }
        render()
    }

    func offlinePause() {
        state.settings.offline = offlineBlock(library: true, fraction: 0.31, paused: true)
        render()
    }

    func offlineResume() {
        state.settings.offline = offlineBlock(library: true, fraction: 0.31)
        render()
    }

    func offlineCancel() {
        state.settings.offline = offlineBlock(library: true, fraction: nil)
        render()
    }

    func clearCache() {
        state.settings.offline?.message = "Cleared 37 files (2.4 MiB); 412 kept files stayed."
        render()
    }

    func removeKeptFiles() {
        state.confirm = FakePopup.removeKept
        render()
    }
}
