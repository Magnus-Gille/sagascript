use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tracing::{info, warn};
use whisper_rs::{
    get_lang_str, FullParams, SamplingStrategy, WhisperContext, WhisperState, WhisperVadParams,
};

use crate::error::DictationError;
use crate::models::{Language, WhisperModel};
use crate::sync::lock_or_recover;
use crate::transcription::chunking::plan_chunks;
use crate::transcription::diagnostics::LanguageDetection;
use crate::transcription::model;

mod parallel;
mod progress;
mod runtime;
mod stitch;
#[cfg(feature = "diarization")]
mod words;
use parallel::MIN_PARALLEL_CHUNK_SAMPLES;
pub use parallel::{recommended_parallel_chunks, MAX_PARALLEL_CHUNKS};
use progress::{clamped_progress_callback, started_progress_callback};
pub use stitch::{assemble_transcript, contains_no_speech_marker};
use stitch::{sanitize_segment_bounds, strip_timestamp_tokens};
#[cfg(feature = "diarization")]
use words::dtw_segment_timestamps;
#[cfg(feature = "diarization")]
pub use words::{
    group_tokens_into_words, DiarizationTranscription, DiarizationTranscriptionTimings, TokenTiming,
};

// Poison policy (ADR 0005). `state` holds the reusable whisper inference
// state; a panic mid-inference leaves it mid-compute, so recovery discards it
// (the same cleanup as an abort; it is rebuilt lazily). `context`,
// `loaded_runtime` and `cached_runtime` each hold one `Option` that is only
// replaced whole, so they are recovered as-is. The multi-lock runtime
// transaction (load or cache swap) always runs under `load_lock`, whose
// poison keeps failing closed with an actionable error (see
// `lock_load_bounded`): those individual recoveries never resume a
// half-finished swap. Progress locks only guard counters.
const STATE_LOCK: &str = "whisper.state";
const CONTEXT_LOCK: &str = "whisper.context";
const LOADED_RUNTIME_LOCK: &str = "whisper.loaded_runtime";
const CACHED_RUNTIME_LOCK: &str = "whisper.cached_runtime";
const PROGRESS_LOCK: &str = "whisper.progress";
const PARALLEL_PROGRESS_LOCK: &str = "whisper.parallel_progress";

/// Default beam width for file (non-live) transcription. File transcription
/// isn't latency-sensitive, so a wider beam trades speed for fewer repetition
/// loops. Shared by the GUI file-transcribe command and the `transcribe` CLI.
pub const FILE_TRANSCRIBE_BEAM: u32 = 5;

/// Privacy-safe timings shared by live dictation and its CLI benchmark.
#[derive(Debug, Default, serde::Serialize)]
pub struct DictationTimings {
    pub model_ms: f64,
    pub inference_ms: f64,
    pub model_cached: bool,
    #[serde(skip)]
    pub model_acquisition_started: bool,
    #[serde(skip)]
    pub inference_started: bool,
    #[serde(skip)]
    pub model_ready_at: Option<Instant>,
}

/// Maximum number of resident language models, including the active model.
/// Two covers the common bilingual hotkey setup without turning the app into an
/// unbounded model server.
pub const WARM_MODEL_CACHE_MAX_MODELS: usize = 2;

/// Maximum combined advertised model size retained across the active and warm
/// secondary model. This admits the Swedish/English base pair (60 + 142 MB)
/// and rejects combinations whose steady-state footprint would be surprising.
pub const WARM_MODEL_CACHE_BUDGET_MB: u32 = 384;

/// Context-level acceleration/timestamp mode. Whisper fixes these parameters
/// when a context is created, so the profile is part of the runtime identity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ContextProfile {
    /// Exact flash-attention kernels for dictation and ordinary file
    /// transcription. This is the default profile.
    #[default]
    FlashAttention,
    /// Cross-attention DTW required for diarization token alignment.
    TokenAlignment,
}

impl ContextProfile {
    /// Select token alignment only for explicit speaker diarization.
    pub const fn for_diarization(enabled: bool) -> Self {
        if enabled {
            Self::TokenAlignment
        } else {
            Self::FlashAttention
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeKey {
    model: WhisperModel,
    profile: ContextProfile,
}

/// Per-transcription tuning knobs (opt-in modes). `Default` reproduces the
/// fast, robust dictation defaults: greedy decoding, temperature fallback on,
/// no VAD, no prompt.
#[derive(Clone)]
pub struct TranscribeOptions {
    /// Optional decoder priming prompt (domain vocabulary).
    pub prompt: Option<String>,
    /// Beam width: 0/1 = greedy (fastest); >=2 enables beam search (more
    /// accurate on hard audio, ~3-5x slower).
    pub beam_size: u32,
    /// Allow whisper's temperature fallback (re-decode harder segments at
    /// higher temperature). `true` preserves robustness; `false` caps
    /// worst-case latency.
    pub temperature_fallback: bool,
    /// Path to a Silero VAD ggml model to skip non-speech regions, or `None`
    /// to disable VAD. The caller must ensure the file exists.
    pub vad_model_path: Option<String>,
    /// Request real Whisper segment timestamps for structured outputs such as
    /// CLI JSON (DTW token timing). Generative timestamps are always on.
    pub segment_timestamps: bool,
    /// Number of independent Whisper states used for long-file transcription.
    /// Values are clamped to [`MAX_PARALLEL_CHUNKS`]; `0` and `1` are serial.
    pub parallel_chunks: usize,
}

/// Validate decoder settings before starting diarization inference.
///
/// The profiled diarization path uses DTW token alignment and therefore has a
/// smaller, explicit decoder contract than ordinary file transcription. In
/// particular, beam width one is not a distinct whisper.cpp strategy and
/// widths above eight are outside the bounded CLI/backend contract.
pub fn validate_diarization_options(options: &TranscribeOptions) -> Result<(), DictationError> {
    if options.beam_size == 1 || options.beam_size > 8 {
        return Err(DictationError::SettingsError(
            "diarization beam width must be 0 (greedy) or between 2 and 8".to_string(),
        ));
    }
    Ok(())
}

/// Effective diarization decoder settings, suitable for CLI provenance and
/// cache diagnostics. VAD parameters are fixed by this backend version so a
/// cache identity can include the actual inference contract rather than only a
/// requested boolean flag.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct DiarizationDecoderConfig {
    pub strategy: &'static str,
    pub beam_size: u32,
    pub temperature_fallback: bool,
    pub vad_enabled: bool,
    pub vad_threshold: f32,
    pub vad_min_silence_duration_ms: i32,
    pub vad_speech_pad_ms: i32,
    pub vad_samples_overlap: f32,
}

pub fn effective_diarization_decoder_config(
    options: &TranscribeOptions,
) -> Result<DiarizationDecoderConfig, DictationError> {
    validate_diarization_options(options)?;
    Ok(DiarizationDecoderConfig {
        strategy: if options.beam_size >= 2 {
            "beam_search"
        } else {
            "greedy"
        },
        beam_size: options.beam_size,
        temperature_fallback: options.temperature_fallback,
        vad_enabled: options.vad_model_path.is_some(),
        vad_threshold: 0.5,
        vad_min_silence_duration_ms: 200,
        vad_speech_pad_ms: 50,
        vad_samples_overlap: 0.1,
    })
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self {
            prompt: None,
            beam_size: 0,
            temperature_fallback: true,
            vad_model_path: None,
            segment_timestamps: false,
            parallel_chunks: 1,
        }
    }
}

/// One whisper output segment with timing and confidence metadata.
///
/// `avg_logprob` is a derived mean (whisper.cpp does not expose its internal
/// segment-level diagnostic): it averages per-token log-probabilities over the
/// segment's text tokens (special tokens excluded), matching whisper.cpp's
/// own confidence examples. `None` when a segment has no scoreable tokens.
/// Typical values: > -0.3 confident, -0.3..-0.8 shaky, < -0.8 suspect.
/// `no_speech_prob` is whisper's own per-segment estimate that the window
/// contains no speech (near 1.0 ⇒ likely hallucinated text).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TranscriptSegment {
    /// Segment start in seconds.
    pub start: f64,
    /// Segment end in seconds.
    pub end: f64,
    /// Raw segment text as Whisper emitted it. Leading space is preserved; the
    /// plain transcript may add a missing sentence-boundary separator at join time.
    pub text: String,
    /// Mean log-probability of the segment's text tokens.
    pub avg_logprob: Option<f32>,
    /// Whisper's probability that the segment window is non-speech.
    pub no_speech_prob: f32,
}

/// Mean of per-token log-probabilities; `None` for an empty slice.
fn mean_logprob(plogs: &[f32]) -> Option<f32> {
    if plogs.is_empty() {
        return None;
    }
    Some(plogs.iter().sum::<f32>() / plogs.len() as f32)
}

/// Derive the same text-token log-probability mean for every Whisper output
/// path. The binding exposes token data, but not Whisper's internal segment
/// confidence value; token IDs at or above EOT are special tokens and are
/// excluded from this diagnostic.
fn segment_mean_logprob(segment: &whisper_rs::WhisperSegment<'_>, token_eot: i32) -> Option<f32> {
    let mut plogs = Vec::with_capacity(segment.n_tokens().max(0) as usize);
    for token_index in 0..segment.n_tokens() {
        if let Some(token) = segment.get_token(token_index) {
            if token.token_id() < token_eot {
                plogs.push(token.token_data().plog);
            }
        }
    }
    mean_logprob(&plogs)
}

/// Which decoding call site is asking for flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecodePath {
    /// File transcription (single or parallel chunk), live dictation, CLI
    /// record and every other text path that goes through `full_params`.
    Text { segment_timestamps: bool },
    /// Word-timestamp / diarization / meeting path.
    #[cfg(feature = "diarization")]
    WordTimestamps,
}

/// Per-call-site whisper decoding flags, kept pure so they can be unit tested.
///
/// Generative timestamps must stay ON everywhere: with `no_timestamps` whisper.cpp
/// advances its seek by a full 30 s window after each decode, so an early
/// end-of-text after a pause silently drops the rest of the window (issue #236,
/// about 11% of words on long Swedish audio, and the tail of any <=30 s clip).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DecodeFlags {
    no_timestamps: bool,
    token_timestamps: bool,
}

impl DecodeFlags {
    fn for_path(path: DecodePath) -> Self {
        match path {
            DecodePath::Text { segment_timestamps } => Self {
                no_timestamps: false,
                token_timestamps: segment_timestamps,
            },
            #[cfg(feature = "diarization")]
            DecodePath::WordTimestamps => Self {
                no_timestamps: false,
                token_timestamps: true,
            },
        }
    }
}

fn has_live_audio_signal(audio: &[f32]) -> Result<bool, DictationError> {
    if audio.is_empty() {
        return Err(DictationError::NoAudioCaptured);
    }
    crate::audio::speech::has_audio_signal(audio)
        .map_err(|reason| DictationError::TranscriptionFailed(reason.to_string()))
}

/// Local transcription backend using whisper-rs (whisper.cpp bindings)
/// Uses GGML model files with optional CoreML acceleration on macOS.
///
/// This is managed as a separate Tauri state (not inside AppController)
/// because transcription is blocking and we must not hold the AppController
/// lock across async boundaries.
pub struct WhisperBackend {
    /// Loaded whisper context (model weights). None until load_model() is called.
    context: Mutex<Option<WhisperContext>>,
    /// Reusable inference state, kept warm across utterances so we don't pay
    /// whisper/Metal state-init (kernel compile + GPU buffer alloc) on every
    /// call. Created lazily on first transcription; reset to None on model reload.
    state: Mutex<Option<WhisperState>>,
    /// Currently loaded model and immutable context profile.
    loaded_runtime: Mutex<Option<RuntimeKey>>,
    /// One bounded secondary runtime. Its context and reusable state stay warm
    /// so a bilingual hotkey switch can swap runtimes without reopening model
    /// weights or recompiling inference state.
    cached_runtime: Mutex<Option<CachedWhisperRuntime>>,
    /// Abort flag — set to true to cancel in-progress transcription
    abort_flag: Arc<AtomicBool>,
    /// Sticky cancellation (CLI Ctrl-C): unlike `abort_flag`, lock acquisition
    /// never clears it, so a cancel landing between inference calls (gap
    /// re-decode slices, language detection) cannot be lost.
    cancel_sticky: AtomicBool,
    /// Serializes model (re)loads so concurrent `ensure_model()` callers — e.g.
    /// the startup warmup thread and the first dictation — don't load the same
    /// model twice or race the warm-state reset.
    load_lock: Mutex<()>,
}

struct CachedWhisperRuntime<C = WhisperContext, S = WhisperState> {
    key: RuntimeKey,
    context: C,
    state: Option<S>,
}

// WhisperContext is Send+Sync (it wraps a C pointer that's thread-safe)
// The Mutex handles interior mutability safely
unsafe impl Send for WhisperBackend {}
unsafe impl Sync for WhisperBackend {}

impl Default for WhisperBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WhisperBackend {
    pub fn new() -> Self {
        // whisper.cpp and GGML otherwise write their native diagnostics directly
        // to stderr, bypassing Rust's UTF-8 and log-level guarantees. Route them
        // through tracing before any context is created: the CLI suppresses
        // routine native model/kernel/token chatter, while the hook's lossy
        // C-string conversion preserves emitted errors as valid UTF-8. Application
        // warnings remain visible. whisper-rs guards installation with `Once`, so
        // concurrent backend construction remains safe.
        crate::transcription::native_log::install();

        Self {
            context: Mutex::new(None),
            state: Mutex::new(None),
            loaded_runtime: Mutex::new(None),
            cached_runtime: Mutex::new(None),
            abort_flag: Arc::new(AtomicBool::new(false)),
            cancel_sticky: AtomicBool::new(false),
            load_lock: Mutex::new(()),
        }
    }

    /// Run Whisper's local language classifier on a bounded, speech-rich
    /// preview of the decoded audio. English-only models cannot classify
    /// languages and return `Ok(None)`.
    ///
    /// The preview is capped at 30 seconds and chosen from the first five
    /// minutes by RMS energy. This keeps the check cheap while avoiding a
    /// silent intro when the recording starts later.
    pub fn detect_language(
        &self,
        audio: &[f32],
    ) -> Result<Option<LanguageDetection>, DictationError> {
        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }

        let is_multilingual = {
            let guard = lock_or_recover(&self.context, CONTEXT_LOCK);
            guard
                .as_ref()
                .ok_or(DictationError::ModelNotLoaded)?
                .is_multilingual()
        };
        if !is_multilingual {
            return Ok(None);
        }

        let preview = language_detection_preview(audio);
        let n_threads = whisper_threads().max(1) as usize;
        self.with_warm_state(|state| {
            state.pcm_to_mel(preview, n_threads).map_err(|error| {
                DictationError::TranscriptionFailed(format!(
                    "Language detection spectrogram failed: {error}"
                ))
            })?;
            let (language_id, probabilities) =
                state.lang_detect(0, n_threads).map_err(|error| {
                    DictationError::TranscriptionFailed(format!(
                        "Language detection failed: {error}"
                    ))
                })?;
            let language = get_lang_str(language_id).ok_or_else(|| {
                DictationError::TranscriptionFailed(format!(
                    "Language detection returned unknown language id {language_id}"
                ))
            })?;
            let probability = probabilities
                .get(language_id as usize)
                .copied()
                .unwrap_or_default();
            Ok(Some(LanguageDetection {
                language: language.to_string(),
                probability,
            }))
        })
    }

    /// Keep selection and inference in one transaction: startup warmup or a
    /// second language must not swap the active context between these steps.
    /// Blocking; desktop callers must use spawn_blocking.
    pub fn transcribe_dictation(
        &self,
        model: WhisperModel,
        audio: &[f32],
        language: Language,
        options: &TranscribeOptions,
        timings: &mut DictationTimings,
    ) -> Result<String, DictationError> {
        *timings = DictationTimings::default();
        let model_started = Instant::now();
        timings.model_acquisition_started = true;
        let _load = match self.lock_load_bounded() {
            Ok(load) => load,
            Err(error) => {
                timings.model_ms = model_started.elapsed().as_secs_f64() * 1000.0;
                return Err(error);
            }
        };
        timings.model_cached = !self.needs_reload(model);
        let selection = self.ensure_model_inner(model, ContextProfile::FlashAttention);
        timings.model_ms = model_started.elapsed().as_secs_f64() * 1000.0;
        selection?;
        timings.model_ready_at = Some(Instant::now());
        timings.inference_started = true;
        let inference_started = Instant::now();
        let result = self.transcribe_sync_with_options(audio, language, options, |_| {}, None);
        timings.inference_ms = inference_started.elapsed().as_secs_f64() * 1000.0;
        result
    }

    /// Transactional live-microphone transcription. The signal guard runs
    /// before model selection so empty, invalid, and near-silent captures do
    /// not acquire or load a model. File/diagnostic callers should continue to
    /// use [`Self::transcribe_dictation`] so intentional warmups remain raw.
    pub fn transcribe_live_dictation(
        &self,
        model: WhisperModel,
        audio: &[f32],
        language: Language,
        options: &TranscribeOptions,
        timings: &mut DictationTimings,
    ) -> Result<String, DictationError> {
        *timings = DictationTimings::default();
        if !has_live_audio_signal(audio)? {
            info!("Skipping near-silent dictation: {} samples", audio.len());
            return Ok(String::new());
        }
        self.transcribe_dictation(model, audio, language, options, timings)
    }

    pub fn warmup_model(
        &self,
        model: WhisperModel,
        language: Language,
    ) -> Result<(), DictationError> {
        let mut timings = DictationTimings::default();
        self.transcribe_dictation(
            model,
            &[0.0; 1600],
            language,
            &TranscribeOptions::default(),
            &mut timings,
        )?;
        Ok(())
    }

    /// Pre-compile inference kernels and warm the reusable state by running a
    /// short silent inference. Call once after the model is loaded so the first
    /// real dictation doesn't pay the Metal/CoreML kernel-compile + state-init
    /// cost. Best-effort — a failure here is non-fatal.
    pub fn warmup(&self, language: Language) -> Result<(), DictationError> {
        // 0.1s of silence is enough to build the compute graph and compile the
        // kernels (whisper pads to its 30s window internally regardless).
        let silence = vec![0.0f32; 1600];
        self.transcribe_sync(&silence, language)?;
        info!("Whisper warmup complete");
        Ok(())
    }

    /// Run transcription on loaded model (blocking — call from spawn_blocking)
    pub fn transcribe_sync(
        &self,
        audio: &[f32],
        language: Language,
    ) -> Result<String, DictationError> {
        self.transcribe_sync_with_progress(audio, language, |_| {})
    }

    /// Run transcription with a progress callback that receives percentage (0–100).
    /// The callback is invoked from the whisper.cpp inference thread.
    pub fn transcribe_sync_with_progress(
        &self,
        audio: &[f32],
        language: Language,
        on_progress: impl FnMut(i32) + Send + 'static,
    ) -> Result<String, DictationError> {
        self.transcribe_sync_with_progress_and_prompt(audio, language, None, on_progress)
    }

    /// Like `transcribe_sync_with_progress` but accepts an optional initial prompt
    /// to prime the decoder with domain-specific vocabulary and context.
    pub fn transcribe_sync_with_progress_and_prompt(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
        on_progress: impl FnMut(i32) + Send + 'static,
    ) -> Result<String, DictationError> {
        let opts = TranscribeOptions {
            prompt: prompt.map(str::to_string),
            ..Default::default()
        };
        self.transcribe_sync_with_options(audio, language, &opts, on_progress, None)
    }

    /// Live microphone transcription with a conservative near-silence guard.
    /// File/diagnostic APIs and intentional silent model warmup remain ungated.
    pub fn transcribe_live_sync_with_options(
        &self,
        audio: &[f32],
        language: Language,
        opts: &TranscribeOptions,
        on_progress: impl FnMut(i32) + Send + 'static,
    ) -> Result<String, DictationError> {
        if !has_live_audio_signal(audio)? {
            info!("Skipping near-silent dictation: {} samples", audio.len());
            return Ok(String::new());
        }
        self.transcribe_sync_with_options(audio, language, opts, on_progress, None)
    }

    /// Core transcription entry point. Honors the opt-in [`TranscribeOptions`]
    /// (prompt, beam search, temperature fallback, VAD). Blocking — call from
    /// spawn_blocking.
    ///
    /// Thin wrapper over [`Self::transcribe_sync_with_options_segments`] that
    /// concatenates the raw segment texts, then applies display-only
    /// non-speech marker normalization. Timestamped segment text stays raw.
    /// `on_encode_start`, when given, fires once compute resources are held
    /// and mel/encoder work is about to begin (the silent window before the
    /// first progress callback).
    pub fn transcribe_sync_with_options(
        &self,
        audio: &[f32],
        language: Language,
        opts: &TranscribeOptions,
        on_progress: impl FnMut(i32) + Send + 'static,
        on_encode_start: Option<&dyn Fn()>,
    ) -> Result<String, DictationError> {
        let segments = self.transcribe_sync_with_options_segments(
            audio,
            language,
            opts,
            on_progress,
            on_encode_start,
        )?;
        let transcript = assemble_transcript(&segments);
        Ok(crate::transcription::normalize_nonspeech_markers(
            transcript.trim(),
            language,
        ))
    }

    /// Plain-file transcription with recovery of speech Whisper's windowing
    /// skipped (#273): runs [`Self::transcribe_sync_with_options_segments`],
    /// then re-decodes dense-speech uncovered spans once, within a bounded
    /// budget (see [`crate::transcription::gap_redecode`]). Shared by the CLI and the app's
    /// plain file transcription; diarization and meeting paths do not use it.
    pub fn transcribe_sync_with_gap_recovery(
        &self,
        audio: &[f32],
        language: Language,
        opts: &TranscribeOptions,
        on_progress: impl FnMut(i32) + Send + 'static,
        on_encode_start: Option<&dyn Fn()>,
    ) -> Result<Vec<TranscriptSegment>, DictationError> {
        let segments = self.transcribe_sync_with_options_segments(
            audio,
            language,
            opts,
            on_progress,
            on_encode_start,
        )?;
        let model = self.loaded_model().ok_or(DictationError::ModelNotLoaded)?;
        let limits = crate::transcription::gap_redecode::RedecodeLimits::for_audio(
            audio.len() as f64 / 16_000.0,
            model.no_speech_threshold(),
        );
        let mut redecode_opts = opts.clone();
        redecode_opts.parallel_chunks = 1;
        let recovered = crate::transcription::gap_redecode::redecode_uncovered_speech_cancellable(
            audio,
            segments,
            &limits,
            &|| self.is_cancel_requested(),
            |slice| {
                self.transcribe_sync_with_options_segments(
                    slice,
                    language,
                    &redecode_opts,
                    |_| {},
                    None,
                )
            },
        );
        if self.is_cancel_requested() {
            return Err(Self::cancelled_error());
        }
        Ok(recovered)
    }

    /// Text form of [`Self::transcribe_sync_with_gap_recovery`], with the
    /// same display normalization as [`Self::transcribe_sync_with_options`].
    pub fn transcribe_sync_with_gap_recovery_text(
        &self,
        audio: &[f32],
        language: Language,
        opts: &TranscribeOptions,
        on_progress: impl FnMut(i32) + Send + 'static,
        on_encode_start: Option<&dyn Fn()>,
    ) -> Result<String, DictationError> {
        let segments = self.transcribe_sync_with_gap_recovery(
            audio,
            language,
            opts,
            on_progress,
            on_encode_start,
        )?;
        let transcript = assemble_transcript(&segments);
        Ok(crate::transcription::normalize_nonspeech_markers(
            transcript.trim(),
            language,
        ))
    }

    /// Like [`Self::transcribe_sync_with_options`] but returns the individual
    /// whisper segments with timing and confidence metadata
    /// ([`TranscriptSegment`]) instead of one joined string. Blocking — call
    /// from spawn_blocking.
    pub fn transcribe_sync_with_options_segments(
        &self,
        audio: &[f32],
        language: Language,
        opts: &TranscribeOptions,
        on_progress: impl FnMut(i32) + Send + 'static,
        on_encode_start: Option<&dyn Fn()>,
    ) -> Result<Vec<TranscriptSegment>, DictationError> {
        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }

        let model = self.loaded_model().ok_or(DictationError::ModelNotLoaded)?;

        // End-of-text token id, used to exclude special tokens from the
        // avg-logprob computation (matching whisper.cpp's confidence examples:
        // text tokens have id < eot). Read in a short scope so the context
        // lock is released before with_warm_state re-acquires it.
        let token_eot = {
            let guard = lock_or_recover(&self.context, CONTEXT_LOCK);
            guard
                .as_ref()
                .ok_or(DictationError::ModelNotLoaded)?
                .token_eot()
        };

        // Leave 0% (#237) once compute resources are held, i.e. right after
        // `on_encode_start` fires (or on the first real callback), so the
        // phase never goes "transcribing 1%" -> "encoding" and state-lock
        // waiting is not counted as transcribing.
        let (on_progress, begin_progress) = started_progress_callback(on_progress);
        let encode_start_then_begin = || {
            if let Some(on_encode_start) = on_encode_start {
                on_encode_start();
            }
            begin_progress();
        };
        let on_encode_start: Option<&dyn Fn()> = Some(&encode_start_then_begin);

        let chunks = plan_chunks(
            audio,
            opts.parallel_chunks.clamp(1, MAX_PARALLEL_CHUNKS),
            MIN_PARALLEL_CHUNK_SAMPLES,
        );

        info!(
            "Starting local transcription: {} samples, {} threads/state, {} chunk(s), lang={:?}, beam={}, temp_fallback={}, vad={}",
            audio.len(),
            whisper_threads(),
            chunks.len(),
            language,
            opts.beam_size,
            opts.temperature_fallback,
            opts.vad_model_path.is_some()
        );

        if chunks.len() == 1 {
            return self.with_warm_state(|state| {
                if let Some(on_encode_start) = on_encode_start {
                    on_encode_start();
                }
                self.transcribe_chunk_segments(
                    state,
                    audio,
                    0.0,
                    model,
                    language,
                    opts,
                    token_eot,
                    on_progress,
                )
            });
        }

        self.transcribe_parallel_segments(
            audio,
            &chunks,
            model,
            language,
            opts,
            token_eot,
            on_progress,
            on_encode_start,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn transcribe_chunk_segments(
        &self,
        state: &mut WhisperState,
        audio: &[f32],
        offset_seconds: f64,
        model: WhisperModel,
        language: Language,
        opts: &TranscribeOptions,
        token_eot: i32,
        on_progress: impl FnMut(i32) + 'static,
    ) -> Result<Vec<TranscriptSegment>, DictationError> {
        let mut params = self.full_params(model, language, opts, on_progress)?;
        self.install_abort_callback(&mut params);
        state.full(params, audio).map_err(|error| {
            DictationError::TranscriptionFailed(format!("Whisper inference failed: {error}"))
        })?;

        let n_segments = state.full_n_segments();
        let mut segments = Vec::with_capacity(n_segments.max(0) as usize);
        let chunk_duration = audio.len() as f64 / 16_000.0;
        let mut previous_end = 0.0;
        for index in 0..n_segments {
            let Some(segment) = state.get_segment(index) else {
                continue;
            };
            let text = match segment.to_str() {
                Ok(text) => strip_timestamp_tokens(text),
                Err(error) => {
                    warn!(
                        "Segment {index} failed UTF-8 conversion, dropping from transcript: {error}"
                    );
                    continue;
                }
            };
            let raw_bounds = {
                #[cfg(feature = "diarization")]
                {
                    if opts.segment_timestamps {
                        dtw_segment_timestamps(&segment, 0.0).unwrap_or_else(|| {
                            (
                                segment.start_timestamp() as f64 / 100.0,
                                segment.end_timestamp() as f64 / 100.0,
                            )
                        })
                    } else {
                        (
                            segment.start_timestamp() as f64 / 100.0,
                            segment.end_timestamp() as f64 / 100.0,
                        )
                    }
                }
                #[cfg(not(feature = "diarization"))]
                {
                    (
                        segment.start_timestamp() as f64 / 100.0,
                        segment.end_timestamp() as f64 / 100.0,
                    )
                }
            };
            let (start, end) =
                sanitize_segment_bounds(raw_bounds.0, raw_bounds.1, chunk_duration, previous_end);
            previous_end = end;
            segments.push(TranscriptSegment {
                start: start + offset_seconds,
                end: end + offset_seconds,
                text,
                avg_logprob: segment_mean_logprob(&segment, token_eot),
                no_speech_prob: segment.no_speech_probability(),
            });
        }
        Ok(segments)
    }

    fn full_params(
        &self,
        model: WhisperModel,
        language: Language,
        opts: &TranscribeOptions,
        on_progress: impl FnMut(i32) + 'static,
    ) -> Result<FullParams<'static, 'static>, DictationError> {
        let beam_size = opts.beam_size.clamp(0, 8);
        let strategy = if beam_size >= 2 {
            SamplingStrategy::BeamSearch {
                beam_size: beam_size as i32,
                patience: -1.0,
            }
        } else {
            SamplingStrategy::Greedy { best_of: 1 }
        };
        let mut params = FullParams::new(strategy);
        params.set_language(language.whisper_code());
        params.set_n_threads(whisper_threads());
        params.set_temperature(0.0);
        params.set_temperature_inc(if opts.temperature_fallback { 0.2 } else { 0.0 });
        params.set_translate(false);
        let flags = DecodeFlags::for_path(DecodePath::Text {
            segment_timestamps: opts.segment_timestamps,
        });
        params.set_no_timestamps(flags.no_timestamps);
        params.set_token_timestamps(flags.token_timestamps);
        params.set_print_timestamps(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_no_speech_thold(model.no_speech_threshold());
        params.set_suppress_blank(true);
        if let Some(prompt) = &opts.prompt {
            if !prompt.is_empty() {
                params.set_initial_prompt(prompt);
            }
        }
        if let Some(vad_path) = &opts.vad_model_path {
            model::verify_vad_model(std::path::Path::new(vad_path))?;
            params.set_vad_model_path(Some(vad_path.as_str()));
            let mut vad = WhisperVadParams::new();
            vad.set_threshold(0.5);
            vad.set_min_silence_duration(200);
            vad.set_speech_pad(50);
            params.set_vad_params(vad);
            params.enable_vad(true);
        }
        params.set_progress_callback_safe(clamped_progress_callback(on_progress));
        Ok(params)
    }
}

/// Choose at most 30 seconds from the first five minutes for language
/// detection, preferring the non-overlapping window with the highest RMS
/// energy. Scanning non-overlapping windows keeps this linear in the bounded
/// prefix and is sufficient to skip long silent intros.
fn language_detection_preview(audio: &[f32]) -> &[f32] {
    const PREVIEW_SAMPLES: usize = 30 * 16_000;
    const SCAN_SAMPLES: usize = 5 * 60 * 16_000;

    if audio.len() <= PREVIEW_SAMPLES {
        return audio;
    }

    let scan_end = audio.len().min(SCAN_SAMPLES);
    let mut best_start = 0usize;
    let mut best_mean_square = f64::NEG_INFINITY;
    for start in (0..scan_end).step_by(PREVIEW_SAMPLES) {
        let end = (start + PREVIEW_SAMPLES).min(scan_end);
        let window = &audio[start..end];
        let mean_square = window
            .iter()
            .map(|&sample| f64::from(sample) * f64::from(sample))
            .sum::<f64>()
            / window.len().max(1) as f64;
        if mean_square > best_mean_square {
            best_mean_square = mean_square;
            best_start = start;
        }
    }
    let best_end = (best_start + PREVIEW_SAMPLES).min(audio.len());
    &audio[best_start..best_end]
}

/// Choose a whisper CPU thread count. `num_cpus` counts all cores; on Apple
/// Silicon that includes the slower efficiency cores, and scheduling whisper's
/// CPU work onto them hurts latency. Target the performance-core count
/// (`hw.perflevel0.logicalcpu`) on macOS, falling back to `num_cpus / 2`
/// elsewhere or if the sysctl is unavailable. Computed once and cached.
fn whisper_threads() -> i32 {
    use std::sync::OnceLock;
    static THREADS: OnceLock<i32> = OnceLock::new();
    *THREADS.get_or_init(|| {
        #[cfg(target_os = "macos")]
        if let Some(n) = macos_perf_cores() {
            return n;
        }
        (num_cpus::get() / 2).max(1) as i32
    })
}

/// Number of performance (P) cores on Apple Silicon via sysctl. Returns `None`
/// on Intel Macs (key absent) or if the query fails.
#[cfg(target_os = "macos")]
fn macos_perf_cores() -> Option<i32> {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "hw.perflevel0.logicalcpu"])
        .output()
        .ok()?;
    String::from_utf8(out.stdout)
        .ok()?
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|&n| n > 0)
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
