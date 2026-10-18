//! Local Whisper backend (whisper-rs). `WhisperBackend` and the types its callers
//! share live here; the behaviour is split by concern (#371):
//!
//! - `runtime`: model load, the warm secondary runtime, the bounded state and
//!   load locks, abort and cancel;
//! - `entry`: the transcription entry points and the decoding parameters;
//! - `parallel`: long-file chunking across several Whisper states;
//! - `stitch`: joining overlapping chunk results and assembling the transcript;
//! - `progress`: progress callbacks;
//! - `words`: DTW word timestamps for diarization (the `diarization` feature).
//!
//! The old path `transcription::whisper_backend` is an alias of this module.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use whisper_rs::{WhisperContext, WhisperState};

use crate::error::DictationError;
use crate::models::WhisperModel;

mod entry;
mod parallel;
mod progress;
mod runtime;
mod stitch;
#[cfg(feature = "diarization")]
mod words;

pub use parallel::{recommended_parallel_chunks, MAX_PARALLEL_CHUNKS};
pub use stitch::{assemble_transcript, contains_no_speech_marker};
#[cfg(feature = "diarization")]
pub use words::{
    group_tokens_into_words, DiarizationTranscription, DiarizationTranscriptionTimings, TokenTiming,
};

// Lock order (docs/ARCHITECTURE.md section 3), outermost first:
//
//     load_lock -> state -> loaded_runtime -> cached_runtime
//
// - `load_lock` serializes model loads and selection; dictation and
//   `with_model` hold it across inference. `state` serializes inference on the
//   one reusable `WhisperState`. Both are taken with a bounded wait
//   (`WARM_STATE_GRACE`), so an inversion or a stuck holder surfaces as
//   `ModelBusy` instead of a deadlock.
// - `loaded_runtime` holds the active model identity and its native context as
//   one value, `Option<Loaded>`: there is no state in which one exists without
//   the other. A runtime swap (`publish_runtime`, `activate_cached_slots`)
//   takes `state`, then `loaded_runtime`, then `cached_runtime`. Everything
//   else only takes `loaded_runtime` briefly, never while holding
//   `cached_runtime`; the context is cloned out of it (`active_context`)
//   instead of being used under the lock, so a lookup never waits for a state
//   to be built.
// - Progress locks only guard counters and are leaves.
//
// Poison policy (ADR 0005). `state` holds the reusable whisper inference
// state; a panic mid-inference leaves it mid-compute, so recovery discards it
// (the same cleanup as an abort; it is rebuilt lazily). `loaded_runtime` and
// `cached_runtime` each hold one `Option` that is only replaced whole, so they
// are recovered as-is. The multi-lock runtime transaction (load or cache swap)
// always runs under `load_lock`, whose poison keeps failing closed with an
// actionable error (see `lock_load_bounded`): those individual recoveries
// never resume a half-finished swap.
const STATE_LOCK: &str = "whisper.state";
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

/// Local transcription backend using whisper-rs (whisper.cpp bindings)
/// Uses GGML model files with optional CoreML acceleration on macOS.
///
/// This is managed as a separate Tauri state (not inside AppController)
/// because transcription is blocking and we must not hold the AppController
/// lock across async boundaries.
pub struct WhisperBackend {
    /// Reusable inference state, kept warm across utterances so we don't pay
    /// whisper/Metal state-init (kernel compile + GPU buffer alloc) on every
    /// call. Created lazily on first transcription; reset to None on model reload.
    state: Mutex<Option<WhisperState>>,
    /// The active runtime: model, immutable context profile and the context
    /// holding its weights. None until `load_model()` is called.
    loaded_runtime: Mutex<Option<Loaded>>,
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

/// The active runtime: which model and profile is loaded, and the context that
/// holds its weights. Both live in one value behind one lock, so an identity
/// without a context, or a context without an identity, cannot be represented.
///
/// The context is shared (`Arc`) only so that a reader can take a handle and
/// release the lock before using it (see `active_context`). Handles are
/// short-lived, held for one native call; if a swap displaces the runtime
/// meanwhile, the last handle frees the context.
struct Loaded<C = WhisperContext> {
    key: RuntimeKey,
    ctx: Arc<C>,
}

/// A runtime kept warm in the secondary slot: its identity and context, and
/// the reusable state that was warm for it.
struct CachedWhisperRuntime<C = WhisperContext, S = WhisperState> {
    runtime: Loaded<C>,
    state: Option<S>,
}

// The backend is shared across threads (Tauri state, `spawn_blocking`). It is
// Send + Sync by composition: every field is a `Mutex` or an atomic, and the
// native `WhisperContext` and `WhisperState` inside are declared Send + Sync by
// whisper-rs 0.15. Nothing is asserted by hand, so a field or dependency update
// that loses either trait stops compiling here.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<WhisperBackend>();
};

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
            state: Mutex::new(None),
            loaded_runtime: Mutex::new(None),
            cached_runtime: Mutex::new(None),
            abort_flag: Arc::new(AtomicBool::new(false)),
            cancel_sticky: AtomicBool::new(false),
            load_lock: Mutex::new(()),
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
