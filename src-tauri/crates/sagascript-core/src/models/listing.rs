//! What the model lists show: `sagascript list-models` and the app's model
//! pickers (#373).
//!
//! Both used to compute the lineup of a language themselves, from the same
//! catalog but with different sizes and a different Pianissimo row. One
//! [`ModelInfo`] per model now carries every fact either needs, and the two
//! surfaces pick from the same entries:
//!
//! - [`catalog`] is every model that serves a language, in the order
//!   `list-models` prints them (Pianissimo last, where it runs);
//! - [`app_picker`] is what the app's pickers offer for a language, in the
//!   app's order: [`FileModel::app_lineup`], with the current choice kept
//!   visible;
//! - [`model_info`] is one entry, for the places that describe a single choice.
//!
//! This module is a leaf (`tests/module_layering.rs`): what is downloaded here,
//! whether Pianissimo can run and what its manifest weighs arrive through
//! [`ModelEnv`]. `crate::machine::{catalog, app_picker, model_info}` are these
//! functions with this machine's answers.

use super::{FileModel, Language, WhisperModel};

const MIB: u64 = 1_048_576;

/// What the catalog asks of the machine. The questions that touch the disk
/// (`*_downloaded`) are asked only for a model that is listed.
pub trait ModelEnv {
    /// Whether Pianissimo can run here.
    fn runs_pianissimo(&self) -> bool;
    /// Whether the Pianissimo files are on disk.
    fn pianissimo_downloaded(&self) -> bool;
    /// Bytes a Pianissimo download transfers (the archive, or all files).
    fn pianissimo_download_bytes(&self) -> u64;
    /// Bytes Pianissimo occupies once installed.
    fn pianissimo_installed_bytes(&self) -> u64;
    /// Whether the file of a Whisper model is on disk.
    fn whisper_downloaded(&self, model: WhisperModel) -> bool;
}

/// One model, as every list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// The choice this describes.
    pub model: FileModel,
    /// Its stable identifier: [`FileModel::id`].
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    /// Megabytes (1 MiB each, rounded down) a download transfers. For a Whisper
    /// model this is [`WhisperModel::size_mb`]; Pianissimo's archive differs
    /// from what it occupies once unpacked.
    pub download_size_mb: u32,
    /// Megabytes the model occupies on disk once installed.
    pub installed_size_mb: u32,
    /// Whether its files are on this machine.
    pub installed: bool,
    /// The languages whose [`catalog`] lists it.
    pub languages: Vec<Language>,
    /// The languages for which it is the "Recommended" engine on this machine
    /// ([`FileModel::recommended_for`]).
    pub recommended_for: Vec<Language>,
    /// Usable by explicit id but not offered in the app's pickers
    /// ([`WhisperModel::is_hidden_in_app`]).
    pub hidden_in_app: bool,
}

impl ModelInfo {
    /// Whether this is the "Recommended" engine for `language`.
    pub fn is_recommended_for(&self, language: Language) -> bool {
        self.recommended_for.contains(&language)
    }
}

fn megabytes(bytes: u64) -> u32 {
    (bytes / MIB) as u32
}

/// The description of Pianissimo: its engine differs by platform.
fn pianissimo_description() -> &'static str {
    if cfg!(windows) {
        "Swedish file transcription · ONNX Runtime on the CPU · Windows on ARM (Snapdragon)"
    } else {
        "Swedish file transcription · Core ML on the Neural Engine · macOS 14+ on Apple Silicon"
    }
}

/// The languages whose catalog lists `choice`.
fn languages_of(choice: FileModel) -> Vec<Language> {
    match choice {
        FileModel::Whisper(model) => Language::ALL
            .into_iter()
            .filter(|language| WhisperModel::models_for_language(*language).contains(&model))
            .collect(),
        FileModel::PianissimoOriginal => vec![Language::Swedish],
    }
}

/// One model, with the facts of this machine from `env`.
pub fn model_info(choice: FileModel, env: &dyn ModelEnv) -> ModelInfo {
    let supported = env.runs_pianissimo();
    let recommended_for = Language::ALL
        .into_iter()
        .filter(|language| FileModel::recommended_for(*language, supported) == choice)
        .collect();
    match choice {
        FileModel::Whisper(model) => ModelInfo {
            model: choice,
            id: model.id(),
            display_name: model.display_name(),
            description: model.description(),
            download_size_mb: model.size_mb(),
            installed_size_mb: model.size_mb(),
            installed: env.whisper_downloaded(model),
            languages: languages_of(choice),
            recommended_for,
            hidden_in_app: model.is_hidden_in_app(),
        },
        FileModel::PianissimoOriginal => ModelInfo {
            model: choice,
            id: FileModel::PIANISSIMO_ID,
            display_name: "Pianissimo",
            description: pianissimo_description(),
            download_size_mb: megabytes(env.pianissimo_download_bytes()),
            installed_size_mb: megabytes(env.pianissimo_installed_bytes()),
            installed: env.pianissimo_downloaded(),
            languages: languages_of(choice),
            recommended_for,
            hidden_in_app: false,
        },
    }
}

/// Every model that serves `language`, in the order `list-models` prints them:
/// the Whisper models of [`WhisperModel::models_for_language`], then Pianissimo
/// for Swedish where it runs. Includes the models the app hides
/// ([`ModelInfo::hidden_in_app`]).
pub fn catalog(language: Language, env: &dyn ModelEnv) -> Vec<ModelInfo> {
    let mut models: Vec<ModelInfo> = WhisperModel::models_for_language(language)
        .iter()
        .map(|model| model_info(FileModel::Whisper(*model), env))
        .collect();
    if language == Language::Swedish && env.runs_pianissimo() {
        models.push(model_info(FileModel::PianissimoOriginal, env));
    }
    models
}

/// What the app's model pickers offer for `language`
/// ([`FileModel::app_lineup`]); `include` keeps a currently selected model
/// visible even when the app no longer offers it.
pub fn app_picker(
    language: Language,
    include: Option<FileModel>,
    env: &dyn ModelEnv,
) -> Vec<ModelInfo> {
    FileModel::app_lineup(language, env.runs_pianissimo(), include)
        .into_iter()
        .map(|choice| model_info(choice, env))
        .collect()
}

#[cfg(test)]
#[path = "listing_tests.rs"]
mod tests;
