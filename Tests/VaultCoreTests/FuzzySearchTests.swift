import XCTest
@testable import VaultCore

final class FuzzySearchTests: XCTestCase {

    let fuzzy = FuzzySearch()

    // MARK: - Match / no match

    func testEmptyQueryMatchesEverythingWithZeroScore() {
        let result = fuzzy.match("", in: "anything")
        XCTAssertEqual(result?.score, 0)
        XCTAssertEqual(result?.positions, [])
    }

    func testExactSubstringMatches() {
        let result = fuzzy.match("hello", in: "say hello world")
        XCTAssertNotNil(result)
        XCTAssertEqual(result?.positions, [4, 5, 6, 7, 8])
    }

    func testCaseInsensitiveMatch() {
        XCTAssertNotNil(fuzzy.match("HELLO", in: "hello world"))
        XCTAssertNotNil(fuzzy.match("hello", in: "HELLO WORLD"))
    }

    func testNonSubsequenceDoesNotMatch() {
        XCTAssertNil(fuzzy.match("xyz", in: "hello world"))
        XCTAssertNil(fuzzy.match("world hello", in: "hello world"))
    }

    func testQueryLongerThanTextNeverMatches() {
        XCTAssertNil(fuzzy.match("abcdefghijk", in: "short"))
    }

    // MARK: - Scoring preferences

    func testPrefixBeatsMidWordInfix() {
        let prefix = fuzzy.match("git", in: "github dashboard")!
        let midWord = fuzzy.match("git", in: "agitator")!
        XCTAssertGreaterThan(prefix.score, midWord.score)
    }

    func testWordBoundaryBeatsMidWord() {
        let boundary = fuzzy.match("dash", in: "github-dashboard")!
        let midWord = fuzzy.match("ash", in: "dashboard")!
        XCTAssertGreaterThan(boundary.score, midWord.score)
    }

    func testContiguousBeatsScattered() {
        let contiguous = fuzzy.match("abc", in: "abcdef")!
        let scattered = fuzzy.match("ace", in: "abcdef")!
        XCTAssertGreaterThan(contiguous.score, scattered.score)
    }

    func testShorterGapBeatsLongerGap() {
        let tight = fuzzy.match("axb", in: "axb")!
        _ = tight
        let small = fuzzy.match("ab", in: "a_b")!
        let wide = fuzzy.match("ab", in: "a_______b")!
        XCTAssertGreaterThan(small.score, wide.score)
    }

    func testCamelCaseBoundaryBeatsPlainMidWord() {
        let hump = fuzzy.match("d", in: "xxxDashboard")!
        let plain = fuzzy.match("d", in: "xxxdashboard")!
        XCTAssertGreaterThan(hump.score, plain.score)
    }

    func testRankingPutsBestCandidateFirst() {
        let fuzzySearch = FuzzySearch()
        let ranked = fuzzySearch.ranked(
            ["scattered g i t here", "my github", "github"],
            query: "github",
            text: { $0 }
        )
        // "github" and "my github" score identically (word-boundary match);
        // the shorter, tighter text wins the tie. The scattered entry lacks
        // a "u" entirely and drops out.
        XCTAssertEqual(ranked.first?.item, "github")
        XCTAssertEqual(ranked.count, 2)
    }

    func testRankedDropsNonMatchesAndPreservesOrderOnTies() {
        let ranked = FuzzySearch().ranked(
            ["first alpha", "unrelated", "second alpha"],
            query: "alpha",
            text: { $0 }
        )
        XCTAssertEqual(ranked.map(\.item), ["first alpha", "second alpha"])
    }

    // MARK: - Highlight positions

    func testSubsequencePositionsAreAscendingAndCorrect() {
        let result = fuzzy.match("hlo", in: "hello world")!
        // "lo" is a consecutive run — the optimal alignment pairs l with
        // index 3, not the gapped l at index 2.
        XCTAssertEqual(result.positions, [0, 3, 4])
        let chars = Array("hello world")
        XCTAssertEqual(result.positions.map { chars[$0] }, ["h", "l", "o"])
    }

    func testTypoQueryStillFindsWord() {
        let result = fuzzy.match("helo", in: "the quick brown hello")!
        XCTAssertNotNil(result)
    }

    func testAbbreviationStyleQuery() {
        // g -> "G" (start), h -> "H" (camel hump), d -> "D" (after space)
        let result = fuzzy.match("ghd", in: "GitHub Dashboard")!
        XCTAssertEqual(result.positions, [0, 3, 7])
    }

    // MARK: - Unicode safety

    func testUnicodeTextSurvivesMatching() {
        let result = fuzzy.match("café", in: "Visiter le café à Paris")
        XCTAssertNotNil(result)
        let emojiResult = fuzzy.match("🚀launch", in: "hit the 🚀 launch button")
        XCTAssertNotNil(emojiResult)
    }
}
