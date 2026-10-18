//! A fixed [`ModelEnv`] for tests of the model lists (#373): the machine's
//! answers are chosen by the test, so a list can be checked for a host that
//! runs Pianissimo (macOS, Windows on ARM) from any host.

use crate::models::{ModelEnv, WhisperModel};

/// The machine state a model list depends on, chosen by the test.
#[derive(Debug, Clone, Default)]
pub struct FixedModelEnv {
    pub pianissimo_supported: bool,
    pub pianissimo_downloaded: bool,
    pub pianissimo_download_bytes: u64,
    pub pianissimo_installed_bytes: u64,
    /// The Whisper models whose files are "on disk".
    pub downloaded: Vec<WhisperModel>,
}

impl FixedModelEnv {
    /// A host that cannot run Pianissimo and has no model on disk.
    pub fn new() -> Self {
        Self::default()
    }

    /// A host that runs Pianissimo, with an archive of 1 000 MiB that unpacks
    /// to 1 400 MiB, and nothing downloaded.
    pub fn with_pianissimo() -> Self {
        Self {
            pianissimo_supported: true,
            pianissimo_download_bytes: 1_000 * 1_048_576 + 123,
            pianissimo_installed_bytes: 1_400 * 1_048_576 + 456,
            ..Self::default()
        }
    }
}

impl ModelEnv for FixedModelEnv {
    fn runs_pianissimo(&self) -> bool {
        self.pianissimo_supported
    }

    fn pianissimo_downloaded(&self) -> bool {
        self.pianissimo_downloaded
    }

    fn pianissimo_download_bytes(&self) -> u64 {
        self.pianissimo_download_bytes
    }

    fn pianissimo_installed_bytes(&self) -> u64 {
        self.pianissimo_installed_bytes
    }

    fn whisper_downloaded(&self, model: WhisperModel) -> bool {
        self.downloaded.contains(&model)
    }
}
