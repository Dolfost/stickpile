import AppKit
import ApplicationServices

/// Cmd+V after a pick (`paste.auto`, docs/macos.md). Rust decides that a
/// paste is due and into which app (`PasteRequest`); this waits until that
/// app is frontmost with the panel gone, then posts the key events. Posting
/// key events to other apps needs the Accessibility permission; without it
/// the popup only copies.
enum Paste {
    /// The explanation was shown once; it is not shown again.
    private static let askedKey = "pasteAccessibilityAsked"
    /// Seconds between looks at the frontmost app while waiting.
    private static let poll = 0.02

    static func after(_ request: PasteRequest, panel: NSWindow) {
        guard trusted() else { return }
        let deadline = Date().addingTimeInterval(Double(request.waitMs) / 1000)
        func ready() -> Bool {
            !panel.isVisible && NSWorkspace.shared.frontmostApplication?.bundleIdentifier == request.bundleId
        }
        func attempt() {
            if ready() {
                DispatchQueue.main.asyncAfter(deadline: .now() + Double(request.delayMs) / 1000) {
                    if ready() {
                        postCmdV()
                    } else {
                        debug("paste: \(request.bundleId) is no longer frontmost")
                    }
                }
            } else if Date() < deadline {
                DispatchQueue.main.asyncAfter(deadline: .now() + poll) { attempt() }
            } else {
                debug("paste: \(request.bundleId) did not come back in time")
            }
        }
        attempt()
    }

    /// The Accessibility permission. Without it, the first time only, say
    /// why it is wanted and let macOS offer System Settings.
    static func trusted() -> Bool {
        if AXIsProcessTrusted() { return true }
        let defaults = UserDefaults.standard
        guard !defaults.bool(forKey: askedKey) else { return false }
        defaults.set(true, forKey: askedKey)
        NSApp.activate(ignoringOtherApps: true)
        let alert = NSAlert()
        alert.messageText = tr("Paste after a pick")
        // One line, so the catalog test finds the text.
        alert.informativeText = tr("Stickpile can press Cmd+V for you in the app it was opened over. macOS allows that only with the Accessibility permission: turn on Stickpile in System Settings, Privacy & Security, Accessibility. Until then it only copies; Paste after a pick in Settings turns this off.")
        alert.addButton(withTitle: tr("Open System Settings"))
        alert.addButton(withTitle: tr("Not now"))
        if alert.runModal() == .alertFirstButtonReturn {
            // The value of kAXTrustedCheckOptionPrompt: macOS lists the app
            // under Accessibility and offers to open System Settings.
            _ = AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary)
        }
        return false
    }

    /// Cmd+V on the V key of a US keyboard (kVK_ANSI_V).
    static func postCmdV() {
        let source = CGEventSource(stateID: .combinedSessionState)
        let v: CGKeyCode = 9
        for down in [true, false] {
            guard let event = CGEvent(keyboardEventSource: source, virtualKey: v, keyDown: down) else { return }
            event.flags = .maskCommand
            event.post(tap: .cghidEventTap)
        }
    }
}
