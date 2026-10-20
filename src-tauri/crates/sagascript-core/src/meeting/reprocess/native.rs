//! The native side of selective meeting reprocessing (#374): the production [`WorkBackend`], which
//! decodes and analyses with the models on this machine and recomputes with the CLI's diarized
//! meeting pipeline, and the entry points that run a validated plan on it.
//!
//! Planning, the orchestration, the staged publication and the `WorkBackend` seam are core's
//! (`sagascript_core::meeting::reprocess::orchestrate`). What stays here is what still needs the
//! transcribe pipeline for the full recomputation; it follows the pipeline into core.

use std::path::Path;

use sagascript_core::diarization::speaker_hint::SpeakerCountHint;
use sagascript_core::diarization::{self, DiarizationAnalysis, DiarizeConfig};
use sagascript_core::error::DictationError;
use sagascript_core::meeting::control::{meeting_check, MeetingControl};
use sagascript_core::meeting::reprocess::orchestrate::{
    execute_with_model_check, ReprocessingInput, ReprocessingResult, StagedReprocessing,
    WorkBackend,
};
use sagascript_core::meeting::reprocess::plan::ReprocessingPlan;
use sagascript_core::meeting::MeetingTranscript;
use sagascript_core::settings::Settings;
use sagascript_core::transcription::WhisperBackend;

use crate::transcribe;

/// Execute exactly the validated plan and publish a requested new cache once
/// the run is validated, for callers whose proposal stays in memory. A new
/// cache output is opt-in and only supported for full recomputation; it never
/// replaces an existing artifact. See [`execute_reprocessing_staged`] to
/// publish the proposal as well.
#[allow(clippy::too_many_arguments)]
pub fn execute_reprocessing(
    plan: &ReprocessingPlan,
    expected_plan_revision: &str,
    input: &ReprocessingInput<'_>,
    stored: &Settings,
    whisper: &WhisperBackend,
    cache_output: Option<&Path>,
    control: Option<&MeetingControl<'_>>,
) -> Result<ReprocessingResult, DictationError> {
    execute_reprocessing_staged(
        plan,
        expected_plan_revision,
        input,
        stored,
        whisper,
        cache_output,
        control,
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
) -> Result<StagedReprocessing, DictationError> {
    execute_with_model_check(
        plan,
        expected_plan_revision,
        input,
        cache_output,
        control,
        crate::GIT_HASH,
        sagascript_core::diarization::model::all_models_downloaded,
        &mut NativeBackend { stored, whisper },
    )
}

pub(crate) struct NativeBackend<'a> {
    pub(crate) stored: &'a Settings,
    pub(crate) whisper: &'a WhisperBackend,
}

impl WorkBackend for NativeBackend<'_> {
    fn decode(
        &mut self,
        audio: &Path,
        control: Option<&MeetingControl<'_>>,
    ) -> Result<Vec<f32>, DictationError> {
        sagascript_core::audio::decoder::decode_audio_file_with_control(audio, &|| {
            meeting_check(control)
        })
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
        transcribe::transcribe_meeting_file_full(
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
        )
    }
}
