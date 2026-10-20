//! A meeting job's entry points (#343, #374): what the app calls to import a meeting and what
//! reprocessing calls to recompute one in full.
//!
//! Both run the diarized file pipeline of [`super::diarized`] on a request that has no command
//! line: typed options ([`meeting_options`]) with the meeting document as the result. Their
//! progress and cancellation come from the caller's [`MeetingControl`]; their narration goes to
//! the process's stderr ([`StderrNarrator`]), as the CLI's does for the same lines.

use std::path::Path;

use crate::error::DictationError;
use crate::glossary::Glossary;
use crate::meeting::control::{Canceller, MeetingControl};
use crate::meeting::MeetingTranscript;
use crate::models::{Language, WhisperModel};
use crate::settings::Settings;
use crate::transcription::WhisperBackend;

use super::diarize::{CachePolicy, OnnxAnalyzer};
use super::diarized::run_diarized;
use super::narrate::StderrNarrator;
use super::plan::{
    paths_refer_to_same_file, plan_file, CacheRequest, DiarizationFlags, FileMode, FileOptions,
    FileRequest,
};
use super::services::Services;

/// Run the diarized file pipeline with explicit progress and cancellation
/// control. The caller owns the flag and must not clear it during a job.
///
/// `build_revision` is the source revision of the running build, which the diarization report
/// records: core has no build identity of its own.
#[allow(clippy::too_many_arguments)]
pub fn transcribe_meeting_file_with_control(
    file: &Path,
    stored: &Settings,
    language: Language,
    model: WhisperModel,
    glossary: &Glossary,
    backend: &WhisperBackend,
    control: &MeetingControl<'_>,
    build_revision: &str,
) -> Result<MeetingTranscript, DictationError> {
    control.check()?;
    let canceller = Canceller::new();
    transcribe_meeting_file_inner(
        file,
        stored,
        language,
        model,
        glossary,
        process_services(backend, &canceller, build_revision),
        Some(control),
        &meeting_options(crate::diarization::DEFAULT_THRESHOLD, None, None),
    )
}

/// Run a full diarized meeting recomputation. This path never reads an
/// existing cache. An optional `cache_output` is written complete, and only if
/// nothing exists there. It is a private staging path: the reprocessing
/// orchestration publishes the cache at the user's destination after it has
/// validated the run (#342), so this function never creates a user-visible file.
#[allow(clippy::too_many_arguments)]
pub fn transcribe_meeting_file_full(
    file: &Path,
    stored: &Settings,
    language: Language,
    model: WhisperModel,
    glossary: &Glossary,
    backend: &WhisperBackend,
    control: Option<&MeetingControl<'_>>,
    threshold: f32,
    speaker_hint: Option<crate::diarization::speaker_hint::SpeakerCountHint>,
    cache_output: Option<&Path>,
    build_revision: &str,
) -> Result<MeetingTranscript, DictationError> {
    if !threshold.is_finite() {
        return Err(DictationError::SettingsError(
            "diarize threshold must be a finite number".to_string(),
        ));
    }
    if !(0.0..=2.0).contains(&threshold) {
        return Err(DictationError::SettingsError(
            "diarize threshold must be between 0.0 and 2.0".to_string(),
        ));
    }
    if let Some(cache_output) = cache_output {
        if paths_refer_to_same_file(file, cache_output) {
            return Err(DictationError::SettingsError(
                "--diarize-cache must not point to the input recording".to_string(),
            ));
        }
        match std::fs::symlink_metadata(cache_output) {
            Ok(_) => {
                return Err(DictationError::FileDecodeError(format!(
                    "Diarization cache output already exists: {}",
                    cache_output.display()
                )))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(DictationError::FileDecodeError(format!(
                    "Failed to inspect diarization cache output '{}': {error}",
                    cache_output.display()
                )))
            }
        }
    }
    if let Some(control) = control {
        control.check()?;
    }
    let canceller = Canceller::new();
    transcribe_meeting_file_inner(
        file,
        stored,
        language,
        model,
        glossary,
        process_services(backend, &canceller, build_revision),
        control,
        &meeting_options(
            threshold,
            speaker_hint,
            cache_output.map(|path| CacheRequest {
                path: path.to_path_buf(),
                policy: CachePolicy::RefreshNew,
            }),
        ),
    )
}

/// The services of a meeting job in the app or of reprocessing: the Whisper backend the caller
/// owns, the ONNX analysis and a canceller of the job's own that nothing ever cancels (a meeting
/// job cancels through its [`MeetingControl`], and no signal handler is installed for it).
fn process_services<'a>(
    backend: &'a WhisperBackend,
    canceller: &'a Canceller,
    build_revision: &'a str,
) -> Services<'a> {
    Services {
        backend,
        analyzer: &OnnxAnalyzer,
        canceller,
        build_revision,
    }
}

/// The options of a meeting run, which has no command line: the diarized
/// pipeline with the meeting document as its result, the given clustering
/// threshold, speaker-count hint and cache, and nothing else switched on.
pub(crate) fn meeting_options(
    threshold: f32,
    speaker_hint: Option<crate::diarization::speaker_hint::SpeakerCountHint>,
    cache: Option<CacheRequest>,
) -> FileOptions {
    FileOptions {
        diarization: Some(DiarizationFlags {
            cache,
            threshold,
            speaker_count: crate::diarization::speaker_hint::SpeakerCountFlags {
                speakers: speaker_hint
                    .and_then(|hint| hint.exact)
                    .filter(|_| !speaker_hint.is_some_and(|h| h.force)),
                min_speakers: speaker_hint.and_then(|hint| hint.min),
                max_speakers: speaker_hint.and_then(|hint| hint.max),
                force_speakers: speaker_hint.filter(|h| h.force).and_then(|h| h.exact),
            },
            meeting_json: true,
            ..DiarizationFlags::default()
        }),
        ..FileOptions::default()
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn transcribe_meeting_file_inner(
    file: &Path,
    stored: &Settings,
    language: Language,
    model: WhisperModel,
    glossary: &Glossary,
    services: Services<'_>,
    control: Option<&MeetingControl<'_>>,
    options: &FileOptions,
) -> Result<MeetingTranscript, DictationError> {
    let mut model_loaded = false;
    let request = FileRequest {
        options,
        file,
        stored,
        language,
        model,
        glossary,
        correction_vocabulary: &[],
        control,
    };
    // The pipeline's progress lines go to the process's stderr; the meeting document is the
    // return value, and nothing is written to stdout.
    let plan = plan_file(&request)?;
    let output = match plan.mode {
        FileMode::Diarized(diarization) => run_diarized(
            &request,
            plan.started,
            *diarization,
            services,
            &mut model_loaded,
            &StderrNarrator,
        )?,
        FileMode::Plain => {
            return Err(DictationError::TranscriptionFailed(
                "Diarized meeting transcript was not produced".to_string(),
            ))
        }
    };
    output.meeting.ok_or_else(|| {
        DictationError::TranscriptionFailed(
            "Diarized meeting transcript was not produced".to_string(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn meeting_control_precancel_checks_before_file_or_model_access() {
        use std::sync::atomic::AtomicBool;

        let cancellation = AtomicBool::new(true);
        let progress = |_| panic!("cancelled job must not emit progress");
        let control = MeetingControl {
            cancellation: &cancellation,
            progress: &progress,
        };
        let backend = WhisperBackend::new();
        let error = transcribe_meeting_file_with_control(
            Path::new("/definitely/missing/meeting.wav"),
            &Settings::default(),
            Language::English,
            WhisperModel::Base,
            &Glossary::parse(""),
            &backend,
            &control,
            "test-revision",
        )
        .expect_err("pre-cancelled job must fail");
        assert!(matches!(error, DictationError::Cancelled), "{error:?}");
        assert!(cancellation.load(Ordering::Acquire));
    }

    /// The signatures the app and reprocessing call (#374; they were pinned in the CLI's contract
    /// test while the CLI owned them) and the phase names the app maps to its job states.
    #[test]
    fn the_entry_points_keep_the_signatures_their_callers_use() {
        use crate::diarization::speaker_hint::SpeakerCountHint;
        use crate::meeting::control::MeetingPhase;

        let _: fn(
            &Path,
            &Settings,
            Language,
            WhisperModel,
            &Glossary,
            &WhisperBackend,
            &MeetingControl<'_>,
            &str,
        ) -> Result<MeetingTranscript, DictationError> = transcribe_meeting_file_with_control;
        let _: fn(
            &Path,
            &Settings,
            Language,
            WhisperModel,
            &Glossary,
            &WhisperBackend,
            Option<&MeetingControl<'_>>,
            f32,
            Option<SpeakerCountHint>,
            Option<&Path>,
            &str,
        ) -> Result<MeetingTranscript, DictationError> = transcribe_meeting_file_full;
        let names: Vec<String> = [
            MeetingPhase::Preparing,
            MeetingPhase::Decoding,
            MeetingPhase::LoadingModel,
            MeetingPhase::Analyzing,
            MeetingPhase::Clustering,
            MeetingPhase::Finalizing,
        ]
        .iter()
        .map(|phase| serde_json::to_string(phase).expect("phase serializes"))
        .collect();
        assert_eq!(
            names,
            [
                "\"preparing\"",
                "\"decoding\"",
                "\"loading_model\"",
                "\"analyzing\"",
                "\"clustering\"",
                "\"finalizing\""
            ]
        );
    }
}
