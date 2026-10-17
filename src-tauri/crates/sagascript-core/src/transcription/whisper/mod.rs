use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use tracing::{info, warn};
use whisper_rs::{
    get_lang_str, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters,
    WhisperState, WhisperVadParams,
};
#[cfg(feature = "diarization")]
use whisper_rs::{DtwMode, DtwParameters};

use crate::error::DictationError;
use crate::models::{Language, WhisperModel};
use crate::sync::{lock_or_recover, recover_poisoned};
use crate::transcription::chunking::{plan_chunks, AudioChunk};
use crate::transcription::diagnostics::LanguageDetection;
use crate::transcription::model;

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

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
fn require_packaged_x64_cpu(
    baseline: Option<&str>,
    features: [bool; 4],
) -> Result<(), DictationError> {
    match baseline {
        None => Ok(()), // Local source builds retain their own compiler policy.
        Some("avx2") if features.into_iter().all(|supported| supported) => Ok(()),
        Some("avx2") => Err(DictationError::TranscriptionFailed(
            "This Windows x64 build requires AVX2, FMA, F16C and BMI2 CPU support. Use a compatible computer or a source build configured for your CPU.".into(),
        )),
        Some(_) => Err(DictationError::TranscriptionFailed(
            "Unrecognized Windows x64 build CPU policy; install a verified Sagascript build.".into(),
        )),
    }
}

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

/// Parallel file transcription is deliberately bounded: each state owns GPU
/// buffers and a Metal command queue, so unbounded fan-out quickly wastes RAM.
pub const MAX_PARALLEL_CHUNKS: usize = 4;

/// Do not split into sub-minute pieces. Whisper loses decoder context at every
/// boundary, and short chunks do not amortize state creation.
const MIN_PARALLEL_CHUNK_SAMPLES: usize = 60 * 16_000;
const AUTO_PARALLEL_MIN_SAMPLES: usize = 10 * 60 * 16_000;
const AUTO_PARALLEL_MAX_MODEL_MB: u32 = 500;

/// Select the measured-safe automatic file-transcription parallelism.
///
/// Beam search benefits from two Metal command queues on long files. Greedy
/// decoding did not speed up in the benchmark, and large models have not yet
/// passed the memory gate, so both remain serial unless explicitly overridden.
pub fn recommended_parallel_chunks(
    audio_samples: usize,
    model: WhisperModel,
    beam_size: u32,
) -> usize {
    if beam_size >= 2
        && audio_samples >= AUTO_PARALLEL_MIN_SAMPLES
        && model.size_mb() <= AUTO_PARALLEL_MAX_MODEL_MB
    {
        2
    } else {
        1
    }
}

/// How long `with_warm_state` waits for the state mutex before giving up with
/// [`DictationError::ModelBusy`] instead of blocking a caller forever.
///
/// A normally-finishing dictation releases the state lock in well under this
/// budget (short utterances transcribe in sub-second to a couple of seconds),
/// so legitimate back-to-back dictations only ever *briefly* wait and never
/// spuriously fail. But it is bounded, so a stuck/aborting inference can no
/// longer wedge every future transcription behind an un-releasable lock — the
/// caller gets a clear "engine busy" error instead of hanging. 3s is the
/// deliberate middle of the 2–5s design range (see WP2b).
const WARM_STATE_GRACE: Duration = Duration::from_secs(3);

/// Poll interval while waiting for the state mutex during the grace budget.
const WARM_STATE_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Maximum number of resident language models, including the active model.
/// Two covers the common bilingual hotkey setup without turning the app into an
/// unbounded model server.
pub const WARM_MODEL_CACHE_MAX_MODELS: usize = 2;

/// Maximum combined advertised model size retained across the active and warm
/// secondary model. This admits the Swedish/English base pair (60 + 142 MB)
/// and rejects combinations whose steady-state footprint would be surprising.
pub const WARM_MODEL_CACHE_BUDGET_MB: u32 = 384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelAvailability {
    Active,
    Cached,
    Missing,
}

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

fn model_availability(
    active: Option<RuntimeKey>,
    cached: Option<RuntimeKey>,
    desired: RuntimeKey,
) -> ModelAvailability {
    if active == Some(desired) {
        ModelAvailability::Active
    } else if cached == Some(desired) {
        ModelAvailability::Cached
    } else {
        ModelAvailability::Missing
    }
}

fn can_cache_pair(active: RuntimeKey, secondary: RuntimeKey) -> bool {
    active != secondary
        && active
            .model
            .size_mb()
            .saturating_add(secondary.model.size_mb())
            <= WARM_MODEL_CACHE_BUDGET_MB
}

fn activate_cached_runtime<C, S>(
    desired: RuntimeKey,
    active_key: &mut Option<RuntimeKey>,
    active_context: &mut Option<C>,
    active_state: &mut Option<S>,
    cached: &mut Option<CachedWhisperRuntime<C, S>>,
) -> bool {
    let Some(cached_runtime) = cached.take() else {
        return false;
    };
    if cached_runtime.key != desired || active_key.is_none() || active_context.is_none() {
        *cached = Some(cached_runtime);
        return false;
    }

    let active_runtime = CachedWhisperRuntime {
        key: active_key.take().unwrap(),
        context: active_context.take().unwrap(),
        state: active_state.take(),
    };
    *active_key = Some(cached_runtime.key);
    *active_context = Some(cached_runtime.context);
    *active_state = cached_runtime.state;
    *cached = Some(active_runtime);
    true
}

/// The four mutexes a runtime swap takes together, borrowed so that the swap
/// and the warm-state protocol are generic over their payloads: the backend
/// passes the native `WhisperContext` and `WhisperState`, tests pass fakes.
struct RuntimeSlots<'a, C, S> {
    state: &'a Mutex<Option<S>>,
    context: &'a Mutex<Option<C>>,
    loaded_runtime: &'a Mutex<Option<RuntimeKey>>,
    cached_runtime: &'a Mutex<Option<CachedWhisperRuntime<C, S>>>,
}

/// Publish a freshly loaded context as the active runtime.
fn publish_runtime<C, S>(
    slots: &RuntimeSlots<'_, C, S>,
    ctx: C,
    desired_key: RuntimeKey,
    grace: Duration,
) -> Result<(), DictationError> {
    // Publish the new context atomically with respect to warm-state users.
    // When the old/new pair fits the explicit cache budget, move the old
    // context and its reusable warm state into the secondary slot. The next
    // switch can then restore both without touching disk or rebuilding the
    // state. Lock order is state -> context -> loaded_runtime -> cache,
    // matching activate_cached_profile().
    //
    // The state lock is acquired with the same bounded policy as
    // with_warm_state: if a stuck inference pins it past the grace budget,
    // return ModelBusy (dropping the freshly loaded `ctx` cleanly and
    // letting ensure_model release load_lock) instead of blocking a model
    // switch forever behind a wedged transcription.
    let mut state = lock_state_slot_bounded(slots.state, grace)?;
    let mut context = lock_or_recover(slots.context, CONTEXT_LOCK);
    let mut loaded_runtime = lock_or_recover(slots.loaded_runtime, LOADED_RUNTIME_LOCK);
    let previous = match (loaded_runtime.take(), context.take()) {
        (Some(key), Some(context)) => Some(CachedWhisperRuntime {
            key,
            context,
            state: state.take(),
        }),
        (None, None) => None,
        _ => panic!("whisper model/context invariant violated"),
    };

    *context = Some(ctx);
    *loaded_runtime = Some(desired_key);
    *state = None;

    let mut cached = lock_or_recover(slots.cached_runtime, CACHED_RUNTIME_LOCK);
    *cached = previous.filter(|runtime| can_cache_pair(desired_key, runtime.key));
    Ok(())
}

/// Swap the active and secondary runtimes while preserving both warm
/// states. The caller holds load_lock, so cache membership cannot change
/// concurrently with this operation.
fn activate_cached_slots<C, S>(
    slots: &RuntimeSlots<'_, C, S>,
    desired: RuntimeKey,
    grace: Duration,
) -> Result<(), DictationError> {
    let mut state = lock_state_slot_bounded(slots.state, grace)?;
    let mut context = lock_or_recover(slots.context, CONTEXT_LOCK);
    let mut loaded_runtime = lock_or_recover(slots.loaded_runtime, LOADED_RUNTIME_LOCK);
    let mut cached = lock_or_recover(slots.cached_runtime, CACHED_RUNTIME_LOCK);

    if !activate_cached_runtime(
        desired,
        &mut loaded_runtime,
        &mut context,
        &mut state,
        &mut cached,
    ) {
        return Err(DictationError::ModelNotLoaded);
    }

    info!(
        "Activated cached whisper model: {} ({:?})",
        desired.model.display_name(),
        desired.profile
    );
    Ok(())
}

/// Acquire the state mutex, polling `try_lock` on a short interval until the
/// `grace` budget elapses. Returns [`DictationError::ModelBusy`] if the lock
/// is still held when the budget runs out (a previous transcription is stuck
/// or genuinely still running), rather than blocking forever like a plain
/// `lock()` would.
///
/// A mutex poisoned by a panic mid-inference is recovered with its warm
/// state discarded (ADR 0005): the interrupted `WhisperState` may be
/// mid-compute, exactly like an aborted one, and is rebuilt lazily from the
/// active context. The recovery is logged, not masked.
fn lock_state_slot_bounded<S>(
    slot: &Mutex<Option<S>>,
    grace: Duration,
) -> Result<MutexGuard<'_, Option<S>>, DictationError> {
    let deadline = Instant::now() + grace;
    loop {
        match slot.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(poisoned)) => {
                let mut guard = recover_poisoned(slot, STATE_LOCK, poisoned);
                *guard = None;
                return Ok(guard);
            }
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    warn!(
                        "Whisper state mutex still held after {:?} grace — returning ModelBusy \
                         (a previous transcription may still be running or stuck)",
                        grace
                    );
                    return Err(DictationError::ModelBusy);
                }
                std::thread::sleep(WARM_STATE_POLL_INTERVAL);
            }
        }
    }
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

/// Timestamped transcript plus phase timings used by the diarization path.
#[cfg(feature = "diarization")]
#[derive(Debug, Clone)]
pub struct DiarizationTranscription {
    pub segments: Vec<(f64, f64, String)>,
    pub coverage_segments: Vec<TranscriptSegment>,
    pub timings: DiarizationTranscriptionTimings,
}

/// Separates native Whisper inference from Rust-side DTW word attribution.
#[cfg(feature = "diarization")]
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct DiarizationTranscriptionTimings {
    pub whisper_inference_seconds: f64,
    pub word_timestamp_attribution_seconds: f64,
}

/// DTW model preset for accurate attention-based token timestamps.
/// KB-Whisper and NB-Whisper are fine-tunes of the corresponding base architecture,
/// so they use the same alignment heads.
#[cfg(feature = "diarization")]
fn dtw_preset(model: WhisperModel) -> whisper_rs::DtwModelPreset {
    use whisper_rs::DtwModelPreset;
    match model {
        WhisperModel::TinyEn => DtwModelPreset::TinyEn,
        WhisperModel::Tiny
        | WhisperModel::FinnishWhisperTiny
        | WhisperModel::KbWhisperTiny
        | WhisperModel::NbWhisperTiny => DtwModelPreset::Tiny,
        WhisperModel::BaseEn => DtwModelPreset::BaseEn,
        WhisperModel::Base | WhisperModel::KbWhisperBase | WhisperModel::NbWhisperBase => {
            DtwModelPreset::Base
        }
        WhisperModel::SmallEn => DtwModelPreset::SmallEn,
        WhisperModel::Small | WhisperModel::KbWhisperSmall | WhisperModel::NbWhisperSmall => {
            DtwModelPreset::Small
        }
        WhisperModel::MediumEn => DtwModelPreset::MediumEn,
        WhisperModel::Medium | WhisperModel::KbWhisperMedium | WhisperModel::NbWhisperMedium => {
            DtwModelPreset::Medium
        }
        WhisperModel::LargeV3Turbo | WhisperModel::LargeV3TurboQ8 => DtwModelPreset::LargeV3Turbo,
        // KbWhisperLarge / NbWhisperLarge are large-v3 fine-tunes
        WhisperModel::KbWhisperLarge | WhisperModel::NbWhisperLarge => DtwModelPreset::LargeV3,
    }
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

/// Return whether a raw Whisper segment contains its exact no-speech marker.
///
/// The marker is only interpreted while assembling user-facing text. Raw
/// timestamped segments remain unchanged for diagnostics.
pub fn contains_no_speech_marker(text: &str) -> bool {
    text.contains("<|nospeech|>")
}

/// Assemble the plain transcript from Whisper's raw segments.
///
/// Whisper normally includes a leading space on each new segment. When it does
/// not, preserve the expected sentence boundary in the user-facing transcript
/// without changing the raw, timestamped segment text.
pub fn assemble_transcript(segments: &[TranscriptSegment]) -> String {
    let mut transcript = String::new();

    for segment in segments {
        // Whisper can prefix a silence hallucination with its internal
        // no-speech token. Dropping only the marker would still expose the
        // fabricated words that follow it, so discard the whole segment from
        // the plain user-facing transcript. Raw timestamped segments remain
        // unchanged for diagnostics.
        if contains_no_speech_marker(&segment.text) {
            continue;
        }

        let previous_ends_with_whitespace = transcript
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace);
        let next_starts_with_whitespace =
            segment.text.chars().next().is_some_and(char::is_whitespace);
        let previous_ends_sentence = transcript
            .trim_end()
            .chars()
            .next_back()
            .is_some_and(|c| matches!(c, '.' | '?' | '!'));

        if !transcript.is_empty()
            && !segment.text.is_empty()
            && !previous_ends_with_whitespace
            && !next_starts_with_whitespace
            && previous_ends_sentence
        {
            transcript.push(' ');
        }

        transcript.push_str(&segment.text);
    }

    separate_sentence_boundaries(&transcript)
}

const MIN_STITCH_ANCHOR_WORDS: usize = 4;
const MIN_STITCH_ANCHOR_CHARS: usize = 12;
const MAX_STITCH_OVERLAP_WORDS: usize = 96;
const MAX_STITCH_UNANCHORED_WORDS: usize = 32;

#[derive(Debug)]
struct StitchWord {
    normalized: String,
    start: usize,
}

/// Join independently decoded, overlapping chunks without repeating their
/// shared prefix. Matching is deliberately conservative: short repetitions
/// are preserved rather than risking deletion of real speech.
fn stitch_chunk_results(
    chunk_results: Vec<Vec<TranscriptSegment>>,
    audio_duration: f64,
) -> Vec<TranscriptSegment> {
    let mut stitched: Vec<TranscriptSegment> = Vec::new();
    let mut previous_end = 0.0;

    for chunk in chunk_results {
        if !stitched.is_empty() && !chunk.is_empty() {
            let previous_text = concatenated_segment_text(&stitched);
            let chunk_text = concatenated_segment_text(&chunk);
            if let Some(suffix_start) = overlapping_suffix_start(&previous_text, &chunk_text) {
                trim_segment_suffix(&mut stitched, suffix_start);
            }
        }

        for mut segment in chunk {
            if segment.text.trim().is_empty() {
                continue;
            }
            (segment.start, segment.end) =
                sanitize_segment_bounds(segment.start, segment.end, audio_duration, previous_end);
            previous_end = segment.end;
            stitched.push(segment);
        }
    }

    stitched
}

fn concatenated_segment_text(segments: &[TranscriptSegment]) -> String {
    let capacity = segments.iter().map(|segment| segment.text.len()).sum();
    let mut text = String::with_capacity(capacity);
    for segment in segments {
        text.push_str(&segment.text);
    }
    text
}

fn overlapping_suffix_start(previous: &str, next: &str) -> Option<usize> {
    let previous_words = stitch_words(previous);
    let next_words = stitch_words(next);
    let previous_floor = previous_words
        .len()
        .saturating_sub(MAX_STITCH_OVERLAP_WORDS);
    let next_limit = next_words.len().min(MAX_STITCH_OVERLAP_WORDS);
    let mut best: Option<(usize, usize, usize)> = None;

    for previous_index in previous_floor..previous_words.len() {
        for next_index in 0..next_limit {
            if previous_index < next_index {
                continue;
            }
            let aligned_start = previous_index - next_index;
            if aligned_start < previous_floor {
                continue;
            }

            let mut anchor_words = 0;
            while previous_index + anchor_words < previous_words.len()
                && next_index + anchor_words < next_limit
                && previous_words[previous_index + anchor_words].normalized
                    == next_words[next_index + anchor_words].normalized
            {
                anchor_words += 1;
            }
            if anchor_words < MIN_STITCH_ANCHOR_WORDS
                || next_index > MAX_STITCH_UNANCHORED_WORDS
                || previous_words.len() - previous_index - anchor_words
                    > MAX_STITCH_UNANCHORED_WORDS
            {
                continue;
            }
            let anchor_chars = next_words[next_index..next_index + anchor_words]
                .iter()
                .map(|word| word.normalized.chars().count())
                .sum::<usize>();
            if anchor_chars < MIN_STITCH_ANCHOR_CHARS {
                continue;
            }

            let candidate = (next_index, usize::MAX - anchor_words, aligned_start);
            if best.is_none_or(|current| candidate < current) {
                best = Some(candidate);
            }
        }
    }

    best.map(|(_, _, aligned_start)| previous_words[aligned_start].start)
}

fn stitch_words(text: &str) -> Vec<StitchWord> {
    let mut words = Vec::new();
    let mut start = None;

    for (index, character) in text.char_indices() {
        let continues_word = character.is_alphanumeric()
            || (start.is_some() && matches!(character, '\'' | '’' | '-'));
        if continues_word {
            if start.is_none() && character.is_alphanumeric() {
                start = Some(index);
            }
        } else if let Some(word_start) = start.take() {
            let normalized = text[word_start..index]
                .trim_end_matches(['\'', '’', '-'])
                .to_lowercase();
            if !normalized.is_empty() {
                words.push(StitchWord {
                    normalized,
                    start: word_start,
                });
            }
        }
    }

    if let Some(word_start) = start {
        let normalized = text[word_start..]
            .trim_end_matches(['\'', '’', '-'])
            .to_lowercase();
        if !normalized.is_empty() {
            words.push(StitchWord {
                normalized,
                start: word_start,
            });
        }
    }

    words
}

fn trim_segment_suffix(segments: &mut Vec<TranscriptSegment>, suffix_start: usize) {
    let mut remaining = suffix_start;
    let mut retained = Vec::with_capacity(segments.len());

    for mut segment in segments.drain(..) {
        if remaining > segment.text.len() {
            remaining -= segment.text.len();
            retained.push(segment);
            continue;
        }

        debug_assert!(segment.text.is_char_boundary(remaining));
        segment.text.truncate(remaining);
        let trimmed_len = segment.text.trim_end().len();
        segment.text.truncate(trimmed_len);
        if !segment.text.is_empty() {
            retained.push(segment);
        }
        break;
    }

    *segments = retained;
}

/// Insert a missing word boundary after sentence-ending punctuation when the
/// following character clearly begins a new sentence. Whisper can omit this
/// separator inside a single raw segment, so repairing only segment joins is
/// insufficient. Requiring an uppercase letter keeps decimals such as `1.2`
/// unchanged, while a one-character lookahead preserves the internal dots in
/// initialisms such as `U.S.A.`.
fn separate_sentence_boundaries(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut word_len = 0usize;
    let mut word_is_single_uppercase = false;

    while let Some(current) = chars.next() {
        let previous_word_is_initial = word_len == 1 && word_is_single_uppercase;
        let continues_initialism = current == '.'
            && previous_word_is_initial
            && chars.peek().is_some_and(|next| next.is_uppercase())
            && chars.clone().nth(1) == Some('.');
        result.push(current);
        if matches!(current, '.' | '?' | '!')
            && chars.peek().is_some_and(|next| next.is_uppercase())
            && !continues_initialism
        {
            result.push(' ');
        }

        if current.is_alphanumeric() {
            word_len += 1;
            word_is_single_uppercase = word_len == 1 && current.is_uppercase();
        } else if !matches!(current, '\'' | '’') {
            word_len = 0;
            word_is_single_uppercase = false;
        }
    }

    result
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

/// Remove any `<|...|>` special/timestamp tokens that could leak into segment
/// text now that generative timestamps are enabled.
fn strip_timestamp_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<|") {
        out.push_str(&rest[..start]);
        match rest[start..].find("|>") {
            Some(end) => rest = &rest[start + end + 2..],
            None => {
                rest = &rest[start..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Bound whisper's segment timestamps to the source audio and preserve output
/// ordering. Whisper can report an end past the audio duration on the padded
/// final window, or (historically, with `no_timestamps`) an internal seek offset
/// as a negative start. Those are not valid media timestamps and must not leak
/// into the CLI JSON contract.
fn sanitize_segment_bounds(
    raw_start: f64,
    raw_end: f64,
    audio_duration: f64,
    previous_end: f64,
) -> (f64, f64) {
    let duration = if audio_duration.is_finite() {
        audio_duration.max(0.0)
    } else {
        0.0
    };
    let floor = previous_end.clamp(0.0, duration);
    let start = if raw_start.is_finite() {
        raw_start.clamp(floor, duration)
    } else {
        floor
    };
    let end = if raw_end.is_finite() {
        raw_end.clamp(start, duration)
    } else {
        start
    };
    (start, end)
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

    fn runtime_slots(&self) -> RuntimeSlots<'_, WhisperContext, WhisperState> {
        RuntimeSlots {
            state: &self.state,
            context: &self.context,
            loaded_runtime: &self.loaded_runtime,
            cached_runtime: &self.cached_runtime,
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

    /// Signal any in-progress inference to abort at the next whisper.cpp compute
    /// step.
    ///
    /// A *real* abort callback (wired by [`Self::install_abort_callback`]) reads
    /// this flag before every ggml graph compute (the encoder pass and each
    /// decoder step). Setting it makes the current `full()` FFI call return
    /// promptly — whisper.cpp reports error `-6` ("failed to encode") when a
    /// compute is aborted — so the blocking task exits and releases the warm
    /// [`Self::state`] mutex instead of running to completion. This un-wedges the
    /// transcription pipeline on the timeout path (see WP2b).
    pub fn request_abort(&self) {
        warn!(
            "Transcription abort requested — signalling whisper to stop at the next compute step"
        );
        self.abort_flag.store(true, Ordering::SeqCst);
    }

    /// Persistent cancellation: aborts the running inference like
    /// [`Self::request_abort`], and makes every later inference call on this
    /// backend fail immediately with a "Cancelled" error. Never cleared; intended
    /// for one-shot CLI runs that exit after cancelling.
    pub fn request_cancel(&self) {
        self.cancel_sticky.store(true, Ordering::SeqCst);
        self.request_abort();
    }

    pub fn is_cancel_requested(&self) -> bool {
        self.cancel_sticky.load(Ordering::SeqCst)
    }

    fn cancelled_error() -> DictationError {
        DictationError::Cancelled
    }

    /// FFI abort callback. whisper.cpp invokes this before each ggml graph
    /// compute (encoder + every decoder step); returning `true` aborts the
    /// computation (`whisper_full` then returns error `-6`). `data` points at the
    /// backend's [`Self::abort_flag`] `AtomicBool`, which lives for the whole
    /// process.
    ///
    /// This raw, correctly-typed trampoline deliberately bypasses whisper-rs
    /// 0.15.1's [`FullParams::set_abort_callback_safe`], which is unsound in that
    /// release: its trampoline is instantiated as `trampoline::<F>` (the caller's
    /// closure type) but the `user_data` it stores is a *double-boxed*
    /// `Box<dyn FnMut() -> bool>`. The closure is therefore invoked through a
    /// mismatched type and returns garbage — in practice `true`, aborting every
    /// inference immediately. That is the historical "error -6 on all models" the
    /// old code worked around by leaving abort disabled. A hand-written trampoline
    /// over an `AtomicBool` avoids the buggy generic entirely.
    unsafe extern "C" fn abort_trampoline(data: *mut c_void) -> bool {
        if data.is_null() {
            return false;
        }
        // Safety: `data` is the address of the `AtomicBool` inside this backend's
        // `abort_flag` Arc, which outlives the `full()` call. We only ever read it
        // (atomically), from whichever thread whisper.cpp runs its compute on.
        let flag = unsafe { &*(data as *const AtomicBool) };
        flag.load(Ordering::Relaxed)
    }

    /// Wire the real abort callback into `params`, pointing it at `abort_flag`.
    /// The flag's lifecycle (clear-when-stale, discard-state-on-abort) is owned
    /// exclusively by the state-lock holder inside [`Self::with_warm_state_grace`]
    /// — see the ownership rules documented there.
    fn install_abort_callback(&self, params: &mut FullParams) {
        let flag_ptr = Arc::as_ptr(&self.abort_flag) as *mut c_void;
        // Safety: `flag_ptr` targets the `AtomicBool` owned by `self.abort_flag`,
        // which the backend keeps alive for its entire lifetime (well past the
        // `full()` call these params drive). The trampoline only reads the flag
        // and never touches the whisper context, satisfying the FFI contract.
        unsafe {
            params.set_abort_callback(Some(Self::abort_trampoline));
            params.set_abort_callback_user_data(flag_ptr);
        }
    }

    /// Load a model for the default low-latency transcription profile.
    pub fn load_model(&self, whisper_model: WhisperModel) -> Result<(), DictationError> {
        self.load_model_with_profile(whisper_model, ContextProfile::FlashAttention)
    }

    /// Load a specific model and immutable context profile, replacing the
    /// active runtime. DTW alignment and flash attention require different
    /// whisper contexts and therefore cannot be toggled per transcription.
    pub fn load_model_with_profile(
        &self,
        whisper_model: WhisperModel,
        profile: ContextProfile,
    ) -> Result<(), DictationError> {
        let _load = self.lock_load_bounded()?;
        self.load_model_inner(whisper_model, profile)
    }

    fn load_model_inner(
        &self,
        whisper_model: WhisperModel,
        profile: ContextProfile,
    ) -> Result<(), DictationError> {
        // Check before model-loading FFI, including default context parameters.
        // The candidate build hook pins its native library to this same policy.
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        require_packaged_x64_cpu(
            option_env!("SAGASCRIPT_WINDOWS_X64_BASELINE"),
            [
                std::is_x86_feature_detected!("avx2"),
                std::is_x86_feature_detected!("fma"),
                std::is_x86_feature_detected!("f16c"),
                std::is_x86_feature_detected!("bmi2"),
            ],
        )?;
        let desired_key = RuntimeKey {
            model: whisper_model,
            profile,
        };
        let model_path = model::model_path(whisper_model);

        if !model_path.exists() {
            return Err(DictationError::TranscriptionFailed(format!(
                "Model '{}' not downloaded. Please download it from Settings first.",
                whisper_model.display_name()
            )));
        }

        // Never hand an unverified GGML file to whisper.cpp's native parser.
        // This also performs a one-time compatibility check for files saved by
        // versions released before download integrity was enforced.
        crate::download::verify_file(&model_path, whisper_model.download_integrity())?;
        model::quarantine_unverified_coreml_encoder(whisper_model)?;
        // Fine-tunes have no Core ML encoder by design; whisper.cpp still
        // probes for one and logs an ERROR. Mark exactly that probe as expected.
        if let Some(dirname) = whisper_model.unsupported_coreml_probe_dirname() {
            crate::transcription::native_log::expect_missing_coreml_encoder(&dirname);
        }

        info!(
            "Loading whisper model: {} ({profile:?}) from {}",
            whisper_model.display_name(),
            model_path.display()
        );

        let mut ctx_params = WhisperContextParameters::default();
        match profile {
            ContextProfile::FlashAttention => {
                ctx_params.flash_attn(true);
            }
            ContextProfile::TokenAlignment => {
                #[cfg(feature = "diarization")]
                ctx_params.dtw_parameters(DtwParameters {
                    mode: DtwMode::ModelPreset {
                        model_preset: dtw_preset(whisper_model),
                    },
                    ..DtwParameters::default()
                });
                #[cfg(not(feature = "diarization"))]
                return Err(DictationError::TranscriptionFailed(
                    "Token-alignment context requires the diarization feature".to_string(),
                ));
            }
        };

        // whisper.cpp's Metal backend registration assumes that macOS returns
        // a GPU device and crashes on a null device before it can return an
        // error. Guard that FFI boundary so headless/restricted processes fail
        // cleanly while normal CLI and GUI execution remain unchanged.
        #[cfg(target_os = "macos")]
        crate::transcription::metal_preflight::ensure_available()?;

        // A third model must never overlap two already-resident runtimes while
        // its weights are being opened. Keep the active runtime available for
        // rollback, but discard the secondary before allocating the new
        // context so transient memory remains bounded to two model contexts.
        lock_or_recover(&self.cached_runtime, CACHED_RUNTIME_LOCK).take();

        let ctx = WhisperContext::new_with_params(
            model_path.to_str().ok_or_else(|| {
                DictationError::TranscriptionFailed("Invalid model path".to_string())
            })?,
            ctx_params,
        )
        .map_err(|e| DictationError::TranscriptionFailed(format!("Failed to load model: {e}")))?;

        // Publish the new context atomically with respect to warm-state users,
        // moving the previous runtime into the secondary slot when the pair fits
        // the cache budget; see publish_runtime for the lock order.
        publish_runtime(&self.runtime_slots(), ctx, desired_key, WARM_STATE_GRACE)?;

        info!(
            "Model loaded: {} ({profile:?})",
            whisper_model.display_name()
        );
        Ok(())
    }

    /// Get the currently loaded model
    pub fn loaded_model(&self) -> Option<WhisperModel> {
        lock_or_recover(&self.loaded_runtime, LOADED_RUNTIME_LOCK).map(|key| key.model)
    }

    /// Get the immutable context profile of the active runtime.
    pub fn loaded_context_profile(&self) -> Option<ContextProfile> {
        lock_or_recover(&self.loaded_runtime, LOADED_RUNTIME_LOCK).map(|key| key.profile)
    }

    /// Models currently resident in memory, active first.
    pub fn resident_models(&self) -> Vec<WhisperModel> {
        let active = self.loaded_model();
        let cached = lock_or_recover(&self.cached_runtime, CACHED_RUNTIME_LOCK)
            .as_ref()
            .map(|runtime| runtime.key.model);
        active.into_iter().chain(cached).collect()
    }

    /// Check whether selecting this model in the default flash-attention
    /// profile requires opening model weights.
    pub fn needs_reload(&self, desired_model: WhisperModel) -> bool {
        self.needs_reload_with_profile(desired_model, ContextProfile::FlashAttention)
    }

    /// Check whether selecting this model/profile runtime requires opening
    /// model weights. A warm cached runtime returns false.
    pub fn needs_reload_with_profile(
        &self,
        desired_model: WhisperModel,
        profile: ContextProfile,
    ) -> bool {
        let desired = RuntimeKey {
            model: desired_model,
            profile,
        };
        let active = *lock_or_recover(&self.loaded_runtime, LOADED_RUNTIME_LOCK);
        let cached = lock_or_recover(&self.cached_runtime, CACHED_RUNTIME_LOCK)
            .as_ref()
            .map(|runtime| runtime.key);
        model_availability(active, cached, desired) == ModelAvailability::Missing
    }

    /// Ensure the correct model is active in the default flash-attention
    /// profile.
    pub fn ensure_model(&self, desired_model: WhisperModel) -> Result<(), DictationError> {
        self.ensure_model_with_profile(desired_model, ContextProfile::FlashAttention)
    }

    /// Ensure the correct model/profile runtime is active. Serialized via
    /// `load_lock` so concurrent callers cannot race a load or cache switch.
    pub fn ensure_model_with_profile(
        &self,
        desired_model: WhisperModel,
        profile: ContextProfile,
    ) -> Result<(), DictationError> {
        let _load = self.lock_load_bounded()?;
        self.ensure_model_inner(desired_model, profile)
    }

    /// Ensure a model/profile is active and run a callback while the model
    /// selection remains pinned. This keeps model selection, context access,
    /// and inference in one transaction for callers that need to use one of
    /// the lower-level transcription entry points.
    ///
    /// The callback must not call `ensure_model*` or `load_model*` on this
    /// backend, because the bounded load lock is held until it returns.
    pub fn with_model<R>(
        &self,
        model: WhisperModel,
        profile: ContextProfile,
        callback: impl FnOnce(&Self) -> Result<R, DictationError>,
    ) -> Result<R, DictationError> {
        let _load = self.lock_load_bounded()?;
        self.ensure_model_inner(model, profile)?;
        callback(self)
    }

    fn ensure_model_inner(
        &self,
        desired_model: WhisperModel,
        profile: ContextProfile,
    ) -> Result<(), DictationError> {
        let desired = RuntimeKey {
            model: desired_model,
            profile,
        };
        let active = *lock_or_recover(&self.loaded_runtime, LOADED_RUNTIME_LOCK);
        let cached = lock_or_recover(&self.cached_runtime, CACHED_RUNTIME_LOCK)
            .as_ref()
            .map(|runtime| runtime.key);
        match model_availability(active, cached, desired) {
            ModelAvailability::Active => {}
            ModelAvailability::Cached => {
                self.activate_cached_profile(desired)?;
            }
            ModelAvailability::Missing => {
                info!("Loading model runtime: {desired:?}");
                self.load_model_inner(desired_model, profile)?;
            }
        }
        Ok(())
    }

    fn lock_load_bounded(&self) -> Result<MutexGuard<'_, ()>, DictationError> {
        let deadline = Instant::now() + WARM_STATE_GRACE;
        loop {
            match self.load_lock.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(_)) => {
                    // This mutex protects the multi-field runtime transaction,
                    // not merely its unit payload. A panic can leave context,
                    // runtime identity or warm state partially updated/poisoned.
                    // Do not recover the guard and reuse potentially invalid
                    // native state, or misreport a permanent fault as contention
                    // (the fail-closed exception in ADR 0005).
                    return Err(DictationError::TranscriptionFailed(
                        "The transcription engine was interrupted and cannot safely resume; restart Sagascript before trying again.".into(),
                    ));
                }
                Err(TryLockError::WouldBlock) if Instant::now() >= deadline => {
                    return Err(DictationError::ModelBusy);
                }
                Err(TryLockError::WouldBlock) => std::thread::sleep(WARM_STATE_POLL_INTERVAL),
            }
        }
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

    /// Swap the active and secondary runtimes while preserving both warm
    /// states; see [`activate_cached_slots`].
    fn activate_cached_profile(&self, desired: RuntimeKey) -> Result<(), DictationError> {
        activate_cached_slots(&self.runtime_slots(), desired, WARM_STATE_GRACE)
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

    /// Run a closure with the reusable warm state, creating it lazily and
    /// locking the context only briefly to do so. The context mutex is NOT held
    /// across the closure, so a long-running inference no longer blocks model
    /// (re)load. The state lock serializes concurrent transcriptions, which is
    /// required anyway — a single whisper state cannot run two `full()` calls at
    /// once.
    ///
    /// The state lock is acquired with a *bounded* wait ([`WARM_STATE_GRACE`]):
    /// if a previous transcription is stuck holding it past the budget, this
    /// returns [`DictationError::ModelBusy`] instead of blocking the caller
    /// forever. That converts the old "permanent wedge" (a hung/aborting call
    /// pinning the lock forced every future transcription to block) into a clear,
    /// recoverable error.
    fn with_warm_state<R>(
        &self,
        f: impl FnOnce(&mut WhisperState) -> Result<R, DictationError>,
    ) -> Result<R, DictationError> {
        self.with_warm_state_grace(WARM_STATE_GRACE, f)
    }

    /// [`Self::with_warm_state`] with an explicit grace budget. Split out so
    /// tests can shrink the budget and exercise the `ModelBusy` path quickly.
    ///
    /// Runs [`Self::with_state_slot`] on this backend's own state mutex, whose
    /// state is created from the loaded context.
    fn with_warm_state_grace<R>(
        &self,
        grace: Duration,
        f: impl FnOnce(&mut WhisperState) -> Result<R, DictationError>,
    ) -> Result<R, DictationError> {
        self.with_state_slot(
            &self.state,
            grace,
            || {
                // Lock order state -> context, matching load_model()'s atomic swap.
                let ctx_guard = lock_or_recover(&self.context, CONTEXT_LOCK);
                let ctx = ctx_guard.as_ref().ok_or(DictationError::ModelNotLoaded)?;
                ctx.create_state().map_err(|e| {
                    DictationError::TranscriptionFailed(format!(
                        "Failed to create whisper state: {e}"
                    ))
                })
            },
            f,
        )
    }

    /// The warm-state protocol, generic over the state payload `S` so that it
    /// can be exercised without a native `WhisperState`.
    ///
    /// This method is the SOLE owner of the abort-flag lifecycle. Ownership rule:
    /// only the call that holds the state lock may touch `abort_flag`, because a
    /// set flag always belongs to the current lock holder's inference:
    ///
    /// 1. On failing to acquire the lock (`ModelBusy`), NOTHING is touched — a
    ///    pending abort request targets the still-running holder, and clearing it
    ///    here would cancel that abort and re-wedge the pipeline.
    /// 2. Once the lock is acquired, any prior abort request is stale by
    ///    definition (its target released the lock), so it is cleared before the
    ///    closure runs.
    /// 3. After the closure returns — STILL holding the guard — if an abort was
    ///    requested during this call, the warm state is discarded (an aborted
    ///    `full()` leaves whisper mid-compute) and the flag cleared, before any
    ///    other call can acquire the lock. No gap exists in which a new call
    ///    could observe the aborted state or race the flag.
    fn with_state_slot<S, R>(
        &self,
        slot: &Mutex<Option<S>>,
        grace: Duration,
        create_state: impl FnOnce() -> Result<S, DictationError>,
        f: impl FnOnce(&mut S) -> Result<R, DictationError>,
    ) -> Result<R, DictationError> {
        let mut state_guard = lock_state_slot_bounded(slot, grace)?;

        // (2) We own the lock, so we own the flag: clear any stale abort left
        // over from a previous, already-finished inference.
        self.abort_flag.store(false, Ordering::SeqCst);
        if self.is_cancel_requested() {
            // Sticky cancel survives the stale-flag clear above: never start
            // new inference after a cancel.
            return Err(Self::cancelled_error());
        }

        // No Drop guard is needed for the abort cleanup below: if `f` panics,
        // the unwind poisons the state mutex and the next lock_state_slot_bounded
        // recovers it by discarding the warm state (the same cleanup), so a
        // plain post-call check suffices.
        let result = (|| {
            if state_guard.is_none() {
                *state_guard = Some(create_state()?);
            }
            f(state_guard.as_mut().unwrap())
        })();

        // (3) Abort cleanup under the guard: if this call's inference was
        // aborted, drop the warm state and clear the flag before releasing the
        // lock, so the next caller starts from a clean slate.
        if self.abort_flag.swap(false, Ordering::SeqCst) {
            warn!(
                "Inference was aborted — discarding warm whisper state; next transcription will rebuild it"
            );
            *state_guard = None;
        }

        result
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
    fn transcribe_parallel_segments(
        &self,
        audio: &[f32],
        chunks: &[AudioChunk],
        model: WhisperModel,
        language: Language,
        opts: &TranscribeOptions,
        token_eot: i32,
        on_progress: impl FnMut(i32) + Send + 'static,
        on_encode_start: Option<&dyn Fn()>,
    ) -> Result<Vec<TranscriptSegment>, DictationError> {
        let progress = Arc::new(Mutex::new(ParallelProgress {
            percentages: vec![0; chunks.len()],
            weights: chunks
                .iter()
                .map(|chunk| chunk.end_sample - chunk.start_sample)
                .collect(),
            total_weight: audio.len(),
            // Suppress zero-only updates before any chunk has progressed.
            last_reported: 0,
            callback: Box::new(on_progress),
        }));

        self.with_warm_state(|warm_state| {
            let mut secondary_states = {
                let context = lock_or_recover(&self.context, CONTEXT_LOCK);
                let context = context.as_ref().ok_or(DictationError::ModelNotLoaded)?;
                (1..chunks.len())
                    .map(|_| {
                        context.create_state().map_err(|error| {
                            DictationError::TranscriptionFailed(format!(
                                "Failed to create parallel whisper state: {error}"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?
            };

            // Compute resources held from here: mel/encoder work begins with
            // the first chunk dispatch below.
            if let Some(on_encode_start) = on_encode_start {
                on_encode_start();
            }

            let mut chunk_results = std::thread::scope(|scope| {
                let first_chunk = chunks[0];
                let first_progress = parallel_progress_callback(Arc::clone(&progress), 0);
                let first = scope.spawn(move || {
                    self.transcribe_chunk_segments(
                        warm_state,
                        &audio[first_chunk.decode_start_sample..first_chunk.decode_end_sample],
                        first_chunk.decode_start_sample as f64 / 16_000.0,
                        model,
                        language,
                        opts,
                        token_eot,
                        first_progress,
                    )
                });

                let others = secondary_states
                    .iter_mut()
                    .zip(chunks.iter().copied().skip(1))
                    .enumerate()
                    .map(|(index, (state, chunk))| {
                        let chunk_progress =
                            parallel_progress_callback(Arc::clone(&progress), index + 1);
                        scope.spawn(move || {
                            self.transcribe_chunk_segments(
                                state,
                                &audio[chunk.decode_start_sample..chunk.decode_end_sample],
                                chunk.decode_start_sample as f64 / 16_000.0,
                                model,
                                language,
                                opts,
                                token_eot,
                                chunk_progress,
                            )
                        })
                    })
                    .collect::<Vec<_>>();

                let mut results = Vec::with_capacity(chunks.len());
                results.push(first.join().map_err(|_| {
                    DictationError::TranscriptionFailed(
                        "Parallel whisper worker panicked".to_string(),
                    )
                })??);
                for worker in others {
                    results.push(worker.join().map_err(|_| {
                        DictationError::TranscriptionFailed(
                            "Parallel whisper worker panicked".to_string(),
                        )
                    })??);
                }
                Ok::<_, DictationError>(results)
            })?;

            let segments = stitch_chunk_results(
                std::mem::take(&mut chunk_results),
                audio.len() as f64 / 16_000.0,
            );
            info!(
                "Parallel local transcription complete: {} segment(s) across {} chunks",
                segments.len(),
                chunks.len()
            );
            Ok(segments)
        })
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

    /// Transcribe audio and return per-segment timestamps.
    ///
    /// For audio longer than 30s, uses overlapping 30s windows to prevent
    /// the Whisper decoder from drifting or looping on long recordings.
    #[cfg(feature = "diarization")]
    pub fn transcribe_sync_with_timestamps(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
    ) -> Result<Vec<(f64, f64, String)>, DictationError> {
        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }
        // Process full audio in one call — whisper.cpp handles long audio internally
        // via its own sliding window. With no_timestamps=true + DTW there is no
        // looping risk, and the internal context is better than manual chunking.
        Ok(self
            .transcribe_chunk_timestamps_profiled(
                audio,
                language,
                0.0,
                prompt,
                Granularity::Segment,
                &TranscribeOptions::default(),
            )?
            .segments)
    }

    /// Transcribe audio and return per-word timestamps.
    ///
    /// Uses DTW per-token timing from whisper.cpp to group BPE tokens into words,
    /// producing fine-grained `(start, end, word)` tuples. This is the preferred
    /// input for the diarization merge pipeline because it avoids collapsing a
    /// multi-speaker Whisper segment into a single speaker label.
    ///
    /// Falls through to empty if DTW timestamps are unavailable (caller should
    /// fall back to `transcribe_sync_with_timestamps`).
    #[cfg(feature = "diarization")]
    pub fn transcribe_sync_with_word_timestamps(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
    ) -> Result<Vec<(f64, f64, String)>, DictationError> {
        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }
        Ok(self
            .transcribe_chunk_timestamps_profiled(
                audio,
                language,
                0.0,
                prompt,
                Granularity::Word,
                &TranscribeOptions::default(),
            )?
            .segments)
    }

    /// Transcribe audio at the finest timestamp granularity available for
    /// speaker attribution.
    ///
    /// Word-level DTW timestamps prevent a long Whisper segment containing
    /// multiple turns from being assigned wholesale to one speaker. Some
    /// models/platforms may not expose valid DTW timestamps, so this preserves
    /// the segment-level fallback in one shared path for both CLI and GUI.
    #[cfg(feature = "diarization")]
    pub fn transcribe_sync_for_diarization(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
    ) -> Result<Vec<(f64, f64, String)>, DictationError> {
        Ok(self
            .transcribe_sync_for_diarization_profiled(audio, language, prompt)?
            .segments)
    }

    /// Profiled variant used by the CLI benchmark and cache path.
    #[cfg(feature = "diarization")]
    pub fn transcribe_sync_for_diarization_profiled(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
    ) -> Result<DiarizationTranscription, DictationError> {
        self.transcribe_sync_for_diarization_profiled_with_options(
            audio,
            language,
            prompt,
            &TranscribeOptions::default(),
        )
    }

    /// Profiled diarization transcription with the effective decoder and VAD
    /// options selected by the caller. The legacy profiled method above keeps
    /// its greedy/no-VAD defaults for GUI and shared reprocessing callers.
    #[cfg(feature = "diarization")]
    pub fn transcribe_sync_for_diarization_profiled_with_options(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
        options: &TranscribeOptions,
    ) -> Result<DiarizationTranscription, DictationError> {
        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }
        let decoder_config = effective_diarization_decoder_config(options)?;
        if self.loaded_context_profile() != Some(ContextProfile::TokenAlignment) {
            return Err(DictationError::TranscriptionFailed(
                "Diarization transcription requires a token-alignment model context".to_string(),
            ));
        }
        // whisper.cpp maps segment timestamps through its VAD mapping table,
        // while DTW token timestamps remain in the compacted audio timeline.
        // Use the mapped segment granularity for VAD runs so speaker merge
        // never receives silently shifted word coordinates. The explicit
        // fallback is surfaced in the CLI diagnostics and keeps timing valid.
        let granularity = if decoder_config.vad_enabled {
            warn!("VAD diarization uses mapped segment timestamps; DTW word timestamps are unavailable for compacted audio");
            Granularity::Segment
        } else {
            Granularity::Word
        };
        let mut words = self.transcribe_chunk_timestamps_profiled(
            audio,
            language,
            0.0,
            prompt,
            granularity,
            options,
        )?;
        if granularity == Granularity::Segment || !words.segments.is_empty() {
            return Ok(words);
        }

        warn!("Word-level DTW timestamps unavailable; falling back to segment timestamps");
        let fallback = self.transcribe_chunk_timestamps_profiled(
            audio,
            language,
            0.0,
            prompt,
            Granularity::Segment,
            options,
        )?;
        words.segments = fallback.segments;
        words.coverage_segments = fallback.coverage_segments;
        words.timings.whisper_inference_seconds += fallback.timings.whisper_inference_seconds;
        words.timings.word_timestamp_attribution_seconds +=
            fallback.timings.word_timestamp_attribution_seconds;
        Ok(words)
    }

    /// Transcribe a single audio chunk with timestamps via DTW.
    ///
    /// `granularity` controls whether to emit one entry per Whisper segment
    /// (`Segment`) or one entry per word (`Word`).
    ///
    /// Decodes with generative timestamps ON (see [`DecodeFlags`]) so whisper.cpp
    /// seeks to the last real timestamp instead of skipping a whole 30 s window
    /// after an early end-of-text. Word timing still comes from `token.t_dtw`
    /// (cross-attention DTW alignment); timestamp tokens are skipped when grouping.
    /// `t_offset` (seconds) is added to all returned timestamps.
    #[cfg(feature = "diarization")]
    fn transcribe_chunk_timestamps_profiled(
        &self,
        audio: &[f32],
        language: Language,
        t_offset: f64,
        prompt: Option<&str>,
        granularity: Granularity,
        options: &TranscribeOptions,
    ) -> Result<DiarizationTranscription, DictationError> {
        let model = self.loaded_model().ok_or(DictationError::ModelNotLoaded)?;
        let token_eot = {
            let guard = lock_or_recover(&self.context, CONTEXT_LOCK);
            guard
                .as_ref()
                .ok_or(DictationError::ModelNotLoaded)?
                .token_eot()
        };

        let n_threads = whisper_threads();
        let no_speech_thold = model.no_speech_threshold();

        let decoder_config = effective_diarization_decoder_config(options)?;
        let strategy = if decoder_config.beam_size >= 2 {
            SamplingStrategy::BeamSearch {
                beam_size: decoder_config.beam_size as i32,
                patience: -1.0,
            }
        } else {
            SamplingStrategy::Greedy { best_of: 1 }
        };
        let mut params = FullParams::new(strategy);
        params.set_language(language.whisper_code());
        params.set_n_threads(n_threads);
        params.set_temperature(0.0);
        params.set_temperature_inc(if decoder_config.temperature_fallback {
            0.2
        } else {
            0.0
        });
        params.set_translate(false);
        let flags = DecodeFlags::for_path(DecodePath::WordTimestamps);
        params.set_no_timestamps(flags.no_timestamps);
        params.set_token_timestamps(flags.token_timestamps); // token.t0/t1/t_dtw via cross-attention
        params.set_print_timestamps(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_no_speech_thold(no_speech_thold);
        params.set_suppress_blank(true);
        if let Some(p) = prompt {
            params.set_initial_prompt(p);
        }
        if let Some(vad_path) = &options.vad_model_path {
            model::verify_vad_model(std::path::Path::new(vad_path))?;
            params.set_vad_model_path(Some(vad_path.as_str()));
            let mut vad = WhisperVadParams::new();
            vad.set_threshold(decoder_config.vad_threshold);
            vad.set_min_silence_duration(decoder_config.vad_min_silence_duration_ms);
            vad.set_speech_pad(decoder_config.vad_speech_pad_ms);
            vad.set_samples_overlap(decoder_config.vad_samples_overlap);
            params.set_vad_params(vad);
            params.enable_vad(true);
        }

        // Same real-abort wiring as the dictation path so a timed-out diarization
        // transcription can be aborted instead of pinning the warm-state mutex.
        // The flag's lifecycle is owned by with_warm_state under the state lock.
        self.install_abort_callback(&mut params);

        self.with_warm_state(|state| {
            let inference_started = Instant::now();
            state.full(params, audio).map_err(|e| {
                DictationError::TranscriptionFailed(format!("Whisper inference failed: {e}"))
            })?;
            let whisper_inference_seconds = inference_started.elapsed().as_secs_f64();

            let attribution_started = Instant::now();
            let n_segments = state.full_n_segments();
            let mut results = Vec::with_capacity(n_segments as usize);

            let coverage_segments = match granularity {
                Granularity::Segment => {
                    let mut coverage_segments = Vec::with_capacity(n_segments as usize);
                    for i in 0..n_segments {
                        let Some(segment) = state.get_segment(i) else { continue };
                        let text = match segment.to_str() {
                            Ok(text) => strip_timestamp_tokens(text).trim().to_string(),
                            Err(error) => {
                                warn!(
                                    "Segment {i} failed UTF-8 conversion, dropping from transcript: {error}"
                                );
                                continue;
                            }
                        };
                        if text.is_empty() {
                            continue;
                        }
                        coverage_segments.push(TranscriptSegment {
                            start: segment.start_timestamp() as f64 / 100.0 + t_offset,
                            end: segment.end_timestamp() as f64 / 100.0 + t_offset,
                            text: text.clone(),
                            avg_logprob: segment_mean_logprob(&segment, token_eot),
                            no_speech_prob: segment.no_speech_probability(),
                        });
                        // Derive segment timing from DTW timestamps on first/last non-special
                        // token. DTW timestamps are in centiseconds; -1 means not computed.
                        // Fall back to segment-level timestamps if DTW was not available.
                        let (t0, t1) = if options.vad_model_path.is_some() {
                            // WhisperSegment exposes the original-timeline
                            // bounds when VAD is active. DTW token values are
                            // in compacted coordinates and must not be used.
                            (
                                segment.start_timestamp() as f64 / 100.0 + t_offset,
                                segment.end_timestamp() as f64 / 100.0 + t_offset,
                            )
                        } else {
                            dtw_segment_timestamps(&segment, t_offset).unwrap_or_else(|| {
                            let t0 = segment.start_timestamp() as f64 / 100.0 + t_offset;
                            let t1 = segment.end_timestamp() as f64 / 100.0 + t_offset;
                            (t0, t1)
                            })
                        };
                        results.push((t0, t1, text));
                    }
                    coverage_segments
                }
                Granularity::Word => {
                    // Collect per-token timings for ALL segments first, then group
                    // in a single pass so cross-segment DTW inheritance is correct.
                    let mut segs: Vec<Vec<TokenTiming>> = Vec::with_capacity(n_segments as usize);
                    let mut coverage_segments = Vec::with_capacity(n_segments as usize);
                    for i in 0..n_segments {
                        let Some(segment) = state.get_segment(i) else { continue };
                        if let Some(text) = meaningful_whisper_segment_coverage_text(&segment, i) {
                            coverage_segments.push(TranscriptSegment {
                                start: segment.start_timestamp() as f64 / 100.0 + t_offset,
                                end: segment.end_timestamp() as f64 / 100.0 + t_offset,
                                text,
                                avg_logprob: segment_mean_logprob(&segment, token_eot),
                                no_speech_prob: segment.no_speech_probability(),
                            });
                        }
                        let n_tok = segment.n_tokens();
                        let mut token_timings = Vec::with_capacity(n_tok as usize);
                        for j in 0..n_tok {
                            let Some(token) = segment.get_token(j) else { continue };
                            let td = token.token_data();
                            let text = token.to_str().unwrap_or("").to_string();
                            // t_dtw in centiseconds; -1 means invalid
                            let t_dtw = if td.t_dtw >= 0 {
                                td.t_dtw as f64 / 100.0 + t_offset
                            } else {
                                -1.0 // invalid sentinel
                            };
                            token_timings.push(TokenTiming { t_dtw, text });
                        }
                        segs.push(token_timings);
                    }
                    // words_from_segments returns empty if no valid DTW exists,
                    // which triggers the CLI fallback to segment-level timestamps.
                    results = words_from_segments(&segs);
                    coverage_segments
                }
            };

            Ok(DiarizationTranscription {
                segments: results,
                coverage_segments,
                timings: DiarizationTranscriptionTimings {
                    whisper_inference_seconds,
                    word_timestamp_attribution_seconds: attribution_started
                        .elapsed()
                        .as_secs_f64(),
                },
            })
        })
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

/// Adapt whisper.cpp's native progress values to the backend's public
/// percentage contract.
fn clamped_progress_callback(
    mut on_progress: impl FnMut(i32) + 'static,
) -> impl FnMut(i32) + 'static {
    move |percentage| on_progress(percentage.clamp(0, 100))
}

/// Wrap a progress callback so the sequence is: one initial `1` (work has
/// started), then strictly increasing real percentages up to 100. Values at or
/// below the last reported one (including whisper's initial 0) are dropped.
///
/// Returns the wrapped callback and a `begin` hook. The initial `1` is emitted
/// lazily by whichever comes first: `begin()` (called when encoding starts) or
/// the first real callback; it is never emitted twice.
fn started_progress_callback<F>(
    on_progress: F,
) -> (
    impl FnMut(i32) + Send + 'static,
    impl Fn() + Send + Sync + 'static,
)
where
    F: FnMut(i32) + Send + 'static,
{
    struct State<F> {
        on_progress: F,
        last: i32,
        started: bool,
    }
    impl<F: FnMut(i32)> State<F> {
        fn begin(&mut self) {
            if !self.started {
                self.started = true;
                self.last = 1;
                (self.on_progress)(1);
            }
        }
    }
    let state = Arc::new(Mutex::new(State {
        on_progress,
        last: 0,
        started: false,
    }));
    let begin_state = Arc::clone(&state);
    let begin = move || lock_or_recover(&begin_state, PROGRESS_LOCK).begin();
    let callback = move |percentage: i32| {
        let mut state = lock_or_recover(&state, PROGRESS_LOCK);
        state.begin();
        let percentage = percentage.clamp(0, 100);
        if percentage > state.last {
            state.last = percentage;
            (state.on_progress)(percentage);
        }
    };
    (callback, begin)
}

struct ParallelProgress {
    percentages: Vec<i32>,
    weights: Vec<usize>,
    total_weight: usize,
    last_reported: i32,
    callback: Box<dyn FnMut(i32) + Send>,
}

fn parallel_progress_callback(
    progress: Arc<Mutex<ParallelProgress>>,
    chunk_index: usize,
) -> impl FnMut(i32) + Send + 'static {
    move |percentage| {
        let mut progress = lock_or_recover(&progress, PARALLEL_PROGRESS_LOCK);
        progress.percentages[chunk_index] = percentage.clamp(0, 100);
        let weighted_sum = progress
            .percentages
            .iter()
            .zip(&progress.weights)
            .map(|(percentage, weight)| *percentage as usize * *weight)
            .sum::<usize>();
        let aggregate = (weighted_sum / progress.total_weight.max(1)) as i32;
        if aggregate > progress.last_reported {
            progress.last_reported = aggregate;
            (progress.callback)(aggregate);
        }
    }
}

/// Controls whether `transcribe_chunk_timestamps` emits one entry per segment or per word.
#[cfg(feature = "diarization")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Granularity {
    Segment,
    Word,
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

/// Return meaningful text after stripping generated timestamp/special tokens.
#[cfg(feature = "diarization")]
fn meaningful_coverage_text(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let text = strip_timestamp_tokens(&text).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Return meaningful text from a Whisper segment for coarse coverage diagnostics.
///
/// Coverage is diagnostic metadata, so invalid UTF-8 is replaced lossily. The
/// user-facing diarized transcript remains strict and is handled separately.
#[cfg(feature = "diarization")]
fn meaningful_whisper_segment_coverage_text(
    segment: &whisper_rs::WhisperSegment<'_>,
    index: i32,
) -> Option<String> {
    let bytes = match segment.to_bytes() {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!(
                "Segment {index} text could not be read, dropping from coverage diagnostics: {error}"
            );
            return None;
        }
    };
    meaningful_coverage_text(bytes)
}

/// Extract segment timing from per-token DTW timestamps.
///
/// Returns `(start_secs, end_secs)` using the DTW timestamps of the first and last
/// content tokens (skipping special tokens which have t_dtw = -1).
/// Returns `None` if no token has a valid DTW timestamp.
#[cfg(feature = "diarization")]
fn dtw_segment_timestamps(
    segment: &whisper_rs::WhisperSegment<'_>,
    t_offset: f64,
) -> Option<(f64, f64)> {
    let n = segment.n_tokens();
    let mut t0 = None::<f64>;
    let mut t1 = None::<f64>;

    for j in 0..n {
        let Some(token) = segment.get_token(j) else {
            continue;
        };
        let td = token.token_data();
        if td.t_dtw >= 0 {
            let t = td.t_dtw as f64 / 100.0 + t_offset;
            if t0.is_none() {
                t0 = Some(t);
            }
            t1 = Some(t);
        }
    }

    match (t0, t1) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

/// Input token with DTW timing and text for word grouping.
/// `t_dtw` is in seconds; use `f64::NAN` or a negative value if invalid.
#[cfg(feature = "diarization")]
#[derive(Debug, Clone)]
pub struct TokenTiming {
    /// DTW timestamp in seconds. Negative or NAN means invalid/not computed.
    pub t_dtw: f64,
    /// Raw token text as emitted by whisper (may have a leading space).
    pub text: String,
}

/// Group raw whisper tokens into words with `(start_secs, end_secs, word_text)`.
///
/// Word boundaries: a new word starts when the token text begins with a leading
/// ASCII space `' '`. The first content token also starts a word. Tokens without
/// a leading space are appended verbatim to the current word (so punctuation
/// attaches to the preceding token).
///
/// Special tokens are skipped: empty text, text starting with `'['`, or text
/// containing `"<|"`.
///
/// Timing: each word's start = min valid t_dtw among its tokens; end = max.
/// If a word has no valid DTW token, it inherits the previous word's end time
/// (best-effort interpolation). Words are never dropped.
#[cfg(feature = "diarization")]
pub fn group_tokens_into_words(tokens: &[TokenTiming]) -> Vec<(f64, f64, String)> {
    // Collect (text, Option<min_t>, Option<max_t>) per word first
    struct WordAcc {
        text: String,
        t_min: Option<f64>,
        t_max: Option<f64>,
    }

    let mut words: Vec<WordAcc> = Vec::new();

    for token in tokens {
        let raw = &token.text;

        // Skip special tokens
        if raw.is_empty() || raw.starts_with('[') || raw.contains("<|") {
            continue;
        }

        let starts_new_word = raw.starts_with(' ');
        let text_part = if starts_new_word {
            raw.trim_start_matches(' ')
        } else {
            raw.as_str()
        };

        // A leading space OR first content token ever starts a new word
        if starts_new_word || words.is_empty() {
            words.push(WordAcc {
                text: text_part.to_string(),
                t_min: None,
                t_max: None,
            });
        } else {
            // Append to the current word (punctuation etc.)
            words.last_mut().unwrap().text.push_str(text_part);
        }

        // Record valid DTW timestamp
        let t = token.t_dtw;
        if t.is_finite() && t >= 0.0 {
            let acc = words.last_mut().unwrap();
            acc.t_min = Some(acc.t_min.map_or(t, |prev| prev.min(t)));
            acc.t_max = Some(acc.t_max.map_or(t, |prev| prev.max(t)));
        }
    }

    // Second pass: resolve missing timestamps by inheriting from neighbours
    // Forward pass: propagate previous word's end to words with no timestamps
    let mut last_valid_end = 0.0f64;
    let mut resolved: Vec<(f64, f64, String)> = Vec::with_capacity(words.len());

    for acc in &words {
        let (start, end) = match (acc.t_min, acc.t_max) {
            (Some(s), Some(e)) => {
                last_valid_end = e;
                (s, e)
            }
            _ => {
                // No valid DTW: inherit previous end for both start and end
                (last_valid_end, last_valid_end)
            }
        };
        resolved.push((start, end, acc.text.clone()));
    }

    resolved
}

/// Flatten per-segment token timings into words, fixing two issues over calling
/// `group_tokens_into_words` per segment: (a) if NO token across all segments has a
/// valid DTW timestamp, return empty so the caller falls back to segment-level
/// timestamps; (b) carry missing-DTW inheritance across segment boundaries by
/// grouping in a single pass. A word boundary is forced at each segment start so
/// words never merge across Whisper segments.
#[cfg(feature = "diarization")]
fn words_from_segments(segments: &[Vec<TokenTiming>]) -> Vec<(f64, f64, String)> {
    // (a) If no token in any segment has a valid DTW timestamp, return empty so
    // the caller can fall back to segment-level timestamps.
    let any_valid = segments
        .iter()
        .flatten()
        .any(|t| t.t_dtw.is_finite() && t.t_dtw >= 0.0);
    if !any_valid {
        return Vec::new();
    }

    // Build one flat Vec<TokenTiming>, injecting a leading space on each
    // segment's first content token (unless it already has one) to force a word
    // boundary at every segment start.
    let mut flat: Vec<TokenTiming> = Vec::new();
    for segment in segments {
        let mut first_content = true;
        for token in segment {
            // Skip special tokens (same predicate as group_tokens_into_words)
            let raw = &token.text;
            if raw.is_empty() || raw.starts_with('[') || raw.contains("<|") {
                // Include as-is so group_tokens_into_words can skip them too;
                // don't touch first_content for specials.
                flat.push(token.clone());
                continue;
            }
            if first_content {
                // Force a word boundary: prepend a space if not already present.
                let text = if token.text.starts_with(' ') {
                    token.text.clone()
                } else {
                    format!(" {}", token.text)
                };
                flat.push(TokenTiming {
                    t_dtw: token.t_dtw,
                    text,
                });
                first_content = false;
            } else {
                flat.push(token.clone());
            }
        }
    }

    // (b) Single-pass grouping carries last_valid_end across all segments.
    group_tokens_into_words(&flat)
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
