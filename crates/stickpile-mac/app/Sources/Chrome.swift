import AppKit

/// A flat button drawn as text: the tabs, the help button, the footer's
/// target and its sticker and safe mode toggles.
final class TextButton: NSView {
    var number = "" { didSet { if oldValue != number { changed() } } }
    var text = "" { didSet { if oldValue != text { changed() } } }
    /// Drawn after the text, smaller and dimmer: an emoji's name.
    var detail = "" { didSet { if oldValue != detail { changed() } } }
    /// Highlighted: the current tab, sticker mode on.
    var current = false { didSet { if oldValue != current { changed() } } }
    var font = NSFont.systemFont(ofSize: 13)
    var currentFont = NSFont.systemFont(ofSize: 13, weight: .semibold)
    var color = NSColor.labelColor { didSet { if oldValue != color { needsDisplay = true } } }
    var padding: CGFloat = 9
    var action: (() -> Void)?
    private var hovered = false
    private var pressed = false

    override var isFlipped: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    private func changed() {
        needsDisplay = true
        superview?.needsLayout = true
    }

    private var numberAttributes: [NSAttributedString.Key: Any] {
        [.font: NSFont.monospacedDigitSystemFont(ofSize: 10.5, weight: current ? .semibold : .regular),
         .foregroundColor: NSColor.secondaryLabelColor]
    }

    private var textAttributes: [NSAttributedString.Key: Any] {
        [.font: current ? currentFont : font, .foregroundColor: color]
    }

    private var detailAttributes: [NSAttributedString.Key: Any] {
        [.font: NSFont.systemFont(ofSize: max(font.pointSize - 1.5, 9)),
         .foregroundColor: NSColor.secondaryLabelColor]
    }

    var fittingWidth: CGFloat {
        var w = ceil((text as NSString).size(withAttributes: textAttributes).width) + 2 * padding
        if !number.isEmpty {
            w += ceil((number as NSString).size(withAttributes: numberAttributes).width) + 5
        }
        if !detail.isEmpty {
            w += ceil((detail as NSString).size(withAttributes: detailAttributes).width) + 4
        }
        return w
    }

    override func draw(_ dirtyRect: NSRect) {
        let box = bounds.insetBy(dx: 0, dy: 2)
        if current || hovered || pressed {
            let alpha: CGFloat = current ? 0.28 : (pressed ? 0.12 : 0.07)
            (current ? NSColor.controlAccentColor : NSColor.labelColor).withAlphaComponent(alpha).setFill()
            NSBezierPath(roundedRect: box, xRadius: 6, yRadius: 6).fill()
        }
        var x = padding
        if !number.isEmpty {
            let n = NSAttributedString(string: number, attributes: numberAttributes)
            let s = n.size()
            n.draw(at: NSPoint(x: x, y: box.midY - s.height / 2))
            x += ceil(s.width) + 5
        }
        let t = NSAttributedString(string: text, attributes: textAttributes)
        let s = t.size()
        let textWidth = detail.isEmpty ? bounds.width - x - padding + 2 : ceil(s.width)
        t.draw(with: NSRect(x: x, y: box.midY - s.height / 2, width: textWidth, height: s.height),
               options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
        if !detail.isEmpty {
            let d = NSAttributedString(string: detail, attributes: detailAttributes)
            let ds = d.size()
            let dx = x + ceil(s.width) + 4
            d.draw(with: NSRect(x: dx, y: box.midY - ds.height / 2, width: max(bounds.width - dx - padding + 2, 0),
                                height: ds.height),
                   options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
        }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                       owner: self))
    }

    override func mouseEntered(with event: NSEvent) { hovered = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovered = false; needsDisplay = true }

    override func mouseDown(with event: NSEvent) {
        pressed = true
        needsDisplay = true
    }

    override func mouseUp(with event: NSEvent) {
        pressed = false
        needsDisplay = true
        if bounds.contains(convert(event.locationInWindow, from: nil)) {
            action?()
        }
    }
}

/// The numbered tabs, the version (it opens the project page) and the help
/// button.
final class TabBar: NSView {
    var onTab: ((TabId) -> Void)?
    var onHelp: (() -> Void)?
    var onVersion: (() -> Void)?
    private var tabs: [TabLabel] = []
    private var buttons: [TextButton] = []
    private let help = TextButton()
    private let version = TextButton()

    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        help.text = "?"
        help.action = { [weak self] in self?.onHelp?() }
        addSubview(help)
        version.text = versionLabel()
        version.font = .monospacedDigitSystemFont(ofSize: 10.5, weight: .regular)
        version.color = .secondaryLabelColor
        version.padding = 6
        version.action = { [weak self] in self?.onVersion?() }
        addSubview(version)
    }

    required init?(coder: NSCoder) { fatalError() }

    func update(tabs new: [TabLabel], current: TabId) {
        if new != tabs {
            tabs = new
            buttons.forEach { $0.removeFromSuperview() }
            buttons = new.map { t in
                let b = TextButton()
                b.number = String(t.number)
                b.text = t.label
                b.toolTip = t.label
                let tab = t.tab
                b.action = { [weak self] in self?.onTab?(tab) }
                addSubview(b)
                return b
            }
            needsLayout = true
        }
        for (t, b) in zip(tabs, buttons) {
            b.current = t.tab == current
        }
    }

    func relabel() {
        help.toolTip = tr("Keys (? or F1)")
        version.toolTip = versionTooltip()
    }

    override func layout() {
        super.layout()
        let h = bounds.height
        var x: CGFloat = 8
        // Tabs shrink evenly when the window is narrow.
        let helpWidth = help.fittingWidth
        let versionWidth = version.fittingWidth
        let room = bounds.width - x - 8 - helpWidth - versionWidth - 6
        let widths = buttons.map(\.fittingWidth)
        let total = widths.reduce(0, +) + CGFloat(max(0, buttons.count - 1)) * 2
        let scale = total > room && total > 0 ? room / total : 1
        for (b, w) in zip(buttons, widths) {
            b.frame = NSRect(x: x, y: 0, width: floor(w * scale), height: h)
            x += floor(w * scale) + 2
        }
        help.frame = NSRect(x: bounds.width - 8 - helpWidth, y: 0, width: helpWidth, height: h)
        version.frame = NSRect(x: help.frame.minX - versionWidth, y: 0, width: versionWidth, height: h)
    }
}

/// The search field: reports when the user puts the focus into it.
final class QueryField: NSSearchField {
    override var mouseDownCanMoveWindow: Bool { false }
}

/// Back button, title, text field, syntax button, item count.
final class HeaderBar: NSView, NSSearchFieldDelegate {
    let back: NSButton
    let title = label("", size: 13, weight: .semibold)
    let field = QueryField()
    /// Next to the field where it searches (Search, an opened pack): the search syntax, as F2.
    let syntax = TextButton()
    let count = label("", size: 11, color: .secondaryLabelColor)
    var onBack: (() -> Void)?
    var onSyntax: (() -> Void)?
    /// The user edited the text.
    var onQuery: ((String) -> Void)?
    /// Not user input: set while the text is changed by a render.
    var updating = false
    private var hasField = true
    private var showBack = false
    private var showSyntax = false
    private var lastQuery = ""

    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        back = symbolButton("chevron.left", "", nil, nil)
        super.init(frame: frame)
        back.target = self
        back.action = #selector(backClicked)
        back.imageScaling = .scaleNone
        field.delegate = self
        field.target = self
        field.action = #selector(fieldAction)
        field.sendsSearchStringImmediately = true
        field.focusRingType = .default
        field.controlSize = .large
        field.font = .systemFont(ofSize: 14)
        syntax.font = .systemFont(ofSize: 12)
        syntax.color = .secondaryLabelColor
        syntax.padding = 7
        syntax.action = { [weak self] in self?.onSyntax?() }
        count.alignment = .right
        [back, title, field, syntax, count].forEach(addSubview)
    }

    required init?(coder: NSCoder) { fatalError() }

    @objc private func backClicked() { onBack?() }

    func relabel() {
        back.toolTip = tr("Back to the packs (-, Backspace)")
        syntax.text = tr("Syntax")
        syntax.toolTip = tr("Search syntax (F2)")
    }

    func update(_ s: ViewState) {
        updating = true
        defer { updating = false }
        hasField = s.hasField
        showBack = s.back
        title.setText(s.title)
        count.setText(s.count)
        if field.placeholderString != s.placeholder {
            field.placeholderString = s.placeholder
        }
        if field.stringValue != s.query {
            field.stringValue = s.query
            if let editor = field.currentEditor() {
                editor.selectedRange = NSRange(location: (s.query as NSString).length, length: 0)
            }
        }
        lastQuery = s.query
        back.setShown(s.back)
        field.setShown(s.hasField)
        // The field searches on Search and inside an opened pack, not on the pack list.
        showSyntax = s.hasField && (s.tab == .search || s.back)
        syntax.setShown(showSyntax)
        // With a field the title only names an opened pack.
        title.setShown(!s.hasField || s.back)
        count.setShown(!s.hasField && !s.count.isEmpty)
        needsLayout = true
    }

    override func layout() {
        super.layout()
        let h = bounds.height
        var x: CGFloat = 12
        let right = bounds.width - 12
        if showBack {
            back.frame = NSRect(x: x - 4, y: (h - 26) / 2, width: 26, height: 26)
            x += 26
        }
        if hasField {
            if showBack {
                let w = min(title.textWidth, (right - x) * 0.35)
                title.frame = NSRect(x: x, y: (h - 18) / 2, width: w, height: 18)
                x += w + 8
            }
            var fieldRight = right
            if showSyntax {
                let sw = syntax.fittingWidth
                syntax.frame = NSRect(x: right - sw, y: (h - 26) / 2, width: sw, height: 26)
                fieldRight = right - sw - 6
            }
            let fh = field.intrinsicContentSize.height
            field.frame = NSRect(x: x, y: floor((h - fh) / 2), width: max(0, fieldRight - x), height: fh)
        } else {
            let cw = count.isHidden ? 0 : count.textWidth
            count.frame = NSRect(x: right - cw, y: (h - 16) / 2, width: cw, height: 16)
            title.frame = NSRect(x: x, y: (h - 18) / 2, width: max(0, right - cw - 8 - x), height: 18)
        }
    }

    private func report() {
        guard !updating else { return }
        let text = field.stringValue
        guard text != lastQuery else { return }
        lastQuery = text
        onQuery?(text)
    }

    func controlTextDidChange(_ obj: Notification) { report() }
    @objc private func fieldAction() { report() }
}

/// The values offered for the filter being typed (`vibe:sar`), in one line
/// under the header; what does not fit is left out, except the chosen one.
final class SuggestBar: NSView {
    static let height: CGFloat = 24
    var onAccept: ((Int) -> Void)?
    private let key = label("", size: 11, color: .secondaryLabelColor)
    private let hint = label("", size: 11, color: .tertiaryLabelColor)
    private var buttons: [TextButton] = []
    private var shown: SuggestionView?

    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        key.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        [key, hint].forEach(addSubview)
    }

    required init?(coder: NSCoder) { fatalError() }

    func relabel() {
        hint.setText(tr("Tab"))
        hint.toolTip = tr("Tab / Shift+Tab: put in the next / previous value")
    }

    func update(_ s: SuggestionView?) {
        guard s != shown else { return }
        shown = s
        buttons.forEach { $0.removeFromSuperview() }
        buttons = (s?.words ?? []).enumerated().map { i, word in
            let b = TextButton()
            b.text = word
            // An emoji of `e:` with its name after it.
            if let labels = s?.labels, i < labels.count { b.detail = labels[i] }
            b.font = .systemFont(ofSize: 11.5)
            b.currentFont = .systemFont(ofSize: 11.5, weight: .semibold)
            b.padding = 7
            b.current = s?.chosen.map { Int($0) } == i
            b.action = { [weak self] in self?.onAccept?(i) }
            addSubview(b)
            return b
        }
        key.setText(s?.key ?? "")
        needsLayout = true
    }

    override func layout() {
        super.layout()
        let h = bounds.height
        var x: CGFloat = 12
        let keyWidth = key.textWidth
        key.frame = NSRect(x: x, y: floor((h - 15) / 2), width: keyWidth, height: 15)
        x += keyWidth + 4
        let hintWidth = hint.textWidth
        let right = bounds.width - 12 - hintWidth - 6
        // Tab may choose a value past the right edge (the words written on
        // the items can be long: x:un): the values before it give way, from
        // the first, until it fits.
        let widths = buttons.map(\.fittingWidth)
        var first = 0
        if let chosen = buttons.firstIndex(where: \.current) {
            while first < chosen, x + widths[first...chosen].reduce(0, { $0 + $1 + 2 }) - 2 > right {
                first += 1
            }
        }
        for (i, b) in buttons.enumerated() {
            let w = widths[i]
            b.isHidden = i < first || x + w > right
            guard !b.isHidden else { continue }
            b.frame = NSRect(x: x, y: 0, width: w, height: h)
            x += w + 2
        }
        hint.frame = NSRect(x: x + 4, y: floor((h - 15) / 2), width: hintWidth, height: 15)
    }
}

/// The selection's stats on top (unless hidden: `zi`); under them mode,
/// pending keys, target, sticker and safe mode toggles, status.
final class FooterBar: NSView {
    /// The row under the stats line.
    static let rowHeight: CGFloat = 30
    /// The whole bar with the stats line.
    static let fullHeight: CGFloat = 48

    /// The bar's height now: without the stats line when it is hidden.
    var height: CGFloat { footer?.detailsRow == false ? FooterBar.rowHeight : FooterBar.fullHeight }

    private let details = label("", size: 11, color: .secondaryLabelColor)
    private let mode = ModeLabel()
    private let pending = label("", size: 12, weight: .bold)
    let target = TextButton()
    private let hint = label("", size: 11, color: .secondaryLabelColor)
    let sticker = TextButton()
    let safe = TextButton()
    private let spinner = NSProgressIndicator()
    private let status = label("", size: 11, color: .secondaryLabelColor)
    private var footer: Footer?

    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        pending.font = .monospacedSystemFont(ofSize: 12, weight: .bold)
        target.font = .systemFont(ofSize: 12, weight: .semibold)
        target.currentFont = target.font
        target.padding = 5

        for toggle in [sticker, safe] {
            toggle.font = .systemFont(ofSize: 11)
            toggle.currentFont = .systemFont(ofSize: 11, weight: .medium)
            toggle.padding = 6
        }
        details.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)

        status.alignment = .right
        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false
        [details, mode, pending, target, hint, sticker, safe, spinner, status].forEach(addSubview)
    }

    required init?(coder: NSCoder) { fatalError() }

    func relabel() {
        target.toolTip = tr("Target app: click for the next one")
        sticker.toolTip = tr("Send as sticker")
        safe.toolTip = tr("Safe mode: hide items tagged nsfw (S, Cmd+Shift+S)")
    }

    func update(_ f: Footer) {
        guard f != footer else { return }
        if f.detailsRow != footer?.detailsRow {
            superview?.needsLayout = true
        }
        footer = f
        details.setShown(f.detailsRow)
        mode.text = f.mode ?? ""
        mode.insert = f.insert
        mode.setShown(f.mode != nil)
        pending.setText(f.pending)
        pending.setShown(!f.pending.isEmpty)
        target.text = f.target
        hint.setText(f.targetHint)
        // Shown for every target; dimmed where it has no sticker route.
        sticker.text = f.stickerLabel
        sticker.current = f.sticker == .on
        sticker.color = f.sticker == .unsupported ? .tertiaryLabelColor : .labelColor
        safe.text = f.safeLabel
        safe.current = f.safe
        details.setText(f.details)
        details.toolTip = f.details.isEmpty ? nil : f.details
        status.setText(f.status)
        status.toolTip = f.status.isEmpty ? nil : f.status
        switch f.level {
        case .info, .busy: status.textColor = .secondaryLabelColor
        case .warning: status.textColor = .systemOrange
        case .error: status.textColor = .systemRed
        }
        if f.level == .busy { spinner.startAnimation(nil) } else { spinner.stopAnimation(nil) }
        needsLayout = true
    }

    override func layout() {
        super.layout()
        let row = FooterBar.rowHeight
        let top = bounds.height - row
        details.frame = NSRect(x: 12, y: max(2, top - 15), width: max(0, bounds.width - 24), height: 15)
        var x: CGFloat = 10
        func place(_ v: NSView, _ w: CGFloat, _ height: CGFloat) {
            guard !v.isHidden else { return }
            v.frame = NSRect(x: x, y: top + floor((row - height) / 2), width: w, height: height)
            x += w + 8
        }
        place(mode, mode.fittingWidth, 17)
        place(pending, pending.textWidth, 16)
        place(target, target.fittingWidth, 22)
        x -= 4
        place(hint, min(hint.textWidth, max(0, bounds.width * 0.3)), 15)
        place(sticker, sticker.fittingWidth, 22)
        x -= 6
        place(safe, safe.fittingWidth, 22)
        let right = bounds.width - 10
        let spinnerWidth: CGFloat = spinner.isHidden || footer?.level != .busy ? 0 : 16
        spinner.frame = NSRect(x: right - spinnerWidth, y: top + (row - 16) / 2, width: 16, height: 16)
        let sx = x + 4
        status.frame = NSRect(x: sx, y: top + (row - 15) / 2,
                              width: max(0, right - spinnerWidth - (spinnerWidth > 0 ? 6 : 0) - sx), height: 15)
    }
}

/// INSERT / NORMAL in a small rounded box.
final class ModeLabel: NSView {
    var text = "" { didSet { if oldValue != text { needsDisplay = true } } }
    var insert = false { didSet { if oldValue != insert { needsDisplay = true } } }
    private let font = NSFont.systemFont(ofSize: 10, weight: .bold)

    override var isFlipped: Bool { true }

    var fittingWidth: CGFloat {
        ceil((text as NSString).size(withAttributes: [.font: font]).width) + 14
    }

    override func draw(_ dirtyRect: NSRect) {
        if insert {
            drawPill(text, in: bounds, font: font, fg: .white, bg: .controlAccentColor, radius: 4)
        } else {
            drawPill(text, in: bounds, font: font, fg: .labelColor, bg: NSColor.labelColor.withAlphaComponent(0.12),
                     radius: 4)
        }
    }
}
