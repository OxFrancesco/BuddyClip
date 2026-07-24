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

    public var errorDescription: String? {
        switch self {
        case .invalidCommand: "Invalid command. Run clipboard-vault help."
        case .unreadableVault: "The encrypted vault could not be read."
        case .missingEntry: "No matching vault entry was found."
        }
    }
}

public actor VaultStore {
    public static let shared = VaultStore()

    private let service = "com.oddofrancesco.clipboard-vault"
    private let account = "encryption-key-v1"
    private let maxEntries = 250
    private let fileURL: URL

    public init() {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appending(path: "ClipboardVault", directoryHint: .isDirectory)
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        fileURL = base.appending(path: "vault.bin")
    }

    public func entries() throws -> [VaultEntry] {
        guard FileManager.default.fileExists(atPath: fileURL.path()) else { return [] }
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
        return all.filter { $0.text.localizedCaseInsensitiveContains(query) }
    }

    public func clear() throws {
        try save([])
    }

    private func save(_ entries: [VaultEntry]) throws {
        let plaintext = try JSONEncoder().encode(entries)
        let sealed = try AES.GCM.seal(plaintext, using: encryptionKey()).combined
        try sealed?.write(to: fileURL, options: .atomic)
    }

    private func encryptionKey() throws -> SymmetricKey {
        if let existing = try readKeychainValue() { return SymmetricKey(data: existing) }
        let data = Data((0..<32).map { _ in UInt8.random(in: .min ... .max) })
        try writeKeychainValue(data)
        return SymmetricKey(data: data)
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

    private func writeKeychainValue(_ data: Data) throws {
        let attributes: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock,
        ]
        guard SecItemAdd(attributes as CFDictionary, nil) == errSecSuccess else { throw VaultError.unreadableVault }
    }
}
