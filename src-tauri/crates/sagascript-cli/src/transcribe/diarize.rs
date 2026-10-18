//! The diarization half of a file run (#343): the cache policy, the decoder
//! the diarized path uses, the concurrent analysis and the performance report.

#[cfg(feature = "diarization")]
use sagascript_core::diarization::report::DecoderEvidence;
#[cfg(feature = "diarization")]
use sagascript_core::error::DictationError;
#[cfg(feature = "diarization")]
use sagascript_core::settings::{Language, Settings};
#[cfg(feature = "diarization")]
use sagascript_core::transcription::model;
#[cfg(feature = "diarization")]
use sagascript_core::transcription::whisper_backend::validate_diarization_options;
#[cfg(feature = "diarization")]
use sagascript_core::transcription::{TranscribeOptions, TranscriptSegment};

#[cfg(feature = "diarization")]
use super::args::TranscribeArgs;
#[cfg(feature = "diarization")]
use super::inference::InferenceBackend;

/// What a diarized run does with `--diarize-cache`: `Normal` reads a matching
/// cache and writes a miss over it; `RefreshNew` never reads and creates the
/// cache only if nothing is there (reprocessing stages it, #342).
#[cfg(feature = "diarization")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CachePolicy {
    Normal,
    RefreshNew,
}

#[cfg(feature = "diarization")]
impl CachePolicy {
    pub(super) fn reads_existing_cache(self) -> bool {
        matches!(self, Self::Normal)
    }
}

#[cfg(feature = "diarization")]
#[derive(Debug, Default, serde::Serialize)]
pub(super) struct DiarizationPerformance {
    pub(super) acceleration_backend: &'static str,
    pub(super) coreml_status: &'static str,
    pub(super) cache_hit: bool,
    pub(super) effective_beam_size: u32,
    pub(super) effective_temperature_fallback: bool,
    pub(super) effective_vad: bool,
    pub(super) model_load_seconds: f64,
    pub(super) decode_resample_seconds: f64,
    pub(super) language_detection_seconds: f64,
    pub(super) cache_lookup_seconds: f64,
    pub(super) diarization_model_load_seconds: f64,
    pub(super) diarization_segmentation_seconds: f64,
    pub(super) diarization_segment_extraction_seconds: f64,
    pub(super) diarization_embeddings_seconds: f64,
    pub(super) diarization_clustering_seconds: f64,
    pub(super) whisper_inference_seconds: f64,
    pub(super) word_timestamp_attribution_seconds: f64,
    pub(super) parallel_analysis_span_seconds: f64,
    pub(super) cache_write_seconds: f64,
    pub(super) merge_diagnostics_seconds: f64,
    pub(super) json_assembly_seconds: f64,
    pub(super) total_seconds: f64,
}

/// Overlap the CPU/ONNX diarization analysis with Metal Whisper inference on
/// macOS. These workloads have no shared native context: only the Whisper half
/// touches `backend`, while the worker owns fresh ONNX sessions. CPU-only
/// targets remain sequential to avoid two heavy CPU runtimes contending.
#[cfg(feature = "diarization")]
pub(super) fn run_diarization_analysis(
    audio: &[f32],
    backend: &dyn InferenceBackend,
    language: Language,
    prompt: Option<&str>,
    config: &sagascript_core::diarization::DiarizeConfig,
    options: &TranscribeOptions,
) -> Result<
    (
        sagascript_core::diarization::DiarizationAnalysis,
        sagascript_core::diarization::DiarizationTimings,
        sagascript_core::transcription::DiarizationTranscription,
    ),
    DictationError,
> {
    #[cfg(target_os = "macos")]
    {
        std::thread::scope(|scope| {
            let diarization = scope.spawn(|| sagascript_core::diarization::analyze(audio, config));
            let transcription =
                backend.transcribe_for_diarization(audio, language, prompt, options);
            let (analysis, timings) = diarization.join().map_err(|_| {
                DictationError::DiarizationError(
                    "Diarization analysis worker terminated unexpectedly".to_string(),
                )
            })??;
            Ok((analysis, timings, transcription?))
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let (analysis, timings) = sagascript_core::diarization::analyze(audio, config)?;
        let transcription = backend.transcribe_for_diarization(audio, language, prompt, options)?;
        Ok((analysis, timings, transcription))
    }
}

#[cfg(feature = "diarization")]
pub(super) fn canonicalize_coverage_segments(
    segments: Vec<TranscriptSegment>,
    duration: f64,
) -> Vec<TranscriptSegment> {
    if !duration.is_finite() || duration < 0.0 {
        return Vec::new();
    }

    segments
        .into_iter()
        .filter_map(|mut segment| {
            if !segment.start.is_finite() || !segment.end.is_finite() {
                return None;
            }
            segment.start = segment.start.clamp(0.0, duration);
            segment.end = segment.end.clamp(0.0, duration);
            (segment.end > segment.start).then_some(segment)
        })
        .collect()
}

/// Resolve the diarization decoder independently from ordinary file defaults.
/// Explicit CLI values win. Omitted flags preserve the legacy greedy/no-VAD,
/// temperature-fallback decoding regardless of ordinary file settings.
#[cfg(feature = "diarization")]
pub(super) fn resolve_diarization_options(
    args: &TranscribeArgs,
    _stored: &Settings,
) -> Result<TranscribeOptions, DictationError> {
    let beam_size = args.beam_size.unwrap_or(0);
    let vad_enabled = args.vad && !args.no_vad;
    let options = TranscribeOptions {
        prompt: None,
        beam_size,
        temperature_fallback: true,
        vad_model_path: vad_enabled.then(|| model::vad_model_path().to_string_lossy().into_owned()),
        segment_timestamps: true,
        parallel_chunks: 1,
    };
    validate_diarization_options(&options)?;
    Ok(options)
}

#[cfg(feature = "diarization")]
pub(super) fn effective_diarization_decoder_evidence(
    options: &TranscribeOptions,
) -> Result<DecoderEvidence, DictationError> {
    Ok(
        sagascript_core::transcription::whisper_backend::effective_diarization_decoder_config(
            options,
        )?
        .into(),
    )
}
