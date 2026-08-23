import AppKit
import Observation
import SwiftUI
import VaultCore

@main
struct ClipboardVaultApp: App {
    @State private var model = VaultViewModel()

    var body: some Scene {
        MenuBarExtra("Clipboard Vault", systemImage: "lock.doc") {
            VaultMenu(model: model)
        }
        .menuBarExtraStyle(.window)
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

    private var filteredEntries: [VaultEntry] {
        query.isEmpty ? model.entries : model.entries.filter { $0.text.localizedCaseInsensitiveContains(query) }
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
            footer
        }
        .padding()
        .frame(width: 380, height: 480)
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
            if !query.isEmpty {
                Button {
                    query = ""
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
                    ForEach(filteredEntries) { entry in
                        EntryRow(
                            entry: entry,
                            copied: model.recentlyCopiedID == entry.id,
                            onCopy: { model.copy(entry) },
                            onCopyOnce: { model.copyOnce(entry) },
                            onDelete: { model.delete(entry) }
                        )
                    }
                }
            }
        }
    }

    private var footer: some View {
        Text("\(model.entries.count) entries · AES-GCM encrypted · never leaves this Mac")
            .font(.caption2)
            .foregroundStyle(.secondary)
    }

    private func resetConfirmationLater() {
        Task {
            try? await Task.sleep(for: .seconds(3))
            confirmingClear = false
        }
    }
}

// MARK: - Rows & banners

private struct EntryRow: View {
    let entry: VaultEntry
    let copied: Bool
    let onCopy: () -> Void
    let onCopyOnce: () -> Void
    let onDelete: () -> Void

    var body: some View {
        Button(action: handleTap) {
            HStack(alignment: .top, spacing: 8) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(entry.text)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                    Text(entry.createdAt, format: .relative(presentation: .named))
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
        .contextMenu {
            Button("Copy") { onCopy() }
            Button("Copy Once — auto-clears in 45s") { onCopyOnce() }
                .keyboardShortcut("c", modifiers: [.command, .option])
            Divider()
            Button("Delete", role: .destructive) { onDelete() }
        }
        .accessibilityLabel("Stored clipboard entry from \(entry.createdAt.formatted(.relative(presentation: .named))). Press to copy.")
        .accessibilityHint("Option-click or use the context menu for a sensitive copy that clears automatically.")
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
