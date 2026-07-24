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
    private var changeCount = NSPasteboard.general.changeCount
    private var timer: Timer?

    init() {
        refresh()
        if let text = NSPasteboard.general.string(forType: .string) {
            persist(text)
        }
        let timer = Timer(timeInterval: 0.75, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.captureIfChanged() }
        }
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }

    func refresh() {
        Task {
            do {
                entries = try await VaultStore.shared.entries()
                lastError = nil
            } catch {
                lastError = error.localizedDescription
            }
        }
    }

    func copy(_ entry: VaultEntry) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(entry.text, forType: .string)
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

    private func captureIfChanged() {
        let pasteboard = NSPasteboard.general
        guard pasteboard.changeCount != changeCount else { return }
        changeCount = pasteboard.changeCount
        guard let text = pasteboard.string(forType: .string) else { return }
        persist(text)
    }

    private func persist(_ text: String) {
        Task {
            do {
                try await VaultStore.shared.add(text)
                entries = try await VaultStore.shared.entries()
                lastError = nil
            } catch {
                lastError = error.localizedDescription
            }
        }
    }
}

struct VaultMenu: View {
    let model: VaultViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Clipboard Vault").font(.headline)
                Spacer()
                Button("Clear", role: .destructive) { model.clear() }
                    .accessibilityLabel("Clear clipboard vault")
            }

            if let error = model.lastError {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
            }

            if model.entries.isEmpty {
                Text("Copy text to begin storing encrypted history.")
                    .foregroundStyle(.secondary)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 6) {
                        ForEach(model.entries) { entry in
                            Button { model.copy(entry) } label: {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(entry.text).lineLimit(2)
                                    Text(entry.createdAt, format: .dateTime.hour().minute())
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                                .frame(maxWidth: .infinity, alignment: .leading)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Copy stored clipboard entry")
                        }
                    }
                }
            }
        }
        .padding()
        .frame(width: 360, height: 420)
    }
}
