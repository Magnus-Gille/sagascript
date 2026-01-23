import XCTest
@testable import FlowDictate

@MainActor
final class SettingsManagerTests: XCTestCase {
    private var settingsManager: SettingsManager!

    override func setUp() async throws {
        try await super.setUp()
        settingsManager = SettingsManager.shared
        settingsManager.resetToDefaults()
    }

    func testDefaultValues() {
        XCTAssertEqual(settingsManager.language, .english)
        XCTAssertEqual(settingsManager.backend, .local)
        XCTAssertEqual(settingsManager.hotkeyMode, .pushToTalk)
        XCTAssertTrue(settingsManager.showOverlay)
    }

    func testLanguageChange() {
        // When
        settingsManager.language = .swedish

        // Then
        XCTAssertEqual(settingsManager.language, .swedish)
    }

    func testBackendChange() {
        // When
        settingsManager.backend = .remote

        // Then
        XCTAssertEqual(settingsManager.backend, .remote)
    }

    func testHotkeyModeChange() {
        // When
        settingsManager.hotkeyMode = .toggle

        // Then
        XCTAssertEqual(settingsManager.hotkeyMode, .toggle)
    }

    func testShowOverlayChange() {
        // When
        settingsManager.showOverlay = false

        // Then
        XCTAssertFalse(settingsManager.showOverlay)
    }

    func testResetToDefaults() {
        // Given
        settingsManager.language = .swedish
        settingsManager.backend = .remote
        settingsManager.hotkeyMode = .toggle
        settingsManager.showOverlay = false

        // When
        settingsManager.resetToDefaults()

        // Then
        XCTAssertEqual(settingsManager.language, .english)
        XCTAssertEqual(settingsManager.backend, .local)
        XCTAssertEqual(settingsManager.hotkeyMode, .pushToTalk)
        XCTAssertTrue(settingsManager.showOverlay)
    }

    func testHotkeyDescription() {
        // Default is Control+Shift+Space
        XCTAssertTrue(settingsManager.hotkeyDescription.contains("Space"))
        XCTAssertTrue(settingsManager.hotkeyDescription.contains("⌃"))  // Control
        XCTAssertTrue(settingsManager.hotkeyDescription.contains("⇧"))  // Shift
    }
}
