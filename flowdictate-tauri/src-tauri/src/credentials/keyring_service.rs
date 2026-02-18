use tracing::{error, info};

const SERVICE_NAME: &str = "com.flowdictate.openai-api-key";
const ACCOUNT: &str = "openai";

/// Cross-platform credential storage using OS keychain
/// macOS: Keychain, Windows: Credential Manager
#[derive(Clone)]
pub struct KeyringService;

impl KeyringService {
    pub fn new() -> Self {
        Self
    }

    /// Save API key to OS credential store
    pub fn save_api_key(&self, key: &str) -> bool {
        let entry = match keyring::Entry::new(SERVICE_NAME, ACCOUNT) {
            Ok(e) => e,
            Err(e) => {
                error!("Failed to create keyring entry: {e}");
                return false;
            }
        };

        match entry.set_password(key) {
            Ok(()) => {
                info!("API key saved to keyring");
                true
            }
            Err(e) => {
                error!("Failed to save API key: {e}");
                false
            }
        }
    }

    /// Retrieve API key from OS credential store
    pub fn get_api_key(&self) -> Option<String> {
        let entry = keyring::Entry::new(SERVICE_NAME, ACCOUNT).ok()?;
        match entry.get_password() {
            Ok(key) => Some(key),
            Err(keyring::Error::NoEntry) => None,
            Err(e) => {
                error!("Failed to get API key: {e}");
                None
            }
        }
    }

    /// Delete API key from OS credential store
    pub fn delete_api_key(&self) -> bool {
        let entry = match keyring::Entry::new(SERVICE_NAME, ACCOUNT) {
            Ok(e) => e,
            Err(_) => return true,
        };

        match entry.delete_credential() {
            Ok(()) => {
                info!("API key deleted from keyring");
                true
            }
            Err(keyring::Error::NoEntry) => true,
            Err(e) => {
                error!("Failed to delete API key: {e}");
                false
            }
        }
    }

    /// Check if API key exists
    pub fn has_api_key(&self) -> bool {
        self.get_api_key().is_some()
    }
}
