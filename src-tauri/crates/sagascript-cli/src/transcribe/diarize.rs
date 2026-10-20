//! The diarization half of a file run (#343): the cache policy, the speaker
//! analysis seam, the decoder the diarized path uses, the concurrent analysis
//! and the performance report.

#[cfg(feature = "diarization")]
use sagascript_core::diarization::report::DecoderEvidence;
#[cfg(feature = "diarization")]
use sagascript_core::diarization::{DiarizationAnalysis, DiarizationTimings, DiarizeConfig};
#[cfg(feature = "diarization")]
use sagascript_core::error::DictationError;
#[cfg(feature = "diarization")]
use sagascript_core::settings::Language;
#[cfg(feature = "diarization")]
use sagascript_core::transcription::model;
#[cfg(feature = "diarization")]
use sagascript_core::transcription::whisper_backend::validate_diarization_options;
#[cfg(feature = "diarization")]
use sagascript_core::transcription::{
    DiarizationTranscription, TranscribeOptions, TranscriptSegment,
};

#[cfg(feature = "diarization")]
use super::file_plan::DecoderFlags;
#[cfg(feature = "diarization")]
use super::services::Services;

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

/// The speaker analysis of a diarized run as the run uses it: whether its
/// models are installed, and the analysis of 16 kHz audio (segmentation and
/// speaker embeddings, before any clustering).
///
/// The same pattern as [`InferenceBackend`](super::inference::InferenceBackend):
/// [`OnnxAnalyzer`] is the one implementation that ships, and a test supplies
/// its own, so the diarized path (the cache, the concurrency, the clustering and
/// the result) can be driven without ONNX models.
///
/// `Sync` because on macOS the analysis runs on another thread while the
/// Whisper half uses the backend.
#[cfg(feature = "diarization")]
pub(super) trait DiarizationAnalyzer: Sync {
    /// Whether the diarization models are installed.
    fn models_downloaded(&self) -> bool;

    /// Segments `audio` and embeds the speakers of its segments.
    fn analyze(
        &self,
        audio: &[f32],
        config: &DiarizeConfig,
    ) -> Result<(DiarizationAnalysis, DiarizationTimings), DictationError>;
}

/// The analysis of an installed copy: the ONNX segmentation and embedding
/// models that `download-model diarization` put in the models directory.
#[cfg(feature = "diarization")]
pub(super) struct OnnxAnalyzer;

#[cfg(feature = "diarization")]
impl DiarizationAnalyzer for OnnxAnalyzer {
    fn models_downloaded(&self) -> bool {
        sagascript_core::diarization::model::all_models_downloaded()
    }

    fn analyze(
        &self,
        audio: &[f32],
        config: &DiarizeConfig,
    ) -> Result<(DiarizationAnalysis, DiarizationTimings), DictationError> {
        sagascript_core::diarization::analyze(audio, config)
    }
}

/// What the two halves of a diarized run bring back: the speaker analysis and
/// its timings, and the Whisper word timings.
#[cfg(feature = "diarization")]
pub(super) type AnalysisOutcome = (
    DiarizationAnalysis,
    DiarizationTimings,
    DiarizationTranscription,
);

/// Runs the speaker analysis and the Whisper half of a diarized run: overlapped
/// on macOS, one after the other elsewhere.
///
/// The overlap puts the CPU/ONNX analysis beside Metal Whisper inference.
/// These workloads have no shared native context: only the Whisper half
/// touches the backend, while the worker owns fresh ONNX sessions. CPU-only
/// targets stay sequential to avoid two heavy CPU runtimes contending. Both
/// are compiled everywhere (the platform picks one), so a test can run either.
#[cfg(feature = "diarization")]
pub(super) fn run_diarization_analysis(
    audio: &[f32],
    services: Services<'_>,
    language: Language,
    prompt: Option<&str>,
    config: &DiarizeConfig,
    options: &TranscribeOptions,
) -> Result<AnalysisOutcome, DictationError> {
    if cfg!(target_os = "macos") {
        analyze_overlapped(audio, services, language, prompt, config, options)
    } else {
        analyze_in_sequence(audio, services, language, prompt, config, options)
    }
}

/// The analysis on a worker thread while this one runs the Whisper half. The
/// analysis' error comes first, as in the sequential order.
#[cfg(feature = "diarization")]
pub(super) fn analyze_overlapped(
    audio: &[f32],
    services: Services<'_>,
    language: Language,
    prompt: Option<&str>,
    config: &DiarizeConfig,
    options: &TranscribeOptions,
) -> Result<AnalysisOutcome, DictationError> {
    let analyzer = services.analyzer;
    std::thread::scope(|scope| {
        let diarization = scope.spawn(move || analyzer.analyze(audio, config));
        let transcription = services
            .backend
            .transcribe_for_diarization(audio, language, prompt, options);
        let (analysis, timings) = diarization.join().map_err(|_| {
            DictationError::DiarizationError(
                "Diarization analysis worker terminated unexpectedly".to_string(),
            )
        })??;
        Ok((analysis, timings, transcription?))
    })
}

/// The analysis, then the Whisper half; a failed analysis ends the run before
/// Whisper starts.
#[cfg(feature = "diarization")]
pub(super) fn analyze_in_sequence(
    audio: &[f32],
    services: Services<'_>,
    language: Language,
    prompt: Option<&str>,
    config: &DiarizeConfig,
    options: &TranscribeOptions,
) -> Result<AnalysisOutcome, DictationError> {
    let (analysis, timings) = services.analyzer.analyze(audio, config)?;
    let transcription = services
        .backend
        .transcribe_for_diarization(audio, language, prompt, options)?;
    Ok((analysis, timings, transcription))
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
    decoder: &DecoderFlags,
) -> Result<TranscribeOptions, DictationError> {
    let beam_size = decoder.beam_size.unwrap_or(0);
    let vad_enabled = decoder.vad && !decoder.no_vad;
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
