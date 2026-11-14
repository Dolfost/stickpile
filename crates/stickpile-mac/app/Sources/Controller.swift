import AppKit

/// The popup's methods the app calls: the Rust `Popup`, or `FakePopup`.
protocol PopupControl: AnyObject {
    func start()
    func toggle(front: FrontApp?, popupActive: Bool)
    func `open`(front: FrontApp?, tab: TabId?)
    func hide()
    func hidden()
    func key(key: KeyInput) -> Bool
    func queryChanged(text: String)
    func select(index: UInt32)
    func activate(index: UInt32)
    func showTab(tab: TabId)
    func toggleHelp()
    func toggleSticker()
    func cycleTarget()
    func packBack()
    func textFocus(focused: Bool)
    func gridSize(width: Double, height: Double)
    func windowSize() -> Size
    func login(username: String, password: String, totp: String)
    func loginTailnet()
    func logout()
    func retryProfile()
    func openServer()
    func setSetting(key: String, value: SettingValue)
    func resetSetting(key: String)
}

extension Popup: PopupControl {}

/// The app's side of the popup: draws what Rust sends, reports input.
///
/// Calls into the popup never nest: one made while another runs (from a
/// render, or a control reacting to one) is queued on the main queue.
final class PopupController: NSObject, PopupUi, MeActions, SettingsActions, @unchecked Sendable {
    let panel: PopupPanel
    let view: PopupView
    let thumbs = ThumbStore()
    let pasteboard: NSPasteboard
    private(set) var popup: PopupControl?
    private(set) var state: ViewState?
    private var monitor: Any?
    /// A popup method is running.
    private var inPopup = false
    /// Drawing a state or showing the window: focus changes are not the user's.
    private var rendering = false
    private var hiding = false
    private var textFocused = false
    private var reportedGrid = NSSize.zero
    private var pendingGrid: NSSize?
    private var appliedSize: Size?
    /// Snapshot runs: the panel is drawn offscreen and never takes the
    /// keyboard, so a test run cannot catch what the user types elsewhere.
    var offscreen = false {
        didSet {
            panel.alphaValue = offscreen ? 0 : 1
            panel.ignoresMouseEvents = offscreen
        }
    }
    /// The language the app's own texts are in; `onLanguage` runs when it changes.
    private var language = ""
    var onLanguage: (() -> Void)?
    /// (view, selected, revision) last scrolled to.
    private var scrolledTo: (ViewKind, UInt32?, UInt64)?

    init(testPasteboard: Bool) {
        pasteboard = testPasteboard
            ? NSPasteboard(name: NSPasteboard.Name("io.github.dolfost.Stickpile.test"))
            : .general
        view = PopupView(thumbs: thumbs)
        panel = PopupPanel(size: NSSize(width: 888, height: 688))
        super.init()
        panel.contentView = PopupBackground(view)
        panel.initialFirstResponder = view
        thumbs.onReady = { [weak self] key in
            self?.view.itemsGrid.thumbnailReady(key)
            self?.view.packsGrid.thumbnailReady(key)
        }
        wire()
    }

    func attach(_ p: PopupControl) {
        popup = p
    }

    private func wire() {
        let v = view
        v.tabBar.onTab = { [weak self] tab in self?.send { $0.showTab(tab: tab) } }
        v.tabBar.onHelp = { [weak self] in self?.send { $0.toggleHelp() } }
        v.header.onBack = { [weak self] in self?.send { $0.packBack() } }
        v.header.onQuery = { [weak self] text in self?.send { $0.queryChanged(text: text) } }
        v.footer.target.action = { [weak self] in self?.send { $0.cycleTarget() } }
        v.footer.sticker.action = { [weak self] in self?.send { $0.toggleSticker() } }
        for grid in [v.itemsGrid, v.packsGrid] {
            grid.collection.onSelect = { [weak self] i in self?.send { $0.select(index: UInt32(i)) } }
            grid.collection.onActivate = { [weak self] i in self?.send { $0.activate(index: UInt32(i)) } }
            grid.onSize = { [weak self, weak grid] size in
                guard let self, let grid, !grid.isHidden else { return }
                self.gridResized(size)
            }
        }
        v.mePage.actions = self
        v.settingsPage.actions = self

        panel.onResignKey = { [weak self] in
            guard let self, !self.hiding, self.panel.isVisible else { return }
            // The user went elsewhere: the popup goes away.
            self.hiding = true
            self.panel.orderOut(nil)
            self.hiding = false
            self.send { $0.hidden() }
        }
        panel.onFirstResponder = { [weak self] in self?.firstResponderChanged() }

        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, event.window === self.panel, self.panel.isKeyWindow else { return event }
            // A key that is composing text (an input method) belongs to the field.
            if let tv = self.panel.firstResponder as? NSTextView, tv.hasMarkedText() { return event }
            return self.key(event) ? nil : event
        }
    }

    // MARK: calling the popup

    /// Run `f` on the popup now, or soon when a popup call is running.
    func send(_ f: @escaping (PopupControl) -> Void) {
        guard let popup else { return }
        if inPopup {
            DispatchQueue.main.async { [weak self] in self?.send(f) }
            return
        }
        inPopup = true
        defer { inPopup = false }
        f(popup)
    }

    private func key(_ event: NSEvent) -> Bool {
        guard let popup, !inPopup else { return false }
        inPopup = true
        defer { inPopup = false }
        return popup.key(key: Keys.input(event))
    }

    /// The frontmost app before the popup shows, unless it is this one.
    static func frontApp() -> FrontApp? {
        guard let app = NSWorkspace.shared.frontmostApplication,
              app.processIdentifier != ProcessInfo.processInfo.processIdentifier
        else { return nil }
        return FrontApp(bundleId: app.bundleIdentifier ?? "", name: app.localizedName ?? "")
    }

    /// The global shortcut.
    func shortcutPressed() {
        let front = Self.frontApp()
        applyWindowSize()
        let active = panel.isVisible && panel.isKeyWindow
        send { $0.toggle(front: front, popupActive: active) }
    }

    /// The menu bar item (and --show).
    func openPopup(tab: TabId?) {
        let front = Self.frontApp()
        applyWindowSize()
        send { $0.open(front: front, tab: tab) }
    }

    /// The window's size for the config, when the config changed it.
    func applyWindowSize() {
        guard let popup, !inPopup else { return }
        inPopup = true
        let size = popup.windowSize()
        inPopup = false
        guard size != appliedSize, size.width > 0, size.height > 0 else { return }
        appliedSize = size
        panel.setContentSize(NSSize(width: size.width, height: size.height))
        view.layoutSubtreeIfNeeded()
    }

    private func gridResized(_ size: NSSize) {
        guard size.width > 0, size.height > 0 else { return }
        pendingGrid = size
        DispatchQueue.main.async { [weak self] in
            guard let self, let s = self.pendingGrid, s != self.reportedGrid else { return }
            self.pendingGrid = nil
            self.reportedGrid = s
            self.send { $0.gridSize(width: Double(s.width), height: Double(s.height)) }
        }
    }

    private func firstResponderChanged() {
        let responder = panel.firstResponder
        view.shownPage?.follow(responder)
        let text = isTextResponder(responder)
        guard !rendering, panel.isKeyWindow || offscreen else {
            textFocused = text
            return
        }
        guard text != textFocused else { return }
        textFocused = text
        // Reported once things settle, if the focus stayed.
        DispatchQueue.main.async { [weak self] in
            guard let self, isTextResponder(self.panel.firstResponder) == text else { return }
            self.send { $0.textFocus(focused: text) }
        }
    }

    // MARK: PopupUi

    func render(view s: ViewState) {
        rendering = true
        let started = uiDebug ? Date() : nil
        defer {
            rendering = false
            if let started {
                debug(String(format: "render %.1f ms (entries %@, columns %d)", Date().timeIntervalSince(started) * 1000,
                             s.entries.map { String($0.count) } ?? "same", s.columns))
            }
        }
        let v = view
        state = s
        if s.language != language {
            language = s.language
            v.relabel()
            onLanguage?()
        }
        v.showError(s.error, configPath: s.configPath)
        v.tabBar.update(tabs: s.tabs, current: s.tab)
        v.header.update(s)
        thumbs.pixelSize = CGFloat(s.thumbPx) * max(2, panel.backingScaleFactor)
        if let e = s.entries { v.itemsGrid.setEntries(e) }
        if let p = s.packs { v.packsGrid.setPacks(p) }
        v.itemsGrid.configure(columns: Int(s.columns), thumbPx: CGFloat(s.thumbPx), tiles: s.tiles)
        v.packsGrid.configure(columns: Int(s.columns), thumbPx: CGFloat(s.thumbPx), tiles: s.tiles)
        v.show(s.view)
        let selected = s.selected.map { Int($0) }
        switch s.view {
        case .items: v.itemsGrid.setSelected(selected)
        case .packs: v.packsGrid.setSelected(selected)
        default: break
        }
        v.mePage.update(s.me)
        v.settingsPage.update(s.settings)
        let cursor = s.footer.mode != nil
        v.mePage.setCursor(on: cursor && s.view == .me)
        v.settingsPage.setCursor(on: cursor && s.view == .settings)
        v.help.update(s.help)
        v.footer.update(s.footer)
        v.needsLayout = true
        if panel.isVisible {
            v.layoutSubtreeIfNeeded()
            scrollIfMoved()
            applyFocus(s.focus)
            checkStaleInsert()
        }
    }

    /// Insert mode on a page whose field went away (the login form after a
    /// login): no text field has the keys any more, so it is Normal mode.
    private func checkStaleInsert() {
        func stale(_ s: ViewState) -> Bool {
            s.visible && s.footer.insert && s.focus == .keep && (s.view == .me || s.view == .settings)
                && !isTextResponder(panel.firstResponder)
        }
        guard let s = state, stale(s) else { return }
        DispatchQueue.main.async { [weak self] in
            guard let self, let now = self.state, stale(now) else { return }
            self.textFocused = false
            self.send { $0.textFocus(focused: false) }
        }
    }

    private func scrollIfMoved() {
        guard let s = state, let grid = view.shownGrid else { return }
        let at = (s.view, s.selected, s.entriesRevision)
        if let last = scrolledTo, last.0 == at.0, last.1 == at.1, last.2 == at.2 { return }
        scrolledTo = at
        grid.scrollToSelected()
    }

    private func applyFocus(_ wish: FocusWish) {
        switch wish {
        case .field:
            let field = view.header.field
            guard !field.isHidden, field.currentEditor() == nil else { return }
            _ = panel.makeFirstResponder(field)
            if let editor = field.currentEditor() {
                editor.selectedRange = NSRange(location: (field.stringValue as NSString).length, length: 0)
            }
        case .noField:
            if isTextResponder(panel.firstResponder) {
                _ = panel.makeFirstResponder(view)
            }
        case .keep:
            break
        }
        textFocused = isTextResponder(panel.firstResponder)
    }

    func present() {
        rendering = true
        defer { rendering = false }
        if !panel.isVisible {
            center()
            scrolledTo = nil
        }
        panel.orderFrontRegardless()
        if !offscreen { panel.makeKey() }
        view.layoutSubtreeIfNeeded()
        scrollIfMoved()
        if let s = state { applyFocus(s.focus) }
    }

    /// Centre the panel on the screen with the mouse pointer.
    private func center() {
        let mouse = NSEvent.mouseLocation
        guard let screen = NSScreen.screens.first(where: { NSMouseInRect(mouse, $0.frame, false) }) ?? NSScreen.main
        else { return }
        let area = screen.visibleFrame
        var f = panel.frame
        f.size.width = min(f.width, area.width)
        f.size.height = min(f.height, area.height)
        f.origin.x = floor(area.midX - f.width / 2)
        f.origin.y = floor(area.midY - f.height / 2)
        panel.setFrame(f, display: false)
    }

    func hide() {
        hiding = true
        panel.orderOut(nil)
        hiding = false
    }

    func thumbnail(key: String, path: String) {
        thumbs.set(key: key, path: path)
    }

    func setClipboard(items: [PasteboardData]) -> String? {
        let item = NSPasteboardItem()
        for d in items {
            guard item.setData(d.data, forType: NSPasteboard.PasteboardType(d.kind)) else {
                return trf("The pasteboard refused {type}", ["type": d.kind])
            }
        }
        pasteboard.clearContents()
        return pasteboard.writeObjects([item]) ? nil : tr("Could not write to the pasteboard")
    }

    func openUrl(url: String) {
        let target: URL
        if let u = URL(string: url), u.scheme != nil {
            target = u
        } else {
            target = URL(fileURLWithPath: url)
        }
        NSWorkspace.shared.open(target)
    }

    func pageKey(key: PageKey) {
        debug("pageKey \(key)")
        // Pressing a button calls the popup: not from inside its call.
        DispatchQueue.main.async { [weak self] in
            self?.view.shownPage?.pageKey(key)
        }
    }

    func post(task: MainTask) {
        DispatchQueue.main.async { [weak self] in
            guard let self else {
                task.run()
                return
            }
            self.send { _ in task.run() }
        }
    }

    // MARK: Me and Settings

    func login(username: String, password: String, totp: String) {
        send { $0.login(username: username, password: password, totp: totp) }
    }

    func loginTailnet() { send { $0.loginTailnet() } }
    func logout() { send { $0.logout() } }
    func retryProfile() { send { $0.retryProfile() } }
    func openServer() { send { $0.openServer() } }
    func setSetting(key: String, value: SettingValue) {
        debug("setSetting \(key) \(value)")
        send { $0.setSetting(key: key, value: value) }
    }
    func resetSetting(key: String) { send { $0.resetSetting(key: key) } }

    // MARK: snapshot

    /// Draw the panel's content into a PNG (no screen recording needed): the
    /// layer tree, which keeps scroll positions (cacheDisplay misplaces the
    /// cells of a scrolled collection view); views without layers otherwise.
    func snapshot(to path: String) -> Bool {
        guard let content = panel.contentView else { return false }
        let backdrop = (content as? PopupBackground)?.backdrop
        backdrop?.isHidden = false
        defer { backdrop?.isHidden = true }
        content.layoutSubtreeIfNeeded()
        content.displayIfNeeded()
        CATransaction.flush()
        let bounds = content.bounds
        let scale = max(1, panel.backingScaleFactor)
        let data: Data?
        if let layer = content.layer,
           let ctx = CGContext(data: nil, width: Int(bounds.width * scale), height: Int(bounds.height * scale),
                               bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                               bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) {
            ctx.scaleBy(x: scale, y: scale)
            content.effectiveAppearance.performAsCurrentDrawingAppearance {
                ctx.setFillColor(NSColor.windowBackgroundColor.cgColor)
                ctx.fill(CGRect(origin: .zero, size: bounds.size))
            }
            layer.render(in: ctx)
            data = ctx.makeImage().flatMap { NSBitmapImageRep(cgImage: $0).representation(using: .png, properties: [:]) }
        } else if let rep = content.bitmapImageRepForCachingDisplay(in: bounds) {
            content.cacheDisplay(in: bounds, to: rep)
            data = rep.representation(using: .png, properties: [:])
        } else {
            data = nil
        }
        guard let png = data else { return false }
        print("panel: \(panel.isVisible ? "shown" : "hidden"), focus: \(isTextResponder(panel.firstResponder) ? "text field" : "none")")
        do {
            try png.write(to: URL(fileURLWithPath: path))
            return true
        } catch {
            FileHandle.standardError.write("snapshot: \(error)\n".data(using: .utf8)!)
            return false
        }
    }
}
