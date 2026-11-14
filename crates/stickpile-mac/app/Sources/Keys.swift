import AppKit
import Carbon.HIToolbox

enum Keys {
    /// A key press as the popup reads it.
    static func input(_ event: NSEvent) -> KeyInput {
        let flags = event.modifierFlags
        return KeyInput(
            keyCode: event.keyCode,
            characters: event.charactersIgnoringModifiers ?? "",
            latin: latin(keyCode: event.keyCode),
            command: flags.contains(.command),
            control: flags.contains(.control),
            option: flags.contains(.option),
            shift: flags.contains(.shift)
        )
    }

    /// The character of this physical key in the current ASCII-capable
    /// layout, without modifiers: "t" for the key that types a Cyrillic ie.
    static func latin(keyCode: UInt16) -> String? {
        guard let source = TISCopyCurrentASCIICapableKeyboardLayoutInputSource()?.takeRetainedValue(),
              let raw = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData)
        else { return nil }
        let data = Unmanaged<CFData>.fromOpaque(raw).takeUnretainedValue()
        guard let bytes = CFDataGetBytePtr(data) else { return nil }
        var dead: UInt32 = 0
        var length = 0
        var chars = [UniChar](repeating: 0, count: 4)
        let status = bytes.withMemoryRebound(to: UCKeyboardLayout.self, capacity: 1) { layout in
            UCKeyTranslate(layout, keyCode, UInt16(kUCKeyActionDown), 0, UInt32(LMGetKbdType()),
                           OptionBits(kUCKeyTranslateNoDeadKeysBit), &dead, chars.count, &length, &chars)
        }
        guard status == noErr, length > 0 else { return nil }
        let s = String(utf16CodeUnits: chars, count: length)
        // Control characters (Return, arrows, ...) are told by the key code.
        guard let c = s.unicodeScalars.first, c.value >= 0x20, c.value != 0x7F, c.value < 0xF700 else { return nil }
        return s
    }
}

/// The global shortcut choices of the menu bar item.
struct Shortcut {
    let id: String
    let title: String
    let keyCode: UInt32
    let carbonModifiers: Int
    let keyEquivalent: String
    let modifierMask: NSEvent.ModifierFlags

    static let all: [Shortcut] = [
        Shortcut(id: "option-space", title: "Option+Space", keyCode: UInt32(kVK_Space),
                 carbonModifiers: optionKey, keyEquivalent: " ", modifierMask: [.option]),
        Shortcut(id: "command-semicolon", title: "Command+;", keyCode: UInt32(kVK_ANSI_Semicolon),
                 carbonModifiers: cmdKey, keyEquivalent: ";", modifierMask: [.command]),
        Shortcut(id: "control-option-space", title: "Control+Option+Space", keyCode: UInt32(kVK_Space),
                 carbonModifiers: controlKey | optionKey, keyEquivalent: " ", modifierMask: [.control, .option]),
        Shortcut(id: "shift-command-space", title: "Shift+Command+Space", keyCode: UInt32(kVK_Space),
                 carbonModifiers: shiftKey | cmdKey, keyEquivalent: " ", modifierMask: [.shift, .command]),
    ]

    static let defaultsKey = "shortcut"

    static var current: Shortcut {
        let id = UserDefaults.standard.string(forKey: defaultsKey)
        return all.first { $0.id == id } ?? all[0]
    }
}

/// A system-wide hot key (Carbon; needs no Accessibility permission).
final class HotKey {
    private var ref: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private let action: () -> Void

    init(action: @escaping () -> Void) {
        self.action = action
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let me = Unmanaged.passUnretained(self).toOpaque()
        InstallEventHandler(GetApplicationEventTarget(), { _, _, user in
            guard let user else { return OSStatus(eventNotHandledErr) }
            Unmanaged<HotKey>.fromOpaque(user).takeUnretainedValue().action()
            return noErr
        }, 1, &spec, me, &handler)
    }

    deinit {
        unregister()
        if let handler { RemoveEventHandler(handler) }
    }

    /// `false`: another app holds this shortcut.
    @discardableResult
    func register(_ s: Shortcut) -> Bool {
        unregister()
        let id = EventHotKeyID(signature: OSType(0x5354_504C), id: 1)
        let status = RegisterEventHotKey(s.keyCode, UInt32(s.carbonModifiers), id, GetApplicationEventTarget(), 0, &ref)
        if status != noErr {
            ref = nil
            NSLog("stickpile: cannot register the shortcut %@ (%d)", s.title, status)
        }
        return status == noErr
    }

    func unregister() {
        if let ref {
            UnregisterEventHotKey(ref)
            self.ref = nil
        }
    }
}
