import XCTest
@testable import VaultCore

final class VaultReducerTests: XCTestCase {

    // MARK: - New captures

    func testEmptyTextIsIgnored() {
        let existing = [VaultEntry(text: "hello")]
        XCTAssertEqual(VaultReducer.merging(existing, captured: ""), existing)
        XCTAssertEqual(VaultReducer.merging(existing, captured: "   \n\t "), existing)
    }

    func testNewTextIsInsertedAtFrontTrimmed() {
        let result = VaultReducer.merging([], captured: "  hello world \n")
        XCTAssertEqual(result.count, 1)
        XCTAssertEqual(result[0].text, "hello world")
        XCTAssertEqual(result[0].copyCount, 1)
        XCTAssertEqual(result[0].copyHistory, [result[0].createdAt])
    }

    // MARK: - Duplicate aggregation

    func testDuplicateAnywhereIsPromotedToFrontAndRecounted() throws {
        let original = VaultEntry(createdAt: Date(timeIntervalSince1970: 1_000), text: "dup")
        let other = VaultEntry(createdAt: Date(timeIntervalSince1970: 2_000), text: "other")
        let vault = [
            other,
            original,
            VaultEntry(createdAt: Date(timeIntervalSince1970: 500), text: "oldest"),
        ]

        let captureDate = Date(timeIntervalSince1970: 3_000)
        let result = VaultReducer.merging(vault, captured: "dup", at: captureDate)

        XCTAssertEqual(result.count, 3, "duplicate must aggregate into the same row")
        XCTAssertEqual(result[0].id, original.id, "existing entry keeps its identity")
        XCTAssertEqual(result[0].text, "dup")
        XCTAssertEqual(result[0].createdAt, captureDate, "row shows the latest copy time")
        XCTAssertEqual(result[0].copyCount, 2)

        let history = try XCTUnwrap(result[0].copyHistory)
        XCTAssertEqual(history.first, captureDate)
        XCTAssertEqual(history.last, Date(timeIntervalSince1970: 1_000))
        XCTAssertEqual(history.count, 2)

        XCTAssertEqual(result[1].text, "other", "relative order of other rows is preserved")
        XCTAssertEqual(result[2].text, "oldest")
    }

    func testRepeatedCapturesKeepAggregatingIntoOneRow() {
        var vault: [VaultEntry] = []
        for tick in 1...5 {
            vault = VaultReducer.merging(vault, captured: "same text", at: Date(timeIntervalSince1970: Double(tick)))
        }
        XCTAssertEqual(vault.count, 1)
        XCTAssertEqual(vault[0].copyCount, 5)
        XCTAssertEqual(vault[0].copyHistory.count, 5)
        XCTAssertEqual(vault[0].createdAt.timeIntervalSince1970, 5)
    }

    func testWhitespaceOnlyDifferenceStillAggregates() {
        let first = VaultReducer.merging([], captured: "token123")
        let second = VaultReducer.merging(first, captured: " token123\n")
        XCTAssertEqual(second.count, 1)
        XCTAssertEqual(second[0].copyCount, 2)
    }

    // MARK: - Capacity

    func testOldestEntriesAreDroppedPastMaxEntries() {
        var vault: [VaultEntry] = (0..<10).map {
            VaultEntry(createdAt: Date(timeIntervalSince1970: Double($0)), text: "e\($0)")
        }.reversed()
        vault = VaultReducer.merging(vault, captured: "brand new", maxEntries: 10)
        XCTAssertEqual(vault.count, 10)
        XCTAssertEqual(vault.first?.text, "brand new")
        XCTAssertFalse(vault.contains { $0.text == "e0" }, "oldest row is evicted")

        // Re-capturing an old entry must not resurrect a dropped row.
        vault = VaultReducer.merging(vault, captured: "e0", maxEntries: 10)
        XCTAssertEqual(vault.count, 10)
        XCTAssertEqual(vault.first?.text, "e0")
        XCTAssertEqual(vault.first?.copyCount, 1, "evicted entry starts fresh")
    }

    // MARK: - Codable compatibility

    func testLegacyEntryWithoutCopyFieldsDecodesWithDefaults() throws {
        struct LegacyShape: Encodable {
            let id: UUID
            let createdAt: Date
            let text: String
        }
        let legacy = LegacyShape(id: UUID(), createdAt: Date(timeIntervalSince1970: 42), text: "legacy")
        let data = try JSONEncoder().encode([legacy])

        let decoded = try JSONDecoder().decode([VaultEntry].self, from: data)
        XCTAssertEqual(decoded.count, 1)
        XCTAssertEqual(decoded[0].copyCount, 1)
        XCTAssertEqual(decoded[0].copyHistory, [decoded[0].createdAt])
    }

    func testRoundTripPreservesCaptureHistory() throws {
        let dates = [Date(timeIntervalSince1970: 300), Date(timeIntervalSince1970: 100)]
        let entry = VaultEntry(
            id: UUID(),
            createdAt: dates[0],
            text: "round trip",
            copyCount: 2,
            copyHistory: dates
        )
        let data = try JSONEncoder().encode([entry])
        let decoded = try JSONDecoder().decode([VaultEntry].self, from: data)
        XCTAssertEqual(decoded[0], entry)
    }

    // MARK: - Storage permissions

    func testPrepareDirectoryTightensExistingDirectoryPermissions() throws {
        let fileManager = FileManager.default
        let directory = fileManager.temporaryDirectory
            .appending(path: "ClipboardVaultTests-\(UUID().uuidString)", directoryHint: .isDirectory)
        try fileManager.createDirectory(at: directory, withIntermediateDirectories: false)
        defer { try? fileManager.removeItem(at: directory) }

        try fileManager.setAttributes(
            [.posixPermissions: 0o755],
            ofItemAtPath: directory.path(percentEncoded: false)
        )
        try VaultFileSecurity.prepareDirectory(at: directory, fileManager: fileManager)

        let attributes = try fileManager.attributesOfItem(atPath: directory.path(percentEncoded: false))
        let permissions = try XCTUnwrap(attributes[.posixPermissions] as? NSNumber)
        XCTAssertEqual(permissions.intValue & 0o777, 0o700)
    }
}
