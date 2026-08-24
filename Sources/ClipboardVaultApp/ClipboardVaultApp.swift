import AppKit
import Observation
import SwiftUI
import VaultCore

/// A history entry filtered by the current query, with the character offsets
/// the fuzzy matcher hit (empty when not searching) for highlighting.
private struct FilteredEntry: Identifiable {
    let entry: VaultEntry
    let positions: [Int]
    var id: UUID { entry.id }
}

extension Notification.Name {
    static let vaultPanelShown = Notification.Name("VaultPanelShown")
}

@main
struct ClipboardVaultApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        // The real UI lives in a status-bar popover owned by the delegate;
        // this scene only satisfies the App protocol without opening windows.
        Settings { EmptyView() }
    }
}

/// Owns the status-bar item and its popover so the panel can be toggled
/// programmatically — which is what makes a global hotkey possible
/// (SwiftUI's MenuBarExtra cannot be opened from code).
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    nonisolated(unsafe) static private(set) weak var shared: AppDelegate?

    private let model = VaultViewModel()
    private let popover = NSPopover()
    private var statusItem: NSStatusItem?
    private var lastPanelClose = Date.distantPast

    func applicationDidFinishLaunching(_ notification: Notification) {
        Self.shared = self
        PanelShortcut.registerDefaults()

        popover.behavior = .transient
        popover.contentSize = NSSize(width: 380, height: 480)
        popover.contentViewController = NSHostingController(rootView: VaultMenu(model: model))
        popover.delegate = self

        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = NSImage(systemSymbolName: "lock.doc", accessibilityDescription: "Clipboard Vault")
        item.button?.target = self
        item.button?.action = #selector(statusItemClicked)
        item.button?.toolTip = "Clipboard Vault"
        statusItem = item

        GlobalHotKeyCenter.install()
        applyHotKey()
    }

    func togglePanel() {
        if popover.isShown {
            popover.performClose(nil)
            return
        }
        // Transient behavior closes the panel before the status-item action
        // runs when the user clicks the icon while it is already open; skip
        // reopening in that case or the panel would never close by clicking.
        guard Date().timeIntervalSince(lastPanelClose) > 0.2 else { return }
        guard let button = statusItem?.button else { return }
        NSApp.activate(ignoringOtherApps: true)
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        NotificationCenter.default.post(name: .vaultPanelShown, object: nil)
    }

    /// (Re-)registers the persisted global hotkey. Called at launch and
    /// whenever the user records a new shortcut or toggles it off.
    func applyHotKey() {
        GlobalHotKeyCenter.unregister()
        guard PanelShortcut.isEnabled else { return }
        GlobalHotKeyCenter.register(
            keyCode: UInt32(PanelShortcut.keyCode),
            modifiers: UInt32(PanelShortcut.carbonModifiers)
        )
        GlobalHotKeyCenter.onKeyDown = { [weak self] in self?.togglePanel() }
    }

    @objc private func statusItemClicked() {
        togglePanel()
    }
}

extension AppDelegate: NSPopoverDelegate {
    func popoverDidClose(_ notification: Notification) {
        lastPanelClose = Date()
    }
}

@MainActor
@Observable
final class VaultViewModel {
    private(set) var entries: [VaultEntry] = []
    private(set) var lastError: String?
    private(set) var now = Date.now
    private(set) var recentlyCopiedID: UUID?
    private(set) var sensitiveClearAt: Date?

    private var changeCount = NSPasteboard.general.changeCount
    private var timer: Timer?
    private var flashTask: Task<Void, Never>?
    private var vaultRetryAfter = Date.distantPast

    init() {
        refresh()
        captureInitialPasteboard()
        let timer = Timer(timeInterval: 0.75, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.tick() }
        }
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }

    /// Seconds until the sensitive clipboard copy auto-clears, if one is armed.
    var sensitiveRemainingSeconds: Int? {
        guard let sensitiveClearAt else { return nil }
        let remaining = Int(sensitiveClearAt.timeIntervalSince(now).rounded(.up))
        return remaining > 0 ? remaining : nil
    }

    func refresh() {
        Task {
            do {
                entries = try await VaultStore.shared.entries()
                lastError = nil
                vaultRetryAfter = .distantPast
            } catch {
                lastError = error.localizedDescription
                // Back off so an unauthorized/rejected keychain read does not
                // re-trigger the system prompt on every poll tick.
                vaultRetryAfter = Date().addingTimeInterval(15)
            }
        }
    }

    func copy(_ entry: VaultEntry) {
        placeOnPasteboard(entry.text)
        flash(entry.id)
    }

    /// Copies without persisting and arms the auto-clear (FRA-496).
    func copyOnce(_ entry: VaultEntry) {
        Task {
            let clearAt = await SensitiveClipboard.shared.copy(entry.text)
            sensitiveClearAt = clearAt
            flash(entry.id)
        }
    }

    /// Wipes the sensitive copy immediately.
    func clearSensitiveNow() {
        Task {
            await SensitiveClipboard.shared.clearNow()
            sensitiveClearAt = nil
        }
    }

    func delete(_ entry: VaultEntry) {
        Task {
            do {
                try await VaultStore.shared.delete(id: entry.id)
                lastError = nil
            } catch {
                lastError = error.localizedDescription
            }
            refresh()
        }
    }

    func clear() {
        Task {
            do {
                try await VaultStore.shared.clear()
                lastError = nil
            } catch {
                lastError = error.localizedDescription
            }
            refresh()
        }
    }

    // MARK: - Private

    private func tick() {
        now = .now
        if let at = sensitiveClearAt, now >= at {
            sensitiveClearAt = nil
        }
        adoptOrphanedSensitiveCopy()
        captureIfChanged()
        // Self-heal: after a transient keychain failure (e.g. a pending
        // permission prompt), retry periodically so the error banner clears
        // on its own instead of sticking forever.
        if lastError != nil, Date() >= vaultRetryAfter {
            refresh()
        }
    }

    /// If another process (e.g. the CLI with --no-wait) left a marked
    /// sensitive copy on the clipboard, take over its scheduled wipe.
    private func adoptOrphanedSensitiveCopy() {
        guard sensitiveClearAt == nil,
              VaultPasteboard.holdsSensitiveContent,
              let text = NSPasteboard.general.string(forType: .string) else { return }
        Task {
            sensitiveClearAt = await SensitiveClipboard.shared.adopt(text)
        }
    }

    private func captureInitialPasteboard() {
        guard !VaultPasteboard.holdsSensitiveContent,
              let text = NSPasteboard.general.string(forType: .string) else { return }
        persist(text)
    }

    private func captureIfChanged() {
        let pasteboard = NSPasteboard.general
        guard pasteboard.changeCount != changeCount else { return }
        changeCount = pasteboard.changeCount
        // Never re-capture transient sensitive copies into the vault.
        guard !VaultPasteboard.holdsSensitiveContent,
              let text = pasteboard.string(forType: .string) else { return }
        persist(text)
    }

    private func placeOnPasteboard(_ text: String) {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
    }

    private func flash(_ id: UUID) {
        recentlyCopiedID = id
        flashTask?.cancel()
        flashTask = Task {
            try? await Task.sleep(for: .seconds(1.2))
            guard !Task.isCancelled else { return }
            if recentlyCopiedID == id { recentlyCopiedID = nil }
        }
    }

    private func persist(_ text: String) {
        guard Date() >= vaultRetryAfter else { return }
        Task {
            do {
                try await VaultStore.shared.add(text)
                entries = try await VaultStore.shared.entries()
                lastError = nil
                vaultRetryAfter = .distantPast
            } catch {
                lastError = error.localizedDescription
                vaultRetryAfter = Date().addingTimeInterval(15)
            }
        }
    }
}

// MARK: - Menu

struct VaultMenu: View {
    let model: VaultViewModel
    @State private var query = ""
    @State private var confirmingClear = false
    @State private var expandedHistoryID: UUID?
    @FocusState private var searchFocused: Bool
    @AppStorage(PanelShortcut.enabledKey) private var hotkeyEnabled = true

    private var filteredEntries: [FilteredEntry] {
        guard !query.isEmpty else {
            return model.entries.map { FilteredEntry(entry: $0, positions: []) }
        }
        return FuzzySearch().ranked(model.entries, query: query, text: \.text)
            .map { FilteredEntry(entry: $0.item, positions: $0.result.positions) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            searchField

            if let error = model.lastError {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.red)
            }

            if let remaining = model.sensitiveRemainingSeconds {
                SensitiveBanner(remainingSeconds: remaining) {
                    model.clearSensitiveNow()
                }
            }

            content
            shortcutSettings
        }
        .padding()
        .frame(width: 380, height: 480)
        .onAppear {
            // Type-to-search immediately after opening the panel.
            searchFocused = true
        }
        .onReceive(NotificationCenter.default.publisher(for: .vaultPanelShown)) { _ in
            prepareForSearch()
        }
    }

    private var header: some View {
        HStack {
            Text("Clipboard Vault").font(.headline)
            Text("\(model.entries.count)")
                .font(.caption2).fontWeight(.semibold)
                .padding(.horizontal, 6).padding(.vertical, 2)
                .background(.quaternary, in: Capsule())
            Spacer()
            Button(confirmingClear ? "Really clear all?" : "Clear All", role: .destructive) {
                if confirmingClear {
                    model.clear()
                    confirmingClear = false
                } else {
                    confirmingClear = true
                    resetConfirmationLater()
                }
            }
            .buttonStyle(.borderless)
            .font(.callout)
            .disabled(model.entries.isEmpty)
            .accessibilityLabel("Clear clipboard vault")
        }
    }

    private var searchField: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .foregroundStyle(.secondary)
            TextField("Search history", text: $query)
                .textFieldStyle(.plain)
                .focused($searchFocused)
            if !query.isEmpty {
                Button {
                    query = ""
                    searchFocused = true
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(.quaternary, in: RoundedRectangle(cornerRadius: 7))
    }

    private var shortcutSettings: some View {
        DisclosureGroup {
            Toggle("Global keyboard shortcut", isOn: $hotkeyEnabled)
                .font(.callout)
                .onChange(of: hotkeyEnabled) { _, _ in
                    AppDelegate.shared?.applyHotKey()
                }
            HStack {
                Text("Open panel with")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer()
                ShortcutField {
                    AppDelegate.shared?.applyHotKey()
                }
            }
            if hotkeyEnabled, PanelShortcut.isEnabled, !GlobalHotKeyCenter.registrationSucceeded {
                Label("That combo is already taken by another app — record a different one.", systemImage: "exclamationmark.triangle")
                    .font(.caption2)
                    .foregroundStyle(.orange)
            }
        } label: {
            Label("Panel Shortcut", systemImage: "keyboard")
                .font(.callout.weight(.medium))
        }
        .disclosureGroupStyle(.automatic)
    }

    @ViewBuilder
    private var content: some View {
        if filteredEntries.isEmpty {
            VStack(spacing: 8) {
                Image(systemName: query.isEmpty ? "doc.on.doc.fill" : "magnifyingglass")
                    .font(.title2)
                    .foregroundStyle(.secondary)
                Text(query.isEmpty ? "Copy text to begin storing encrypted history." : "No matches for “\(query)”.")
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 4) {
                    ForEach(filteredEntries) { filtered in
                        EntryRow(
                            entry: filtered.entry,
                            displayText: Self.highlightedText(
                                of: filtered.entry.text,
                                positions: filtered.positions
                            ),
                            copied: model.recentlyCopiedID == filtered.entry.id,
                            expanded: expandedHistoryID == filtered.entry.id,
                            onCopy: { model.copy(filtered.entry) },
                            onCopyOnce: { model.copyOnce(filtered.entry) },
                            onDelete: { model.delete(filtered.entry) },
                            onToggleHistory: {
                                expandedHistoryID = expandedHistoryID == filtered.entry.id ? nil : filtered.entry.id
                            }
                        )
                    }
                }
            }
        }
    }

    /// Builds the row text with matched characters emphasized. Chunking
    /// consecutive offsets keeps the number of attributed runs small.
    private static func highlightedText(of text: String, positions: [Int]) -> AttributedString {
        guard !positions.isEmpty else { return AttributedString(text) }
        let chars = Array(text)
        var result = AttributedString()
        var index = 0
        let matches = Set(positions)
        while index < chars.count {
            var end = index
            while end < chars.count, matches.contains(end) == matches.contains(index) {
                end += 1
            }
            let chunk = String(chars[index..<end])
            if matches.contains(index) {
                var attributed = AttributedString(chunk)
                attributed.font = .body.bold()
                attributed.foregroundColor = .accentColor
                result.append(attributed)
            } else {
                result.append(AttributedString(chunk))
            }
            index = end
        }
        return result
    }

    private func resetConfirmationLater() {
        Task {
            try? await Task.sleep(for: .seconds(3))
            confirmingClear = false
        }
    }

    private func prepareForSearch() {
        query = ""
        confirmingClear = false
        expandedHistoryID = nil
        searchFocused = false
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(60))
            searchFocused = true
        }
    }
}

// MARK: - Rows & banners

private struct EntryRow: View {
    let entry: VaultEntry
    let displayText: AttributedString
    let copied: Bool
    let expanded: Bool
    let onCopy: () -> Void
    let onCopyOnce: () -> Void
    let onDelete: () -> Void
    let onToggleHistory: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Button(action: handleTap) {
                HStack(alignment: .top, spacing: 8) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(displayText)
                            .lineLimit(2)
                            .multilineTextAlignment(.leading)
                        HStack(spacing: 6) {
                            Text(entry.createdAt, format: .relative(presentation: .named))
                            if entry.copyCount > 1 {
                                Text("×\(entry.copyCount)")
                                    .font(.caption2.weight(.bold))
                                    .padding(.horizontal, 5)
                                    .padding(.vertical, 1)
                                    .background(Color.accentColor.opacity(0.18), in: Capsule())
                                    .help("Copied \(entry.copyCount) times — press and hold to see every capture")
                            }
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 0)
                    Image(systemName: copied ? "checkmark.circle.fill" : "doc.on.doc")
                        .foregroundStyle(copied ? AnyShapeStyle(.green) : AnyShapeStyle(.secondary))
                        .accessibilityHidden(true)
                }
                .padding(.vertical, 4)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            // Press-and-hold reveals every time this text was captured;
            // a quick click still copies.
            .onLongPressGesture(minimumDuration: 0.35, maximumDistance: 12) {
                onToggleHistory()
            }

            if expanded {
                historyPanel
                    .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
        .contextMenu {
            Button("Copy") { onCopy() }
            Button("Copy Once — auto-clears in 45s") { onCopyOnce() }
                .keyboardShortcut("c", modifiers: [.command, .option])
            Button(expanded ? "Hide Copy Times" : "Show Copy Times") { onToggleHistory() }
            Divider()
            Button("Delete", role: .destructive) { onDelete() }
        }
        .accessibilityLabel("Stored clipboard entry from \(entry.createdAt.formatted(.relative(presentation: .named))), copied \(entry.copyCount) \(entry.copyCount == 1 ? "time" : "times"). Press to copy.")
        .accessibilityHint("Option-click or use the context menu for a sensitive copy that clears automatically. Use the context menu to show all copy times.")
    }

    /// Every capture timestamp for this text, newest first. Shown on
    /// press-and-hold (or via the context menu).
    private var historyPanel: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Label(
                    "Copied \(entry.copyCount) \(entry.copyCount == 1 ? "time" : "times")",
                    systemImage: "clock.arrow.circlepath"
                )
                .font(.caption.weight(.semibold))
                Spacer()
                Button {
                    onToggleHistory()
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Hide copy times")
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(Array(entry.copyHistory.enumerated()), id: \.offset) { pair in
                        HStack(spacing: 6) {
                            Image(systemName: "doc.on.doc")
                                .font(.caption2)
                                .foregroundStyle(.tertiary)
                            Text(pair.element, format: .dateTime.month().day().hour().minute().second())
                                .font(.caption2)
                            if pair.offset == 0 {
                                Text("latest")
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(maxHeight: 110)
        }
        .padding(8)
        .background(.quaternary.opacity(0.6), in: RoundedRectangle(cornerRadius: 8))
    }

    /// Plain click copies normally; Option-click performs a sensitive,
    /// auto-clearing copy that is never stored.
    private func handleTap() {
        if let event = NSApp.currentEvent, event.modifierFlags.contains(.option) {
            onCopyOnce()
        } else {
            onCopy()
        }
    }
}

private struct SensitiveBanner: View {
    let remainingSeconds: Int
    let onClearNow: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "timer")
            Text("Sensitive copy on clipboard — clears in \(remainingSeconds)s")
                .font(.caption)
            Spacer()
            Button("Clear Now") { onClearNow() }
                .buttonStyle(.borderless)
                .font(.caption.weight(.semibold))
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(.orange.opacity(0.15), in: RoundedRectangle(cornerRadius: 7))
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Sensitive clipboard copy clears in \(remainingSeconds) seconds")
    }
}
