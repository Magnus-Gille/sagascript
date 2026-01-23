import XCTest
@testable import FlowDictate

final class DictationErrorTests: XCTestCase {

    func testMicrophonePermissionError() {
        let error = DictationError.microphonePermissionDenied
        XCTAssertNotNil(error.errorDescription)
        XCTAssertTrue(error.errorDescription!.contains("Microphone"))
    }

    func testAccessibilityPermissionError() {
        let error = DictationError.accessibilityPermissionDenied
        XCTAssertNotNil(error.errorDescription)
        XCTAssertTrue(error.errorDescription!.contains("Accessibility"))
    }

    func testModelNotLoadedError() {
        let error = DictationError.modelNotLoaded
        XCTAssertNotNil(error.errorDescription)
        XCTAssertTrue(error.errorDescription!.contains("model"))
    }

    func testTranscriptionFailedError() {
        let error = DictationError.transcriptionFailed("Test failure")
        XCTAssertNotNil(error.errorDescription)
        XCTAssertTrue(error.errorDescription!.contains("Test failure"))
    }

    func testNoAudioCapturedError() {
        let error = DictationError.noAudioCaptured
        XCTAssertNotNil(error.errorDescription)
        XCTAssertTrue(error.errorDescription!.contains("audio"))
    }

    func testAPIKeyMissingError() {
        let error = DictationError.apiKeyMissing
        XCTAssertNotNil(error.errorDescription)
        XCTAssertTrue(error.errorDescription!.contains("API key"))
    }

    func testNetworkError() {
        let error = DictationError.networkError("Connection refused")
        XCTAssertNotNil(error.errorDescription)
        XCTAssertTrue(error.errorDescription!.contains("Connection refused"))
    }
}
