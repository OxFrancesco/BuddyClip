import AppKit
import CryptoKit
import Foundation
import Security

public struct VaultEntry: Codable, Identifiable, Sendable {
    public let id: UUID
    public let createdAt: Date
    public let text: String

    public init(id: UUID = UUID(), createdAt: Date = .now, text: String) {
        self.id = id
        self.createdAt = createdAt
        self.text = text
    }
}

public enum VaultError: LocalizedError {
    case invalidCommand
    case unreadableVault
    case missingEntry
    case invalidEntryID
    case emptyInput

    public var errorDescription: String? {
        switch self {
        case .invalidCommand: "Invalid command. Run clipboard-vault help."
        case .unreadableVault: "The encrypted vault could not be read."
        case .missingEntry: "No matching vault entry was found."
        case .invalidEntryID: "The argument is not a valid entry ID (expected a UUID)."
        case .emptyInput: "No text was provided."
        }
    }
}

/// Pasteboard helpers shared by the app and the CLI.
///
/// Sensitive copies are tagged with a marker pasteboard type so that any
/// ClipboardVault process (menu-bar poller included) can recognize the
/// contents as transient and refuse to persist them.
public enum VaultPasteboard {
    public static let sensitiveMarkerType = NSPasteboard.PasteboardType(
        "com.oddofrancesco.clipboard-vault.sensitive"
    )

    /// True when the current pasteboard contents were written as a sensitive,
    /// auto-clearing copy and must never be captured into the vault.
    public static var holdsSensitiveContent: Bool {
        NSPasteboard.general.data(forType: sensitiveMarkerType) != nil
    }
}

/// Places text on the clipboard without storing it anywhere, and wipes the
/// clipboard automatically after a delay (or on demand).
public actor SensitiveClipboard {
    public static let shared = SensitiveClipboard()
    public static let defaultClearAfter: TimeInterval = 45

    private var clearTask: Task<Void, Never>?
    private(set) var armedText: String?

    /// Copies `text` to the clipboard, tagging it as sensitive, and schedules
    /// an automatic clear. Returns the moment the clipboard will be wiped.
    @discardableResult
    public func copy(_ text: String, clearAfter seconds: TimeInterval = SensitiveClipboard.defaultClearAfter) -> Date {
        cancelPending()
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setData(Data("sensitive".utf8), forType: VaultPasteboard.sensitiveMarkerType)
        pasteboard.setString(text, forType: .string)
        armedText = text
        return scheduleClear(after: seconds)
    }

    private func scheduleClear(after seconds: TimeInterval) -> Date {
        let clearAt = Date().addingTimeInterval(seconds)
        clearTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(seconds))
            guard !Task.isCancelled else { return }
            await self?.performScheduledClear()
        }
        return clearAt
    }

    /// Arms an auto-clear for content already on the clipboard (e.g. written
    /// by another process with the sensitive marker). Returns the clear time.
    @discardableResult
    public func adopt(_ text: String, clearAfter seconds: TimeInterval = SensitiveClipboard.defaultClearAfter) -> Date {
        cancelPending()
        armedText = text
        return scheduleClear(after: seconds)
    }

    /// Wipes the clipboard immediately if it still holds the sensitive text.
    public func clearNow() {
        cancelPending()
        let pasteboard = NSPasteboard.general
        if pasteboard.string(forType: .string) == armedText || VaultPasteboard.holdsSensitiveContent {
            pasteboard.clearContents()
        }
        armedText = nil
    }

    /// Cancels the scheduled clear, leaving the text on the clipboard.
    public func cancelPending() {
        clearTask?.cancel()
        clearTask = nil
    }

    private func performScheduledClear() {
        clearTask = nil
        guard let armed = armedText else { return }
        let pasteboard = NSPasteboard.general
        // Only wipe if the user has not copied something else meanwhile.
        if pasteboard.string(forType: .string) == armed {
            pasteboard.clearContents()
        }
        armedText = nil
    }
}

public actor VaultStore {
    public static let shared = VaultStore()

    private let service = "com.oddofrancesco.clipboard-vault"
    private let account = "encryption-key-v1"
    private let maxEntries = 250
    private let fileURL: URL
    private var didUpgradeKeychainItem = false

    private init() {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appending(path: "ClipboardVault", directoryHint: .isDirectory)
        try? FileManager.default.createDirectory(
            at: base,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        fileURL = base.appending(path: "vault.bin")
    }

    public func entries() throws -> [VaultEntry] {
        guard FileManager.default.fileExists(atPath: fileURL.path(percentEncoded: false)) else { return [] }
        let sealed = try Data(contentsOf: fileURL)
        let key = try encryptionKey()
        guard let box = try? AES.GCM.SealedBox(combined: sealed) else { throw VaultError.unreadableVault }
        let plaintext = try AES.GCM.open(box, using: key)
        return try JSONDecoder().decode([VaultEntry].self, from: plaintext)
    }

    public func add(_ text: String) throws {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        var all = try entries()
        guard all.first?.text != trimmed else { return }
        all.insert(VaultEntry(text: trimmed), at: 0)
        if all.count > maxEntries { all.removeLast(all.count - maxEntries) }
        try save(all)
    }

    public func find(query: String?) throws -> [VaultEntry] {
        let all = try entries()
        guard let query, !query.isEmpty else { return all }
        return FuzzySearch().ranked(all, query: query, text: \.text).map(\.item)
    }

    public func entry(id: UUID) throws -> VaultEntry? {
        try entries().first(where: { $0.id == id })
    }

    public func delete(id: UUID) throws {
        var all = try entries()
        all.removeAll { $0.id == id }
        try save(all)
    }

    public func clear() throws {
        try save([])
    }

    private func save(_ entries: [VaultEntry]) throws {
        let plaintext = try JSONEncoder().encode(entries)
        let sealed = try AES.GCM.seal(plaintext, using: encryptionKey()).combined
        try sealed?.write(to: fileURL, options: .atomic)
        // Restrict the sealed vault to the current user.
        try? FileManager.default.setAttributes(
            [.posixPermissions: 0o600],
            ofItemAtPath: fileURL.path(percentEncoded: false)
        )
    }

    private func encryptionKey() throws -> SymmetricKey {
        if let existing = try readKeychainValue() {
            upgradeKeychainItemAccessibility()
            return SymmetricKey(data: existing)
        }
        let data = Data((0..<32).map { _ in UInt8.random(in: .min ... .max) })
        let status = writeKeychainValue(data)
        if status == errSecDuplicateItem, let existing = try readKeychainValue() {
            upgradeKeychainItemAccessibility()
            return SymmetricKey(data: existing)
        }
        guard status == errSecSuccess else { throw VaultError.unreadableVault }
        return SymmetricKey(data: data)
    }

    /// Tightens accessibility of keys created by older versions
    /// (`AfterFirstUnlock`) to `WhenUnlockedThisDeviceOnly`: the key is only
    /// readable while the user session is unlocked and never leaves this Mac
    /// through backups.
    private func upgradeKeychainItemAccessibility() {
        guard !didUpgradeKeychainItem else { return }
        didUpgradeKeychainItem = true
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        let update: [String: Any] = [
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        ]
        SecItemUpdate(query as CFDictionary, update as CFDictionary)
    }

    private func readKeychainValue() throws -> Data? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw VaultError.unreadableVault }
        return result as? Data
    }

    private func writeKeychainValue(_ data: Data) -> OSStatus {
        let attributes: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        ]
        return SecItemAdd(attributes as CFDictionary, nil)
    }
}
