import AppKit
import Carbon.HIToolbox
import SwiftUI

typealias ShortcutCommit = (_ keyCode: Int, _ carbonModifiers: Int, _ symbol: String) -> Void

/// Key capture for the shortcut recorder.
///
/// All mutable state is confined to the main thread by construction (event
/// monitors deliver there, SwiftUI drives it there); `@unchecked Sendable`
/// documents that invariant instead of threading it through actors.
final class ShortcutCapture: ObservableObject, @unchecked Sendable {
    static let shared = ShortcutCapture()

    @Published private(set) var isRecording = false
    @Published private(set) var hint: String?
    var onCommit: ShortcutCommit?

    private var monitor: Any?

    func begin() {
        dispatchPrecondition(condition: .onQueue(.main))
        endMonitoring()
        hint = nil
        isRecording = true
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, self.isRecording else { return event }
            return self.consume(event)
        }
    }

    func cancel() {
        dispatchPrecondition(condition: .onQueue(.main))
        isRecording = false
        endMonitoring()
    }

    /// Returns nil to swallow handled keystrokes while recording.
    private func consume(_ event: NSEvent) -> NSEvent? {
        // Escape cancels without changing the current shortcut.
        if event.keyCode == UInt16(kVK_Escape) {
            cancel()
            return nil
        }

        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let meaningful = flags.intersection([.command, .option, .control, .shift])
        // Bare function keys are acceptable shortcuts on their own; plain
        // letters/digits would hijack typing everywhere, so require a modifier.
        guard !meaningful.isEmpty || flags.contains(.function) else {
            hint = "Add a modifier — ⌘ ⌥ ⌃ or ⇧"
            return nil
        }

        let keyCode = Int(event.keyCode)
        let carbonMods = HotKeyFormat.carbonModifiers(from: flags)
        let symbol = HotKeyFormat.symbol(
            carbonModifiers: carbonMods,
            keyCode: keyCode,
            charactersIgnoringModifiers: event.charactersIgnoringModifiers
        )
        cancel()
        onCommit?(keyCode, carbonMods, symbol)
        return nil
    }

    private func endMonitoring() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
    }
}

/// Click-to-record shortcut field. Persists through `PanelShortcut` and
/// notifies via `onCommit` so the hotkey can be re-registered immediately.
struct ShortcutField: View {
    @ObservedObject private var capture = ShortcutCapture.shared
    let onCommit: () -> Void

    var body: some View {
        Button(action: toggleRecording) {
            HStack(spacing: 5) {
                Text(capture.isRecording ? "Press keys…" : PanelShortcut.symbol)
                    .font(.callout.monospaced())
                Image(systemName: capture.isRecording ? "record.circle" : "keyboard")
                    .font(.caption)
            }
            .padding(.horizontal, 9)
            .padding(.vertical, 4)
            .background(
                capture.isRecording ? AnyShapeStyle(Color.accentColor.opacity(0.22)) : AnyShapeStyle(.quaternary),
                in: RoundedRectangle(cornerRadius: 6)
            )
        }
        .buttonStyle(.plain)
        .disabled(!PanelShortcut.isEnabled)
        .help("Click, then press the new global shortcut. Esc cancels.")
        .onChange(of: capture.isRecording, initial: true) { _, recording in
            if !recording { capture.onCommit = nil }
        }
    }

    private func toggleRecording() {
        if capture.isRecording {
            capture.cancel()
            return
        }
        capture.onCommit = { keyCode, modifiers, symbol in
            PanelShortcut.save(keyCode: keyCode, modifiers: modifiers, symbol: symbol)
            onCommit()
        }
        capture.begin()
    }
}
