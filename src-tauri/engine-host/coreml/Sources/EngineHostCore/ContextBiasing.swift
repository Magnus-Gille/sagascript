// Context biasing (shallow-fusion phrase boosting) for greedy TDT decoding.
// Dictionary terms are tokenized with the model's SentencePiece pieces into a
// prefix trie; during decoding, candidate tokens that start or continue a trie
// path receive a logit bonus before the argmax. Original implementation.
import Foundation

public struct BoostConfig: Sendable {
    public static let maxTerms = 500
    public static let maxTermLength = 64
    public let terms: [String]
    public let weight: Float
    public init(terms: [String], weight: Float) {
        self.terms = terms
        self.weight = weight
    }
}

/// Splits a word (with leading `▁`) into vocabulary pieces. Returns the
/// minimum-piece segmentation and the longest-match-first segmentation.
func segmentWord(_ word: String, pieces: [String: Int]) -> [[Int]] {
    let chars = Array("▁" + word)
    let n = chars.count
    var best: [[Int]?] = Array(repeating: nil, count: n + 1)
    best[0] = []
    for end in 1...n {
        for start in max(0, end - 24)..<end {
            guard let prefix = best[start], let id = pieces[String(chars[start..<end])] else { continue }
            if best[end] == nil || prefix.count + 1 < best[end]!.count { best[end] = prefix + [id] }
        }
    }
    var results: [[Int]] = []
    if let minimal = best[n] { results.append(minimal) }
    var greedy: [Int] = []
    var position = 0
    while position < n {
        var matched = false
        var end = min(n, position + 24)
        while end > position {
            if let id = pieces[String(chars[position..<end])] { greedy.append(id); position = end; matched = true; break }
            end -= 1
        }
        if !matched { greedy = []; break }
    }
    if !greedy.isEmpty && !results.contains(greedy) { results.append(greedy) }
    return results
}

public final class BoostTrie {
    private var children: [[Int: Int]] = [[:]]  // node 0 is the root
    private var terminal: [Bool] = [false]
    public let weight: Float
    public private(set) var phraseCount = 0

    public init(config: BoostConfig, vocabulary: [Int: String], blankID: Int) {
        weight = config.weight
        var pieces: [String: Int] = [:]
        for (id, text) in vocabulary where !text.isEmpty && !text.hasPrefix("<") && id != blankID {
            pieces[text] = id
        }
        for raw in config.terms.prefix(BoostConfig.maxTerms) {
            let term = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !term.isEmpty, term.count <= BoostConfig.maxTermLength else { continue }
            var forms = [term]
            let capitalized = term.prefix(1).uppercased() + term.dropFirst()
            if capitalized != term { forms.append(capitalized) }
            for form in forms {
                var sequences: [[Int]] = [[]]
                for word in form.split(whereSeparator: { $0.isWhitespace }) {
                    let options = segmentWord(String(word), pieces: pieces)
                    if options.isEmpty { sequences = []; break }
                    sequences = sequences.flatMap { prefix in options.map { prefix + $0 } }
                }
                for sequence in sequences where !sequence.isEmpty { insert(sequence) }
            }
        }
    }

    func insert(_ sequence: [Int]) {
        var node = 0
        for token in sequence {
            if let next = children[node][token] { node = next } else {
                children.append([:]); terminal.append(false)
                children[node][token] = children.count - 1
                node = children.count - 1
            }
        }
        terminal[node] = true
        phraseCount += 1
    }

    public var isEmpty: Bool { children[0].isEmpty }
    func child(_ node: Int, _ token: Int) -> Int? { children[node][token] }
    func hasChildren(_ node: Int) -> Bool { !children[node].isEmpty }
}

/// Per-decode state: the set of live partial matches (root is implicit).
public struct BiasState {
    let trie: BoostTrie
    let startFactor: Float
    let maxGapFrames: Int
    var active: [Int] = []
    var lastEmitFrame = 0

    public init(trie: BoostTrie, startFactor: Float = 0.25, maxGapFrames: Int = 8) {
        self.trie = trie
        self.startFactor = startFactor
        self.maxGapFrames = maxGapFrames
    }

    func bonus(for token: Int) -> Float {
        var value: Float = 0
        for node in active where trie.child(node, token) != nil { value = max(value, trie.weight) }
        if value == 0, trie.child(0, token) != nil { value = trie.weight * startFactor }
        return value
    }

    /// Picks the token among the model's top-K candidates after adding bonuses.
    /// Returns nil when the model's own choice stands (so output is unchanged).
    public mutating func choose(ids: [Int], logits: [Float], frame: Int) -> (id: Int, probability: Float)? {
        if !active.isEmpty && frame - lastEmitFrame > maxGapFrames { active = [] }
        guard !ids.isEmpty, ids.count == logits.count else { return nil }
        var bestIndex = 0
        var bestScore = -Float.infinity
        var boosted = false
        for index in 0..<ids.count {
            let bonus = bonus(for: ids[index])
            if bonus > 0 { boosted = true }
            let score = logits[index] + bonus
            if score > bestScore { bestScore = score; bestIndex = index }
        }
        guard boosted else { return nil }
        let originalBest = logits.indices.max(by: { logits[$0] < logits[$1] }) ?? 0
        guard bestIndex != originalBest else { return nil }
        let peak = logits[originalBest]
        let denominator = logits.reduce(Float(0)) { $0 + exp($1 - peak) }
        return (ids[bestIndex], exp(logits[bestIndex] - peak) / denominator)
    }

    /// Advance the partial matches after a non-blank token was emitted.
    public mutating func emitted(token: Int, frame: Int) {
        var next: [Int] = []
        for node in active + [0] {
            if let child = trie.child(node, token), trie.hasChildren(child), !next.contains(child) { next.append(child) }
        }
        active = next
        lastEmitFrame = frame
    }
}
