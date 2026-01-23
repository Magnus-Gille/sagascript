import XCTest
@testable import FlowDictate

final class KeychainServiceTests: XCTestCase {
    private var keychainService: KeychainService!
    private let testKey = "sk-test-key-12345"

    override func setUp() {
        super.setUp()
        keychainService = KeychainService.shared
        // Clean up before each test
        keychainService.deleteAPIKey()
    }

    override func tearDown() {
        // Clean up after each test
        keychainService.deleteAPIKey()
        super.tearDown()
    }

    func testSaveAndRetrieveAPIKey() {
        // Given
        XCTAssertNil(keychainService.getAPIKey())

        // When
        let saved = keychainService.saveAPIKey(testKey)

        // Then
        XCTAssertTrue(saved)
        XCTAssertEqual(keychainService.getAPIKey(), testKey)
    }

    func testHasAPIKey() {
        // Given
        XCTAssertFalse(keychainService.hasAPIKey)

        // When
        keychainService.saveAPIKey(testKey)

        // Then
        XCTAssertTrue(keychainService.hasAPIKey)
    }

    func testDeleteAPIKey() {
        // Given
        keychainService.saveAPIKey(testKey)
        XCTAssertTrue(keychainService.hasAPIKey)

        // When
        let deleted = keychainService.deleteAPIKey()

        // Then
        XCTAssertTrue(deleted)
        XCTAssertNil(keychainService.getAPIKey())
    }

    func testUpdateAPIKey() {
        // Given
        let newKey = "sk-new-key-67890"
        keychainService.saveAPIKey(testKey)

        // When
        keychainService.saveAPIKey(newKey)

        // Then
        XCTAssertEqual(keychainService.getAPIKey(), newKey)
    }

    func testDeleteNonExistentKey() {
        // Given
        XCTAssertFalse(keychainService.hasAPIKey)

        // When
        let deleted = keychainService.deleteAPIKey()

        // Then
        XCTAssertTrue(deleted) // Should succeed even if key doesn't exist
    }
}
