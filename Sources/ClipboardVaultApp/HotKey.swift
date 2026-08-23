import AppKit
import Carbon.HIToolbox

/// A user-selected panel-opening shortcut, persisted in UserDefaults as a
/// virtual key code plus Carbon modifiers (what RegisterEventHotKey wants),
/// alongside a pre-rendered symbol for display (e.g. "⇧⌘V").
struct PanelShortcut {
    static let enabledKey = "hotkey.enabled"
    static let keyCodeKey = "hotkey.keyCode"
    static let modifiersKey = "hotkey.modifiers"
    static let symbolKey = "hotkey.symbol"

    /// ⇧⌘V — familiar from other clipboard managers, re-selectable in settings.
    static func registerDefaults() {
        UserDefaults.standard.register(defaults: [
            enabledKey: true,
            keyCodeKey: kVK_ANSI_V,
            modifiersKey: cmdKey | shiftKey,
            symbolKey: "⇧⌘V",
        ])
    }

    static var isEnabled: Bool { UserDefaults.standard.bool(forKey: enabledKey) }
    static var keyCode: Int { UserDefaults.standard.integer(forKey: keyCodeKey) }
    static var carbonModifiers: Int { UserDefaults.standard.integer(forKey: modifiersKey) }
    static var symbol: String { UserDefaults.standard.string(forKey: symbolKey) ?? "?" }

    static func save(enabled: Bool? = nil, keyCode: Int, modifiers: Int, symbol: String) {
        let defaults = UserDefaults.standard
        if let enabled { defaults.set(enabled, forKey: enabledKey) }
        defaults.set(keyCode, forKey: keyCodeKey)
        defaults.set(modifiers, forKey: modifiersKey)
        defaults.set(symbol, forKey: symbolKey)
    }
}

/// Formatting helpers between NSEvents, Carbon hotkeys and display symbols.
enum HotKeyFormat {
    static func carbonModifiers(from flags: NSEvent.ModifierFlags) -> Int {
        var carbon = 0
        if flags.contains(.command) { carbon |= cmdKey }
        if flags.contains(.option) { carbon |= optionKey }
        if flags.contains(.control) { carbon |= controlKey }
        if flags.contains(.shift) { carbon |= shiftKey }
        return carbon
    }

    /// Symbol string in conventional order ⌃ ⌥ ⇧ ⌘ followed by the key glyph.
    static func symbol(carbonModifiers: Int, keyCode: Int, charactersIgnoringModifiers: String?) -> String {
        modifierPrefix(carbonModifiers: carbonModifiers)
            + keyGlyph(keyCode: keyCode, characters: charactersIgnoringModifiers)
    }

    private static func modifierPrefix(carbonModifiers: Int) -> String {
        var prefix = ""
        if carbonModifiers & controlKey != 0 { prefix += "⌃" }
        if carbonModifiers & optionKey != 0 { prefix += "⌥" }
        if carbonModifiers & shiftKey != 0 { prefix += "⇧" }
        if carbonModifiers & cmdKey != 0 { prefix += "⌘" }
        return prefix
    }

    private static func keyGlyph(keyCode: Int, characters: String?) -> String {
        switch keyCode {
        case kVK_Return: "↩"
        case kVK_Tab: "⇥"
        case kVK_Space: "␣"
        case kVK_Delete: "⌫"
        case kVK_ForwardDelete: "⌦"
        case kVK_Escape: "⎋"
        case kVK_LeftArrow: "←"
        case kVK_RightArrow: "→"
        case kVK_DownArrow: "↓"
        case kVK_UpArrow: "↑"
        case kVK_Home: "↖"
        case kVK_End: "↘"
        case kVK_PageUp: "⇞"
        case kVK_PageDown: "⇟"
        case kVK_F1: "F1"
        case kVK_F2: "F2"
        case kVK_F3: "F3"
        case kVK_F4: "F4"
        case kVK_F5: "F5"
        case kVK_F6: "F6"
        case kVK_F7: "F7"
        case kVK_F8: "F8"
        case kVK_F9: "F9"
        case kVK_F10: "F10"
        case kVK_F11: "F11"
        case kVK_F12: "F12"
        default:
            if let first = characters?.folding(options: .diacriticInsensitive, locale: nil)
                .uppercased().first, first.isLetter || first.isNumber {
                String(first)
            } else {
                "Key \(keyCode)"
            }
        }
    }
}

/// Registers a system-wide hotkey via Carbon's Hit Toolbox. Works globally
/// (even when another app has focus) and needs no Accessibility permission,
/// unlike CGEvent-tap approaches.
enum GlobalHotKeyCenter {
    // Hotkey events arrive on the main run loop; all mutable state below is
    // therefore confined to the main thread in practice.
    nonisolated(unsafe) private static var hotKeyRef: EventHotKeyRef?
    nonisolated(unsafe) private static var handlerRef: EventHandlerRef?
    nonisolated(unsafe) static var onKeyDown: (@MainActor () -> Void)?
    nonisolated(unsafe) private(set) static var registrationSucceeded = false

    private static let hotKeySignature = OSType(0x636C7674) // "clvt"

    private static let eventCallback: EventHandlerUPP = { _, event, _ in
        guard let event, GetEventKind(event) == EventKind(kEventHotKeyPressed) else { return noErr }
        MainActor.assumeIsolated { GlobalHotKeyCenter.onKeyDown?() }
        return noErr
    }

    /// Installs the shared event handler exactly once. Call early at launch.
    static func install() {
        guard handlerRef == nil else { return }
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), eventCallback, 1, &spec, nil, &handlerRef)
    }

    /// Replaces any existing registration. `registrationSucceeded` reports
    /// whether the combo was actually claimed (it fails when another app or
    /// system service already owns it).
    static func register(keyCode: UInt32, modifiers: UInt32) {
        unregister()
        var ref: EventHotKeyRef?
        let hotKeyID = EventHotKeyID(signature: hotKeySignature, id: 1)
        let status = RegisterEventHotKey(keyCode, modifiers, hotKeyID, GetApplicationEventTarget(), 0, &ref)
        registrationSucceeded = status == noErr
        hotKeyRef = ref
    }

    static func unregister() {
        if let hotKeyRef { UnregisterEventHotKey(hotKeyRef) }
        hotKeyRef = nil
        registrationSucceeded = false
    }
}
