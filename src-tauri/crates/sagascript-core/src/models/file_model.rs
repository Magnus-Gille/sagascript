use super::whisper::UnknownModelId;
use super::{Language, PianissimoModel, WhisperModel};

/// Shown wherever Pianissimo is refused for platform reasons.
pub const PIANISSIMO_UNSUPPORTED_MESSAGE: &str =
    "Pianissimo requires macOS 14+ on Apple Silicon or Windows on ARM (Snapdragon)";

/// A file-transcription model choice. Unlike [`WhisperModel`], this can also
/// select a Pianissimo model, each of which serves only its own language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileModel {
    Whisper(WhisperModel),
    Pianissimo(PianissimoModel),
}

impl FileModel {
    /// Stable identifier of this choice: the Whisper model's [`WhisperModel::id`]
    /// or the Pianissimo model's [`PianissimoModel::id`].
    pub fn id(self) -> &'static str {
        match self {
            FileModel::Whisper(model) => model.id(),
            FileModel::Pianissimo(model) => model.id(),
        }
    }

    /// The choice with this stable identifier, if any.
    pub fn from_id(id: &str) -> Option<FileModel> {
        PianissimoModel::from_id(id)
            .map(FileModel::Pianissimo)
            .or_else(|| WhisperModel::from_id(id).map(FileModel::Whisper))
    }

    /// Whether this model can transcribe `language`: Pianissimo only its own,
    /// Whisper per [`WhisperModel::is_compatible_with`].
    pub fn is_compatible_with(self, language: Language) -> bool {
        match self {
            FileModel::Whisper(model) => model.is_compatible_with(language),
            FileModel::Pianissimo(model) => model.language() == language,
        }
    }

    /// The engine behind "Recommended for <language>": the language's
    /// Pianissimo model where Pianissimo runs (`pianissimo_supported`),
    /// otherwise the language's recommended Whisper model. Single source of
    /// truth for every automatic resolution (dictation profiles, file
    /// transcription, CLI, UI labels).
    pub fn recommended_for(language: Language, pianissimo_supported: bool) -> FileModel {
        match PianissimoModel::for_language(language) {
            Some(model) if pianissimo_supported => FileModel::Pianissimo(model),
            _ => FileModel::Whisper(WhisperModel::recommended(language)),
        }
    }

    /// Models the app offers for `language`, after the "Recommended" entry.
    /// Swedish: Pianissimo + KB-Whisper Large where Pianissimo runs, otherwise
    /// KB-Whisper Medium + Large. Danish: Pianissimo where it runs, then the
    /// multilingual Whisper models. `include` keeps an already-selected model
    /// (e.g. a legacy KB-Whisper Base) selectable instead of hiding the
    /// current choice.
    pub fn app_lineup(
        language: Language,
        pianissimo_supported: bool,
        include: Option<FileModel>,
    ) -> Vec<FileModel> {
        let pianissimo = PianissimoModel::for_language(language)
            .filter(|_| pianissimo_supported)
            .map(FileModel::Pianissimo);
        let mut models: Vec<FileModel> = if language == Language::Swedish {
            if pianissimo_supported {
                vec![
                    FileModel::Pianissimo(PianissimoModel::Swedish),
                    FileModel::Whisper(WhisperModel::KbWhisperLarge),
                ]
            } else {
                vec![
                    FileModel::Whisper(WhisperModel::KbWhisperMedium),
                    FileModel::Whisper(WhisperModel::KbWhisperLarge),
                ]
            }
        } else {
            pianissimo
                .into_iter()
                .chain(
                    WhisperModel::models_for_language(language)
                        .iter()
                        .copied()
                        .map(FileModel::Whisper),
                )
                .collect()
        };
        if let Some(extra) = include {
            let valid = extra.is_compatible_with(language)
                && (pianissimo_supported || !matches!(extra, FileModel::Pianissimo(_)));
            if valid && !models.contains(&extra) {
                models.push(extra);
            }
        }
        models
    }
}

impl std::str::FromStr for FileModel {
    type Err = UnknownModelId;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        FileModel::from_id(id).ok_or_else(|| UnknownModelId(id.to_owned()))
    }
}

#[cfg(test)]
#[path = "file_model_tests.rs"]
mod tests;
