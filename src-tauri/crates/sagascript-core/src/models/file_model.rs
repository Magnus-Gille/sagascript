use super::whisper::UnknownModelId;
use super::{Language, WhisperModel};

/// A file-transcription model choice. Unlike [`WhisperModel`], this can also
/// select the Swedish-only Pianissimo Original model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileModel {
    Whisper(WhisperModel),
    PianissimoOriginal,
}

impl FileModel {
    /// Identifier of [`FileModel::PianissimoOriginal`] in the settings file, the
    /// CLI and the frontend.
    pub const PIANISSIMO_ID: &'static str = "pianissimo-sv";

    /// Stable identifier of this choice: the Whisper model's [`WhisperModel::id`]
    /// or [`Self::PIANISSIMO_ID`].
    pub fn id(self) -> &'static str {
        match self {
            FileModel::Whisper(model) => model.id(),
            FileModel::PianissimoOriginal => Self::PIANISSIMO_ID,
        }
    }

    /// The choice with this stable identifier, if any.
    pub fn from_id(id: &str) -> Option<FileModel> {
        if id == Self::PIANISSIMO_ID {
            Some(FileModel::PianissimoOriginal)
        } else {
            WhisperModel::from_id(id).map(FileModel::Whisper)
        }
    }

    /// The engine behind "Recommended for <language>": Pianissimo for Swedish
    /// where it runs (`pianissimo_supported`), otherwise the language's
    /// recommended Whisper model. Single source of truth for every automatic
    /// resolution (dictation profiles, file transcription, CLI, UI labels).
    pub fn recommended_for(language: Language, pianissimo_supported: bool) -> FileModel {
        if language == Language::Swedish && pianissimo_supported {
            FileModel::PianissimoOriginal
        } else {
            FileModel::Whisper(WhisperModel::recommended(language))
        }
    }

    /// Models the app offers for `language`, after the "Recommended" entry.
    /// Swedish: Pianissimo + KB-Whisper Large where Pianissimo runs, otherwise
    /// KB-Whisper Medium + Large. `include` keeps an already-selected model
    /// (e.g. a legacy KB-Whisper Base) selectable instead of hiding the
    /// current choice.
    pub fn app_lineup(
        language: Language,
        pianissimo_supported: bool,
        include: Option<FileModel>,
    ) -> Vec<FileModel> {
        let mut models: Vec<FileModel> = if language == Language::Swedish {
            if pianissimo_supported {
                vec![
                    FileModel::PianissimoOriginal,
                    FileModel::Whisper(WhisperModel::KbWhisperLarge),
                ]
            } else {
                vec![
                    FileModel::Whisper(WhisperModel::KbWhisperMedium),
                    FileModel::Whisper(WhisperModel::KbWhisperLarge),
                ]
            }
        } else {
            WhisperModel::models_for_language(language)
                .iter()
                .copied()
                .map(FileModel::Whisper)
                .collect()
        };
        if let Some(extra) = include {
            let valid = match extra {
                FileModel::Whisper(model) => model.is_compatible_with(language),
                FileModel::PianissimoOriginal => {
                    language == Language::Swedish && pianissimo_supported
                }
            };
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
