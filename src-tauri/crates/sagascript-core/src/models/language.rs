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
    /// Every language, in the order the app lists them.
    pub const ALL: [Language; 5] = [
        Language::English,
        Language::Swedish,
        Language::Norwegian,
        Language::Finnish,
        Language::Auto,
    ];

    /// Stable identifier of this language: what the settings file stores, what
    /// `sagascript config` prints and the frontend sends. Equal to the serde
    /// name, which a test pins.
    pub fn id(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Swedish => "sv",
            Language::Norwegian => "no",
            Language::Finnish => "fi",
            Language::Auto => "auto",
        }
    }

    /// The language with this stable identifier, if any. Only the identifier:
    /// [`FromStr`](std::str::FromStr) also takes the language's English name.
    pub fn from_id(id: &str) -> Option<Language> {
        Language::ALL
            .into_iter()
            .find(|language| language.id() == id)
    }

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

/// A string that names no [`Language`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownLanguage(pub String);

impl std::fmt::Display for UnknownLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<&str> = Language::ALL.iter().map(|language| language.id()).collect();
        write!(
            f,
            "Unknown language '{}'. Valid: {}",
            self.0,
            ids.join(", ")
        )
    }
}

impl std::error::Error for UnknownLanguage {}

/// The spellings of a language that the command line accepts, for every flag and
/// every `sagascript config` value (#373): the identifier (`sv`) or the
/// language's lower-case English name (`swedish`). Case matters.
///
/// The settings file keeps to the identifier (serde); a name is only ever typed.
impl std::str::FromStr for Language {
    type Err = UnknownLanguage;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "english" => Ok(Language::English),
            "swedish" => Ok(Language::Swedish),
            "norwegian" => Ok(Language::Norwegian),
            "finnish" => Ok(Language::Finnish),
            id => Language::from_id(id).ok_or_else(|| UnknownLanguage(text.to_owned())),
        }
    }
}

#[cfg(test)]
#[path = "language_tests.rs"]
mod tests;
