import AppKit

/// The key help over the main area: sections in two columns on a card.
final class HelpOverlay: NSView {
    private let card = HelpCard()
    private let scroll = NSScrollView()
    private let content = FlippedView()
    private var sections: [HelpSection] = []

    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.borderType = .noBorder
        scroll.documentView = content
        card.addSubview(scroll)
        addSubview(card)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func hitTest(_ point: NSPoint) -> NSView? {
        let p = convert(point, from: superview)
        return card.frame.contains(p) ? super.hitTest(point) : nil
    }

    func update(_ help: [HelpSection]?) {
        setShown(help != nil)
        guard let help, help != sections else { return }
        sections = help
        rebuild()
    }

    /// Two columns of about equal height, sections kept whole.
    private func columns() -> [[HelpSection]] {
        let height = { (s: HelpSection) in s.keys.count + 2 }
        let total = sections.map(height).reduce(0, +)
        var out: [[HelpSection]] = [[], []]
        var left = 0
        for s in sections {
            let col = left >= total / 2 ? 1 : 0
            if col == 0 { left += height(s) }
            out[col].append(s)
        }
        return out.filter { !$0.isEmpty }
    }

    private var contentSize = NSSize.zero

    private func rebuild() {
        content.subviews.forEach { $0.removeFromSuperview() }
        let keyWidth: CGFloat = 132, actionWidth: CGFloat = 200, colGap: CGFloat = 28
        let keyFont = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .semibold)
        let actionFont = NSFont.systemFont(ofSize: 12)
        var maxY: CGFloat = 0
        for (c, col) in columns().enumerated() {
            let x0 = CGFloat(c) * (keyWidth + 12 + actionWidth + colGap)
            var y: CGFloat = 0
            for (n, section) in col.enumerated() {
                if n > 0 { y += 12 }
                let title = label(section.title, size: 13, weight: .semibold)
                title.frame = NSRect(x: x0, y: y, width: keyWidth + 12 + actionWidth, height: 18)
                content.addSubview(title)
                y += 22
                for k in section.keys {
                    let key = wrappingLabel(k.keys, size: 11.5)
                    key.font = keyFont
                    let what = wrappingLabel(k.action, size: 12, color: .secondaryLabelColor)
                    what.font = actionFont
                    let kh = ceil(key.cell!.cellSize(forBounds: NSRect(x: 0, y: 0, width: keyWidth, height: 1000)).height)
                    let wh = ceil(what.cell!.cellSize(forBounds: NSRect(x: 0, y: 0, width: actionWidth, height: 1000)).height)
                    key.frame = NSRect(x: x0, y: y, width: keyWidth, height: kh)
                    what.frame = NSRect(x: x0 + keyWidth + 12, y: y, width: actionWidth, height: wh)
                    content.addSubview(key)
                    content.addSubview(what)
                    y += max(kh, wh) + 3
                }
            }
            maxY = max(maxY, y)
        }
        let cols = CGFloat(columns().count)
        let width = cols * (keyWidth + 12 + actionWidth) + max(0, cols - 1) * colGap
        let hint = label(tr("? or Esc closes this"), size: 11, color: .secondaryLabelColor)
        hint.alignment = .right
        hint.frame = NSRect(x: 0, y: maxY + 10, width: width, height: 15)
        content.addSubview(hint)
        contentSize = NSSize(width: width, height: maxY + 25)
        content.frame = NSRect(origin: .zero, size: contentSize)
        needsLayout = true
    }

    override func layout() {
        super.layout()
        let pad: CGFloat = 20
        let w = min(contentSize.width + 2 * pad, bounds.width - 24)
        let h = min(contentSize.height + 2 * pad, bounds.height - 24)
        card.frame = NSRect(x: floor((bounds.width - w) / 2), y: floor((bounds.height - h) / 2), width: w, height: h)
        scroll.frame = card.bounds.insetBy(dx: pad, dy: pad - 6)
        content.frame = NSRect(x: 0, y: 0, width: max(contentSize.width, scroll.contentSize.width), height: contentSize.height)
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        // The card's shadow.
        NSGraphicsContext.saveGraphicsState()
        let shadow = NSShadow()
        shadow.shadowBlurRadius = 16
        shadow.shadowOffset = NSSize(width: 0, height: -3)
        shadow.shadowColor = NSColor.black.withAlphaComponent(0.35)
        shadow.set()
        NSColor.windowBackgroundColor.setFill()
        NSBezierPath(roundedRect: card.frame, xRadius: 12, yRadius: 12).fill()
        NSGraphicsContext.restoreGraphicsState()
    }
}

final class HelpCard: NSView {
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 12, yRadius: 12)
        NSColor.windowBackgroundColor.setFill()
        path.fill()
        NSColor.separatorColor.setStroke()
        path.stroke()
    }
}

/// "stickpile cannot start": the error and the config file.
final class ErrorPage: NSView {
    private let stack = NSStackView()
    private let icon = NSImageView()
    private let title = label("", size: 20, weight: .semibold)
    private let message = wrappingLabel("", size: 13)
    private let config = wrappingLabel("", size: 12, color: .secondaryLabelColor)

    override init(frame: NSRect) {
        super.init(frame: frame)
        icon.image = symbol("exclamationmark.triangle", size: 44, color: .secondaryLabelColor)
        message.alignment = .center
        message.isSelectable = true
        config.alignment = .center
        config.isSelectable = true
        stack.orientation = .vertical
        stack.alignment = .centerX
        stack.spacing = 10
        stack.translatesAutoresizingMaskIntoConstraints = false
        [icon, title, message, config].forEach(stack.addArrangedSubview)
        stack.setCustomSpacing(16, after: title)
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
            stack.widthAnchor.constraint(lessThanOrEqualTo: widthAnchor, constant: -60),
            stack.widthAnchor.constraint(lessThanOrEqualToConstant: 560),
            message.widthAnchor.constraint(lessThanOrEqualToConstant: 560),
            config.widthAnchor.constraint(lessThanOrEqualToConstant: 560),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    func update(error: String, configPath: String) {
        title.setText(tr("stickpile cannot start"))
        message.setText(error)
        config.setText(trf("Config: {file}", ["file": configPath]))
    }
}
