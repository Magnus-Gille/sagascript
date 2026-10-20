//! The native side of selective meeting reprocessing (#374): the production [`WorkBackend`], which
//! decodes and analyses with the models on this machine and recomputes with the diarized meeting
//! pipeline of [`crate::meeting::pipeline`], and the entry points that run a validated plan on it.
//!
//! Planning, the orchestration, the staged publication and the `WorkBackend` seam are in
//! [`super::orchestrate`]; a test of them replaces the backend. The CLI's `meeting reprocess
//! execute` and the app's reprocessing job run a plan through this module.

use std::path::Path;

use crate::diarization::speaker_hint::SpeakerCountHint;
use crate::diarization::{self, DiarizationAnalysis, DiarizeConfig};
use crate::error::DictationError;
use crate::meeting::control::{meeting_check, MeetingControl};
use crate::meeting::pipeline::entry::transcribe_meeting_file_full;
use crate::meeting::reprocess::orchestrate::{
    execute_with_model_check, ReprocessingInput, ReprocessingResult, StagedReprocessing,
    WorkBackend,
};
use crate::meeting::reprocess::plan::ReprocessingPlan;
use crate::meeting::MeetingTranscript;
use crate::settings::Settings;
use crate::transcription::WhisperBackend;

/// Execute exactly the validated plan and publish a requested new cache once
/// the run is validated, for callers whose proposal stays in memory. A new
/// cache output is opt-in and only supported for full recomputation; it never
/// replaces an existing artifact. See [`execute_reprocessing_staged`] to
/// publish the proposal as well. `build_revision` is the source revision of the running build,
/// which a diarization report records.
#[allow(clippy::too_many_arguments)]
pub fn execute_reprocessing(
    plan: &ReprocessingPlan,
    expected_plan_revision: &str,
    input: &ReprocessingInput<'_>,
    stored: &Settings,
    whisper: &WhisperBackend,
    cache_output: Option<&Path>,
    control: Option<&MeetingControl<'_>>,
    build_revision: &str,
) -> Result<ReprocessingResult, DictationError> {
    execute_reprocessing_staged(
        plan,
        expected_plan_revision,
        input,
        stored,
        whisper,
        cache_output,
        control,
        build_revision,
    )?
    .finish()
}

/// Execute exactly the validated plan, leaving a requested new cache staged
/// and unpublished; see the module documentation of core's `orchestrate` for
/// what to do next.
#[allow(clippy::too_many_arguments)]
pub fn execute_reprocessing_staged(
    plan: &ReprocessingPlan,
    expected_plan_revision: &str,
    input: &ReprocessingInput<'_>,
    stored: &Settings,
    whisper: &WhisperBackend,
    cache_output: Option<&Path>,
    control: Option<&MeetingControl<'_>>,
    build_revision: &str,
) -> Result<StagedReprocessing, DictationError> {
    execute_with_model_check(
        plan,
        expected_plan_revision,
        input,
        cache_output,
        control,
        build_revision,
        crate::diarization::model::all_models_downloaded,
        &mut NativeBackend {
            stored,
            whisper,
            build_revision,
        },
    )
}

/// The production [`WorkBackend`]: this machine's models and a Whisper backend the caller owns.
pub struct NativeBackend<'a> {
    pub stored: &'a Settings,
    pub whisper: &'a WhisperBackend,
    /// The source revision of the running build, which a full recomputation's report records.
    pub build_revision: &'a str,
}

impl WorkBackend for NativeBackend<'_> {
    fn decode(
        &mut self,
        audio: &Path,
        control: Option<&MeetingControl<'_>>,
    ) -> Result<Vec<f32>, DictationError> {
        crate::audio::decoder::decode_audio_file_with_control(audio, &|| meeting_check(control))
    }

    fn analyze(
        &mut self,
        audio: &[f32],
        config: &DiarizeConfig,
        control: Option<&MeetingControl<'_>>,
    ) -> Result<DiarizationAnalysis, DictationError> {
        diarization::analyze_with_control(audio, config, &|| meeting_check(control))
            .map(|(analysis, _)| analysis)
    }

    fn full(
        &mut self,
        input: &ReprocessingInput<'_>,
        threshold: f32,
        speaker_hint: Option<SpeakerCountHint>,
        cache_output: Option<&Path>,
        control: Option<&MeetingControl<'_>>,
    ) -> Result<MeetingTranscript, DictationError> {
        transcribe_meeting_file_full(
            input.audio,
            self.stored,
            input.language,
            input.model,
            input.glossary,
            self.whisper,
            control,
            threshold,
            speaker_hint,
            cache_output,
            self.build_revision,
        )
    }
}
