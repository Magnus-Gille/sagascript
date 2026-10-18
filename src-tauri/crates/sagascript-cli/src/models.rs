use clap::Args;
use unicode_width::UnicodeWidthStr;

use sagascript_core::error::DictationError;
use sagascript_core::machine;
use sagascript_core::models::ModelInfo;
use sagascript_core::settings::Language;
use sagascript_core::transcription::model;
use sagascript_core::transcription::pianissimo_model;

use super::transcribe::{parse_language, parse_model};
use crate::output::{errln, outln, Out};

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

fn list_note(info: &ModelInfo, language: Language) -> &'static str {
    if language == Language::Swedish && info.hidden_in_app {
        HIDDEN_IN_APP_NOTE
    } else {
        ""
    }
}

/// One `list-models` table row, kept as cells so column widths can be derived
/// from the data instead of hard-coded.
struct ModelRow {
    id: String,
    name: String,
    size: String,
    downloaded: &'static str,
    language: String,
    note: &'static str,
}

/// The row of one catalog entry in the list of `language`. The size column is
/// the size on disk, which for Pianissimo is not what the app's picker shows
/// (its download).
fn model_row(info: &ModelInfo, language: Language) -> ModelRow {
    ModelRow {
        id: info.id.to_string(),
        name: info.display_name.to_string(),
        size: format!("{} MB", info.installed_size_mb),
        downloaded: yes_no(info.installed),
        language: language.display_name().to_string(),
        note: list_note(info, language),
    }
}

/// The rows of the listed languages, each from that language's catalog.
fn catalog_rows(
    languages: &[Language],
    catalog: impl Fn(Language) -> Vec<ModelInfo>,
) -> Vec<ModelRow> {
    languages
        .iter()
        .flat_map(|&language| {
            catalog(language)
                .iter()
                .map(|info| model_row(info, language))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn pad_right(cell: &str, width: usize) -> String {
    let fill = width.saturating_sub(UnicodeWidthStr::width(cell));
    format!("{cell}{}", " ".repeat(fill))
}

fn pad_left(cell: &str, width: usize) -> String {
    let fill = width.saturating_sub(UnicodeWidthStr::width(cell));
    format!("{}{cell}", " ".repeat(fill))
}

/// Render rows as aligned lines (header and separator first). Widths come from
/// the widest cell per column, measured in terminal display columns so wide or
/// combining characters do not skew the table. Trailing whitespace is trimmed.
fn format_table(rows: &[ModelRow]) -> Vec<String> {
    const HEADERS: [&str; 5] = ["MODEL ID", "NAME", "SIZE", "DOWNLOADED", "LANGUAGE"];
    let width = |header: &str, cells: &dyn Fn(&ModelRow) -> &str| {
        rows.iter()
            .map(|r| UnicodeWidthStr::width(cells(r)))
            .chain([UnicodeWidthStr::width(header)])
            .max()
            .unwrap_or(0)
    };
    let w_id = width(HEADERS[0], &|r| &r.id);
    let w_name = width(HEADERS[1], &|r| &r.name);
    let w_size = width(HEADERS[2], &|r| &r.size);
    let w_dl = width(HEADERS[3], &|r| r.downloaded);
    let w_lang = width(HEADERS[4], &|r| &r.language);

    let line = |id: &str, name: &str, size: &str, dl: &str, lang: &str, note: &str| {
        format!(
            "{}  {}  {}  {}  {}  {}",
            pad_right(id, w_id),
            pad_right(name, w_name),
            pad_left(size, w_size),
            pad_right(dl, w_dl),
            pad_right(lang, w_lang),
            note
        )
        .trim_end()
        .to_string()
    };

    let mut lines = vec![
        line(
            HEADERS[0], HEADERS[1], HEADERS[2], HEADERS[3], HEADERS[4], "",
        ),
        "-".repeat(w_id + w_name + w_size + w_dl + w_lang + 8),
    ];
    lines.extend(
        rows.iter()
            .map(|r| line(&r.id, &r.name, &r.size, r.downloaded, &r.language, r.note)),
    );
    lines
}

pub fn list(args: ListModelsArgs, out: &Out) -> Result<(), DictationError> {
    let languages: Vec<Language> = if let Some(lang_str) = &args.language {
        vec![parse_language(lang_str)?]
    } else {
        Language::ALL.to_vec()
    };

    // Only the diarization rows below add to it.
    #[cfg_attr(not(feature = "diarization"), allow(unused_mut))]
    let mut rows = catalog_rows(&languages, machine::catalog);

    // Diarization models (only when no language filter) share the table so
    // their columns line up with the Whisper rows above.
    #[cfg(feature = "diarization")]
    if args.language.is_none() {
        use sagascript_core::diarization::model as diar_model;
        use sagascript_core::diarization::model::DiarizationModel;

        for &m in DiarizationModel::ALL {
            rows.push(ModelRow {
                id: m.model_id().to_string(),
                name: m.display_name().to_string(),
                size: format!("{} MB", m.size_mb()),
                downloaded: yes_no(diar_model::is_model_downloaded(m)),
                language: "—".into(),
                note: "",
            });
        }
    }

    for line in format_table(&rows) {
        outln!(out, "{line}")?;
    }

    Ok(())
}

#[derive(Args)]
pub struct DeleteModelArgs {
    /// Model ID to delete [see: sagascript list-models]
    pub model: String,
}

pub fn delete(args: DeleteModelArgs, out: &Out) -> Result<(), DictationError> {
    if args.model == "pianissimo-sv" {
        pianissimo_model::delete()?;
        errln!(
            out,
            "Deleted Pianissimo (model files and any legacy NeMo/GGUF files)"
        );
        return Ok(());
    }
    let whisper_model = parse_model(&args.model)?;

    if !model::is_model_downloaded(whisper_model) {
        errln!(
            out,
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

    errln!(
        out,
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
    out: Out,
}

impl DownloadProgress {
    pub(crate) fn new(out: &Out) -> std::sync::Arc<Self> {
        use std::io::IsTerminal;
        std::sync::Arc::new(Self {
            state: std::sync::Mutex::new((ProgressThrottle::default(), false)),
            start: std::time::Instant::now(),
            is_tty: std::io::stderr().is_terminal(),
            out: out.clone(),
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
            format!(
                "  {:.1}/{:.1} MB ({pct}%)",
                mb_done,
                total as f64 / 1_048_576.0
            )
        } else {
            format!("  {:.1} MB downloaded", mb_done)
        };
        match action {
            ProgressAction::Redraw => {
                self.out.err_text(&format!("\r{text}"));
                state.1 = true;
            }
            _ => errln!(self.out, "{text}"),
        }
    }

    /// Terminate an in-place TTY line.
    pub(crate) fn finish(&self) {
        if self.state.lock().unwrap_or_else(|e| e.into_inner()).1 {
            errln!(self.out);
        }
    }
}

pub async fn download(args: DownloadModelArgs, out: &Out) -> Result<(), DictationError> {
    if args.model == "pianissimo-sv" {
        if !sagascript_core::transcription::pianissimo_backend::runtime_supported_on_this_os() {
            return Err(DictationError::TranscriptionFailed(
                sagascript_core::transcription::pianissimo_backend::UNSUPPORTED_MESSAGE.into(),
            ));
        }
        errln!(
            out,
            "Downloading Pianissimo model (~{} MB, CC BY 4.0, Klang AI AB)...",
            pianissimo_model::download_size_bytes() / 1_048_576
        );
        let progress = DownloadProgress::new(out);
        let cb_progress = progress.clone();
        let path = pianissimo_model::download(move |downloaded, total| {
            cb_progress.report(downloaded, total);
        })
        .await?;
        progress.finish();
        errln!(out, "Pianissimo model ready.");
        outln!(out, "{}", path.display())?;
        return Ok(());
    }
    // Try diarization model IDs first (when feature is enabled)
    #[cfg(feature = "diarization")]
    {
        use sagascript_core::diarization::model::DiarizationModel;

        // "diarization" meta-ID downloads both models
        if DiarizationModel::is_meta_id(&args.model) {
            for &m in DiarizationModel::ALL {
                download_diarization_model(m, out).await?;
            }
            return Ok(());
        }

        if let Some(diar) = DiarizationModel::from_id(&args.model) {
            return download_diarization_model(diar, out).await;
        }
    }

    let whisper_model = parse_model(&args.model)?;
    let was_present = model::is_model_downloaded(whisper_model);
    if was_present {
        errln!(out, "Verifying {}...", whisper_model.display_name());
    } else {
        errln!(
            out,
            "Downloading {} (~{} MB)...",
            whisper_model.display_name(),
            whisper_model.size_mb()
        );
    }

    let progress = DownloadProgress::new(out);
    let cb_progress = progress.clone();
    let path = model::download_model(whisper_model, move |downloaded, total| {
        cb_progress.report(downloaded, total);
    })
    .await?;

    progress.finish();
    errln!(out, "Model ready.");
    outln!(out, "{}", path.display())?;
    Ok(())
}

#[cfg(feature = "diarization")]
async fn download_diarization_model(
    model: sagascript_core::diarization::model::DiarizationModel,
    out: &Out,
) -> Result<(), DictationError> {
    use sagascript_core::diarization::model as diar_model;

    if diar_model::is_model_downloaded(model) {
        errln!(out, "Verifying {}...", model.display_name());
    } else {
        errln!(
            out,
            "Downloading {} (~{} MB)...",
            model.display_name(),
            model.size_mb()
        );
    }

    let progress = DownloadProgress::new(out);
    let cb_progress = progress.clone();
    let path = diar_model::download_model(model, move |downloaded, total| {
        cb_progress.report(downloaded, total);
    })
    .await?;

    progress.finish();
    errln!(out, "Model ready.");
    outln!(out, "{}", path.display())?;
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
        assert_eq!(
            t.decide(49 * 1_048_576, 0, ms(1), false),
            ProgressAction::Skip
        );
        assert_eq!(
            t.decide(50 * 1_048_576, 0, ms(2), false),
            ProgressAction::Line
        );
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
    use crate::transcribe::model_id_string;
    use sagascript_core::models::{self, FileModel};
    use sagascript_core::settings::WhisperModel;
    use sagascript_core::test_support::model_env::FixedModelEnv;

    fn whisper_row(model: WhisperModel, language: Language, installed: bool) -> ModelRow {
        let env = FixedModelEnv {
            downloaded: if installed { vec![model] } else { Vec::new() },
            ..FixedModelEnv::new()
        };
        model_row(
            &models::model_info(FileModel::Whisper(model), &env),
            language,
        )
    }

    #[test]
    fn list_marks_retired_swedish_models_but_keeps_their_ids() {
        for (model, id) in [
            (WhisperModel::KbWhisperTiny, "kb-whisper-tiny"),
            (WhisperModel::KbWhisperBase, "kb-whisper-base"),
            (WhisperModel::KbWhisperSmall, "kb-whisper-small"),
        ] {
            let row = format_table(&[whisper_row(model, Language::Swedish, false)]).remove(2);
            assert!(row.starts_with(id), "{row}");
            assert!(row.ends_with(HIDDEN_IN_APP_NOTE), "{row}");
        }
        for model in [WhisperModel::KbWhisperMedium, WhisperModel::KbWhisperLarge] {
            let row = format_table(&[whisper_row(model, Language::Swedish, false)]).remove(2);
            assert!(!row.contains("hidden"), "{row}");
            assert!(!row.ends_with(' '), "{row:?}");
        }
        // Other languages are unchanged.
        assert!(
            !format_table(&[whisper_row(WhisperModel::Tiny, Language::Auto, false)])[2]
                .contains("hidden")
        );
        // Every Swedish model is still listed and parseable.
        for model in WhisperModel::models_for_language(Language::Swedish) {
            assert_eq!(parse_model(model_id_string(*model)).unwrap(), *model);
        }
    }

    fn table(languages: &[Language], env: &FixedModelEnv) -> Vec<String> {
        format_table(&catalog_rows(languages, |language| {
            models::catalog(language, env)
        }))
    }

    /// Where Pianissimo runs, its row follows the Swedish models, carries its
    /// size on disk (1 400 MB here; the app's picker shows the 1 000 MB
    /// download), and says nothing about the app.
    #[test]
    fn swedish_list_closes_with_pianissimo_at_its_installed_size() {
        let env = FixedModelEnv {
            pianissimo_downloaded: true,
            downloaded: vec![WhisperModel::KbWhisperLarge],
            ..FixedModelEnv::with_pianissimo()
        };
        assert_eq!(
            table(&[Language::Swedish], &env),
            [
                "MODEL ID           NAME                  SIZE  DOWNLOADED  LANGUAGE",
                "-------------------------------------------------------------------",
                "kb-whisper-tiny    KB-Whisper Tiny      40 MB  no          Swedish   (hidden in app)",
                "kb-whisper-base    KB-Whisper Base      60 MB  no          Swedish   (hidden in app)",
                "kb-whisper-small   KB-Whisper Small    190 MB  no          Swedish   (hidden in app)",
                "kb-whisper-medium  KB-Whisper Medium   514 MB  no          Swedish",
                "kb-whisper-large   KB-Whisper Large   1031 MB  yes         Swedish",
                "pianissimo-sv      Pianissimo         1400 MB  yes         Swedish",
            ]
        );
    }

    #[test]
    fn a_host_without_pianissimo_lists_no_pianissimo_row() {
        let lines = table(&Language::ALL, &FixedModelEnv::new());
        assert!(lines.iter().all(|line| !line.contains("pianissimo")));
        // Header, separator and every language's models, once each.
        let models: usize = Language::ALL
            .iter()
            .map(|language| WhisperModel::models_for_language(*language).len())
            .sum();
        assert_eq!(lines.len(), 2 + models);
    }

    #[test]
    fn the_language_column_is_the_language_listed() {
        let lines = table(&[Language::Finnish, Language::Auto], &FixedModelEnv::new());
        // `base` serves both: one row under each language.
        let base: Vec<&String> = lines
            .iter()
            .filter(|line| line.starts_with("base "))
            .collect();
        assert_eq!(base.len(), 2, "{lines:#?}");
        assert!(base[0].contains("Finnish") && base[1].contains("Auto-detect"));
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;

    fn row(id: &str, name: &str, size: &str, language: &str) -> ModelRow {
        ModelRow {
            id: id.into(),
            name: name.into(),
            size: size.into(),
            downloaded: "no",
            language: language.into(),
            note: "",
        }
    }

    #[test]
    fn columns_align_for_long_and_wide_names() {
        let rows = vec![
            row("tiny", "Tiny", "75 MB", "English"),
            row("kb-whisper-large", "KB-Whisper", "1031 MB", "Swedish"),
            row("pianissimo-sv", "Pianissimo", "900 MB", "Swedish"),
            // Double-width CJK occupies two terminal columns per character.
            row("wide", "日本語", "1 MB", "—"),
        ];
        let lines = format_table(&rows);
        assert_eq!(lines.len(), rows.len() + 2);
        // Every cell after the name starts at the same display column: the
        // size column is right-aligned, so each line's SIZE cell ends at the
        // same display column.
        let size_end = |line: &str| {
            let idx = line
                .find("MB")
                .map(|i| i + 2)
                .or_else(|| line.find("SIZE").map(|i| i + 4))
                .unwrap();
            UnicodeWidthStr::width(&line[..idx])
        };
        let ends: Vec<_> = [&lines[0], &lines[2], &lines[3], &lines[4], &lines[5]]
            .iter()
            .map(|l| size_end(l))
            .collect();
        assert!(
            ends.windows(2).all(|w| w[0] == w[1]),
            "{ends:?}\n{}",
            lines.join("\n")
        );
        // Separator is dashes only; no trailing whitespace anywhere.
        assert!(lines.iter().all(|l| l == l.trim_end()));
        assert!(lines[1].chars().all(|c| c == '-'));
    }

    #[test]
    fn widths_follow_the_data_not_fixed_numbers() {
        let lines = format_table(&[row(
            "a-very-long-model-identifier-over-20",
            "N",
            "1 MB",
            "English",
        )]);
        let id = "a-very-long-model-identifier-over-20";
        assert!(
            lines[0].starts_with(&format!("{:<w$}  NAME", "MODEL ID", w = id.len())),
            "{}",
            lines[0]
        );
        assert!(
            lines[2].starts_with("a-very-long-model-identifier-over-20  N"),
            "{}",
            lines[2]
        );
    }
}
