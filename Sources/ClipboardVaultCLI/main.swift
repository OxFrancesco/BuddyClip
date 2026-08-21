import AppKit
import Foundation
import VaultCore

@main
struct ClipboardVaultCLI {
    static func main() async {
        do {
            try await run()
        } catch let error as VaultError {
            fail(error.localizedDescription)
        } catch {
            fail(error.localizedDescription)
        }
    }

    // MARK: - Argument parsing

    private struct Arguments {
        var flags: Set<String> = []
        var values: [String: String] = [:]
        var positional: [String] = []

        init(_ raw: [String]) {
            let valueFlags: Set<String> = ["limit", "clear-after"]
            var index = 0
            while index < raw.count {
                let arg = raw[index]
                if arg == "-" {
                    positional.append(arg)
                } else if arg.hasPrefix("--") {
                    let body = arg.dropFirst(2)
                    if body.contains("=") {
                        let parts = body.split(separator: "=", maxSplits: 1)
                        values[String(parts[0])] = String(parts.dropFirst().joined(separator: "="))
                    } else if valueFlags.contains(String(body)), index + 1 < raw.count {
                        values[String(body)] = raw[index + 1]
                        index += 1
                    } else {
                        flags.insert(String(body))
                    }
                } else {
                    positional.append(arg)
                }
                index += 1
            }
        }

        var limit: Int? {
            guard let raw = values["limit"], let parsed = Int(raw), parsed > 0 else { return nil }
            return parsed
        }

        var clearAfter: TimeInterval? {
            guard let raw = values["clear-after"], let parsed = TimeInterval(raw), parsed > 0 else { return nil }
            return parsed
        }
    }

    // MARK: - Dispatch

    private static func run() async throws {
        let args = Arguments(Array(CommandLine.arguments.dropFirst()))
        let command = args.positional.first ?? "help"
        let store = VaultStore.shared

        switch command {
        case "list":
            let entries = try await store.find(query: nil)
            let limited = Array(entries.prefix(args.limit ?? entries.count))
            if args.flags.contains("json") {
                printJSON(limited)
            } else {
                limited.forEach { printRow($0) }
            }

        case "search":
            let query = args.positional.dropFirst().joined(separator: " ")
            let entries = try await store.find(query: query)
            let limited = Array(entries.prefix(args.limit ?? entries.count))
            if args.flags.contains("json") {
                printJSON(limited)
            } else {
                limited.forEach { printRow($0) }
            }

        case "get":
            guard let id = entryID(from: args) else { throw VaultError.invalidEntryID }
            guard let entry = try await store.entry(id: id) else { throw VaultError.missingEntry }
            FileHandle.standardOutput.write(Data(entry.text.utf8))
            FileHandle.standardOutput.write(Data("\n".utf8))

        case "copy":
            guard let id = entryID(from: args) else { throw VaultError.invalidEntryID }
            guard let entry = try await store.entry(id: id) else { throw VaultError.missingEntry }
            if args.flags.contains("sensitive") {
                let clearAt = await SensitiveClipboard.shared.copy(
                    entry.text,
                    clearAfter: args.clearAfter ?? SensitiveClipboard.defaultClearAfter
                )
                try await finishSensitiveCopy(id: id.uuidString, clearAt: clearAt, noWait: args.flags.contains("no-wait"))
            } else {
                placeOnPasteboard(entry.text)
                print("Copied \(id.uuidString).")
            }

        case "latest":
            guard let entry = try await store.find(query: nil).first else { throw VaultError.missingEntry }
            if args.flags.contains("sensitive") {
                let clearAt = await SensitiveClipboard.shared.copy(
                    entry.text,
                    clearAfter: args.clearAfter ?? SensitiveClipboard.defaultClearAfter
                )
                try await finishSensitiveCopy(id: "latest entry", clearAt: clearAt, noWait: args.flags.contains("no-wait"))
            } else {
                placeOnPasteboard(entry.text)
                print("Copied latest entry (\(entry.id.uuidString)).")
            }

        case "secret":
            let text: String
            if args.positional.count > 1, args.positional[1] != "-" {
                text = args.positional.dropFirst().joined(separator: " ")
            } else {
                // No inline text, or explicit "-": read from stdin.
                let data = FileHandle.standardInput.readDataToEndOfFile()
                text = String(data: data, encoding: .utf8) ?? ""
            }
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else { throw VaultError.emptyInput }
            let clearAt = await SensitiveClipboard.shared.copy(
                trimmed,
                clearAfter: args.clearAfter ?? SensitiveClipboard.defaultClearAfter
            )
            try await finishSensitiveCopy(id: nil, clearAt: clearAt, noWait: args.flags.contains("no-wait"))

        case "forget":
            guard let id = entryID(from: args) else { throw VaultError.invalidEntryID }
            guard try await store.entry(id: id) != nil else { throw VaultError.missingEntry }
            try await store.delete(id: id)
            print("Forgot \(id.uuidString).")

        case "count":
            print(try await store.entries().count)

        case "clear":
            try await store.clear()
            print("Vault cleared.")

        case "help":
            printUsage()

        default:
            throw VaultError.invalidCommand
        }
    }

    // MARK: - Helpers

    /// Keeps a short-lived process alive until the scheduled wipe has fired.
    /// Without this, the auto-clear task would die with the process and the
    /// password would stay on the clipboard forever. `--no-wait` skips the
    /// wait; the menu-bar app then adopts and wipes the copy instead.
    private static func finishSensitiveCopy(id: String?, clearAt: Date, noWait: Bool) async throws {
        let copied = id.map { "Copied \($0) as sensitive." } ?? "Placed on clipboard without storing it."
        if noWait {
            print("\(copied) Clears automatically at \(clearAt.formatted(date: .omitted, time: .standard)) (menu-bar app must be running).")
            return
        }
        print("\(copied) Clipboard clears automatically at \(clearAt.formatted(date: .omitted, time: .standard)); waiting…")
        while Date() < clearAt {
            try await Task.sleep(for: .seconds(0.5))
            // Stop early if something else replaced the clipboard.
            if !VaultPasteboard.holdsSensitiveContent,
               NSPasteboard.general.string(forType: .string) != nil { break }
        }
        // Give the scheduled wipe a moment to fire before exiting.
        try? await Task.sleep(for: .seconds(0.6))
    }

    private static func entryID(from args: Arguments) -> UUID? {
        guard let raw = args.positional.dropFirst().first else { return nil }
        return UUID(uuidString: raw)
    }

    private static func placeOnPasteboard(_ text: String) {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
    }

    private static func printRow(_ entry: VaultEntry) {
        let preview = entry.text
            .replacingOccurrences(of: "\n", with: " ")
            .replacingOccurrences(of: "\t", with: " ")
        let truncated = preview.count > 160 ? preview.prefix(160) + "…" : preview
        print("\(entry.id.uuidString)\t\(entry.createdAt.ISO8601Format())\t\(truncated)")
    }

    private static func printJSON(_ entries: [VaultEntry]) {
        struct EntryJSON: Encodable {
            let id: UUID
            let createdAt: String
            let text: String
        }
        let formatter = ISO8601DateFormatter()
        let payload = entries.map {
            EntryJSON(id: $0.id, createdAt: formatter.string(from: $0.createdAt), text: $0.text)
        }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        guard let data = try? encoder.encode(payload), let json = String(data: data, encoding: .utf8) else {
            fail("Could not encode vault entries as JSON.")
        }
        print(json)
    }

    private static func printUsage() {
        print(
            """
            clipboard-vault list [--json] [--limit N]
                List stored entries (newest first).
            clipboard-vault search <text> [--json] [--limit N]
                Search stored entries.
            clipboard-vault get <entry-id>
                Print an entry's full text to stdout.
            clipboard-vault copy <entry-id> [--sensitive] [--clear-after SECONDS] [--no-wait]
                Copy an entry to the clipboard. With --sensitive the copy is
                never stored and the clipboard auto-clears (default 45s).
                The command waits for the wipe unless --no-wait is given.
            clipboard-vault latest [--sensitive] [--clear-after SECONDS] [--no-wait]
                Copy the most recent entry.
            clipboard-vault secret (<text> | -) [--clear-after SECONDS] [--no-wait]
                Put text on the clipboard WITHOUT storing it; it auto-clears.
                Pass "-" to read the text from stdin.
            clipboard-vault forget <entry-id>
                Delete a single entry from the vault.
            clipboard-vault count
                Print the number of stored entries.
            clipboard-vault clear
                Delete every stored entry.
            """
        )
    }

    private static func fail(_ message: String) -> Never {
        FileHandle.standardError.write(Data("clipboard-vault: \(message)\n".utf8))
        Foundation.exit(1)
    }
}
