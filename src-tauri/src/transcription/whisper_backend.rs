use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tracing::{info, warn};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    WhisperVadParams,
};
#[cfg(feature = "diarization")]
use whisper_rs::{DtwMode, DtwParameters};

use crate::error::DictationError;
use crate::settings::{Language, WhisperModel};
use crate::transcription::model;

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
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self {
            prompt: None,
            beam_size: 0,
            temperature_fallback: true,
            vad_model_path: None,
        }
    }
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
    /// Currently loaded model
    loaded_model: Mutex<Option<WhisperModel>>,
    /// Abort flag — set to true to cancel in-progress transcription
    abort_flag: Arc<AtomicBool>,
}

// WhisperContext is Send+Sync (it wraps a C pointer that's thread-safe)
// The Mutex handles interior mutability safely
unsafe impl Send for WhisperBackend {}
unsafe impl Sync for WhisperBackend {}

impl WhisperBackend {
    pub fn new() -> Self {
        Self {
            context: Mutex::new(None),
            state: Mutex::new(None),
            loaded_model: Mutex::new(None),
            abort_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Signal the whisper inference to abort.
    ///
    /// NOTE: The abort callback that reads this flag is currently disabled
    /// (whisper-rs 0.15.1 `set_abort_callback_safe` causes error -6).
    /// This flag is still set for forward compatibility but has no effect
    /// on in-progress inference until the callback is restored.
    pub fn request_abort(&self) {
        warn!("Transcription abort requested (NOTE: abort callback disabled — inference will run to completion)");
        self.abort_flag.store(true, Ordering::SeqCst);
    }

    /// Load a specific model, replacing any previously loaded model
    pub fn load_model(&self, whisper_model: WhisperModel) -> Result<(), DictationError> {
        let model_path = model::model_path(whisper_model);

        if !model_path.exists() {
            return Err(DictationError::TranscriptionFailed(format!(
                "Model '{}' not downloaded. Please download it from Settings first.",
                whisper_model.display_name()
            )));
        }

        info!(
            "Loading whisper model: {} from {}",
            whisper_model.display_name(),
            model_path.display()
        );

        // Flash attention is an exact (not approximate) attention kernel that is
        // accelerated on Metal — a free speedup with identical output. It is
        // incompatible with DTW: whisper.cpp silently disables DTW token
        // timestamps when flash_attn is on. So the default (dictation) build
        // turns it ON, and the diarization build leaves it off and uses DTW for
        // attention-based token timestamps (used by --diarize) instead.
        let ctx_params = {
            let mut p = WhisperContextParameters::default();
            #[cfg(not(feature = "diarization"))]
            p.flash_attn(true);
            #[cfg(feature = "diarization")]
            p.dtw_parameters(DtwParameters {
                mode: DtwMode::ModelPreset {
                    model_preset: whisper_model.dtw_preset(),
                },
                ..DtwParameters::default()
            });
            p
        };

        let ctx = WhisperContext::new_with_params(
            model_path.to_str().ok_or_else(|| {
                DictationError::TranscriptionFailed("Invalid model path".to_string())
            })?,
            ctx_params,
        )
        .map_err(|e| {
            DictationError::TranscriptionFailed(format!("Failed to load model: {e}"))
        })?;

        *self.context.lock().unwrap() = Some(ctx);
        *self.loaded_model.lock().unwrap() = Some(whisper_model);
        // Drop any state bound to the previous model; the next transcription
        // lazily creates a fresh state for the new context.
        *self.state.lock().unwrap() = None;

        info!("Model loaded: {}", whisper_model.display_name());
        Ok(())
    }

    /// Get the currently loaded model
    pub fn loaded_model(&self) -> Option<WhisperModel> {
        *self.loaded_model.lock().unwrap()
    }

    /// Check if the correct model is loaded for the given settings
    pub fn needs_reload(&self, desired_model: WhisperModel) -> bool {
        self.loaded_model() != Some(desired_model)
    }

    /// Ensure the correct model is loaded
    pub fn ensure_model(&self, desired_model: WhisperModel) -> Result<(), DictationError> {
        if self.needs_reload(desired_model) {
            info!("Loading model: {:?}", desired_model);
            self.load_model(desired_model)?;
        }
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

    /// Run a closure with the reusable warm state, creating it lazily and
    /// locking the context only briefly to do so. The context mutex is NOT held
    /// across the closure, so a long-running inference no longer blocks model
    /// (re)load. The state lock serializes concurrent transcriptions, which is
    /// required anyway — a single whisper state cannot run two `full()` calls at
    /// once.
    fn with_warm_state<R>(
        &self,
        f: impl FnOnce(&mut WhisperState) -> Result<R, DictationError>,
    ) -> Result<R, DictationError> {
        let mut state_guard = self.state.lock().unwrap();
        if state_guard.is_none() {
            let ctx_guard = self.context.lock().unwrap();
            let ctx = ctx_guard.as_ref().ok_or(DictationError::ModelNotLoaded)?;
            let st = ctx.create_state().map_err(|e| {
                DictationError::TranscriptionFailed(format!("Failed to create whisper state: {e}"))
            })?;
            *state_guard = Some(st);
        }
        f(state_guard.as_mut().unwrap())
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
        on_progress: impl FnMut(i32) + 'static,
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
        on_progress: impl FnMut(i32) + 'static,
    ) -> Result<String, DictationError> {
        let opts = TranscribeOptions {
            prompt: prompt.map(str::to_string),
            ..Default::default()
        };
        self.transcribe_sync_with_options(audio, language, &opts, on_progress)
    }

    /// Core transcription entry point. Honors the opt-in [`TranscribeOptions`]
    /// (prompt, beam search, temperature fallback, VAD). Blocking — call from
    /// spawn_blocking.
    pub fn transcribe_sync_with_options(
        &self,
        audio: &[f32],
        language: Language,
        opts: &TranscribeOptions,
        on_progress: impl FnMut(i32) + 'static,
    ) -> Result<String, DictationError> {
        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }

        let model = self
            .loaded_model()
            .ok_or(DictationError::ModelNotLoaded)?;

        let n_threads = whisper_threads();
        let no_speech_thold = model.no_speech_threshold();

        // Beam search (opt-in) is more accurate on hard audio but several times
        // slower; greedy best_of=1 is the fast default.
        let strategy = if opts.beam_size >= 2 {
            SamplingStrategy::BeamSearch {
                beam_size: opts.beam_size as i32,
                patience: -1.0, // whisper default
            }
        } else {
            SamplingStrategy::Greedy { best_of: 1 }
        };

        let mut params = FullParams::new(strategy);
        params.set_language(language.whisper_code());
        params.set_n_threads(n_threads);
        params.set_temperature(0.0);
        // Temperature fallback re-decodes hard segments at higher temperature.
        // Disabling it caps worst-case latency at the cost of some robustness.
        params.set_temperature_inc(if opts.temperature_fallback { 0.2 } else { 0.0 });
        params.set_translate(false);
        params.set_no_timestamps(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_no_speech_thold(no_speech_thold);
        params.set_suppress_blank(true);
        if let Some(p) = &opts.prompt {
            if !p.is_empty() {
                params.set_initial_prompt(p);
            }
        }

        // VAD (opt-in): skip non-speech regions. The model path MUST be set
        // before enable_vad — whisper-rs panics otherwise. The caller guarantees
        // the file exists.
        if let Some(vad_path) = &opts.vad_model_path {
            params.set_vad_model_path(Some(vad_path.as_str()));
            let mut vad = WhisperVadParams::new();
            vad.set_threshold(0.5);
            vad.set_min_silence_duration(200); // ms — slightly longer for dictation
            vad.set_speech_pad(50); // ms — avoid clipping word edges
            params.set_vad_params(vad);
            params.enable_vad(true);
        }

        params.set_progress_callback_safe(on_progress);

        // Abort callback: DISABLED — whisper-rs 0.15.1 set_abort_callback_safe()
        // causes error -6 on all models. request_abort() sets the flag but nothing
        // reads it during inference; the tokio timeout in commands.rs / main.rs
        // returns an error while the blocking task runs to completion.
        // TODO: Restore when whisper-rs fixes the FFI callback lifetime issue.
        self.abort_flag.store(false, Ordering::SeqCst);

        info!(
            "Starting local transcription: {} samples, {} threads, lang={:?}, beam={}, temp_fallback={}, vad={}",
            audio.len(),
            n_threads,
            language,
            opts.beam_size,
            opts.temperature_fallback,
            opts.vad_model_path.is_some()
        );

        self.with_warm_state(|state| {
            state.full(params, audio).map_err(|e| {
                DictationError::TranscriptionFailed(format!("Whisper inference failed: {e}"))
            })?;

            let n_segments = state.full_n_segments();
            let mut transcript = String::new();
            for i in 0..n_segments {
                if let Some(segment) = state.get_segment(i) {
                    if let Ok(text) = segment.to_str() {
                        transcript.push_str(text);
                    }
                }
            }

            let result = transcript.trim().to_string();
            info!("Local transcription complete: {} chars", result.len());
            Ok(result)
        })
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
        self.transcribe_chunk_timestamps(audio, language, 0.0, prompt)
    }

    /// Transcribe a single audio chunk (≤30s) with timestamps via DTW.
    ///
    /// Uses `set_no_timestamps(true)` for clean text generation, then derives
    /// per-segment timing from `token.t_dtw` (cross-attention DTW alignment)
    /// rather than the generative `<|t.xx|>` tokens that degrade quality.
    /// `t_offset` (seconds) is added to all returned timestamps.
    #[cfg(feature = "diarization")]
    fn transcribe_chunk_timestamps(
        &self,
        audio: &[f32],
        language: Language,
        t_offset: f64,
        prompt: Option<&str>,
    ) -> Result<Vec<(f64, f64, String)>, DictationError> {
        let model = self
            .loaded_model()
            .ok_or(DictationError::ModelNotLoaded)?;

        let n_threads = whisper_threads();
        let no_speech_thold = model.no_speech_threshold();

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(language.whisper_code());
        params.set_n_threads(n_threads);
        params.set_temperature(0.0);
        params.set_temperature_inc(0.2);
        params.set_translate(false);
        params.set_no_timestamps(true);   // clean text — no generative timestamp tokens
        params.set_token_timestamps(true); // populate token.t0/t1/t_dtw via cross-attention
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_no_speech_thold(no_speech_thold);
        params.set_suppress_blank(true);
        if let Some(p) = prompt {
            params.set_initial_prompt(p);
        }

        self.with_warm_state(|state| {
            state.full(params, audio).map_err(|e| {
                DictationError::TranscriptionFailed(format!("Whisper inference failed: {e}"))
            })?;

            let n_segments = state.full_n_segments();
            let mut results = Vec::with_capacity(n_segments as usize);

            for i in 0..n_segments {
                let Some(segment) = state.get_segment(i) else { continue };

                let text = segment.to_str().unwrap_or("").trim().to_string();
                if text.is_empty() {
                    continue;
                }

                // Derive segment timing from DTW timestamps on first/last non-special
                // token. DTW timestamps are in centiseconds; -1 means not computed.
                // Fall back to segment-level timestamps if DTW was not available.
                let (t0, t1) = dtw_segment_timestamps(&segment, t_offset).unwrap_or_else(|| {
                    let t0 = segment.start_timestamp() as f64 / 100.0 + t_offset;
                    let t1 = segment.end_timestamp() as f64 / 100.0 + t_offset;
                    (t0, t1)
                });

                results.push((t0, t1, text));
            }

            Ok(results)
        })
    }
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
        let Some(token) = segment.get_token(j) else { continue };
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

