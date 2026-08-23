import Foundation

/// The outcome of a successful fuzzy match.
public struct FuzzyResult: Equatable, Sendable {
    /// Higher is better. Only meaningful relative to other results for the
    /// same query — scores are not comparable across queries.
    public let score: Int

    /// Character offsets into the matched text, ascending. Contiguous for
    /// substring hits; scattered for subsequence (typo) hits.
    public let positions: [Int]

    init(score: Int, positions: [Int]) {
        self.score = score
        self.positions = positions
    }
}

/// fzf-style fuzzy matcher: ranks exact substrings highest, then word-boundary
/// and prefix matches, and still finds scattered subsequences (typos,
/// abbreviations like "ghd" -> "GitHub Dashboard").
///
/// Pure and deterministic — safe to call from any thread.
public struct FuzzySearch: Sendable {

    // MARK: Tuning constants (roughly fzf's defaults)

    private let scoreMatch = 16
    private let bonusBoundary = 8
    private let bonusCamel = 7
    private let bonusConsecutive = 4
    private let firstCharMultiplier = 2
    private let gapStartPenalty = 3
    private let gapExtensionPenalty = 1

    public init() {}

    /// Scores `query` against `text`. Returns `nil` when the query is not a
    /// subsequence of the text. An empty query matches everything with score 0.
    public func match(_ query: String, in text: String) -> FuzzyResult? {
        let needle = Array(query.lowercased())
        guard !needle.isEmpty else { return FuzzyResult(score: 0, positions: []) }
        let haystack = Array(text)
        let lowered = Array(text.lowercased())
        guard needle.count <= haystack.count else { return nil }

        let bonuses = positionBonuses(in: haystack)
        let n = needle.count
        let m = haystack.count
        let negativeInfinity = Int.min / 4

        // dpEnd[i][j]: best score of an alignment of needle[0...j] whose final
        // character lands exactly on haystack[i].
        var dpEnd = [[Int]](repeating: [Int](repeating: negativeInfinity, count: n), count: m)
        // prev[i][j]: haystack index matched by needle[j-1] in the best
        // alignment ending at (i, j); -1 when j == 0.
        var prev = [[Int]](repeating: [Int](repeating: -1, count: n), count: m)
        // runningMax[j]: max over already-processed rows p of
        // dpEnd[p][j] + gapExtensionPenalty * p — enables O(1) gap transitions.
        var runningMax = [Int](repeating: negativeInfinity, count: n)
        var runningArg = [Int](repeating: -1, count: n)

        for i in 0..<m {
            let boundaryBonus = bonuses[i]
            for j in 0..<n {
                guard lowered[i] == needle[j] else { continue }
                let bonus = j == 0 ? boundaryBonus * firstCharMultiplier : boundaryBonus

                if j == 0 {
                    dpEnd[i][0] = scoreMatch + bonus
                    continue
                }

                var best = negativeInfinity
                var bestPrev = -1

                // Consecutive: needle[j-1] matched at i-1, no gap.
                if i > 0, dpEnd[i - 1][j - 1] > negativeInfinity {
                    let candidate = dpEnd[i - 1][j - 1] + scoreMatch + bonusConsecutive + bonus
                    if candidate > best { best = candidate; bestPrev = i - 1 }
                }

                // Gap: needle[j-1] matched at some p <= i-2.
                if i >= 2, runningMax[j - 1] > negativeInfinity {
                    let candidate = runningMax[j - 1] - gapStartPenalty
                        - gapExtensionPenalty * (i - 1) + scoreMatch + bonus
                    if candidate > best { best = candidate; bestPrev = runningArg[j - 1] }
                }

                dpEnd[i][j] = best
                prev[i][j] = bestPrev
            }
            // Fold this row into the running maxima for gap transitions later.
            for j in 0..<n where dpEnd[i][j] > negativeInfinity {
                let candidate = dpEnd[i][j] + gapExtensionPenalty * i
                if candidate > runningMax[j] { runningMax[j] = candidate; runningArg[j] = i }
            }
        }

        var endScore = negativeInfinity
        var endIndex = -1
        for i in 0..<m where dpEnd[i][n - 1] > endScore {
            endScore = dpEnd[i][n - 1]
            endIndex = i
        }
        guard endIndex >= 0 else { return nil }

        var positions = [Int](repeating: 0, count: n)
        positions[n - 1] = endIndex
        var cursor = endIndex
        for j in stride(from: n - 1, to: 0, by: -1) {
            let p = prev[cursor][j]
            precondition(p >= 0, "fuzzy backtrace failed at (\(cursor), \(j))")
            positions[j - 1] = p
            cursor = p
        }

        return FuzzyResult(score: endScore, positions: positions)
    }

    /// Ranks `items` by fuzzy score against `query`, best first. Items that do
    /// not match are dropped. Equal scores prefer the shorter text (a tighter,
    /// more precise match), then keep the original order (recency, given how
    /// the vault stores history).
    public func ranked<Item>(
        _ items: [Item],
        query: String,
        text: (Item) -> String
    ) -> [(item: Item, result: FuzzyResult)] {
        guard !query.isEmpty else {
            return items.map { ($0, FuzzyResult(score: 0, positions: [])) }
        }
        var scored: [(index: Int, item: Item, result: FuzzyResult)] = []
        scored.reserveCapacity(items.count)
        for (index, item) in items.enumerated() {
            if let result = match(query, in: text(item)) {
                scored.append((index, item, result))
            }
        }
        scored.sort { lhs, rhs in
            if lhs.result.score != rhs.result.score { return lhs.result.score > rhs.result.score }
            let lhsLen = text(lhs.item).count
            let rhsLen = text(rhs.item).count
            if lhsLen != rhsLen { return lhsLen < rhsLen }
            return lhs.index < rhs.index
        }
        return scored.map { ($0.item, $0.result) }
    }

    // MARK: - Positional bonuses

    /// Per-haystack-position bonus: start-of-string and after-separator get the
    /// boundary bonus; camelCase humps and letter->digit edges get a smaller one.
    private func positionBonuses(in chars: [Character]) -> [Int] {
        var bonuses = [Int](repeating: 0, count: chars.count)
        for i in 0..<chars.count {
            guard i > 0 else { bonuses[i] = bonusBoundary; continue }
            let previous = chars[i - 1].lowercased().first ?? chars[i - 1]
            let current = chars[i].lowercased().first ?? chars[i]
            if !isWordCharacter(previous) {
                bonuses[i] = bonusBoundary
            } else if chars[i - 1].isLowercase, chars[i].isUppercase {
                bonuses[i] = bonusCamel
            } else if !previous.isNumber, current.isNumber {
                bonuses[i] = bonusCamel
            }
        }
        return bonuses
    }

    private func isWordCharacter(_ c: Character) -> Bool {
        c.isLetter || c.isNumber
    }
}
