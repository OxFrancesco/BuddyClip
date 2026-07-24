import AppKit
import Observation
import SwiftUI
import VaultCore

@main
struct ClipboardVaultApp: App {
    var body: some Scene {
        MenuBarExtra("Clipboard Vault", systemImage: "lock.doc") {
            VaultMenu()
        }
        .menuBarExtraStyle(.window)
    }
}

@MainActor
@Observable
final class VaultViewModel {
    private(set) var entries: [VaultEntry] = []
    private var changeCount = NSPasteboard.general.changeCount
    private var timer: Timer?

    init() {
        refresh()
        timer = .scheduledTimer(withTimeInterval: 0.75, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.captureIfChanged() }
        }
    }

    func refresh() {
        Task {
            entries = (try? await VaultStore.shared.entries()) ?? []
        }
    }

    func copy(_ entry: VaultEntry) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(entry.text, forType: .string)
    }

    func clear() {
        Task {
            try? await VaultStore.shared.clear()
            refresh()
        }
    }

    private func captureIfChanged() {
        let pasteboard = NSPasteboard.general
        guard pasteboard.changeCount != changeCount else { return }
        changeCount = pasteboard.changeCount
        guard let text = pasteboard.string(forType: .string) else { return }
        Task {
            try? await VaultStore.shared.add(text)
            refresh()
        }
    }
}

struct VaultMenu: View {
    @State private var model = VaultViewModel()

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Clipboard Vault").font(.headline)
                Spacer()
                Button("Clear", role: .destructive) { model.clear() }
                    .accessibilityLabel("Clear clipboard vault")
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
