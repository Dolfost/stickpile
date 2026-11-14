import AppKit

// Translation: the English text is the key. Static labels are written as
// calls of these with a string literal, so the catalog test finds them.
func tr(_ s: String) -> String { tr(text: s) }
func trf(_ s: String, _ args: [String: String]) -> String { trf(text: s, args: args) }

/// Padding around a thumbnail in a grid cell (bridge.rs CELL_EXTRA).
let cellExtra: CGFloat = 16
/// Height the pack list adds under a cover for the name and detail.
let packLabels: CGFloat = 40

class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

func label(_ text: String = "", size: CGFloat = NSFont.systemFontSize, weight: NSFont.Weight = .regular,
           color: NSColor = .labelColor) -> NSTextField {
    let l = NSTextField(labelWithString: text)
    l.font = .systemFont(ofSize: size, weight: weight)
    l.textColor = color
    l.lineBreakMode = .byTruncatingTail
    l.maximumNumberOfLines = 1
    l.cell?.truncatesLastVisibleLine = true
    l.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    return l
}

func wrappingLabel(_ text: String = "", size: CGFloat = NSFont.systemFontSize, weight: NSFont.Weight = .regular,
                   color: NSColor = .labelColor) -> NSTextField {
    let l = NSTextField(wrappingLabelWithString: text)
    l.font = .systemFont(ofSize: size, weight: weight)
    l.textColor = color
    l.isSelectable = false
    l.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    return l
}

func pushButton(_ title: String, _ target: AnyObject?, _ action: Selector) -> NSButton {
    let b = NSButton(title: title, target: target, action: action)
    b.bezelStyle = .push
    b.setContentHuggingPriority(.required, for: .horizontal)
    return b
}

func symbolButton(_ symbol: String, _ tooltip: String, _ target: AnyObject?, _ action: Selector?) -> NSButton {
    let image = NSImage(systemSymbolName: symbol, accessibilityDescription: tooltip) ?? NSImage()
    let b = NSButton(image: image, target: target, action: action)
    b.bezelStyle = .accessoryBarAction
    b.isBordered = false
    b.toolTip = tooltip
    b.setContentHuggingPriority(.required, for: .horizontal)
    return b
}

func symbol(_ name: String, size: CGFloat, weight: NSFont.Weight = .regular, color: NSColor? = nil) -> NSImage? {
    var config = NSImage.SymbolConfiguration(pointSize: size, weight: weight)
    if let color {
        config = config.applying(NSImage.SymbolConfiguration(paletteColors: [color]))
    }
    return NSImage(systemSymbolName: name, accessibilityDescription: nil)?.withSymbolConfiguration(config)
}

extension NSTextField {
    func setText(_ s: String) {
        if stringValue != s { stringValue = s }
    }

    /// Width of the text on one line, with the cell's margins.
    var textWidth: CGFloat {
        stringValue.isEmpty ? 0 : ceil(attributedStringValue.size().width) + 6
    }
}

extension NSView {
    func setShown(_ shown: Bool) {
        if isHidden == shown { isHidden = !shown }
    }

    /// Shown, and so is everything up to the window.
    var reallyShown: Bool { window != nil && !isHiddenOrHasHiddenAncestor }
}

extension NSStackView {
    /// Add `view` stretched to the stack's width.
    func addFullWidth(_ view: NSView) {
        addArrangedSubview(view)
        view.widthAnchor.constraint(equalTo: widthAnchor).isActive = true
    }
}

/// Attributes for one line of text, cut at the end.
func lineAttributes(font: NSFont, color: NSColor, align: NSTextAlignment = .left) -> [NSAttributedString.Key: Any] {
    let p = NSMutableParagraphStyle()
    p.lineBreakMode = .byTruncatingTail
    p.alignment = align
    return [.font: font, .foregroundColor: color, .paragraphStyle: p]
}

/// A rounded rectangle with text in it (the mode label, the badges on tiles).
func drawPill(_ text: String, in rect: NSRect, font: NSFont, fg: NSColor, bg: NSColor, radius: CGFloat) {
    bg.setFill()
    NSBezierPath(roundedRect: rect, xRadius: radius, yRadius: radius).fill()
    let s = NSAttributedString(string: text, attributes: lineAttributes(font: font, color: fg, align: .center))
    let h = s.size().height
    s.draw(with: NSRect(x: rect.minX + 3, y: rect.midY - h / 2, width: rect.width - 6, height: h),
           options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
}

/// STICKPILE_UI_DEBUG=1: trace the app's side on stderr.
let uiDebug = ProcessInfo.processInfo.environment["STICKPILE_UI_DEBUG"] == "1"

func debug(_ s: @autoclosure () -> String) {
    if uiDebug { FileHandle.standardError.write((s() + "\n").data(using: .utf8)!) }
}

/// Is this responder a text field (or the field editor of one)?
func isTextResponder(_ r: NSResponder?) -> Bool {
    if let tv = r as? NSTextView { return tv.isFieldEditor || tv.isEditable }
    return r is NSTextField
}
