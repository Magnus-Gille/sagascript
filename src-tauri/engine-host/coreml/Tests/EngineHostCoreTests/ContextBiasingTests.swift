import Foundation
import Testing
@testable import SagascriptEngineHostCore

/// Shares `engine-host/test-vectors/context-biasing.json` with the ONNX host's Rust tests, so the
/// two implementations cannot drift apart silently.
struct ContextBiasingTests {
    private struct Vectors {
        let vocabulary: [Int: String]
        let blankID: Int
        let topK: Int
        let defaultLogit: Float
        let root: [String: Any]
    }

    private static func load() throws -> Vectors {
        // .../engine-host/coreml/Tests/EngineHostCoreTests/<this file>
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<4 { url.deleteLastPathComponent() }
        url.appendPathComponent("test-vectors/context-biasing.json")
        let object = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
        let pieces = object["vocab"] as! [String]
        var vocabulary: [Int: String] = [:]
        for (id, piece) in pieces.enumerated() { vocabulary[id] = piece }
        return Vectors(
            vocabulary: vocabulary,
            blankID: object["blank_id"] as! Int,
            topK: object["top_k"] as! Int,
            defaultLogit: (object["default_logit"] as! NSNumber).floatValue,
            root: object
        )
    }

    private func trie(_ terms: [String], _ vectors: Vectors, weight: Float = 2) -> BoostTrie {
        BoostTrie(config: BoostConfig(terms: terms, weight: weight), vocabulary: vectors.vocabulary, blankID: vectors.blankID)
    }

    @Test func sharedVectorsTrieConstruction() throws {
        let vectors = try Self.load()
        #expect(vectors.topK == BoostConfig.topK)
        for item in vectors.root["trie_cases"] as! [[String: Any]] {
            let terms = item["terms"] as! [String]
            let expected = item["phrases"] as! [[Int]]
            #expect(trie(terms, vectors).phrases() == expected, "trie case \(item["name"] as! String)")
        }
    }

    @Test func sharedVectorsChoice() throws {
        let vectors = try Self.load()
        let vocabularySize = vectors.vocabulary.count
        for item in vectors.root["choose_cases"] as! [[String: Any]] {
            let name = item["name"] as! String
            let params = item["params"] as! [String: Any]
            var biasParams = BiasParams(weight: (params["weight"] as! NSNumber).floatValue)
            biasParams.startFactor = (params["start_factor"] as! NSNumber).floatValue
            biasParams.blankMargin = (params["blank_margin"] as! NSNumber).floatValue
            biasParams.maxGapFrames = params["max_gap_frames"] as! Int
            let built = trie(item["terms"] as! [String], vectors, weight: biasParams.weight)
            var state = BiasState(trie: built, params: biasParams, blankID: vectors.blankID)
            for (index, step) in (item["steps"] as! [[String: Any]]).enumerated() {
                if let emit = step["emit"] as? [String: Any] {
                    state.emitted(token: emit["token"] as! Int, frame: emit["frame"] as! Int)
                }
                var dense = [Float](repeating: vectors.defaultLogit, count: vocabularySize)
                for (key, value) in step["logits"] as! [String: Any] { dense[Int(key)!] = (value as! NSNumber).floatValue }
                // What the Core ML joint returns: the top-K logits, highest first, ties to the lower id.
                let order = (0..<vocabularySize).sorted { dense[$0] != dense[$1] ? dense[$0] > dense[$1] : $0 < $1 }.prefix(vectors.topK)
                let pick = state.choose(ids: Array(order), logits: order.map { dense[$0] }, frame: step["frame"] as! Int)
                if let expected = step["expected"] as? [String: Any] {
                    #expect(pick?.id == expected["id"] as? Int, "\(name) step \(index)")
                    #expect(pick?.overrodeBlank == expected["overrode_blank"] as? Bool, "\(name) step \(index)")
                } else {
                    #expect(pick == nil, "\(name) step \(index)")
                }
            }
        }
    }

    @Test func confidenceIsTheRawTopKSoftmaxOfTheChosenToken() throws {
        let vectors = try Self.load()
        var params = BiasParams(weight: 2)
        params.startFactor = 1
        var state = BiasState(trie: trie(["Magnus"], vectors), params: params, blankID: vectors.blankID)
        // Raw logits 3.0 (blank) and 1.5 (term): bonus flips the choice, probability stays unbiased.
        let pick = state.choose(ids: [80, 0], logits: [3.0, 1.5], frame: 0)
        let expected = exp(Float(1.5 - 3.0)) / (1 + exp(Float(1.5 - 3.0)))
        #expect(abs((pick?.probability ?? 0) - expected) < 1e-6)
    }

    @Test func segmentationFindsPieces() {
        let pieces = ["▁Mag": 0, "nus": 1]
        #expect(segmentWord("Magnus", pieces: pieces) == [[0, 1]])
        #expect(segmentWord("zzz", pieces: pieces).isEmpty)
    }

    @Test func trieCacheReusesTheTrieForTheSameTerms() throws {
        let vectors = try Self.load()
        let cache = BoostTrieCache()
        let config = BoostConfig(terms: ["Magnus"], weight: 2)
        let first = cache.trie(for: config, vocabulary: vectors.vocabulary, blankID: vectors.blankID)
        let second = cache.trie(for: BoostConfig(terms: ["Magnus"], weight: 9), vocabulary: vectors.vocabulary, blankID: vectors.blankID)
        #expect(first.0 === second.0)
        #expect(second.1 == 0)
        let other = cache.trie(for: BoostConfig(terms: ["Gille"], weight: 2), vocabulary: vectors.vocabulary, blankID: vectors.blankID)
        #expect(other.0 !== first.0)
    }

    @Test func nodeCapBoundsTheTrie() {
        var vocabulary: [Int: String] = [0: "▁a"]
        for i in 1..<200 { vocabulary[i] = "x\(i)" }
        let terms = (0..<200).map { "a" + String(repeating: "x\($0 % 199 + 1)", count: 1) }
        let built = BoostTrie(config: BoostConfig(terms: terms, weight: 1), vocabulary: vocabulary, blankID: 199)
        #expect(built.nodeCount <= BoostConfig.maxNodes)
    }

    @Test func helloAdvertisesBiasingAndResultsReportItOnlyWhenRequested() {
        #expect(HostCapabilities().jsonObject()["context_biasing"] as? Bool == true)
        var result = WindowResult(tokens: [], audioSeconds: 1, preprocessMilliseconds: 0, encodeMilliseconds: 0, decodeMilliseconds: 0)
        #expect(result.jsonObject()["boost_active"] == nil)
        #expect((result.jsonObject()["timings"] as? [String: Any])?["boost_us"] == nil)
        result.boostActive = false
        #expect(result.jsonObject()["boost_active"] as? Bool == false)
    }
}
