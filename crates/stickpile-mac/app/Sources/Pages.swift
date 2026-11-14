import AppKit

/// What the keyboard cursor of the Me and Settings pages can stop on.
enum PageControl {
    case button(NSButton)
    case toggle(NSSwitch)
    case choice(NSPopUpButton)
    case number(NSTextField, NSStepper)
    case text(NSTextField)

    var view: NSView {
        switch self {
        case .button(let b): return b
        case .toggle(let s): return s
        case .choice(let p): return p
        case .number(let f, _): return f
        case .text(let f): return f
        }
    }

    var enabled: Bool { (view as? NSControl)?.isEnabled ?? true }

    /// The control has the keyboard focus (a field being edited).
    func owns(_ r: NSResponder?) -> Bool {
        guard let r else { return false }
        if r === view { return true }
        if let f = view as? NSTextField, let editor = f.currentEditor(), editor === r { return true }
        return false
    }

    /// Left / Right: a choice or a number one step, a switch off / on.
    func step(forward: Bool) {
        switch self {
        case .choice(let p):
            let n = p.numberOfItems
            guard n > 0 else { return }
            let cur = p.indexOfSelectedItem
            let next = cur < 0 ? 0 : (forward ? min(cur + 1, n - 1) : max(cur - 1, 0))
            if next != cur {
                p.selectItem(at: next)
                p.sendAction(p.action, to: p.target)
            }
        case .number(_, let stepper):
            let v = stepper.integerValue + (forward ? 1 : -1)
            if v >= Int(stepper.minValue), v <= Int(stepper.maxValue), v != stepper.integerValue {
                stepper.integerValue = v
                stepper.sendAction(stepper.action, to: stepper.target)
            }
        case .toggle(let s):
            let state: NSControl.StateValue = forward ? .on : .off
            if s.state != state {
                s.state = state
                s.sendAction(s.action, to: s.target)
            }
        case .button, .text:
            break
        }
    }

    /// Enter / Space: press, flip, open the list, or edit the field.
    func activate() {
        switch self {
        case .button(let b): b.performClick(nil)
        case .toggle(let s):
            s.state = s.state == .on ? .off : .on
            s.sendAction(s.action, to: s.target)
        case .choice(let p): p.performClick(nil)
        case .number, .text: edit()
        }
    }

    func edit() {
        switch self {
        case .number(let f, _), .text(let f):
            f.window?.makeFirstResponder(f)
        default:
            break
        }
    }
}

struct PageStop {
    /// What the cursor highlights: a row, or the control itself.
    let highlight: NSView
    let control: PageControl
    var outline: Bool { highlight === control.view }
}

/// The page cursor: a tinted row, or an outline around a button.
final class CursorView: NSView {
    var outline = false
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func draw(_ dirtyRect: NSRect) {
        let accent = NSColor.controlAccentColor
        if outline {
            accent.withAlphaComponent(0.85).setStroke()
            let p = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: 7, yRadius: 7)
            p.lineWidth = 2
            p.stroke()
        } else {
            accent.withAlphaComponent(0.14).setFill()
            NSBezierPath(roundedRect: bounds, xRadius: 6, yRadius: 6).fill()
            accent.setFill()
            NSBezierPath(roundedRect: NSRect(x: 0, y: 3, width: 3, height: bounds.height - 6), xRadius: 1.5, yRadius: 1.5).fill()
        }
    }
}

final class PageDocument: FlippedView {
    var onLayout: (() -> Void)?
    override func layout() {
        super.layout()
        onLayout?()
    }
}

/// The page's column; rows moving (one hidden above them) move the cursor.
final class PageColumn: NSStackView {
    var onLayout: (() -> Void)?
    override func layout() {
        super.layout()
        onLayout?()
    }
}

/// A scrolling page with a centred column and a keyboard cursor; the Me
/// and Settings pages fill `column` and list their `stops()`.
class PageView: NSScrollView {
    let document = PageDocument()
    let column = PageColumn()
    private let cursor = CursorView()
    private var cursorControl: NSView?
    private var cursorOn = false

    init(maxWidth: CGFloat) {
        super.init(frame: .zero)
        hasVerticalScroller = true
        hasHorizontalScroller = false
        autohidesScrollers = true
        drawsBackground = false
        borderType = .noBorder
        document.translatesAutoresizingMaskIntoConstraints = false
        documentView = document
        let clip = contentView
        NSLayoutConstraint.activate([
            document.leadingAnchor.constraint(equalTo: clip.leadingAnchor),
            document.topAnchor.constraint(equalTo: clip.topAnchor),
            document.widthAnchor.constraint(equalTo: clip.widthAnchor),
        ])
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 14
        column.detachesHiddenViews = true
        column.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(column)
        let width = column.widthAnchor.constraint(equalTo: document.widthAnchor, constant: -32)
        width.priority = .defaultHigh
        NSLayoutConstraint.activate([
            column.topAnchor.constraint(equalTo: document.topAnchor, constant: 16),
            column.bottomAnchor.constraint(equalTo: document.bottomAnchor, constant: -20),
            column.centerXAnchor.constraint(equalTo: document.centerXAnchor),
            column.widthAnchor.constraint(lessThanOrEqualToConstant: maxWidth),
            width,
        ])
        cursor.isHidden = true
        document.addSubview(cursor)
        document.onLayout = { [weak self] in self?.placeCursor(scroll: false) }
        column.onLayout = { [weak self] in self?.placeCursor(scroll: false) }
    }

    required init?(coder: NSCoder) { fatalError() }

    /// Every stop in page order, shown or not.
    func stops() -> [PageStop] { [] }

    private func shown(_ all: [PageStop]) -> [PageStop] {
        all.filter { $0.control.view.reallyShown && $0.highlight.reallyShown }
    }

    /// The cursor's position among `shown`; a stop that went away falls
    /// back to the nearest shown one before it.
    private func index(_ all: [PageStop], _ shown: [PageStop]) -> Int {
        guard let c = cursorControl else { return 0 }
        if let i = shown.firstIndex(where: { $0.control.view === c }) { return i }
        guard let at = all.firstIndex(where: { $0.control.view === c }) else { return 0 }
        for s in all[..<at].reversed() {
            if let i = shown.firstIndex(where: { $0.control.view === s.control.view }) { return i }
        }
        return 0
    }

    /// After the page changed what it shows: lay it out, move the cursor along.
    func contentChanged() {
        document.layoutSubtreeIfNeeded()
        placeCursor(scroll: false)
    }

    /// Back to the first stop (the page shows something else now).
    func resetCursor() {
        cursorControl = nil
    }

    func setCursor(on: Bool) {
        guard on != cursorOn else { return }
        cursorOn = on
        placeCursor(scroll: on)
    }

    func pageKey(_ key: PageKey) {
        let all = stops()
        let list = shown(all)
        guard !list.isEmpty else { return }
        let i = index(all, list)
        let stop = list[i]
        let usable = stop.control.enabled
        debug("page key \(key) on stop \(i)/\(list.count) \(type(of: stop.control.view)) usable=\(usable)")
        // On first; a render the action causes may take it away (modes turned off).
        cursorOn = true
        switch key {
        case .up:
            cursorControl = list[max(i - 1, 0)].control.view
        case .down:
            cursorControl = list[min(i + 1, list.count - 1)].control.view
        case .left, .right:
            if usable { stop.control.step(forward: key == .right) }
        case .activate:
            if usable { stop.control.activate() }
        case .edit:
            if usable { stop.control.edit() }
        }
        placeCursor(scroll: true)
    }

    /// The keyboard focus moved (a click into a field): the cursor follows.
    func follow(_ responder: NSResponder?) {
        guard let stop = stops().first(where: { $0.control.owns(responder) }) else { return }
        cursorControl = stop.control.view
        placeCursor(scroll: false)
    }

    func placeCursor(scroll: Bool) {
        let all = stops()
        let list = shown(all)
        guard cursorOn, !list.isEmpty else {
            cursor.isHidden = true
            return
        }
        let i = index(all, list)
        let stop = list[i]
        var r = document.convert(stop.highlight.bounds, from: stop.highlight)
        if stop.outline { r = r.insetBy(dx: -4, dy: -4) }
        if cursor.frame != r || cursor.outline != stop.outline || cursor.isHidden {
            cursor.outline = stop.outline
            cursor.frame = r
            cursor.isHidden = false
            cursor.needsDisplay = true
        }
        if scroll {
            if i == 0 {
                document.scroll(.zero)
            } else {
                document.scrollToVisible(r.insetBy(dx: 0, dy: -10))
            }
        }
    }
}

/// A rounded box of rows with lines between them.
final class RowBox: NSView {
    let rows = NSStackView()

    override var isFlipped: Bool { true }

    init() {
        super.init(frame: .zero)
        rows.orientation = .vertical
        rows.alignment = .leading
        rows.spacing = 0
        rows.detachesHiddenViews = true
        rows.translatesAutoresizingMaskIntoConstraints = false
        addSubview(rows)
        NSLayoutConstraint.activate([
            rows.leadingAnchor.constraint(equalTo: leadingAnchor),
            rows.trailingAnchor.constraint(equalTo: trailingAnchor),
            rows.topAnchor.constraint(equalTo: topAnchor),
            rows.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    func add(_ row: NSView) {
        rows.addFullWidth(row)
    }

    func removeAll() {
        for v in rows.arrangedSubviews {
            rows.removeArrangedSubview(v)
            v.removeFromSuperview()
        }
    }

    override func layout() {
        super.layout()
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        let r = bounds.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: r, xRadius: 9, yRadius: 9)
        NSColor.labelColor.withAlphaComponent(0.045).setFill()
        path.fill()
        NSColor.separatorColor.setStroke()
        path.lineWidth = 1
        path.stroke()
        let visible = rows.arrangedSubviews.filter { !$0.isHidden }
        NSColor.separatorColor.setFill()
        for v in visible.dropLast() {
            let y = convert(v.frame, from: rows).maxY
            NSRect(x: 12, y: floor(y), width: bounds.width - 12, height: 1).fill()
        }
    }
}

/// A group: a title, an optional description and a box of rows.
final class GroupView: NSStackView {
    let title = label("", size: 13, weight: .semibold)
    let detail = wrappingLabel("", size: 11, color: .secondaryLabelColor)
    let box = RowBox()

    init(title text: String = "") {
        super.init(frame: .zero)
        orientation = .vertical
        alignment = .leading
        spacing = 6
        detachesHiddenViews = true
        title.stringValue = text
        addFullWidth(title)
        addFullWidth(detail)
        detail.isHidden = true
        addFullWidth(box)
        setCustomSpacing(8, after: detail)
    }

    required init(coder: NSCoder) { fatalError() }
}

/// A row: title and subtitle on the left, controls on the right.
final class FormRow: NSView {
    let title = label("", size: 13)
    let subtitle = wrappingLabel("", size: 11, color: .secondaryLabelColor)
    let trailing = NSStackView()

    override var isFlipped: Bool { true }

    init(trailing views: [NSView] = []) {
        super.init(frame: .zero)
        let labels = NSStackView(views: [title, subtitle])
        labels.orientation = .vertical
        labels.alignment = .leading
        labels.spacing = 2
        labels.detachesHiddenViews = true
        labels.translatesAutoresizingMaskIntoConstraints = false
        title.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        trailing.orientation = .horizontal
        trailing.spacing = 8
        trailing.alignment = .centerY
        trailing.detachesHiddenViews = true
        trailing.translatesAutoresizingMaskIntoConstraints = false
        trailing.setHuggingPriority(.required, for: .horizontal)
        trailing.setContentCompressionResistancePriority(.required, for: .horizontal)
        views.forEach { trailing.addArrangedSubview($0) }
        addSubview(labels)
        addSubview(trailing)
        NSLayoutConstraint.activate([
            labels.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            labels.topAnchor.constraint(greaterThanOrEqualTo: topAnchor, constant: 8),
            labels.bottomAnchor.constraint(lessThanOrEqualTo: bottomAnchor, constant: -8),
            labels.centerYAnchor.constraint(equalTo: centerYAnchor),
            labels.trailingAnchor.constraint(lessThanOrEqualTo: trailing.leadingAnchor, constant: -12),
            trailing.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            trailing.centerYAnchor.constraint(equalTo: centerYAnchor),
            trailing.topAnchor.constraint(greaterThanOrEqualTo: topAnchor, constant: 6),
            heightAnchor.constraint(greaterThanOrEqualToConstant: 38),
        ])
        // As low as the labels and controls allow.
        let low = heightAnchor.constraint(equalToConstant: 0)
        low.priority = .fittingSizeCompression
        low.isActive = true
        subtitle.isHidden = true
    }

    required init?(coder: NSCoder) { fatalError() }

    func setSubtitle(_ s: String) {
        subtitle.setText(s)
        subtitle.setShown(!s.isEmpty)
    }
}

/// A row with a label on the left and a text field filling the rest.
final class FieldRow: NSView {
    let title = label("", size: 13)
    let field: NSTextField

    init(field: NSTextField) {
        self.field = field
        super.init(frame: .zero)
        title.translatesAutoresizingMaskIntoConstraints = false
        field.translatesAutoresizingMaskIntoConstraints = false
        field.isBezeled = true
        field.bezelStyle = .roundedBezel
        field.font = .systemFont(ofSize: 13)
        addSubview(title)
        addSubview(field)
        NSLayoutConstraint.activate([
            title.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            title.centerYAnchor.constraint(equalTo: centerYAnchor),
            title.widthAnchor.constraint(equalToConstant: 150),
            field.leadingAnchor.constraint(equalTo: title.trailingAnchor, constant: 8),
            field.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            field.centerYAnchor.constraint(equalTo: centerYAnchor),
            heightAnchor.constraint(equalToConstant: 40),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }
}
