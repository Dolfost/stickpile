import AppKit
import ServiceManagement

struct Options {
    var configPath: String?
    var demoDir: String?
    var logFile: String?
    var show = false
    var tab: TabId?
    var testPasteboard = false
    /// A scene of made-up data instead of the Rust driver.
    var fake: String?
    var snapshot: String?
    var snapshotDelay = 2.0
    var appearance: String?
    /// Steps run after the popup opened (testing): query=TEXT, key=CODE[:chars][+cmd...],
    /// type=TEXT, newline, tab=NAME, select=N, activate=N, wait=SECONDS, toggle, hide, help.
    var script: [String] = []
    /// Check the menu, the shortcut and the window's hit testing, print, quit.
    var selftest = false

    static let usage = """
    Usage: Stickpile [options]
      --config PATH        config.toml to use
      --demo DIR           serve the media files of DIR instead of a server
      --log FILE           append the log here (default ~/Library/Logs/stickpile/stickpile.log)
      --show               open the popup at launch
      --tab NAME           with --show or --snapshot: open on this tab
      --test-pasteboard    copy to a private pasteboard, not the general one
      --fake[=SCENE]       made-up data instead of the driver (items, recent, packs,
                           pack, many, me, login, settings, help, error)
      --snapshot OUT.png   open the popup, draw it into OUT.png, quit
      --snapshot-delay S   seconds to wait before the snapshot (default 2)
      --appearance light|dark
      --script STEPS       comma separated test steps, run after opening
      --selftest           check the menu, the shortcut and the window, then quit
    """

    static func parse(_ args: [String]) -> Options {
        var o = Options()
        var it = args.dropFirst().makeIterator()
        func value(_ flag: String) -> String {
            guard let v = it.next() else { fail("\(flag) needs a value") }
            return v
        }
        while let a = it.next() {
            switch a {
            case "--config": o.configPath = value(a)
            case "--demo": o.demoDir = value(a)
            case "--log": o.logFile = value(a)
            case "--show": o.show = true
            case "--tab":
                let name = value(a)
                guard let t = tabId(name) else { fail("unknown tab \(name)") }
                o.tab = t
            case "--test-pasteboard": o.testPasteboard = true
            case "--fake": o.fake = "items"
            case "--snapshot": o.snapshot = value(a)
            case "--snapshot-delay": o.snapshotDelay = Double(value(a)) ?? 2
            case "--appearance": o.appearance = value(a)
            case "--script": o.script = value(a).split(separator: ",").map(String.init)
            case "--selftest": o.selftest = true
            case "-h", "--help":
                print(usage)
                exit(0)
            default:
                if a.hasPrefix("--fake=") {
                    o.fake = String(a.dropFirst("--fake=".count))
                } else if a.hasPrefix("-psn_") || a.hasPrefix("-NS") || a.hasPrefix("-Apple") {
                    // Added by the system when launched from the Finder or a debugger.
                    _ = a.hasPrefix("-psn_") ? nil : it.next()
                } else {
                    fail("unknown option \(a)")
                }
            }
        }
        return o
    }

    static func fail(_ message: String) -> Never {
        FileHandle.standardError.write("Stickpile: \(message)\n\(usage)\n".data(using: .utf8)!)
        exit(2)
    }

    static func tabId(_ name: String) -> TabId? {
        switch name.lowercased() {
        case "search": return .search
        case "recent": return .recent
        case "most_used", "most-used", "mostused": return .mostUsed
        case "packs": return .packs
        case "favorites": return .favorites
        case "me": return .me
        case "settings": return .settings
        default: return nil
        }
    }

    var startOptions: StartOptions {
        var log = logFile
        if log == nil {
            let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/stickpile")
            try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            log = dir.appendingPathComponent("stickpile.log").path
        }
        return StartOptions(configPath: configPath, demoDir: demoDir, logFile: log,
                            systemLanguages: Locale.preferredLanguages)
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
    let options: Options
    private var controller: PopupController!
    private var statusItem: NSStatusItem?
    private var hotKey: HotKey?
    private var openItem: NSMenuItem?
    private var shortcutItems: [NSMenuItem] = []
    private var loginItem: NSMenuItem?

    init(options: Options) {
        self.options = options
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        switch options.appearance {
        case "dark": NSApp.appearance = NSAppearance(named: .darkAqua)
        case "light": NSApp.appearance = NSAppearance(named: .aqua)
        default: break
        }
        NSApp.mainMenu = mainMenu()
        controller = PopupController(testPasteboard: options.testPasteboard)
        controller.offscreen = options.snapshot != nil
        let popup: PopupControl
        if let scene = options.fake {
            popup = FakePopup(ui: controller, scene: scene)
        } else {
            popup = Popup(options: options.startOptions, ui: controller)
        }
        controller.attach(popup)
        controller.onLanguage = { [weak self] in
            guard let self else { return }
            NSApp.mainMenu = self.mainMenu()
            if let item = self.statusItem {
                item.menu = self.statusMenu()
                self.refreshMenu()
            }
        }
        controller.applyWindowSize()
        controller.send { $0.start() }

        if options.selftest {
            selftest()
            exit(0)
        }
        if options.snapshot == nil {
            setUpStatusItem()
            hotKey = HotKey { [weak self] in self?.controller.shortcutPressed() }
            hotKey?.register(Shortcut.current)
        }
        if options.show || options.snapshot != nil {
            controller.openPopup(tab: options.tab)
        }
        var delay = 0.3
        for step in options.script {
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in self?.run(step) }
            if step.hasPrefix("wait=") { delay += Double(step.dropFirst(5)) ?? 0 } else { delay += 0.2 }
        }
        if let out = options.snapshot {
            DispatchQueue.main.asyncAfter(deadline: .now() + delay + options.snapshotDelay) { [weak self] in
                let ok = self?.controller.snapshot(to: out) ?? false
                print(ok ? "snapshot: \(out)" : "snapshot failed")
                exit(ok ? 0 : 1)
            }
        }
    }

    /// One test step of --script.
    private func run(_ step: String) {
        let parts = step.split(separator: "=", maxSplits: 1).map(String.init)
        let arg = parts.count > 1 ? parts[1] : ""
        switch parts[0] {
        case "query": controller.send { $0.queryChanged(text: arg) }
        case "tab": if let t = Options.tabId(arg) { controller.send { $0.showTab(tab: t) } }
        case "select": controller.send { $0.select(index: UInt32(arg) ?? 0) }
        case "activate": controller.send { $0.activate(index: UInt32(arg) ?? 0) }
        case "toggle": controller.shortcutPressed()
        case "hide": controller.send { $0.hide() }
        case "help": controller.send { $0.toggleHelp() }
        case "key":
            // key=CODE or key=CODE:chars, with +cmd +shift +ctrl +opt suffixes
            var spec = arg
            var mods: Set<String> = []
            while let r = spec.range(of: "+", options: .backwards) {
                mods.insert(String(spec[r.upperBound...]))
                spec = String(spec[..<r.lowerBound])
            }
            let kc = spec.split(separator: ":", maxSplits: 1).map(String.init)
            let code = UInt16(kc[0]) ?? 0
            let chars = kc.count > 1 ? kc[1] : ""
            let input = KeyInput(keyCode: code, characters: chars, latin: Keys.latin(keyCode: code),
                                 command: mods.contains("cmd"), control: mods.contains("ctrl"),
                                 option: mods.contains("opt"), shift: mods.contains("shift"))
            controller.send { popup in
                let used = popup.key(key: input)
                print("key \(arg): \(used ? "used" : "passed on")")
            }
        case "type":
            // Typing into the focused field, as its field editor sees it.
            if let editor = controller.panel.firstResponder as? NSTextView {
                editor.insertText(arg, replacementRange: editor.selectedRange())
            } else {
                print("type: no text field has the focus")
            }
        case "newline":
            if let editor = controller.panel.firstResponder as? NSTextView {
                editor.doCommand(by: #selector(NSResponder.insertNewline(_:)))
            } else {
                print("newline: no text field has the focus")
            }
        case "size":
            let wh = arg.split(separator: "x").compactMap { Double($0) }
            if wh.count == 2 { controller.panel.setContentSize(NSSize(width: wh[0], height: wh[1])) }
        case "wait": break
        default: print("unknown script step \(step)")
        }
    }

    private func selftest() {
        let menu = statusMenu()
        refreshMenu()
        for item in menu.items where !item.isSeparatorItem {
            let key = item.keyEquivalent.isEmpty ? "" : " [\(item.keyEquivalentModifierMask.rawValue >> 16):\(item.keyEquivalent == " " ? "space" : item.keyEquivalent)]"
            let state = item.state == .on ? " (on)" : ""
            print("menu: \(item.title)\(key)\(state)")
            for sub in item.submenu?.items ?? [] {
                print("menu:   \(sub.title)\(sub.state == .on ? " (on)" : "")")
            }
        }
        print("launch at login: \(SMAppService.mainApp.status.rawValue) (0 not registered, 1 enabled, 2 needs approval, 3 not found)")
        let hk = HotKey {}
        let s = Shortcut.current
        print("shortcut \(s.title): \(hk.register(s) ? "registered" : "taken")")
        hk.unregister()
        // Clicks on the tab row, under the hidden title bar, reach the tabs.
        let v = controller.view
        v.frame = controller.panel.contentView?.bounds ?? .zero
        v.layoutSubtreeIfNeeded()
        if let frame = controller.panel.contentView?.superview {
            for b in v.tabBar.subviews.prefix(2) + [v.header.field] {
                let p = b.convert(NSPoint(x: b.bounds.midX, y: b.bounds.midY), to: nil)
                let hit = frame.hitTest(frame.convert(p, from: nil))
                var name = hit.map { String(describing: type(of: $0)) } ?? "nothing"
                if let hit, hit !== b, hit.isDescendant(of: b) { name += " (inside)" }
                print("hit test \(type(of: b)) at \(Int(p.x)),\(Int(p.y)): \(name)")
            }
        }
        print("window: \(Int(controller.panel.frame.width))x\(Int(controller.panel.frame.height)), version \(appVersion())")
        // With --snapshot: the menu bar icon, drawn large, black on white.
        if let out = options.snapshot,
           let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 144, pixelsHigh: 144, bitsPerSample: 8,
                                      samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                      bytesPerRow: 0, bitsPerPixel: 0) {
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
            NSColor.white.setFill()
            NSRect(x: 0, y: 0, width: 144, height: 144).fill()
            AppDelegate.statusImage().draw(in: NSRect(x: 0, y: 0, width: 144, height: 144))
            NSGraphicsContext.restoreGraphicsState()
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: out))
            print("status icon: \(out)")
        }
    }

    // MARK: menus

    private func mainMenu() -> NSMenu {
        // Not shown (the app never becomes active), but text fields need
        // these key equivalents for Cmd+C, Cmd+V, Cmd+A.
        let menu = NSMenu()
        let app = NSMenuItem()
        app.submenu = NSMenu()
        app.submenu?.addItem(withTitle: tr("Quit stickpile"), action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        menu.addItem(app)
        let edit = NSMenuItem()
        let m = NSMenu(title: tr("Edit"))
        m.addItem(withTitle: tr("Undo"), action: Selector(("undo:")), keyEquivalent: "z")
        let redo = m.addItem(withTitle: tr("Redo"), action: Selector(("redo:")), keyEquivalent: "z")
        redo.keyEquivalentModifierMask = [.command, .shift]
        m.addItem(.separator())
        m.addItem(withTitle: tr("Cut"), action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        m.addItem(withTitle: tr("Copy"), action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        m.addItem(withTitle: tr("Paste"), action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        m.addItem(withTitle: tr("Select All"), action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        edit.submenu = m
        menu.addItem(edit)
        return menu
    }

    private func setUpStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = AppDelegate.statusImage()
        item.button?.toolTip = "stickpile"
        item.menu = statusMenu()
        statusItem = item
        refreshMenu()
    }

    private func statusMenu() -> NSMenu {
        let menu = NSMenu()
        menu.delegate = self
        let open = menu.addItem(withTitle: tr("Open stickpile"), action: #selector(openClicked), keyEquivalent: "")
        open.target = self
        openItem = open
        let settings = menu.addItem(withTitle: tr("Settings") + "...", action: #selector(settingsClicked), keyEquivalent: "")
        settings.target = self
        menu.addItem(.separator())
        let shortcut = menu.addItem(withTitle: tr("Shortcut"), action: nil, keyEquivalent: "")
        let sub = NSMenu()
        shortcutItems = Shortcut.all.map { s in
            let i = sub.addItem(withTitle: s.title, action: #selector(shortcutChosen(_:)), keyEquivalent: "")
            i.target = self
            i.representedObject = s.id
            return i
        }
        shortcut.submenu = sub
        let login = menu.addItem(withTitle: tr("Launch at login"), action: #selector(launchAtLoginClicked),
                                 keyEquivalent: "")
        login.target = self
        loginItem = login
        menu.addItem(.separator())
        let about = menu.addItem(withTitle: tr("About stickpile"), action: #selector(aboutClicked), keyEquivalent: "")
        about.target = self
        let quit = menu.addItem(withTitle: tr("Quit stickpile"), action: #selector(NSApplication.terminate(_:)),
                                keyEquivalent: "q")
        quit.target = NSApp
        return menu
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        refreshMenu()
    }

    private func refreshMenu() {
        let current = Shortcut.current
        openItem?.keyEquivalent = current.keyEquivalent
        openItem?.keyEquivalentModifierMask = current.modifierMask
        for i in shortcutItems {
            i.state = (i.representedObject as? String) == current.id ? .on : .off
        }
        loginItem?.state = SMAppService.mainApp.status == .enabled ? .on : .off
    }

    @objc private func openClicked() { controller.openPopup(tab: nil) }
    @objc private func settingsClicked() { controller.openPopup(tab: .settings) }

    @objc private func shortcutChosen(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String,
              let s = Shortcut.all.first(where: { $0.id == id }) else { return }
        UserDefaults.standard.set(id, forKey: Shortcut.defaultsKey)
        if hotKey?.register(s) == false {
            alert(trf("{shortcut} is taken by another app", ["shortcut": s.title]))
        }
        refreshMenu()
    }

    @objc private func launchAtLoginClicked() {
        let service = SMAppService.mainApp
        do {
            if service.status == .enabled {
                try service.unregister()
            } else {
                try service.register()
            }
        } catch {
            alert(error.localizedDescription)
        }
        refreshMenu()
    }

    @objc private func aboutClicked() {
        NSApp.activate(ignoringOtherApps: true)
        NSApp.orderFrontStandardAboutPanel(options: [
            .applicationName: "stickpile",
            .applicationVersion: appVersion(),
            .version: "",
        ])
    }

    private func alert(_ text: String) {
        NSApp.activate(ignoringOtherApps: true)
        let a = NSAlert()
        a.messageText = "stickpile"
        a.informativeText = text
        a.runModal()
    }

    /// The menu bar icon: a sticker with a peeled corner, as a template.
    static func statusImage() -> NSImage {
        let image = NSImage(size: NSSize(width: 18, height: 18), flipped: true) { _ in
            // The app icon's shape (128 units) scaled to 18 points.
            let s: CGFloat = 18.0 / 128.0
            func p(_ x: CGFloat, _ y: CGFloat) -> NSPoint { NSPoint(x: x * s, y: y * s) }
            let body = NSBezierPath()
            body.move(to: p(30, 14))
            body.line(to: p(98, 14))
            body.curve(to: p(114, 30), controlPoint1: p(107, 14), controlPoint2: p(114, 21))
            body.line(to: p(114, 76))
            body.line(to: p(76, 114))
            body.line(to: p(30, 114))
            body.curve(to: p(14, 98), controlPoint1: p(21, 114), controlPoint2: p(14, 107))
            body.line(to: p(14, 30))
            body.curve(to: p(30, 14), controlPoint1: p(14, 21), controlPoint2: p(21, 14))
            body.close()
            NSColor.black.setFill()
            body.fill()
            NSGraphicsContext.current?.compositingOperation = .clear
            NSBezierPath(ovalIn: NSRect(x: 41 * s, y: 45 * s, width: 14 * s, height: 14 * s)).fill()
            NSBezierPath(ovalIn: NSRect(x: 73 * s, y: 45 * s, width: 14 * s, height: 14 * s)).fill()
            let smile = NSBezierPath()
            smile.move(to: p(42, 74))
            smile.curve(to: p(80, 74), controlPoint1: p(52, 86), controlPoint2: p(70, 86))
            smile.lineWidth = 9 * s
            smile.lineCapStyle = .round
            smile.stroke()
            // The peeled corner: cut out, then drawn as an outline.
            let fold = NSBezierPath()
            fold.move(to: p(114, 76))
            fold.line(to: p(76, 114))
            fold.line(to: p(76, 90))
            fold.curve(to: p(90, 76), controlPoint1: p(76, 82), controlPoint2: p(82, 76))
            fold.close()
            fold.fill()
            NSGraphicsContext.current?.compositingOperation = .sourceOver
            let corner = NSBezierPath()
            corner.move(to: p(76, 108))
            corner.line(to: p(76, 90))
            corner.curve(to: p(90, 76), controlPoint1: p(76, 82), controlPoint2: p(82, 76))
            corner.line(to: p(108, 76))
            corner.lineWidth = 8 * s
            corner.lineJoinStyle = .round
            corner.stroke()
            return true
        }
        image.isTemplate = true
        return image
    }
}

@main
enum StickpileMain {
    static func main() {
        let options = Options.parse(CommandLine.arguments)
        let app = NSApplication.shared
        let delegate = AppDelegate(options: options)
        app.delegate = delegate
        app.setActivationPolicy(.accessory)
        withExtendedLifetime(delegate) {
            app.run()
        }
    }
}
