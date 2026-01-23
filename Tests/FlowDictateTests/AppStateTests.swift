import XCTest
@testable import FlowDictate

final class AppStateTests: XCTestCase {

    func testIdleState() {
        let state = AppState.idle
        XCTAssertFalse(state.isRecording)
        XCTAssertFalse(state.isTranscribing)
        XCTAssertFalse(state.isBusy)
    }

    func testRecordingState() {
        let state = AppState.recording
        XCTAssertTrue(state.isRecording)
        XCTAssertFalse(state.isTranscribing)
        XCTAssertTrue(state.isBusy)
    }

    func testTranscribingState() {
        let state = AppState.transcribing
        XCTAssertFalse(state.isRecording)
        XCTAssertTrue(state.isTranscribing)
        XCTAssertTrue(state.isBusy)
    }

    func testErrorState() {
        let state = AppState.error("Test error")
        XCTAssertFalse(state.isRecording)
        XCTAssertFalse(state.isTranscribing)
        XCTAssertFalse(state.isBusy)
    }

    func testStateEquality() {
        XCTAssertEqual(AppState.idle, AppState.idle)
        XCTAssertEqual(AppState.recording, AppState.recording)
        XCTAssertEqual(AppState.transcribing, AppState.transcribing)
        XCTAssertEqual(AppState.error("test"), AppState.error("test"))
        XCTAssertNotEqual(AppState.error("a"), AppState.error("b"))
        XCTAssertNotEqual(AppState.idle, AppState.recording)
    }
}
