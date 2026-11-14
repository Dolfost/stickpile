import AppKit

/// What the Me page asks the popup to do.
protocol MeActions: AnyObject {
    func login(username: String, password: String, totp: String)
    func loginTailnet()
    func logout()
    func retryProfile()
    func openServer()
}

/// A circle with the first letter of a name.
final class AvatarView: NSView {
    var name = "" { didSet { if oldValue != name { needsDisplay = true } } }
    override var intrinsicContentSize: NSSize { NSSize(width: 48, height: 48) }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.controlAccentColor.withAlphaComponent(0.85).setFill()
        NSBezierPath(ovalIn: bounds).fill()
        if name.isEmpty, let person = symbol("person.fill", size: 22, color: .white) {
            let s = person.size
            person.draw(in: NSRect(x: bounds.midX - s.width / 2, y: bounds.midY - s.height / 2, width: s.width, height: s.height))
            return
        }
        let letter = name.first.map { String($0).uppercased() } ?? "?"
        let s = NSAttributedString(string: letter, attributes: [
            .font: NSFont.systemFont(ofSize: 21, weight: .semibold), .foregroundColor: NSColor.white,
        ])
        let size = s.size()
        s.draw(at: NSPoint(x: bounds.midX - size.width / 2, y: bounds.midY - size.height / 2))
    }
}

/// The account's numbers, four to a row.
final class StatsView: NSView {
    var stats: [Stat] = [] {
        didSet {
            guard oldValue != stats else { return }
            invalidateIntrinsicContentSize()
            needsDisplay = true
        }
    }
    private let tileHeight: CGFloat = 54
    private let gap: CGFloat = 8

    override var isFlipped: Bool { true }

    override var intrinsicContentSize: NSSize {
        let rows = (stats.count + 3) / 4
        return NSSize(width: NSView.noIntrinsicMetric,
                      height: rows == 0 ? 0 : CGFloat(rows) * tileHeight + CGFloat(rows - 1) * gap)
    }

    override func draw(_ dirtyRect: NSRect) {
        let w = (bounds.width - 3 * gap) / 4
        for (i, stat) in stats.enumerated() {
            let r = NSRect(x: CGFloat(i % 4) * (w + gap), y: CGFloat(i / 4) * (tileHeight + gap), width: w, height: tileHeight)
            let path = NSBezierPath(roundedRect: r.insetBy(dx: 0.5, dy: 0.5), xRadius: 9, yRadius: 9)
            NSColor.labelColor.withAlphaComponent(0.045).setFill()
            path.fill()
            NSColor.separatorColor.setStroke()
            path.stroke()
            let value = NSAttributedString(string: stat.value, attributes: lineAttributes(
                font: .systemFont(ofSize: 17, weight: .semibold), color: .labelColor, align: .center))
            value.draw(with: NSRect(x: r.minX + 4, y: r.minY + 8, width: r.width - 8, height: 22),
                       options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
            let name = NSAttributedString(string: stat.label, attributes: lineAttributes(
                font: .systemFont(ofSize: 11), color: .secondaryLabelColor, align: .center))
            name.draw(with: NSRect(x: r.minX + 4, y: r.minY + 31, width: r.width - 8, height: 15),
                      options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
        }
    }
}

/// The Me tab: who is signed in, the account, the login form.
final class MePage: PageView, NSTextFieldDelegate {
    weak var actions: MeActions?
    private var me: MeView?

    private let loading = NSStackView()
    private let spinner = NSProgressIndicator()
    private let loadingLabel = label("", size: 13, color: .secondaryLabelColor)

    private let header = NSStackView()
    private let avatar = AvatarView()
    private let who = label("", size: 20, weight: .semibold)
    private let detail = label("", size: 13, color: .secondaryLabelColor)

    private let account = GroupView()
    private let serverRow: FormRow
    private let serverValue = label("", size: 13, color: .secondaryLabelColor)
    private var openServer: NSButton!
    private let authRow = FormRow()
    private let authValue = label("", size: 13, color: .secondaryLabelColor)
    private let tailnetRow = FormRow()
    private let tailnetValue = label("", size: 13, color: .secondaryLabelColor)
    private let totpRow = FormRow()
    private let totpValue = label("", size: 13, color: .secondaryLabelColor)
    private let stats = StatsView()
    private let signedIn = NSStackView()
    private var logout: NSButton!
    private var storeToken: NSButton!

    private let form = GroupView()
    private let username = NSTextField()
    private let password = NSSecureTextField()
    private let totp = NSTextField()
    private var usernameRow: FieldRow!
    private var passwordRow: FieldRow!
    private var totpFieldRow: FieldRow!
    private let signedOut = NSStackView()
    private var login: NSButton!
    private var tailnetLogin: NSButton!

    private let message = wrappingLabel("", size: 13)
    private var retry: NSButton!

    init() {
        serverRow = FormRow(trailing: [serverValue])
        super.init(maxWidth: 600)

        spinner.style = .spinning
        spinner.controlSize = .small
        loading.orientation = .horizontal
        loading.spacing = 8
        loading.addArrangedSubview(spinner)
        loading.addArrangedSubview(loadingLabel)
        column.addArrangedSubview(loading)

        let names = NSStackView(views: [who, detail])
        names.orientation = .vertical
        names.alignment = .leading
        names.spacing = 2
        header.orientation = .horizontal
        header.spacing = 12
        header.alignment = .centerY
        header.addArrangedSubview(avatar)
        header.addArrangedSubview(names)
        avatar.widthAnchor.constraint(equalToConstant: 48).isActive = true
        avatar.heightAnchor.constraint(equalToConstant: 48).isActive = true
        column.addFullWidth(header)

        openServer = symbolButton("safari", "", self, #selector(openServerClicked))
        serverRow.trailing.addArrangedSubview(openServer)
        serverValue.isSelectable = true
        authRow.trailing.addArrangedSubview(authValue)
        tailnetRow.trailing.addArrangedSubview(tailnetValue)
        totpRow.trailing.addArrangedSubview(totpValue)
        for v in [serverValue, authValue, tailnetValue, totpValue] {
            v.alignment = .right
            v.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        }
        [serverRow, authRow, tailnetRow, totpRow].forEach(account.box.add)
        account.title.isHidden = true
        column.addFullWidth(account)
        column.addFullWidth(stats)

        logout = pushButton("", self, #selector(logoutClicked))
        logout.hasDestructiveAction = true
        storeToken = pushButton("", self, #selector(tailnetClicked))
        signedIn.orientation = .horizontal
        signedIn.spacing = 8
        signedIn.detachesHiddenViews = true
        signedIn.addArrangedSubview(logout)
        signedIn.addArrangedSubview(storeToken)
        column.addArrangedSubview(signedIn)

        for f in [username, password, totp] {
            f.delegate = self
            f.cell?.isScrollable = true
            f.cell?.wraps = false
        }
        username.placeholderString = ""
        totp.placeholderString = "123456"
        usernameRow = FieldRow(field: username)
        passwordRow = FieldRow(field: password)
        totpFieldRow = FieldRow(field: totp)
        [usernameRow!, passwordRow!, totpFieldRow!].forEach(form.box.add)
        column.addFullWidth(form)

        login = pushButton("", self, #selector(loginClicked))
        login.bezelColor = .controlAccentColor
        tailnetLogin = pushButton("", self, #selector(tailnetClicked))
        signedOut.orientation = .horizontal
        signedOut.spacing = 8
        signedOut.detachesHiddenViews = true
        signedOut.addArrangedSubview(login)
        signedOut.addArrangedSubview(tailnetLogin)
        column.addArrangedSubview(signedOut)

        message.isSelectable = true
        column.addFullWidth(message)
        retry = pushButton("", self, #selector(retryClicked))
        column.addArrangedSubview(retry)
        column.setCustomSpacing(8, after: header)
        relabel()
    }

    /// The page's own texts, in the current language.
    func relabel() {
        loadingLabel.stringValue = tr("Loading...")
        openServer.toolTip = tr("Open the web UI")
        serverRow.title.stringValue = tr("Server")
        authRow.title.stringValue = tr("Signed in")
        tailnetRow.title.stringValue = tr("Tailnet account")
        totpRow.title.stringValue = tr("Two-factor login")
        logout.title = tr("Log out")
        storeToken.title = tr("Store a token")
        form.title.stringValue = tr("Log in")
        usernameRow.title.stringValue = tr("Username")
        passwordRow.title.stringValue = tr("Password")
        totpFieldRow.title.stringValue = tr("Two-factor code")
        login.title = tr("Log in")
        tailnetLogin.title = tr("Log in with tailnet")
        retry.title = tr("Try again")
    }

    required init?(coder: NSCoder) { fatalError() }

    override func stops() -> [PageStop] {
        var out: [PageStop] = [PageStop(highlight: serverRow, control: .button(openServer))]
        for b in [logout!, storeToken!] {
            out.append(PageStop(highlight: b, control: .button(b)))
        }
        out.append(PageStop(highlight: usernameRow, control: .text(username)))
        out.append(PageStop(highlight: passwordRow, control: .text(password)))
        out.append(PageStop(highlight: totpFieldRow, control: .text(totp)))
        for b in [login!, tailnetLogin!, retry!] {
            out.append(PageStop(highlight: b, control: .button(b)))
        }
        return out
    }

    func update(_ m: MeView) {
        if m.clearSecrets {
            password.stringValue = ""
            totp.stringValue = ""
        }
        guard m != me else { return }
        let before = me
        me = m
        loading.setShown(m.loading)
        if m.loading { spinner.startAnimation(nil) } else { spinner.stopAnimation(nil) }
        header.setShown(m.who != nil)
        who.setText(m.who ?? "")
        avatar.name = m.account != nil ? m.who ?? "" : ""
        detail.setText(m.detail)
        detail.setShown(!m.detail.isEmpty)

        account.setShown(m.account != nil)
        if let a = m.account {
            serverValue.setText(a.serverUrl)
            serverValue.toolTip = a.serverUrl
            openServer.setShown(a.openServer)
            authValue.setText(a.auth)
            tailnetRow.setShown(a.tailnet != nil)
            tailnetValue.setText(a.tailnet ?? "")
            totpValue.setText(a.twoFactor)
            stats.stats = a.stats
        }
        stats.setShown(!(m.account?.stats.isEmpty ?? true))
        logout.setShown(m.account?.logout ?? false)
        storeToken.setShown(m.account?.storeToken ?? false)
        signedIn.setShown(m.account != nil && (m.account!.logout || m.account!.storeToken))

        form.setShown(m.form != nil)
        if let f = m.form {
            form.detail.setText(f.description)
            form.detail.setShown(!f.description.isEmpty)
            totpFieldRow.setShown(f.totp)
        }
        login.setShown(m.form != nil)
        // With the login form only, as on Linux (signed in, "Store a token" does it).
        tailnetLogin.setShown(m.tailnetLogin && m.form != nil)
        signedOut.setShown(m.form != nil)

        message.setShown(m.message != nil)
        message.setText(m.message?.text ?? "")
        message.textColor = m.message?.error == true ? .systemRed : .labelColor
        retry.setShown(m.retry)

        for b in [openServer!, logout!, storeToken!, login!, tailnetLogin!, retry!] {
            b.isEnabled = !m.busy
        }
        // Signed in or out now: the cursor starts over.
        if (before?.account == nil) != (m.account == nil) || (before?.form == nil) != (m.form == nil) {
            resetCursor()
        }
        contentChanged()
    }

    /// A two-factor field appeared: put the focus into it.
    var totpShown: Bool { me?.form?.totp ?? false }

    @objc private func openServerClicked() { actions?.openServer() }
    @objc private func logoutClicked() { actions?.logout() }
    @objc private func tailnetClicked() { actions?.loginTailnet() }
    @objc private func retryClicked() { actions?.retryProfile() }

    @objc private func loginClicked() {
        actions?.login(username: username.stringValue.trimmingCharacters(in: .whitespaces),
                       password: password.stringValue, totp: totp.stringValue.trimmingCharacters(in: .whitespaces))
    }

    // Return in the username field moves on to the password; in the others it logs in.
    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        guard selector == #selector(NSResponder.insertNewline(_:)) else { return false }
        if control === username {
            window?.makeFirstResponder(password)
        } else if !(me?.busy ?? false) {
            loginClicked()
        }
        return true
    }
}
