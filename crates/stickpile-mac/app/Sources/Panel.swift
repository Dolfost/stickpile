import AppKit

/// The popup window: a floating panel that takes the keyboard without
/// activating the app, so the chat app stays frontmost and gets the focus
/// back when the popup hides.
final class PopupPanel: NSPanel {
    var onResignKey: (() -> Void)?
    var onFirstResponder: (() -> Void)?

    init(size: NSSize) {
        super.init(contentRect: NSRect(origin: .zero, size: size),
                   styleMask: [.titled, .resizable, .fullSizeContentView, .nonactivatingPanel],
                   backing: .buffered, defer: false)
        title = "stickpile"
        isFloatingPanel = true
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        titleVisibility = .hidden
        titlebarAppearsTransparent = true
        hidesOnDeactivate = false
        becomesKeyOnlyIfNeeded = false
        isReleasedWhenClosed = false
        isMovableByWindowBackground = false
        animationBehavior = .utilityWindow
        minSize = NSSize(width: 520, height: 380)
        for b in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
            standardWindowButton(b)?.isHidden = true
        }
    }

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    override func resignKey() {
        super.resignKey()
        onResignKey?()
    }

    override func makeFirstResponder(_ responder: NSResponder?) -> Bool {
        let ok = super.makeFirstResponder(responder)
        onFirstResponder?()
        return ok
    }

    // Escape is the popup's; nothing else closes the panel.
    override func cancelOperation(_ sender: Any?) {}
}

/// Everything inside the panel.
final class PopupView: NSView {
    let tabBar = TabBar()
    let header = HeaderBar()
    let content = FlippedView()
    let itemsGrid: GridView
    let packsGrid: GridView
    let mePage = MePage()
    let settingsPage = SettingsPage()
    let help = HelpOverlay()
    let footer = FooterBar()
    let errorPage = ErrorPage()
    private let topLine = NSBox()
    private let bottomLine = NSBox()
    private var kind = ViewKind.items
    /// Height of the panel's (invisible) title bar the tab row sits in.
    private let topInset: CGFloat = 6

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    // Keys the popup did not take end here instead of beeping.
    override func keyDown(with event: NSEvent) {}

    init(thumbs: ThumbStore) {
        itemsGrid = GridView(kind: .items, thumbs: thumbs)
        packsGrid = GridView(kind: .packs, thumbs: thumbs)
        super.init(frame: .zero)
        for line in [topLine, bottomLine] {
            line.boxType = .separator
        }
        [itemsGrid, packsGrid, mePage, settingsPage].forEach {
            $0.autoresizingMask = [.width, .height]
            $0.isHidden = true
            content.addSubview($0)
        }
        itemsGrid.isHidden = false
        help.autoresizingMask = [.width, .height]
        help.isHidden = true
        content.addSubview(help)
        [tabBar, header, topLine, content, bottomLine, footer, errorPage].forEach(addSubview)
        errorPage.isHidden = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        let w = bounds.width, h = bounds.height
        tabBar.frame = NSRect(x: 0, y: topInset, width: w, height: 32)
        header.frame = NSRect(x: 0, y: topInset + 34, width: w, height: 42)
        topLine.frame = NSRect(x: 0, y: header.frame.maxY, width: w, height: 1)
        let footerHeight: CGFloat = 30
        footer.frame = NSRect(x: 0, y: h - footerHeight, width: w, height: footerHeight)
        bottomLine.frame = NSRect(x: 0, y: h - footerHeight - 1, width: w, height: 1)
        content.frame = NSRect(x: 0, y: topLine.frame.maxY, width: w, height: max(0, bottomLine.frame.minY - topLine.frame.maxY))
        for v in content.subviews { v.frame = content.bounds }
        errorPage.frame = bounds
    }

    /// The grid shown now, if any.
    var shownGrid: GridView? {
        switch kind {
        case .items: return itemsGrid
        case .packs: return packsGrid
        default: return nil
        }
    }

    var shownPage: PageView? {
        switch kind {
        case .me: return mePage
        case .settings: return settingsPage
        default: return nil
        }
    }

    /// The app's own texts in the current language (Rust sends the rest).
    func relabel() {
        tabBar.relabel()
        header.relabel()
        footer.relabel()
        mePage.relabel()
    }

    func show(_ k: ViewKind) {
        kind = k
        itemsGrid.setShown(k == .items)
        packsGrid.setShown(k == .packs)
        mePage.setShown(k == .me)
        settingsPage.setShown(k == .settings)
    }

    func showError(_ error: String?, configPath: String) {
        let failed = error != nil
        errorPage.setShown(failed)
        for v in [tabBar, header, topLine, content, bottomLine, footer] as [NSView] {
            v.setShown(!failed)
        }
        if let error { errorPage.update(error: error, configPath: configPath) }
    }
}

/// The panel's content: the window material with the popup on it.
final class PopupBackground: NSVisualEffectView {
    /// An opaque window colour under the popup for snapshots: the material
    /// does not draw offscreen.
    let backdrop = Backdrop()

    init(_ popup: PopupView) {
        super.init(frame: .zero)
        material = .popover
        blendingMode = .behindWindow
        state = .active
        backdrop.frame = bounds
        backdrop.autoresizingMask = [.width, .height]
        backdrop.isHidden = true
        addSubview(backdrop)
        popup.frame = bounds
        popup.autoresizingMask = [.width, .height]
        addSubview(popup)
    }

    required init?(coder: NSCoder) { fatalError() }
}

final class Backdrop: NSView {
    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        dirtyRect.fill()
    }
}
