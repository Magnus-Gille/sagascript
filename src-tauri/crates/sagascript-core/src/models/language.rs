use serde::{Deserialize, Serialize};

/// Supported transcription languages
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[serde(rename = "en")]
    #[default]
    English,
    #[serde(rename = "sv")]
    Swedish,
    #[serde(rename = "no")]
    Norwegian,
    #[serde(rename = "fi")]
    Finnish,
    #[serde(rename = "auto")]
    Auto,
}

impl Language {
    pub fn display_name(&self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Swedish => "Swedish",
            Language::Norwegian => "Norwegian",
            Language::Finnish => "Finnish",
            Language::Auto => "Auto-detect",
        }
    }

    /// Whisper language code (None for auto-detect)
    pub fn whisper_code(&self) -> Option<&'static str> {
        match self {
            Language::English => Some("en"),
            Language::Swedish => Some("sv"),
            Language::Norwegian => Some("no"),
            Language::Finnish => Some("fi"),
            Language::Auto => None,
        }
    }
}

#[cfg(test)]
#[path = "language_tests.rs"]
mod tests;
