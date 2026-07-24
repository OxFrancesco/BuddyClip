import AppKit
import Foundation
import VaultCore

@main
struct ClipboardVaultCLI {
    static func main() async {
        do {
            let args = Array(CommandLine.arguments.dropFirst())
            let command = args.first ?? "help"
            let store = VaultStore.shared

            switch command {
            case "list", "search":
                let query = command == "search" ? args.dropFirst().joined(separator: " ") : nil
                for entry in try await store.find(query: query) {
                    let preview = entry.text.replacingOccurrences(of: "\n", with: " ")
                    print("\(entry.id.uuidString)\t\(entry.createdAt.ISO8601Format())\t\(preview)")
                }
            case "copy":
                guard let id = args.dropFirst().first, let uuid = UUID(uuidString: id) else { throw VaultError.invalidCommand }
                guard let entry = try await store.entries().first(where: { $0.id == uuid }) else { throw VaultError.missingEntry }
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(entry.text, forType: .string)
            case "clear":
                try await store.clear()
            case "help":
                print("""
                clipboard-vault list
                clipboard-vault search <text>
                clipboard-vault copy <entry-id>
                clipboard-vault clear
                """)
            default:
                throw VaultError.invalidCommand
            }
        } catch {
            FileHandle.standardError.write(Data("clipboard-vault: \(error.localizedDescription)\n".utf8))
            Foundation.exit(1)
        }
    }
}
