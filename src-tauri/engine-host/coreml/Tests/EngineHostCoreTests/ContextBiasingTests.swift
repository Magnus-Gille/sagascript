import Testing
@testable import SagascriptEngineHostCore

struct ContextBiasingTests {
    // ids: 0 ▁Mag, 1 nus, 2 ▁m, 3 ag, 4 ▁hej, 5 ▁Gil, 6 le, 7 blank
    private let vocabulary: [Int: String] = [0: "▁Mag", 1: "nus", 2: "▁M", 3: "ag", 4: "▁hej", 5: "▁Gil", 6: "le"]

    private func trie(_ terms: [String], weight: Float = 2) -> BoostTrie {
        BoostTrie(config: BoostConfig(terms: terms, weight: weight), vocabulary: vocabulary, blankID: 7)
    }

    private func candidates(_ pairs: [(Int, Float)]) -> (ids: [Int], logits: [Float]) {
        (pairs.map { $0.0 }, pairs.map { $0.1 })
    }

    @Test func segmentationFindsPieces() {
        let pieces = ["▁Mag": 0, "nus": 1]
        #expect(segmentWord("Magnus", pieces: pieces) == [[0, 1]])
        #expect(segmentWord("zzz", pieces: pieces).isEmpty)
    }

    @Test func emptyDictionaryNeverChangesTheChoice() {
        var state = BiasState(trie: trie([]))
        let c = candidates([(4, 1.0), (0, 0.9)])
        #expect(state.choose(ids: c.ids, logits: c.logits, frame: 0) == nil)
    }

    @Test func bonusFlipsCloseCallButNotClearOne() {
        var state = BiasState(trie: trie(["Magnus"]), startFactor: 1)
        var c = candidates([(7, 3.0), (0, 1.5)])
        #expect(state.choose(ids: c.ids, logits: c.logits, frame: 0)?.id == 0)
        c = candidates([(7, 8.0), (0, 3.0)])
        #expect(state.choose(ids: c.ids, logits: c.logits, frame: 0) == nil)
    }

    @Test func continuationBoostedOnlyAfterPrefix() {
        var state = BiasState(trie: trie(["Magnus"]), startFactor: 0)
        let c = candidates([(7, 1.0), (1, 0.0)])
        #expect(state.choose(ids: c.ids, logits: c.logits, frame: 0) == nil)
        state.emitted(token: 0, frame: 0)
        #expect(state.choose(ids: c.ids, logits: c.logits, frame: 1)?.id == 1)
    }

    @Test func abandonedAndStalePartialMatchesAreDropped() {
        var state = BiasState(trie: trie(["Magnus"]), startFactor: 0)
        let c = candidates([(7, 1.0), (1, 0.0)])
        state.emitted(token: 0, frame: 0)
        state.emitted(token: 4, frame: 1)
        #expect(state.choose(ids: c.ids, logits: c.logits, frame: 2) == nil)
        state.emitted(token: 0, frame: 2)
        #expect(state.choose(ids: c.ids, logits: c.logits, frame: 40) == nil)
    }

    @Test func multiplePartialMatchesAreTracked() {
        var state = BiasState(trie: trie(["Magnus", "Mag Gille"]), startFactor: 0)
        state.emitted(token: 0, frame: 0)
        var c = candidates([(7, 1.0), (1, 0.0)])
        #expect(state.choose(ids: c.ids, logits: c.logits, frame: 1)?.id == 1)
        c = candidates([(7, 1.0), (5, 0.0)])
        #expect(state.choose(ids: c.ids, logits: c.logits, frame: 1)?.id == 5)
    }

    @Test func boundedTermsAreSkipped() {
        let long = String(repeating: "a", count: BoostConfig.maxTermLength + 1)
        #expect(trie([long, "Qzx"]).isEmpty)
    }
}
