import AppKit

/// What the Settings page asks the popup to do.
protocol SettingsActions: AnyObject {
    func setSetting(key: String, value: SettingValue)
    func resetSetting(key: String)
}

/// One setting: label and help, the control, the reset button.
final class SettingRowView: NSObject, NSTextFieldDelegate {
    let key: String
    let kind: SettingKind
    let row: FormRow
    let reset: NSButton
    let control: PageControl
    weak var actions: SettingsActions?
    /// The value last drawn, to tell an edit from a repeated signal.
    private var shown: SettingValue?

    init(_ r: SettingRow) {
        key = r.key
        kind = r.kind
        let resetButton = symbolButton("arrow.uturn.backward", "", nil, nil)
        reset = resetButton
        switch r.kind {
        case .bool:
            let s = NSSwitch()
            s.controlSize = .small
            control = .toggle(s)
        case .int(let min, let max):
            let f = NSTextField()
            let format = NumberFormatter()
            format.allowsFloats = false
            format.usesGroupingSeparator = false
            format.minimum = NSNumber(value: min)
            format.maximum = NSNumber(value: max)
            f.formatter = format
            f.alignment = .right
            f.widthAnchor.constraint(equalToConstant: 64).isActive = true
            let stepper = NSStepper()
            stepper.minValue = Double(min)
            stepper.maxValue = Double(max)
            stepper.increment = 1
            stepper.valueWraps = false
            control = .number(f, stepper)
        case .choice(let options):
            let p = NSPopUpButton(frame: .zero, pullsDown: false)
            p.addItems(withTitles: options.map(\.label))
            control = .choice(p)
        case .text:
            let f = NSTextField()
            f.cell?.isScrollable = true
            f.cell?.wraps = false
            f.widthAnchor.constraint(equalToConstant: 240).isActive = true
            control = .text(f)
        }
        var trailing: [NSView] = [resetButton]
        if case .number(let f, let stepper) = control {
            trailing += [f, stepper]
        } else {
            trailing.append(control.view)
        }
        row = FormRow(trailing: trailing)
        super.init()
        resetButton.target = self
        resetButton.action = #selector(resetClicked)
        switch control {
        case .toggle(let s):
            s.target = self
            s.action = #selector(changed)
        case .choice(let p):
            p.target = self
            p.action = #selector(changed)
        case .number(let f, let stepper):
            f.delegate = self
            f.target = self
            f.action = #selector(changed)
            stepper.target = self
            stepper.action = #selector(stepped)
        case .text(let f):
            f.delegate = self
            f.target = self
            f.action = #selector(changed)
        case .button:
            break
        }
        update(r)
    }

    func update(_ r: SettingRow) {
        reset.toolTip = tr("Reset to the default")
        row.title.setText(r.label)
        row.setSubtitle(r.help)
        row.toolTip = r.key
        reset.setShown(r.resettable && !r.locked)
        (control.view as? NSControl)?.isEnabled = !r.locked
        if case .number(_, let stepper) = control { stepper.isEnabled = !r.locked }
        shown = r.value
        switch (control, r.value) {
        case (.toggle(let s), .bool(let v)):
            let state: NSControl.StateValue = v ? .on : .off
            if s.state != state { s.state = state }
        case (.choice(let p), .text(let v)):
            if case .choice(let options) = kind, let i = options.firstIndex(where: { $0.value == v }),
               p.indexOfSelectedItem != i {
                p.selectItem(at: i)
            }
        case (.number(let f, let stepper), .int(let v)):
            if f.currentEditor() == nil { f.setText(String(v)) }
            stepper.integerValue = Int(v)
        case (.text(let f), .text(let v)):
            // Being edited (a value that was not saved): the text stays to be fixed.
            if f.currentEditor() == nil { f.setText(v) }
        default:
            break
        }
    }

    /// What the control shows now.
    private func value() -> SettingValue? {
        switch control {
        case .toggle(let s):
            return .bool(value: s.state == .on)
        case .choice(let p):
            guard case .choice(let options) = kind, p.indexOfSelectedItem >= 0,
                  p.indexOfSelectedItem < options.count else { return nil }
            return .text(value: options[p.indexOfSelectedItem].value)
        case .number(let f, let stepper):
            let text = f.stringValue.trimmingCharacters(in: .whitespaces)
            guard var v = Int64(text) else {
                if case .int(let old) = shown { f.stringValue = String(old) }
                return nil
            }
            v = min(max(v, Int64(stepper.minValue)), Int64(stepper.maxValue))
            return .int(value: v)
        case .text(let f):
            return .text(value: f.stringValue.trimmingCharacters(in: .whitespaces))
        case .button:
            return nil
        }
    }

    @objc private func changed() {
        debug("changed \(key): \(String(describing: value())) shown \(String(describing: shown))")
        guard let v = value(), v != shown else { return }
        shown = v
        actions?.setSetting(key: key, value: v)
    }

    @objc private func stepped() {
        guard case .number(let f, let stepper) = control else { return }
        f.stringValue = String(stepper.integerValue)
        changed()
    }

    @objc private func resetClicked() {
        actions?.resetSetting(key: key)
    }

    // A text or number field is saved when it loses the focus too.
    func controlTextDidEndEditing(_ obj: Notification) {
        changed()
    }
}

/// The Settings tab: groups of rows generated from what Rust sends.
final class SettingsPage: PageView {
    weak var actions: SettingsActions? {
        didSet { rows.forEach { $0.actions = actions } }
    }
    private var settings: SettingsView?
    private let note = wrappingLabel("", size: 11, color: .secondaryLabelColor)
    private let warnings = wrappingLabel("", size: 11, color: .systemOrange)
    private let error = wrappingLabel("", size: 13, color: .systemRed)
    private let saveError = wrappingLabel("", size: 12, color: .systemRed)
    private var groups: [GroupView] = []
    private var rows: [SettingRowView] = []
    /// Group titles and row keys and kinds: the page is rebuilt when it changes.
    private var shape: [String] = []

    init() {
        super.init(maxWidth: 680)
        column.spacing = 18
        for l in [note, warnings, error, saveError] {
            column.addFullWidth(l)
            l.isHidden = true
        }
        column.setCustomSpacing(8, after: note)
        column.setCustomSpacing(8, after: warnings)
        column.setCustomSpacing(8, after: error)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func stops() -> [PageStop] {
        var out: [PageStop] = []
        for r in rows {
            out.append(PageStop(highlight: r.row, control: r.control))
            out.append(PageStop(highlight: r.reset, control: .button(r.reset)))
        }
        return out
    }

    func update(_ s: SettingsView) {
        guard s != settings else { return }
        settings = s
        note.setText(s.note)
        note.toolTip = s.path
        note.setShown(!s.note.isEmpty)
        warnings.setText(s.warnings.joined(separator: "\n"))
        warnings.setShown(!s.warnings.isEmpty)
        error.setText(s.error ?? "")
        error.setShown(s.error != nil)
        saveError.setText(s.saveError ?? "")
        saveError.setShown(s.saveError != nil)

        let newShape = s.groups.flatMap { g in [g.title] + g.rows.map { "\($0.key) \($0.kind)" } }
        if newShape != shape {
            rebuild(s.groups)
            shape = newShape
        } else {
            var i = 0
            for (g, view) in zip(s.groups, groups) {
                view.title.setText(g.title)
                for r in g.rows {
                    rows[i].update(r)
                    i += 1
                }
            }
        }
        groups.forEach { $0.setShown(s.error == nil) }
        contentChanged()
    }

    private func rebuild(_ new: [SettingsGroup]) {
        groups.forEach {
            column.removeArrangedSubview($0)
            $0.removeFromSuperview()
        }
        groups = []
        rows = []
        for g in new {
            let view = GroupView(title: g.title)
            for r in g.rows {
                let row = SettingRowView(r)
                row.actions = actions
                view.box.add(row.row)
                rows.append(row)
            }
            column.addFullWidth(view)
            groups.append(view)
        }
        resetCursor()
    }
}
