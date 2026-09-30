use clap::Args;

use sagascript_core::error::DictationError;
use sagascript_core::settings::{Language, WhisperModel};
use sagascript_core::transcription::model;
use sagascript_core::transcription::pianissimo_model;

use super::transcribe::{model_id_string, parse_language, parse_model};

#[derive(Args)]
pub struct ListModelsArgs {
    /// Filter by language [possible values: en, sv, no, fi, auto (less accurate)]
    #[arg(short, long, value_name = "LANG")]
    pub language: Option<String>,
}

#[derive(Args)]
pub struct DownloadModelArgs {
    /// Model ID to download [see: sagascript list-models]
    pub model: String,
}

/// Marker appended to models the app's pickers no longer offer. They stay
/// listed and usable through an explicit `--model` / `download-model` id.
pub const HIDDEN_IN_APP_NOTE: &str = "(hidden in app)";

fn list_note(model: WhisperModel, language: Language) -> &'static str {
    if language == Language::Swedish && model.is_hidden_in_app() {
        HIDDEN_IN_APP_NOTE
    } else {
        ""
    }
}

fn whisper_row(model: WhisperModel, language: Language, downloaded: &str) -> String {
    format!(
        "{:<20} {:<10} {:>5} MB  {:<12} {:<12} {}",
        model_id_string(model),
        model.display_name(),
        model.size_mb(),
        downloaded,
        language.display_name(),
        list_note(model, language),
    )
    .trim_end()
    .to_string()
}

pub fn list(args: ListModelsArgs) -> Result<(), DictationError> {
    let languages: Vec<Language> = if let Some(lang_str) = &args.language {
        vec![parse_language(lang_str)?]
    } else {
        vec![
            Language::English,
            Language::Swedish,
            Language::Norwegian,
            Language::Finnish,
            Language::Auto,
        ]
    };

    // Header
    println!(
        "{:<20} {:<10} {:<8} {:<12} {:<12}",
        "MODEL ID", "NAME", "SIZE", "DOWNLOADED", "LANGUAGE"
    );
    println!("{}", "-".repeat(62));

    for lang in &languages {
        let models = WhisperModel::models_for_language(*lang);
        for &m in models {
            let downloaded = if model::is_model_downloaded(m) {
                "yes"
            } else {
                "no"
            };

            println!("{}", whisper_row(m, *lang, downloaded));
        }
        if *lang == Language::Swedish && sagascript_core::transcription::pianissimo_backend::runtime_supported_on_this_os() {
            println!(
                "{:<20} {:<10} {:>5} MB  {:<12} {:<12}",
                "pianissimo-sv",
                "Pianissimo",
                (pianissimo_model::installed_size_bytes() / 1_048_576) as u32,
                if pianissimo_model::is_downloaded() { "yes" } else { "no" },
                lang.display_name(),
            );
        }
    }

    // Diarization models section (only when no language filter, or always show)
    #[cfg(feature = "diarization")]
    if args.language.is_none() {
        use sagascript_core::diarization::model as diar_model;
        use sagascript_core::diarization::model::DiarizationModel;

        println!();
        println!("Diarization models (speaker identification):");
        println!("{}", "-".repeat(62));

        for &m in DiarizationModel::ALL {
            let downloaded = if diar_model::is_model_downloaded(m) {
                "yes"
            } else {
                "no"
            };

            println!(
                "{:<20} {:<10} {:>5} MB  {:<12} {:<12}",
                m.model_id(),
                m.display_name(),
                m.size_mb(),
                downloaded,
                "—",
            );
        }
    }

    Ok(())
}

#[derive(Args)]
pub struct DeleteModelArgs {
    /// Model ID to delete [see: sagascript list-models]
    pub model: String,
}

pub fn delete(args: DeleteModelArgs) -> Result<(), DictationError> {
    if args.model == "pianissimo-sv" {
        pianissimo_model::delete()?;
        eprintln!("Deleted Pianissimo (model files and any legacy NeMo/GGUF files)");
        return Ok(());
    }
    let whisper_model = parse_model(&args.model)?;

    if !model::is_model_downloaded(whisper_model) {
        eprintln!(
            "Model '{}' is not downloaded.",
            whisper_model.display_name()
        );
        return Ok(());
    }

    let path = model::model_path(whisper_model);
    std::fs::remove_file(&path).map_err(|e| {
        DictationError::SettingsError(format!(
            "Failed to delete model file '{}': {e}",
            path.display()
        ))
    })?;

    sagascript_core::download::remove_verification_stamp(&path);

    eprintln!(
        "Deleted {} ({})",
        whisper_model.display_name(),
        path.display()
    );
    Ok(())
}

/// What a progress update should do on the terminal.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum ProgressAction {
    Skip,
    /// Redraw the current line in place (TTY).
    Redraw,
    /// Print a newline-terminated line (non-TTY logs).
    Line,
}

/// Pure throttling state: TTY redraws at most every 100 ms; non-TTY prints
/// only when a new 10% step is crossed (the first call and 100% included).
#[derive(Debug, Default)]
pub(crate) struct ProgressThrottle {
    last_redraw: Option<std::time::Duration>,
    last_step: Option<u64>,
}

impl ProgressThrottle {
    pub(crate) fn decide(
        &mut self,
        bytes: u64,
        total: u64,
        elapsed: std::time::Duration,
        is_tty: bool,
    ) -> ProgressAction {
        if is_tty {
            let done = total > 0 && bytes >= total;
            let due = match self.last_redraw {
                None => true,
                Some(last) => elapsed.saturating_sub(last) >= std::time::Duration::from_millis(100),
            };
            if due || done {
                self.last_redraw = Some(elapsed);
                return ProgressAction::Redraw;
            }
            return ProgressAction::Skip;
        }
        // Unknown total: step by 50 MiB instead of percent.
        let step = match (bytes.min(total) * 10).checked_div(total) {
            Some(tenths) => tenths.min(10),
            None => bytes / (50 * 1_048_576),
        };
        if self.last_step.is_none_or(|last| step > last) {
            self.last_step = Some(step);
            ProgressAction::Line
        } else {
            ProgressAction::Skip
        }
    }
}

/// Progress printer for model downloads (stderr).
pub(crate) struct DownloadProgress {
    state: std::sync::Mutex<(ProgressThrottle, bool)>, // (throttle, drew in place)
    start: std::time::Instant,
    is_tty: bool,
}

impl DownloadProgress {
    pub(crate) fn new() -> std::sync::Arc<Self> {
        use std::io::IsTerminal;
        std::sync::Arc::new(Self {
            state: std::sync::Mutex::new((ProgressThrottle::default(), false)),
            start: std::time::Instant::now(),
            is_tty: std::io::stderr().is_terminal(),
        })
    }

    pub(crate) fn report(&self, downloaded: u64, total: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let action = state
            .0
            .decide(downloaded, total, self.start.elapsed(), self.is_tty);
        if action == ProgressAction::Skip {
            return;
        }
        let mb_done = downloaded as f64 / 1_048_576.0;
        let text = if total > 0 {
            let pct = (downloaded as f64 / total as f64 * 100.0) as u32;
            format!("  {:.1}/{:.1} MB ({pct}%)", mb_done, total as f64 / 1_048_576.0)
        } else {
            format!("  {:.1} MB downloaded", mb_done)
        };
        match action {
            ProgressAction::Redraw => {
                eprint!("\r{text}");
                state.1 = true;
            }
            _ => eprintln!("{text}"),
        }
    }

    /// Terminate an in-place TTY line.
    pub(crate) fn finish(&self) {
        if self.state.lock().unwrap_or_else(|e| e.into_inner()).1 {
            eprintln!();
        }
    }
}

pub async fn download(args: DownloadModelArgs) -> Result<(), DictationError> {
    if args.model == "pianissimo-sv" {
        if !sagascript_core::transcription::pianissimo_backend::runtime_supported_on_this_os() {
            return Err(DictationError::TranscriptionFailed(
                sagascript_core::transcription::pianissimo_backend::UNSUPPORTED_MESSAGE.into(),
            ));
        }
        eprintln!(
            "Downloading Pianissimo model (~{} MB, CC BY 4.0, Klang AI AB)...",
            pianissimo_model::download_size_bytes() / 1_048_576
        );
        let progress = DownloadProgress::new();
    let cb_progress = progress.clone();
        let path = pianissimo_model::download(move |downloaded, total| {
            cb_progress.report(downloaded, total);
        })
        .await?;
        progress.finish();
        eprintln!("Pianissimo model ready.");
        println!("{}", path.display());
        return Ok(());
    }
    // Try diarization model IDs first (when feature is enabled)
    #[cfg(feature = "diarization")]
    {
        use sagascript_core::diarization::model::DiarizationModel;

        // "diarization" meta-ID downloads both models
        if DiarizationModel::is_meta_id(&args.model) {
            for &m in DiarizationModel::ALL {
                download_diarization_model(m).await?;
            }
            return Ok(());
        }

        if let Some(diar) = DiarizationModel::from_id(&args.model) {
            return download_diarization_model(diar).await;
        }
    }

    let whisper_model = parse_model(&args.model)?;
    let was_present = model::is_model_downloaded(whisper_model);
    if was_present {
        eprintln!("Verifying {}...", whisper_model.display_name());
    } else {
        eprintln!(
            "Downloading {} (~{} MB)...",
            whisper_model.display_name(),
            whisper_model.size_mb()
        );
    }

    let progress = DownloadProgress::new();
    let cb_progress = progress.clone();
    let path = model::download_model(whisper_model, move |downloaded, total| {
        cb_progress.report(downloaded, total);
    })
    .await?;

    progress.finish();
    eprintln!("Model ready.");
    println!("{}", path.display());
    Ok(())
}

#[cfg(feature = "diarization")]
async fn download_diarization_model(
    model: sagascript_core::diarization::model::DiarizationModel,
) -> Result<(), DictationError> {
    use sagascript_core::diarization::model as diar_model;

    if diar_model::is_model_downloaded(model) {
        eprintln!("Verifying {}...", model.display_name());
    } else {
        eprintln!(
            "Downloading {} (~{} MB)...",
            model.display_name(),
            model.size_mb()
        );
    }

    let progress = DownloadProgress::new();
    let cb_progress = progress.clone();
    let path = diar_model::download_model(model, move |downloaded, total| {
        cb_progress.report(downloaded, total);
    })
    .await?;

    progress.finish();
    eprintln!("Model ready.");
    println!("{}", path.display());
    Ok(())
}

#[cfg(test)]
mod progress_tests {
    use super::*;
    use std::time::Duration;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn tty_redraws_at_most_ten_times_per_second() {
        let mut t = ProgressThrottle::default();
        assert_eq!(t.decide(0, 1000, ms(0), true), ProgressAction::Redraw);
        assert_eq!(t.decide(10, 1000, ms(50), true), ProgressAction::Skip);
        assert_eq!(t.decide(20, 1000, ms(99), true), ProgressAction::Skip);
        assert_eq!(t.decide(30, 1000, ms(100), true), ProgressAction::Redraw);
        assert_eq!(t.decide(40, 1000, ms(150), true), ProgressAction::Skip);
    }

    #[test]
    fn tty_always_draws_completion() {
        let mut t = ProgressThrottle::default();
        t.decide(0, 1000, ms(0), true);
        assert_eq!(t.decide(1000, 1000, ms(10), true), ProgressAction::Redraw);
    }

    #[test]
    fn non_tty_prints_start_every_ten_percent_and_end() {
        let mut t = ProgressThrottle::default();
        let mut lines = 0;
        for bytes in (0..=1_000_000u64).step_by(1000) {
            if t.decide(bytes, 1_000_000, ms(bytes), false) == ProgressAction::Line {
                lines += 1;
            }
        }
        assert_eq!(lines, 11); // 0%, 10%, ..., 100%
    }

    #[test]
    fn non_tty_unknown_total_steps_by_50_mib() {
        let mut t = ProgressThrottle::default();
        assert_eq!(t.decide(0, 0, ms(0), false), ProgressAction::Line);
        assert_eq!(t.decide(49 * 1_048_576, 0, ms(1), false), ProgressAction::Skip);
        assert_eq!(t.decide(50 * 1_048_576, 0, ms(2), false), ProgressAction::Line);
    }

    #[test]
    fn non_tty_ignores_bytes_beyond_total() {
        let mut t = ProgressThrottle::default();
        t.decide(1000, 1000, ms(0), false);
        assert_eq!(t.decide(2000, 1000, ms(1), false), ProgressAction::Skip);
    }
}

#[cfg(test)]
mod lineup_tests {
    use super::*;

    #[test]
    fn list_marks_retired_swedish_models_but_keeps_their_ids() {
        for (model, id) in [
            (WhisperModel::KbWhisperTiny, "kb-whisper-tiny"),
            (WhisperModel::KbWhisperBase, "kb-whisper-base"),
            (WhisperModel::KbWhisperSmall, "kb-whisper-small"),
        ] {
            let row = whisper_row(model, Language::Swedish, "no");
            assert!(row.starts_with(id), "{row}");
            assert!(row.ends_with(HIDDEN_IN_APP_NOTE), "{row}");
        }
        for model in [WhisperModel::KbWhisperMedium, WhisperModel::KbWhisperLarge] {
            let row = whisper_row(model, Language::Swedish, "no");
            assert!(!row.contains("hidden"), "{row}");
            assert!(!row.ends_with(' '), "{row:?}");
        }
        // Other languages are unchanged.
        assert!(!whisper_row(WhisperModel::Tiny, Language::Auto, "no").contains("hidden"));
        // Every Swedish model is still listed and parseable.
        for model in WhisperModel::models_for_language(Language::Swedish) {
            assert_eq!(parse_model(model_id_string(*model)).unwrap(), *model);
        }
    }
}
