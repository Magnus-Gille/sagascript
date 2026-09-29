//! ONNX Runtime (CPU execution provider) engine for KlangAI's official Pianissimo
//! ONNX int8 export: `nemo128.onnx` (log-mel front end) -> `encoder-model.int8.onnx`
//! -> greedy TDT loop over `decoder_joint-model.int8.onnx`.
//!
//! ONNX Runtime itself is loaded at run time (`load-dynamic`) from `ORT_DYLIB_PATH` or
//! from next to the executable, so building this crate needs no ORT binary.

use crate::pcm;
use crate::server::{Engine, HostError, LoadParams, WindowRequest};
use crate::tdt::{self, DecodeError, DecoderState, Joint, HIDDEN};
use crate::vocab::Vocab;
use ort::ep;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use sagascript_engine_protocol::{
    Capabilities, ErrorCode, LoadResult, TranscribeWindowResult, WindowTimings, WireToken,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::Instant;

pub const ENCODER_FILE: &str = "encoder-model.int8.onnx";
pub const DECODER_FILE: &str = "decoder_joint-model.int8.onnx";
pub const PREPROCESSOR_FILE: &str = "nemo128.onnx";
pub const VOCAB_FILE: &str = "vocab.txt";
pub const CONFIG_FILE: &str = "config.json";

const ORT_CRATE_VERSION: &str = "2.0.0-rc.13";
const SAMPLE_RATE: u32 = 16_000;
const WINDOW_S: f64 = 30.0;
const DECODER_LAYERS: usize = 2;
const DECODER_UNITS: usize = 640;

#[cfg(target_os = "windows")]
const ORT_LIBRARY: &str = "onnxruntime.dll";
#[cfg(target_os = "macos")]
const ORT_LIBRARY: &str = "libonnxruntime.dylib";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const ORT_LIBRARY: &str = "libonnxruntime.so";

/// Tuning knobs. Defaults are the shipped configuration; the environment variables exist
/// for measurements and are read once per load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub encoder_threads: usize,
    pub decoder_threads: usize,
    pub preprocessor_threads: usize,
    pub arena: bool,
    /// Advertised `max_in_flight` (>= 2 so the client reserves an interactive slot).
    pub max_in_flight: u32,
    /// Windows executing at once (the rest queue by priority).
    pub execution_slots: usize,
}

impl RuntimeConfig {
    pub fn from_env() -> Self {
        fn var(name: &str) -> Option<usize> {
            std::env::var(name).ok()?.trim().parse().ok().filter(|n| *n > 0)
        }
        let cores = std::thread::available_parallelism().map_or(4, usize::from);
        let encoder_threads = var("SAGASCRIPT_ORT_THREADS").unwrap_or_else(|| cores.min(8));
        Self {
            encoder_threads,
            preprocessor_threads: var("SAGASCRIPT_ORT_PRE_THREADS").unwrap_or(encoder_threads),
            decoder_threads: var("SAGASCRIPT_ORT_DEC_THREADS").unwrap_or_else(|| encoder_threads.min(2)),
            arena: std::env::var("SAGASCRIPT_ORT_ARENA").is_ok_and(|v| v == "1"),
            max_in_flight: 2,
            execution_slots: var("SAGASCRIPT_ORT_SLOTS").unwrap_or(1),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn ort_error(context: &str, error: impl std::fmt::Display) -> HostError {
    HostError::new(ErrorCode::Engine, format!("{context}: {error}"))
}

static RUNTIME: OnceLock<Result<String, String>> = OnceLock::new();

/// Locate and load the ONNX Runtime shared library once per process.
fn ensure_runtime() -> Result<&'static str, HostError> {
    RUNTIME
        .get_or_init(|| {
            let executable_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .unwrap_or_default();
            let path = match std::env::var_os("ORT_DYLIB_PATH").filter(|v| !v.is_empty()) {
                Some(value) => {
                    let value = PathBuf::from(value);
                    if value.is_absolute() {
                        value
                    } else {
                        executable_dir.join(value)
                    }
                }
                None => executable_dir.join(ORT_LIBRARY),
            };
            ort::init_from(&path)
                .map_err(|e| format!("cannot load ONNX Runtime from {}: {e}", path.display()))?
                .commit();
            Ok(runtime_version(&path))
        })
        .as_ref()
        .map(String::as_str)
        .map_err(|message| HostError::new(ErrorCode::ModelLoadFailed, message.clone()))
}

/// Exact library version via `OrtGetApiBase()->GetVersionString()`; the `ort` crate does not
/// expose it. The library is already loaded, so this only bumps its reference count.
fn runtime_version(path: &Path) -> String {
    #[repr(C)]
    struct ApiBase {
        get_api: unsafe extern "C" fn(u32) -> *const std::ffi::c_void,
        get_version_string: unsafe extern "C" fn() -> *const std::ffi::c_char,
    }
    // SAFETY: `OrtGetApiBase` and `OrtApiBase` are ONNX Runtime's stable C ABI; the
    // returned pointers are static for the life of the library, which stays loaded.
    let version = unsafe {
        libloading::Library::new(path).ok().and_then(|library| {
            let get_base: libloading::Symbol<unsafe extern "C" fn() -> *const ApiBase> =
                library.get(b"OrtGetApiBase").ok()?;
            let base = get_base();
            if base.is_null() {
                return None;
            }
            let text = ((*base).get_version_string)();
            (!text.is_null()).then(|| std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned())
        })
    };
    match version {
        Some(version) => format!("onnxruntime {version} (ort {ORT_CRATE_VERSION})"),
        None => format!("onnxruntime ({})", ort::info()),
    }
}

struct Loaded {
    model_id: String,
    model_dir: PathBuf,
    result: LoadResult,
    vocab: Vocab,
    max_tokens_per_step: usize,
    preprocessor: Mutex<Session>,
    encoder: Mutex<Session>,
    decoder: Mutex<Session>,
}

pub struct OrtEngine {
    config: RuntimeConfig,
    loaded: RwLock<Option<Arc<Loaded>>>,
    loading: AtomicBool,
}

impl OrtEngine {
    pub fn new(config: RuntimeConfig) -> Self {
        Self {
            config,
            loaded: RwLock::new(None),
            loading: AtomicBool::new(false),
        }
    }

    fn current(&self) -> Option<Arc<Loaded>> {
        self.loaded
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn session(&self, path: &Path, threads: usize) -> Result<Session, HostError> {
        let arena = ep::CPU::default().with_arena_allocator(self.config.arena).build();
        let build = || -> Result<Session, ort::Error> {
            let mut builder = Session::builder()?
                .with_execution_providers([arena])?
                .with_intra_threads(threads)?
                .with_inter_threads(1)?
                .with_memory_pattern(false)?
                .with_optimization_level(GraphOptimizationLevel::All)?;
            builder.commit_from_file(path)
        };
        build().map_err(|e| {
            HostError::new(ErrorCode::ModelLoadFailed, format!("cannot load {}: {e}", path.display()))
        })
    }

    fn load_inner(&self, params: &LoadParams) -> Result<LoadResult, HostError> {
        if let Some(current) = self.current() {
            if current.model_id == params.model_id && current.model_dir == params.model_dir {
                let mut result = current.result.clone();
                result.load_ms = 0;
                return Ok(result);
            }
        }
        let started = Instant::now();
        let dir = &params.model_dir;
        for file in [ENCODER_FILE, DECODER_FILE, PREPROCESSOR_FILE, VOCAB_FILE, CONFIG_FILE] {
            if !dir.join(file).is_file() {
                return Err(HostError::new(
                    ErrorCode::ModelMissing,
                    format!("{file} is missing from {}", dir.display()),
                ));
            }
        }
        // Any requested compute unit (ane, gpu, cpu, all) maps to the CPU provider.
        ensure_runtime()?;
        let config: serde_json::Value = std::fs::read_to_string(dir.join(CONFIG_FILE))
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
            .map_err(|e| HostError::new(ErrorCode::ModelLoadFailed, format!("bad {CONFIG_FILE}: {e}")))?;
        let max_tokens_per_step = config["max_tokens_per_step"].as_u64().unwrap_or(10) as usize;
        let subsampling = config["subsampling_factor"].as_u64().unwrap_or(8) as f64;
        let vocab = std::fs::read_to_string(dir.join(VOCAB_FILE))
            .map_err(|e| e.to_string())
            .and_then(|text| Vocab::parse(&text))
            .map_err(|e| HostError::new(ErrorCode::ModelLoadFailed, format!("bad {VOCAB_FILE}: {e}")))?;

        let preprocessor = self.session(&dir.join(PREPROCESSOR_FILE), self.config.preprocessor_threads)?;
        let encoder = self.session(&dir.join(ENCODER_FILE), self.config.encoder_threads)?;
        let decoder = self.session(&dir.join(DECODER_FILE), self.config.decoder_threads)?;
        let result = LoadResult {
            model_id: params.model_id.clone(),
            load_ms: started.elapsed().as_millis() as u64,
            compiled: false,
            window_s: WINDOW_S,
            frame_s: 0.01 * subsampling,
            vocab_size: vocab.len() as u32,
            blank_id: vocab.blank_id(),
        };
        let loaded = Arc::new(Loaded {
            model_id: params.model_id.clone(),
            model_dir: dir.clone(),
            result: result.clone(),
            vocab,
            max_tokens_per_step,
            preprocessor: Mutex::new(preprocessor),
            encoder: Mutex::new(encoder),
            decoder: Mutex::new(decoder),
        });
        *self.loaded.write().unwrap_or_else(|p| p.into_inner()) = Some(loaded);
        Ok(result)
    }
}

/// Clears the `loading` flag on every exit path.
struct LoadingGuard<'a>(&'a AtomicBool);
impl Drop for LoadingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

struct OrtJoint<'a> {
    session: &'a mut Session,
}

impl Joint for OrtJoint<'_> {
    fn step(
        &mut self,
        frame: &[f32],
        target: i32,
        state: &DecoderState,
    ) -> Result<(Vec<f32>, DecoderState), String> {
        let e = |what: &str, err: ort::Error| format!("decoder_joint {what}: {err}");
        let state_shape = [DECODER_LAYERS, 1, DECODER_UNITS];
        let inputs = ort::inputs![
            "encoder_outputs" => Tensor::from_array(([1usize, HIDDEN, 1], frame.to_vec())).map_err(|x| e("input", x))?,
            "targets" => Tensor::from_array(([1usize, 1], vec![target])).map_err(|x| e("input", x))?,
            "target_length" => Tensor::from_array(([1usize], vec![1i32])).map_err(|x| e("input", x))?,
            "input_states_1" => Tensor::from_array((state_shape, state.s1.clone())).map_err(|x| e("input", x))?,
            "input_states_2" => Tensor::from_array((state_shape, state.s2.clone())).map_err(|x| e("input", x))?,
        ];
        let outputs = self.session.run(inputs).map_err(|x| e("run", x))?;
        let logits = outputs["outputs"]
            .try_extract_tensor::<f32>()
            .map_err(|x| e("outputs", x))?
            .1
            .to_vec();
        let s1 = outputs["output_states_1"]
            .try_extract_tensor::<f32>()
            .map_err(|x| e("output_states_1", x))?
            .1
            .to_vec();
        let s2 = outputs["output_states_2"]
            .try_extract_tensor::<f32>()
            .map_err(|x| e("output_states_2", x))?
            .1
            .to_vec();
        Ok((logits, DecoderState { s1, s2 }))
    }

    fn initial_state(&self) -> DecoderState {
        let zeros = vec![0.0; DECODER_LAYERS * DECODER_UNITS];
        DecoderState { s1: zeros.clone(), s2: zeros }
    }
}

fn millis(started: Instant) -> u64 {
    (started.elapsed().as_secs_f64() * 1000.0).round() as u64
}

fn transcribe_loaded(
    loaded: &Loaded,
    request: &WindowRequest,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<TranscribeWindowResult, HostError> {
    let cancelled = || HostError::new(ErrorCode::Cancelled, "Transcription was cancelled");
    let samples = pcm::read_window(&request.pcm_path, request.offset_samples, request.num_samples)
        .map_err(|m| HostError::new(ErrorCode::BadRequest, m))?;
    let audio_s = samples.len() as f64 / f64::from(SAMPLE_RATE);
    if is_cancelled() {
        return Err(cancelled());
    }

    // Front end (nemo128.onnx): waveform -> log-mel features (1, 128, T).
    let started = Instant::now();
    let count = samples.len();
    let (features_shape, features, features_len) = {
        let mut session = lock(&loaded.preprocessor);
        let outputs = session
            .run(ort::inputs![
                "waveforms" => Tensor::from_array(([1usize, count], samples)).map_err(|e| ort_error("waveforms", e))?,
                "waveforms_lens" => Tensor::from_array(([1usize], vec![count as i64])).map_err(|e| ort_error("waveforms_lens", e))?,
            ])
            .map_err(|e| ort_error("preprocessor", e))?;
        let (shape, data) = outputs["features"]
            .try_extract_tensor::<f32>()
            .map_err(|e| ort_error("features", e))?;
        let lens = outputs["features_lens"]
            .try_extract_tensor::<i64>()
            .map_err(|e| ort_error("features_lens", e))?
            .1
            .to_vec();
        (shape.iter().copied().collect::<Vec<i64>>(), data.to_vec(), lens)
    };
    let preprocess_ms = millis(started);
    if is_cancelled() {
        return Err(cancelled());
    }

    // Encoder: (1, 128, T) -> (1, 1024, T/8), then transpose to frame-major.
    let started = Instant::now();
    let (frames, valid) = {
        let mut session = lock(&loaded.encoder);
        let outputs = session
            .run(ort::inputs![
                "audio_signal" => Tensor::from_array((features_shape, features)).map_err(|e| ort_error("audio_signal", e))?,
                "length" => Tensor::from_array(([features_len.len()], features_len)).map_err(|e| ort_error("length", e))?,
            ])
            .map_err(|e| ort_error("encoder", e))?;
        let (shape, data) = outputs["outputs"]
            .try_extract_tensor::<f32>()
            .map_err(|e| ort_error("encoder outputs", e))?;
        let lengths = outputs["encoded_lengths"]
            .try_extract_tensor::<i64>()
            .map_err(|e| ort_error("encoded_lengths", e))?
            .1;
        if shape.len() != 3 || shape[0] != 1 || shape[1] as usize != HIDDEN {
            return Err(ort_error("encoder outputs", format!("unexpected shape {shape:?}")));
        }
        let total = shape[2] as usize;
        let valid = (lengths.first().copied().unwrap_or(0).max(0) as usize).min(total);
        let mut frames = vec![0.0f32; valid * HIDDEN];
        for channel in 0..HIDDEN {
            let row = &data[channel * total..channel * total + valid];
            for (t, value) in row.iter().enumerate() {
                frames[t * HIDDEN + channel] = *value;
            }
        }
        (frames, valid)
    };
    let encode_ms = millis(started);
    if is_cancelled() {
        return Err(cancelled());
    }

    let started = Instant::now();
    let raw = if valid == 0 {
        Vec::new()
    } else {
        let mut session = lock(&loaded.decoder);
        let mut joint = OrtJoint { session: &mut session };
        tdt::decode(
            &frames,
            &mut joint,
            loaded.vocab.len(),
            loaded.vocab.blank_id(),
            loaded.max_tokens_per_step,
            is_cancelled,
        )
        .map_err(|e| match e {
            DecodeError::Cancelled => cancelled(),
            DecodeError::Joint(message) => HostError::new(ErrorCode::Engine, message),
        })?
    };
    let decode_ms = millis(started);

    let mut tokens = Vec::with_capacity(raw.len());
    for token in raw {
        let text = loaded.vocab.piece(token.id).ok_or_else(|| {
            HostError::new(ErrorCode::Engine, format!("decoder produced unknown token id {}", token.id))
        })?;
        tokens.push(WireToken {
            id: token.id,
            text: text.to_string(),
            start: token.frame as f64 * tdt::FRAME_S,
            duration: token.duration_frames as f64 * tdt::FRAME_S,
            confidence: None,
        });
    }
    Ok(TranscribeWindowResult {
        tokens,
        audio_s,
        timings: WindowTimings { preprocess_ms, encode_ms, decode_ms },
    })
}

impl Engine for OrtEngine {
    fn engine_name(&self) -> &'static str {
        "onnx"
    }

    fn engine_version(&self) -> String {
        // Never loads the library: `hello` must answer fast, even on a cold disk.
        match RUNTIME.get() {
            Some(Ok(version)) => version.clone(),
            _ => "unloaded".to_string(),
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            sample_rate: SAMPLE_RATE,
            max_window_s: WINDOW_S,
            preferred_window_s: WINDOW_S,
            preferred_overlap_s: 6.0,
            max_in_flight: self.config.max_in_flight,
            token_timestamps: true,
            languages: vec!["sv".into()],
            compute_units: vec!["cpu".into()],
            min_macos: None,
        }
    }

    fn execution_slots(&self) -> usize {
        self.config.execution_slots
    }

    fn load(&self, params: &LoadParams) -> Result<LoadResult, HostError> {
        self.loading.store(true, Ordering::SeqCst);
        let _guard = LoadingGuard(&self.loading);
        self.load_inner(params)
    }

    fn transcribe(
        &self,
        request: &WindowRequest,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<TranscribeWindowResult, HostError> {
        let loaded = self
            .current()
            .ok_or_else(|| HostError::new(ErrorCode::NotLoaded, "No model is loaded"))?;
        transcribe_loaded(&loaded, request, is_cancelled)
    }

    fn unload(&self) {
        *self.loaded.write().unwrap_or_else(|p| p.into_inner()) = None;
    }

    fn loaded_model_id(&self) -> Option<String> {
        self.current().map(|l| l.model_id.clone())
    }

    fn is_loading(&self) -> bool {
        self.loading.load(Ordering::SeqCst)
    }

    fn rss_bytes(&self) -> u64 {
        memory_stats::memory_stats().map_or(0, |s| s.physical_mem as u64)
    }
}
