// Context biasing (shallow-fusion phrase boosting) for greedy TDT decoding.
// Dictionary terms are tokenized with the model's SentencePiece pieces into a
// prefix trie; during decoding, candidate tokens that start or continue a trie
// path receive a logit bonus before the argmax. Original implementation.
//
// Parity contract with the ONNX host (engine-host/ort/src/boost.rs): both hosts
// share engine-host/test-vectors/context-biasing.json. Lengths and segmentation
// use Unicode scalars (not grapheme clusters); candidates are the top-64 token
// logits; ties break to the lower token id.
import Foundation

public struct BoostConfig: Sendable {
    public static let maxTerms = 500
    public static let maxTermLength = 64  // Unicode scalars
    public static let maxNodes = 50_000
    public static let topK = 64
    /// Segmentation alternatives are combined per term only up to this many token sequences.
    public static let maxSequencesPerTerm = 16
    public let terms: [String]
    public let weight: Float
    public init(terms: [String], weight: Float) {
        self.terms = terms
        self.weight = weight
    }

    /// Cache key: the trie depends on the terms only (the weight lives in `BiasParams`).
    var cacheKey: String { terms.map { "\($0.utf8.count):\($0)" }.joined() }  // length-prefixed: no collisions
}

/// Bonus and gating parameters for one decode.
public struct BiasParams: Sendable {
    public var weight: Float
    /// Start-of-term tokens get `weight * startFactor`; continuations get `weight`.
    public var startFactor: Float = 0
    /// A start-of-term bonus may override a blank only when the term's raw logit is within this
    /// margin of the blank logit (continuations are never gated).
    public var blankMargin: Float = 4.0
    /// Partial matches are dropped after this many frames without an emitted token.
    public var maxGapFrames: Int = 8
    /// A boosted token that overrode blank skips at most this many frames.
    public var overrideDurationCap: Int = 1

    public init(weight: Float) { self.weight = weight }

    /// Measurement overrides (`SAGASCRIPT_BOOST_START_FACTOR`, `SAGASCRIPT_BOOST_BLANK_MARGIN`,
    /// `SAGASCRIPT_BOOST_OVERRIDE_DURATION_CAP`).
    public static func fromEnvironment(weight: Float, environment: [String: String] = ProcessInfo.processInfo.environment) -> BiasParams {
        var params = BiasParams(weight: weight)
        if let value = environment["SAGASCRIPT_BOOST_START_FACTOR"].flatMap(Float.init), value.isFinite {
            params.startFactor = min(max(value, 0), 1)
        }
        if let value = environment["SAGASCRIPT_BOOST_BLANK_MARGIN"].flatMap(Float.init), !value.isNaN {
            params.blankMargin = max(value, 0)
        }
        if let value = environment["SAGASCRIPT_BOOST_OVERRIDE_DURATION_CAP"].flatMap(Int.init), value >= 0 {
            params.overrideDurationCap = value
        }
        return params
    }
}

/// Splits a word (with leading `▁`) into vocabulary pieces over Unicode scalars. Returns the
/// minimum-piece segmentation and the longest-match-first segmentation (when different).
func segmentWord(_ word: String, pieces: [String: Int]) -> [[Int]] {
    let scalars = Array("▁".unicodeScalars) + Array(word.unicodeScalars)
    let n = scalars.count
    func slice(_ a: Int, _ b: Int) -> String {
        var view = String.UnicodeScalarView()
        view.append(contentsOf: scalars[a..<b])
        return String(view)
    }
    var best: [[Int]?] = Array(repeating: nil, count: n + 1)
    best[0] = []
    for end in 1...n {
        for start in max(0, end - 24)..<end {
            guard let prefix = best[start], let id = pieces[slice(start, end)] else { continue }
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
            if let id = pieces[slice(position, end)] { greedy.append(id); position = end; matched = true; break }
            end -= 1
        }
        if !matched { greedy = []; break }
    }
    if !greedy.isEmpty && !results.contains(greedy) { results.append(greedy) }
    return results
}

/// Immutable prefix trie over token ids; safe to share across windows and threads.
public final class BoostTrie: @unchecked Sendable {
    private var children: [[Int: Int]] = [[:]]  // node 0 is the root
    private var terminal: [Bool] = [false]
    public private(set) var rootChildren: [Int] = []

    private let maxNodes: Int

    public init(config: BoostConfig, vocabulary: [Int: String], blankID: Int, maxNodes: Int = BoostConfig.maxNodes) {
        self.maxNodes = maxNodes
        var pieces: [String: Int] = [:]
        for (id, text) in vocabulary where !text.isEmpty && !text.hasPrefix("<") && id != blankID {
            pieces[text] = id
        }
        for raw in config.terms.prefix(BoostConfig.maxTerms) {
            let term = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !term.isEmpty, term.unicodeScalars.count <= BoostConfig.maxTermLength else { continue }
            var forms = [term]
            if let first = term.unicodeScalars.first {
                var view = String.UnicodeScalarView()
                view.append(contentsOf: String(first).uppercased().unicodeScalars)
                view.append(contentsOf: term.unicodeScalars.dropFirst())
                let capitalized = String(view)
                if capitalized != term { forms.append(capitalized) }
            }
            for form in forms {
                var sequences: [[Int]] = [[]]
                for word in form.split(whereSeparator: { $0.isWhitespace }) {
                    let options = segmentWord(String(word), pieces: pieces)
                    if options.isEmpty { sequences = []; break }
                    sequences = Array(sequences.flatMap { prefix in options.map { prefix + $0 } }.prefix(BoostConfig.maxSequencesPerTerm))
                }
                for sequence in sequences where !sequence.isEmpty { insert(sequence) }
            }
        }
        rootChildren = children[0].keys.sorted()
    }

    func insert(_ sequence: [Int]) {
        guard children.count + sequence.count <= maxNodes else { return }
        var node = 0
        for token in sequence {
            if let next = children[node][token] { node = next } else {
                children.append([:]); terminal.append(false)
                children[node][token] = children.count - 1
                node = children.count - 1
            }
        }
        terminal[node] = true
    }

    public var isEmpty: Bool { children[0].isEmpty }
    public var nodeCount: Int { children.count }
    func child(_ node: Int, _ token: Int) -> Int? { children[node][token] }
    func childTokens(_ node: Int) -> Dictionary<Int, Int>.Keys { children[node].keys }
    func hasChildren(_ node: Int) -> Bool { !children[node].isEmpty }

    /// Every complete phrase as a token sequence, sorted (tests and diagnostics).
    func phrases() -> [[Int]] {
        var result: [[Int]] = []
        func walk(_ node: Int, _ path: [Int]) {
            if terminal[node] { result.append(path) }
            for token in children[node].keys { walk(children[node][token]!, path + [token]) }
        }
        walk(0, [])
        return result.sorted { $0.lexicographicallyPrecedes($1) }
    }
}

public struct BiasChoice: Equatable {
    public let id: Int
    /// The model's own blank was overridden; the joint's duration head described blank, not this
    /// token, so the decoder caps the skip.
    public let overrodeBlank: Bool
    /// Probability of the chosen token under the model's unadjusted top-K distribution (a
    /// softmax over the K candidate logits, no bonus); used only as the token confidence.
    public let probability: Float
}

/// Per-decode state: the set of live partial matches (root is implicit).
public struct BiasState {
    let trie: BoostTrie
    let params: BiasParams
    let blankID: Int
    var active: [Int] = []
    var lastEmitFrame = 0

    public init(trie: BoostTrie, params: BiasParams, blankID: Int) {
        self.trie = trie
        self.params = params
        self.blankID = blankID
    }

    /// Picks the token among the model's top-K candidates after adding bonuses. Returns nil when
    /// the model's own choice (highest logit, lowest id on ties) stands, so output is unchanged.
    public mutating func choose(ids: [Int], logits: [Float], frame: Int) -> BiasChoice? {
        if !active.isEmpty && frame - lastEmitFrame > params.maxGapFrames { active = [] }
        guard !ids.isEmpty, ids.count == logits.count else { return nil }
        var original = 0
        var blankLogit = -Float.infinity
        for index in 0..<ids.count {
            if logits[index] > logits[original] || (logits[index] == logits[original] && ids[index] < ids[original]) {
                original = index
            }
            if ids[index] == blankID { blankLogit = logits[index] }
        }
        let overridingBlank = ids[original] == blankID
        // Every candidate, the model's own winner included, is scored by the same rules, so a
        // dictionary token that is already winning keeps its bonus against a weaker start.
        var chosen = original
        var chosenScore = -Float.infinity
        for index in 0..<ids.count {
            let token = ids[index]
            var bonus: Float = 0
            if active.contains(where: { trie.child($0, token) != nil }) {
                bonus = params.weight
            } else if trie.child(0, token) != nil {
                if index == original || !overridingBlank || logits[index] >= blankLogit - params.blankMargin {
                    bonus = params.weight * params.startFactor
                }
            }
            let score = logits[index] + bonus
            if score > chosenScore || (score == chosenScore && token < ids[chosen]) {
                chosen = index
                chosenScore = score
            }
        }
        guard chosen != original else { return nil }
        let peak = logits[original]
        let denominator = logits.reduce(Float(0)) { $0 + exp($1 - peak) }
        return BiasChoice(
            id: ids[chosen],
            overrodeBlank: overridingBlank,
            probability: exp(logits[chosen] - peak) / denominator
        )
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

/// Small LRU of tries keyed by the term list, so consecutive windows and dictations with the same
/// dictionary build the trie once per loaded model (the cache lives and dies with the vocabulary).
public final class BoostTrieCache: @unchecked Sendable {
    private let lock = NSLock()
    private var entries: [(key: String, trie: BoostTrie)] = []
    private let capacity = 4
    public init() {}

    /// Returns the trie and the build time in microseconds (0 on a cache hit).
    public func trie(for config: BoostConfig, vocabulary: [Int: String], blankID: Int) -> (BoostTrie, Int) {
        let key = config.cacheKey
        lock.lock()
        if let index = entries.firstIndex(where: { $0.key == key }) {
            let entry = entries.remove(at: index)
            entries.append(entry)
            lock.unlock()
            return (entry.trie, 0)
        }
        lock.unlock()
        let started = DispatchTime.now().uptimeNanoseconds
        let built = BoostTrie(config: config, vocabulary: vocabulary, blankID: blankID)
        let micros = Int((DispatchTime.now().uptimeNanoseconds - started) / 1000)
        lock.lock()
        entries.append((key, built))
        if entries.count > capacity { entries.removeFirst() }
        lock.unlock()
        return (built, micros)
    }
}
