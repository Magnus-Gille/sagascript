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
}
