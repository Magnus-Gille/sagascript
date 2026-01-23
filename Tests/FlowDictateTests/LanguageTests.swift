import XCTest
@testable import FlowDictate

final class LanguageTests: XCTestCase {

    func testLanguageDisplayNames() {
        XCTAssertEqual(Language.english.displayName, "English")
        XCTAssertEqual(Language.swedish.displayName, "Swedish")
        XCTAssertEqual(Language.auto.displayName, "Auto-detect")
    }

    func testLanguageWhisperCodes() {
        XCTAssertEqual(Language.english.whisperCode, "en")
        XCTAssertEqual(Language.swedish.whisperCode, "sv")
        XCTAssertNil(Language.auto.whisperCode)
    }

    func testLanguageAllCases() {
        XCTAssertEqual(Language.allCases.count, 3)
        XCTAssertTrue(Language.allCases.contains(.english))
        XCTAssertTrue(Language.allCases.contains(.swedish))
        XCTAssertTrue(Language.allCases.contains(.auto))
    }

    func testTranscriptionBackendDisplayNames() {
        XCTAssertEqual(TranscriptionBackend.local.displayName, "Local (WhisperKit)")
        XCTAssertEqual(TranscriptionBackend.remote.displayName, "Remote (OpenAI)")
    }

    func testHotkeyModeDisplayNames() {
        XCTAssertEqual(HotkeyMode.pushToTalk.displayName, "Push-to-talk")
        XCTAssertEqual(HotkeyMode.toggle.displayName, "Toggle")
    }

    // MARK: - WhisperModel Tests

    func testWhisperModelAllCases() {
        // Should include both standard and Swedish-optimized models
        XCTAssertEqual(WhisperModel.allCases.count, 7)
        XCTAssertTrue(WhisperModel.allCases.contains(.tinyEn))
        XCTAssertTrue(WhisperModel.allCases.contains(.tiny))
        XCTAssertTrue(WhisperModel.allCases.contains(.baseEn))
        XCTAssertTrue(WhisperModel.allCases.contains(.base))
        XCTAssertTrue(WhisperModel.allCases.contains(.kbWhisperTiny))
        XCTAssertTrue(WhisperModel.allCases.contains(.kbWhisperBase))
        XCTAssertTrue(WhisperModel.allCases.contains(.kbWhisperSmall))
    }

    func testWhisperModelEnglishOnly() {
        // English-only models
        XCTAssertTrue(WhisperModel.tinyEn.isEnglishOnly)
        XCTAssertTrue(WhisperModel.baseEn.isEnglishOnly)

        // Multilingual models
        XCTAssertFalse(WhisperModel.tiny.isEnglishOnly)
        XCTAssertFalse(WhisperModel.base.isEnglishOnly)

        // KB-Whisper models (multilingual/Swedish)
        XCTAssertFalse(WhisperModel.kbWhisperTiny.isEnglishOnly)
        XCTAssertFalse(WhisperModel.kbWhisperBase.isEnglishOnly)
        XCTAssertFalse(WhisperModel.kbWhisperSmall.isEnglishOnly)
    }

    func testWhisperModelSwedishOptimized() {
        // Standard models are NOT Swedish-optimized
        XCTAssertFalse(WhisperModel.tinyEn.isSwedishOptimized)
        XCTAssertFalse(WhisperModel.tiny.isSwedishOptimized)
        XCTAssertFalse(WhisperModel.baseEn.isSwedishOptimized)
        XCTAssertFalse(WhisperModel.base.isSwedishOptimized)

        // KB-Whisper models ARE Swedish-optimized
        XCTAssertTrue(WhisperModel.kbWhisperTiny.isSwedishOptimized)
        XCTAssertTrue(WhisperModel.kbWhisperBase.isSwedishOptimized)
        XCTAssertTrue(WhisperModel.kbWhisperSmall.isSwedishOptimized)
    }

    func testWhisperModelRequiresLocalPath() {
        // Standard models don't require local path (downloaded from HuggingFace)
        XCTAssertFalse(WhisperModel.tinyEn.requiresLocalPath)
        XCTAssertFalse(WhisperModel.tiny.requiresLocalPath)
        XCTAssertFalse(WhisperModel.baseEn.requiresLocalPath)
        XCTAssertFalse(WhisperModel.base.requiresLocalPath)

        // KB-Whisper models require local path
        XCTAssertTrue(WhisperModel.kbWhisperTiny.requiresLocalPath)
        XCTAssertTrue(WhisperModel.kbWhisperBase.requiresLocalPath)
        XCTAssertTrue(WhisperModel.kbWhisperSmall.requiresLocalPath)
    }

    func testWhisperModelLocalPath() {
        // Standard models have no local path
        XCTAssertNil(WhisperModel.tinyEn.localModelPath)
        XCTAssertNil(WhisperModel.tiny.localModelPath)
        XCTAssertNil(WhisperModel.baseEn.localModelPath)
        XCTAssertNil(WhisperModel.base.localModelPath)

        // KB-Whisper models have local paths
        XCTAssertNotNil(WhisperModel.kbWhisperTiny.localModelPath)
        XCTAssertNotNil(WhisperModel.kbWhisperBase.localModelPath)
        XCTAssertNotNil(WhisperModel.kbWhisperSmall.localModelPath)

        // Verify path structure
        let basePath = WhisperModel.kbWhisperBase.localModelPath!
        XCTAssertTrue(basePath.path.contains("FlowDictate/Models"))
        XCTAssertTrue(basePath.path.contains("kblab_kb-whisper-base"))
    }

    func testWhisperModelRecommendedForLanguage() {
        // English should recommend English-only model
        XCTAssertEqual(WhisperModel.recommendedModel(for: .english), .baseEn)

        // Swedish should recommend Swedish-optimized model
        XCTAssertEqual(WhisperModel.recommendedModel(for: .swedish), .kbWhisperBase)

        // Auto-detect should recommend multilingual model
        XCTAssertEqual(WhisperModel.recommendedModel(for: .auto), .base)
    }

    func testWhisperModelStandardAndSwedishLists() {
        // Standard models list
        let standardModels = WhisperModel.standardModels
        XCTAssertEqual(standardModels.count, 4)
        XCTAssertTrue(standardModels.contains(.tinyEn))
        XCTAssertTrue(standardModels.contains(.tiny))
        XCTAssertTrue(standardModels.contains(.baseEn))
        XCTAssertTrue(standardModels.contains(.base))

        // Swedish models list
        let swedishModels = WhisperModel.swedishModels
        XCTAssertEqual(swedishModels.count, 3)
        XCTAssertTrue(swedishModels.contains(.kbWhisperTiny))
        XCTAssertTrue(swedishModels.contains(.kbWhisperBase))
        XCTAssertTrue(swedishModels.contains(.kbWhisperSmall))
    }

    func testWhisperModelParameterCounts() {
        // Standard models
        XCTAssertEqual(WhisperModel.tinyEn.parameterCount, 39)
        XCTAssertEqual(WhisperModel.tiny.parameterCount, 39)
        XCTAssertEqual(WhisperModel.baseEn.parameterCount, 74)
        XCTAssertEqual(WhisperModel.base.parameterCount, 74)

        // KB-Whisper models
        XCTAssertEqual(WhisperModel.kbWhisperTiny.parameterCount, 58)
        XCTAssertEqual(WhisperModel.kbWhisperBase.parameterCount, 99)
        XCTAssertEqual(WhisperModel.kbWhisperSmall.parameterCount, 300)
    }

    func testWhisperModelDisplayNames() {
        XCTAssertEqual(WhisperModel.kbWhisperTiny.displayName, "KB-Whisper Tiny (Swedish)")
        XCTAssertEqual(WhisperModel.kbWhisperBase.displayName, "KB-Whisper Base (Swedish)")
        XCTAssertEqual(WhisperModel.kbWhisperSmall.displayName, "KB-Whisper Small (Swedish)")
    }

    func testWhisperModelGgmlProperties() {
        // Standard models have no GGML properties
        XCTAssertEqual(WhisperModel.tinyEn.ggmlFilename, "")
        XCTAssertNil(WhisperModel.tinyEn.ggmlDownloadURL)
        XCTAssertEqual(WhisperModel.base.ggmlSizeMB, 0)

        // KB-Whisper models have GGML properties
        XCTAssertEqual(WhisperModel.kbWhisperTiny.ggmlFilename, "kb-whisper-tiny-q5_0.bin")
        XCTAssertEqual(WhisperModel.kbWhisperBase.ggmlFilename, "kb-whisper-base-q5_0.bin")
        XCTAssertEqual(WhisperModel.kbWhisperSmall.ggmlFilename, "kb-whisper-small-q5_0.bin")

        // Download URLs should be valid
        XCTAssertNotNil(WhisperModel.kbWhisperTiny.ggmlDownloadURL)
        XCTAssertNotNil(WhisperModel.kbWhisperBase.ggmlDownloadURL)
        XCTAssertNotNil(WhisperModel.kbWhisperSmall.ggmlDownloadURL)

        // Size estimates
        XCTAssertEqual(WhisperModel.kbWhisperTiny.ggmlSizeMB, 40)
        XCTAssertEqual(WhisperModel.kbWhisperBase.ggmlSizeMB, 60)
        XCTAssertEqual(WhisperModel.kbWhisperSmall.ggmlSizeMB, 190)
    }
}
