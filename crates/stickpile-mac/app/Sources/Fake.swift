import AppKit

/// A stand-in for the Rust popup with made-up data (`--fake[=SCENE]`), to
/// look at the layouts without a driver or a server. Scenes: items, recent,
/// packs, pack, many, me, login, settings, help, error.
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
            tabs: tabs.enumerated().map { TabLabel(tab: $1.0, label: $1.1, number: UInt8($0 + 1)) },
            tab: .search, view: .items, focus: .field, title: "Search", hasField: true, query: "cat",
            placeholder: "Search, or try e:... set:cats type:gif vibe:tired", count: "", back: false,
            entriesRevision: 0, entries: nil, packs: nil, selected: 0, columns: 6, thumbPx: 128,
            tiles: Tiles(favorites: true, emojis: true, tags: true), help: nil,
            footer: Footer(mode: "INSERT", insert: true, pending: "", target: "Telegram",
                           targetHint: "auto: com.tdesktop.Telegram", sticker: .off, stickerLabel: "sticker: off",
                           status: "24 results", level: .info),
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

    private func makeEntries(_ n: Int) -> [EntryView] {
        (0..<n).map { i in
            let k = i % FakePopup.names.count
            return EntryView(
                id: "item-\(i)", name: FakePopup.names[k],
                badge: i % 6 == 2 ? "GIF" : (i % 11 == 5 ? "WEBP" : nil),
                tooltip: "\(FakePopup.names[k])\nPack: Cats\n\(FakePopup.emojis[k % FakePopup.emojis.count]) cat, cute",
                incompatible: i % 9 == 7, liked: i % 4 == 1,
                emojis: i % 3 == 0 ? FakePopup.emojis[k % FakePopup.emojis.count] : "",
                tags: i % 5 == 0 ? "cat, cute, wave" : "")
        }
    }

    private func makePacks() -> [PackView] {
        let names = ["Cats", "Reactions", "Work memes", "Telegram favourites", "Animals", "Gifs", "Empty pack"]
        return names.enumerated().map { i, n in
            PackView(id: "pack-\(i)", coverKey: "cover-\(i)", name: n,
                     detail: "\(12 + i * 7) items - \(i % 2 == 0 ? "alice" : "bob")")
        }
    }

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
                            warnings: ["settings.toml: unknown key ui.colums (skipped)"], groups: [popup, server],
                            saveError: nil)
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
        entries = makeEntries(name == "many" ? 600 : 36)
        packs = makePacks()
        revision += 1
        switch name {
        case "recent":
            s.tab = .recent; s.title = "Recent"; s.hasField = false; s.query = ""; s.count = "36 items"
            s.focus = .noField; s.footer.mode = "NORMAL"; s.footer.insert = false; s.footer.pending = "3"
            s.footer.sticker = .on; s.footer.stickerLabel = "sticker: on"; s.selected = 8
        case "packs":
            s.tab = .packs; s.view = .packs; s.title = "Packs"; s.query = ""; s.placeholder = "Filter packs by name"
            s.selected = 1; s.footer.status = ""; s.footer.target = "Discord"; s.footer.targetHint = "default"
            s.footer.sticker = .unsupported
        case "pack":
            s.tab = .packs; s.title = "Packs / Cats"; s.back = true; s.query = ""
            s.placeholder = "Filter Cats by name, keyword or emoji"; s.selected = 3
            s.footer.status = "Searching..."; s.footer.level = .busy
        case "many":
            s.selected = 599; s.footer.status = "600 results"
        case "me":
            s.tab = .me; s.view = .me; s.title = "Me"; s.hasField = false; s.query = ""; s.focus = .noField
            s.footer.mode = "NORMAL"; s.footer.insert = false; s.me = meSignedIn(); s.footer.status = ""
        case "login":
            s.tab = .me; s.view = .me; s.title = "Me"; s.hasField = false; s.query = ""; s.focus = .keep
            s.me = meLogin(); s.footer.status = "Not logged in; log in on the Me tab (6)"; s.footer.level = .warning
        case "settings":
            s.tab = .settings; s.view = .settings; s.title = "Settings"; s.hasField = false; s.query = ""
            s.focus = .noField; s.footer.mode = "NORMAL"; s.footer.insert = false; s.settings = settingsView()
            s.footer.status = "Could not save: server_url must be http or https"; s.footer.level = .error
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

    func toggleSticker() {
        guard state.footer.sticker != .unsupported else { return }
        let on = state.footer.sticker != .on
        state.footer.sticker = on ? .on : .off
        state.footer.stickerLabel = on ? "sticker: on" : "sticker: off"
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
}
