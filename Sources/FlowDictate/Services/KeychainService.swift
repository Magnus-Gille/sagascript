import Foundation
import Security
import os.log

/// Service for securely storing and retrieving API keys from macOS Keychain
final class KeychainService {
    // MARK: - Constants

    private let service = "com.flowdictate.openai-api-key"
    private let account = "openai"

    // MARK: - Singleton

    static let shared = KeychainService()

    // MARK: - Private State

    private let logger = Logger(subsystem: "com.flowdictate", category: "Keychain")

    // MARK: - Initialization

    private init() {}

    // MARK: - Public Methods

    /// Save API key to Keychain
    /// - Parameter key: The API key to store
    /// - Returns: True if successful
    @discardableResult
    func saveAPIKey(_ key: String) -> Bool {
        guard let keyData = key.data(using: .utf8) else {
            return false
        }

        // Delete existing key first
        deleteAPIKey()

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecValueData as String: keyData,
            // Only accessible when device is unlocked, not synced to other devices
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        ]

        let status = SecItemAdd(query as CFDictionary, nil)

        if status == errSecSuccess {
            logger.info("API key saved to Keychain")
            return true
        } else {
            logger.error("Failed to save API key: \(status)")
            return false
        }
    }

    /// Retrieve API key from Keychain
    /// - Returns: The API key if found
    func getAPIKey() -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]

        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)

        guard status == errSecSuccess,
              let keyData = result as? Data,
              let key = String(data: keyData, encoding: .utf8) else {
            return nil
        }

        // Never log the actual key value
        logger.debug("API key retrieved from Keychain")
        return key
    }

    /// Delete API key from Keychain
    /// - Returns: True if successful or key didn't exist
    @discardableResult
    func deleteAPIKey() -> Bool {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account
        ]

        let status = SecItemDelete(query as CFDictionary)

        if status == errSecSuccess || status == errSecItemNotFound {
            logger.info("API key deleted from Keychain")
            return true
        } else {
            logger.error("Failed to delete API key: \(status)")
            return false
        }
    }

    /// Check if API key exists in Keychain
    var hasAPIKey: Bool {
        getAPIKey() != nil
    }
}
