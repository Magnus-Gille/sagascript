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
#[cfg(all(feature = "dev-overrides", windows))]
use ort::environment::Environment;
use ort::ep;
#[cfg(all(feature = "dev-overrides", windows))]
use ort::memory::DeviceType;
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
    /// `SAGASCRIPT_ORT_TRACE=1`: one stderr line per window with per-stage detail.
    pub trace: bool,
}

impl RuntimeConfig {
    pub fn from_env() -> Self {
        fn var(name: &str) -> Option<usize> {
            std::env::var(name)
                .ok()?
                .trim()
                .parse()
                .ok()
                .filter(|n| *n > 0)
        }
        let cores = std::thread::available_parallelism().map_or(4, usize::from);
        let encoder_threads = var("SAGASCRIPT_ORT_THREADS").unwrap_or_else(|| cores.min(8));
        Self {
            encoder_threads,
            preprocessor_threads: var("SAGASCRIPT_ORT_PRE_THREADS").unwrap_or(encoder_threads),
            decoder_threads: var("SAGASCRIPT_ORT_DEC_THREADS")
                .unwrap_or_else(|| encoder_threads.min(2)),
            arena: std::env::var("SAGASCRIPT_ORT_ARENA").is_ok_and(|v| v == "1"),
            max_in_flight: 2,
            execution_slots: var("SAGASCRIPT_ORT_SLOTS").unwrap_or(1),
            trace: std::env::var("SAGASCRIPT_ORT_TRACE").is_ok_and(|v| v == "1"),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn ort_error(context: &str, error: impl std::fmt::Display) -> HostError {
    HostError::new(ErrorCode::Engine, format!("{context}: {error}"))
}

static RUNTIME: OnceLock<Result<String, String>> = OnceLock::new();

#[cfg(all(feature = "dev-overrides", windows))]
static QNN_PLUGIN: OnceLock<Result<PathBuf, String>> = OnceLock::new();

#[cfg(feature = "dev-overrides")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LabExecutionProvider {
    Cpu,
    DirectMl,
    Qnn,
}

#[cfg(feature = "dev-overrides")]
fn parse_lab_ep(value: Option<&str>) -> Result<LabExecutionProvider, &'static str> {
    match value {
        None | Some("") | Some("cpu") => Ok(LabExecutionProvider::Cpu),
        Some("dml") => Ok(LabExecutionProvider::DirectMl),
        Some("qnn") => Ok(LabExecutionProvider::Qnn),
        Some(_) => Err("expected cpu, dml, or qnn"),
    }
}

#[cfg(feature = "dev-overrides")]
fn lab_ep_mode() -> Result<LabExecutionProvider, ort::Error> {
    let value = match std::env::var("SAGASCRIPT_LAB_EP") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(ort::Error::new("SAGASCRIPT_LAB_EP must be valid Unicode"));
        }
    };
    let mode = parse_lab_ep(value.as_deref()).map_err(ort::Error::new)?;
    #[cfg(not(windows))]
    if mode == LabExecutionProvider::Qnn {
        return Err(ort::Error::new(
            "SAGASCRIPT_LAB_EP=qnn is supported only by the Windows lab host",
        ));
    }
    Ok(mode)
}

#[cfg(feature = "dev-overrides")]
#[derive(Debug)]
struct LabQnnConfig {
    library: PathBuf,
    backend: PathBuf,
    context_cache: Option<PathBuf>,
}

#[cfg(feature = "dev-overrides")]
fn validate_lab_qnn_paths(
    library: &Path,
    backend: &Path,
    context_cache: Option<&Path>,
) -> Result<(), &'static str> {
    if !library.is_absolute() || !library.is_file() {
        return Err("QNN plugin library must be an existing absolute file path");
    }
    if !backend.is_absolute() || !backend.is_file() {
        return Err("QNN backend must be an existing absolute file path");
    }
    if let Some(cache) = context_cache {
        if !cache.is_absolute() {
            return Err("QNN context cache path must be absolute");
        }
        if !cache.parent().is_some_and(Path::is_dir) {
            return Err("QNN context cache parent directory must exist");
        }
    }
    Ok(())
}

#[cfg(feature = "dev-overrides")]
fn lab_qnn_config() -> Result<LabQnnConfig, ort::Error> {
    let library = std::env::var_os("SAGASCRIPT_LAB_QNN_LIBRARY")
        .map(PathBuf::from)
        .ok_or_else(|| {
            ort::Error::new("SAGASCRIPT_LAB_EP=qnn requires SAGASCRIPT_LAB_QNN_LIBRARY")
        })?;
    let backend = library
        .parent()
        .ok_or_else(|| ort::Error::new("QNN plugin library path has no parent directory"))?
        .join("QnnHtp.dll");
    let context_cache = std::env::var_os("SAGASCRIPT_LAB_QNN_CONTEXT_CACHE").map(PathBuf::from);
    validate_lab_qnn_paths(&library, &backend, context_cache.as_deref())
        .map_err(ort::Error::new)?;
    Ok(LabQnnConfig {
        library,
        backend,
        context_cache,
    })
}

#[cfg(all(feature = "dev-overrides", windows))]
fn register_lab_qnn(library: &Path) -> Result<(), ort::Error> {
    let registered = QNN_PLUGIN.get_or_init(|| {
        let environment = Environment::current().map_err(|error| error.to_string())?;
        environment
            .register_ep_library("QNNExecutionProvider", library)
            .map_err(|error| error.to_string())?;
        Ok(library.to_path_buf())
    });
    match registered {
        Ok(path) if path == library => Ok(()),
        Ok(path) => Err(ort::Error::new(format!(
            "QNN plugin already registered from {}; requested {}",
            path.display(),
            library.display()
        ))),
        Err(error) => Err(ort::Error::new(format!(
            "cannot register QNN plugin {}: {error}",
            library.display()
        ))),
    }
}

/// Locate and load the ONNX Runtime shared library once per process.
fn ensure_runtime() -> Result<&'static str, HostError> {
    RUNTIME
        .get_or_init(|| {
            let executable_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .unwrap_or_default();
            let path = resolve_library_path(&executable_dir);
            #[cfg(windows)]
            preload_windows_providers(&executable_dir);
            ort::init_from(&path)
                .map_err(|e| format!("cannot load ONNX Runtime from {}: {e}", path.display()))?
                .commit();
            Ok(runtime_version(&path))
        })
        .as_ref()
        .map(String::as_str)
        .map_err(|message| HostError::new(ErrorCode::ModelLoadFailed, message.clone()))
}

/// `ORT_DYLIB_PATH` is a development aid: honoured in debug builds or with the
/// `dev-overrides` feature, otherwise only the library beside the executable is used.
const DEV_OVERRIDES: bool = cfg!(any(debug_assertions, feature = "dev-overrides"));

fn resolve_library_path(executable_dir: &Path) -> PathBuf {
    library_path(
        executable_dir,
        std::env::var_os("ORT_DYLIB_PATH"),
        DEV_OVERRIDES,
    )
}

fn library_path(
    executable_dir: &Path,
    env_value: Option<std::ffi::OsString>,
    dev_overrides: bool,
) -> PathBuf {
    match env_value.filter(|v| !v.is_empty()) {
        Some(value) if dev_overrides => {
            let value = PathBuf::from(value);
            if value.is_absolute() {
                value
            } else {
                executable_dir.join(value)
            }
        }
        Some(_) => {
            eprintln!(
                "warning: ORT_DYLIB_PATH is ignored in this build (no dev-overrides); using {}",
                executable_dir.join(ORT_LIBRARY).display()
            );
            executable_dir.join(ORT_LIBRARY)
        }
        None => executable_dir.join(ORT_LIBRARY),
    }
}

/// Restrict the Windows DLL search to the application directory and System32, so neither
/// the current directory nor PATH can supply `onnxruntime*.dll`. Call first thing in `main`.
#[cfg(windows)]
pub fn restrict_dll_search() {
    use windows_sys::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    // SAFETY: plain Win32 call with constant flags.
    let ok = unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32)
    };
    if ok == 0 {
        eprintln!(
            "warning: SetDefaultDllDirectories failed: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// Preload `onnxruntime_providers_shared.dll` by absolute path beside the executable (if
/// present) so ONNX Runtime's own delay-load resolves to it. The handle is intentionally leaked.
#[cfg(windows)]
fn preload_windows_providers(executable_dir: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{
        LoadLibraryExW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    let dll = executable_dir.join("onnxruntime_providers_shared.dll");
    if !dll.is_file() {
        return;
    }
    let wide: Vec<u16> = dll
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a NUL-terminated absolute path; the reserved handle argument is null.
    let handle = unsafe {
        LoadLibraryExW(
            wide.as_ptr(),
            0,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if handle == 0 {
        eprintln!(
            "warning: cannot preload {}: {}",
            dll.display(),
            std::io::Error::last_os_error()
        );
    }
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
            (!text.is_null()).then(|| {
                std::ffi::CStr::from_ptr(text)
                    .to_string_lossy()
                    .into_owned()
            })
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
    trace: bool,
    boost_tries: crate::boost::BoostTrieCache,
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
        #[cfg(feature = "dev-overrides")]
        let is_encoder = path.file_name().is_some_and(|name| name == ENCODER_FILE);
        let arena = ep::CPU::default()
            .with_arena_allocator(self.config.arena)
            .build();
        let build = || -> Result<Session, ort::Error> {
            let mut builder = Session::builder()?
                .with_intra_threads(threads)?
                .with_inter_threads(1)?
                .with_memory_pattern(false)?
                .with_optimization_level(GraphOptimizationLevel::All)?;
            #[cfg(feature = "dev-overrides")]
            if is_encoder {
                match lab_ep_mode()? {
                    LabExecutionProvider::Qnn => {
                        #[cfg(windows)]
                        {
                            let qnn = lab_qnn_config()?;
                            register_lab_qnn(&qnn.library)?;
                            let environment = Environment::current()?;
                            let devices: Vec<_> = environment.devices().collect();
                            let observed: Vec<_> = devices
                                .iter()
                                .map(|device| {
                                    format!(
                                        "{}:{:?}",
                                        device.ep().unwrap_or("unknown"),
                                        device.hardware_device().ty()
                                    )
                                })
                                .collect();
                            let device = devices.into_iter()
                            .find(|device| {
                                device.ep().ok() == Some("QNNExecutionProvider")
                                    && device.hardware_device().ty() == DeviceType::NPU
                            })
                            .ok_or_else(|| {
                                ort::Error::new(format!(
                                    "QNN plugin registered, but ONNX Runtime exposed no QNN NPU device; observed {observed:?}"
                                ))
                            })?;
                            let options = vec![
                                (
                                    "QNNExecutionProvider.backend_path".to_string(),
                                    qnn.backend.display().to_string(),
                                ),
                                (
                                    "QNNExecutionProvider.htp_performance_mode".to_string(),
                                    "burst".to_string(),
                                ),
                            ];
                            if let Some(cache_path) = qnn.context_cache {
                                builder = builder
                                    .with_config_entry("ep.context_enable", "1")?
                                    .with_config_entry(
                                        "ep.context_file_path",
                                        cache_path.display().to_string(),
                                    )?;
                            }
                            // ORT's device API routes through SessionOptionsAppendExecutionProvider_V2.
                            // This avoids the legacy name-based QNN append path.
                            builder = builder
                                .with_devices([device], Some(&options))?
                                .with_disable_cpu_fallback()?;
                        }
                        #[cfg(not(windows))]
                        return Err(ort::Error::new(
                            "SAGASCRIPT_LAB_EP=qnn is supported only by the Windows lab host",
                        ));
                    }
                    LabExecutionProvider::DirectMl => {
                        builder = builder
                            .with_execution_providers([ep::DirectML::default()
                                .with_device_id(0)
                                .build()
                                .error_on_failure()])?
                            .with_parallel_execution(false)?;
                    }
                    LabExecutionProvider::Cpu => {
                        builder = builder.with_execution_providers([arena])?;
                    }
                }
            } else {
                builder = builder.with_execution_providers([arena])?;
            }
            #[cfg(not(feature = "dev-overrides"))]
            {
                builder = builder.with_execution_providers([arena])?;
            }
            // Lab-only knobs: never enabled in shipped builds. They apply to the encoder
            // alone so preprocessing and greedy decoding remain unchanged during comparison.
            #[cfg(feature = "dev-overrides")]
            if is_encoder {
                if std::env::var("SAGASCRIPT_LAB_BASIC").as_deref() == Ok("1") {
                    builder = builder.with_optimization_level(GraphOptimizationLevel::Level1)?;
                }
                if let Ok(profile) = std::env::var("SAGASCRIPT_LAB_PROFILE") {
                    builder = builder.with_profiling(profile)?;
                }
                if let Ok(file) = std::env::var("SAGASCRIPT_LAB_ENCODER") {
                    return builder.commit_from_file(file);
                }
            }
            builder.commit_from_file(path)
        };
        build().map_err(|e| {
            HostError::new(
                ErrorCode::ModelLoadFailed,
                format!("cannot load {}: {e}", path.display()),
            )
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
        let _qos = crate::power_qos::high_qos_scope();
        let started = Instant::now();
        let dir = &params.model_dir;
        for file in [
            ENCODER_FILE,
            DECODER_FILE,
            PREPROCESSOR_FILE,
            VOCAB_FILE,
            CONFIG_FILE,
        ] {
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
            .map_err(|e| {
                HostError::new(
                    ErrorCode::ModelLoadFailed,
                    format!("bad {CONFIG_FILE}: {e}"),
                )
            })?;
        let max_tokens_per_step = config["max_tokens_per_step"].as_u64().unwrap_or(10) as usize;
        let subsampling = config["subsampling_factor"].as_u64().unwrap_or(8) as f64;
        let vocab = std::fs::read_to_string(dir.join(VOCAB_FILE))
            .map_err(|e| e.to_string())
            .and_then(|text| Vocab::parse(&text))
            .map_err(|e| {
                HostError::new(ErrorCode::ModelLoadFailed, format!("bad {VOCAB_FILE}: {e}"))
            })?;

        let preprocessor = self.session(
            &dir.join(PREPROCESSOR_FILE),
            self.config.preprocessor_threads,
        )?;
        let encoder = self.session(&dir.join(ENCODER_FILE), self.config.encoder_threads)?;
        let decoder = self.session(&dir.join(DECODER_FILE), self.config.decoder_threads)?;
        let result = LoadResult {
            model_id: params.model_id.clone(),
            load_ms: started.elapsed().as_millis() as u64,
            compiled: false,
            window_s: effective_window_s()?,
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
            trace: self.config.trace,
            boost_tries: crate::boost::BoostTrieCache::default(),
            preprocessor: Mutex::new(preprocessor),
            encoder: Mutex::new(encoder),
            decoder: Mutex::new(decoder),
        });
        *self.loaded.write().unwrap_or_else(|p| p.into_inner()) = Some(loaded);
        if self.config.trace {
            eprintln!(
                "ort-trace load: {:?} load_ms={}",
                self.config, result.load_ms
            );
        }
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
    /// Joint evaluations and time inside `Session::run`, for the trace line.
    steps: usize,
    run_us: u128,
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
        let run_started = Instant::now();
        let outputs = self.session.run(inputs).map_err(|x| e("run", x))?;
        self.steps += 1;
        self.run_us += run_started.elapsed().as_micros();
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
        DecoderState {
            s1: zeros.clone(),
            s2: zeros,
        }
    }
}

fn millis(started: Instant) -> u64 {
    (started.elapsed().as_secs_f64() * 1000.0).round() as u64
}

#[cfg(feature = "dev-overrides")]
fn window_for_pad_frames(frames: Option<usize>) -> f64 {
    // nemo128 uses a 10 ms hop and includes the centered boundary frame.
    // Whole seconds leave a conservative margin for a fixed encoder graph.
    frames.map_or(WINDOW_S, |n| (((n - 1) / 100) as f64).min(WINDOW_S))
}

fn effective_window_s() -> Result<f64, HostError> {
    #[cfg(feature = "dev-overrides")]
    return lab_pad_frames().map(window_for_pad_frames);
    #[cfg(not(feature = "dev-overrides"))]
    Ok(WINDOW_S)
}

#[cfg(feature = "dev-overrides")]
fn parse_lab_pad_frames(value: Option<&str>) -> Result<Option<usize>, &'static str> {
    value
        .map(|value| {
            let frames = value
                .parse::<usize>()
                .map_err(|_| "expected a positive integer")?;
            if frames <= 100 {
                return Err("expected at least 101 frames for a one-second window");
            }
            i64::try_from(frames).map_err(|_| "value exceeds the ONNX dimension range")?;
            Ok(frames)
        })
        .transpose()
}

#[cfg(feature = "dev-overrides")]
fn lab_pad_frames() -> Result<Option<usize>, HostError> {
    match std::env::var("SAGASCRIPT_LAB_PAD_FRAMES") {
        Ok(value) => parse_lab_pad_frames(Some(&value))
            .map_err(|error| HostError::new(ErrorCode::Engine, error)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(HostError::new(
            ErrorCode::Engine,
            "SAGASCRIPT_LAB_PAD_FRAMES must be a positive integer",
        )),
    }
}

#[cfg(feature = "dev-overrides")]
fn pad_encoder_features(
    shape: &[i64],
    features: Vec<f32>,
    pad_frames: usize,
) -> Result<(Vec<i64>, Vec<f32>), &'static str> {
    if shape.len() != 3 {
        return Err("expected a rank-3 [batch, channels, frames] tensor");
    }
    if pad_frames == 0 {
        return Err("expected a positive frame count");
    }
    let pad_dimension = i64::try_from(pad_frames).map_err(|_| "frame count is too large")?;
    let batch = usize::try_from(shape[0]).map_err(|_| "batch dimension must be positive")?;
    let channels = usize::try_from(shape[1]).map_err(|_| "channel dimension must be positive")?;
    let frames = usize::try_from(shape[2]).map_err(|_| "frame dimension must be positive")?;
    if batch == 0 || channels == 0 || frames == 0 {
        return Err("tensor dimensions must be positive");
    }
    if pad_frames < frames {
        return Err("requested frame count is shorter than input; refusing to truncate");
    }

    let rows = batch
        .checked_mul(channels)
        .ok_or("tensor dimensions are too large")?;
    let expected_len = rows
        .checked_mul(frames)
        .ok_or("tensor dimensions are too large")?;
    if features.len() != expected_len {
        return Err("feature data length does not match its tensor shape");
    }
    let padded_len = rows
        .checked_mul(pad_frames)
        .ok_or("padded tensor dimensions are too large")?;

    let mut padded = Vec::new();
    padded
        .try_reserve_exact(padded_len)
        .map_err(|_| "could not allocate padded feature tensor")?;
    padded.resize(padded_len, 0.0);
    for row in 0..rows {
        let source_start = row * frames;
        let target_start = row * pad_frames;
        padded[target_start..target_start + frames]
            .copy_from_slice(&features[source_start..source_start + frames]);
    }
    let mut padded_shape = shape.to_vec();
    padded_shape[2] = pad_dimension;
    Ok((padded_shape, padded))
}

fn transcribe_loaded(
    loaded: &Loaded,
    request: &WindowRequest,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<TranscribeWindowResult, HostError> {
    let cancelled = || HostError::new(ErrorCode::Cancelled, "Transcription was cancelled");
    let total_started = Instant::now();
    let samples = pcm::read_window(
        &request.pcm_path,
        request.offset_samples,
        request.num_samples,
    )
    .map_err(|m| HostError::new(ErrorCode::BadRequest, m))?;
    let audio_s = samples.len() as f64 / f64::from(SAMPLE_RATE);
    let read_ms = millis(total_started);
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
        (
            shape.iter().copied().collect::<Vec<i64>>(),
            data.to_vec(),
            lens,
        )
    };
    #[cfg(feature = "dev-overrides")]
    let (features_shape, features) = match lab_pad_frames()? {
        Some(pad_frames) => {
            pad_encoder_features(&features_shape, features, pad_frames).map_err(|error| {
                HostError::new(
                    ErrorCode::Engine,
                    format!("SAGASCRIPT_LAB_PAD_FRAMES: {error}"),
                )
            })?
        }
        None => (features_shape, features),
    };
    let preprocess_ms = millis(started);
    if is_cancelled() {
        return Err(cancelled());
    }

    // Encoder: (1, 128, T) -> (1, 1024, T/8), then transpose to frame-major.
    let started = Instant::now();
    let encoder_run_ms: u64;
    let (frames, valid) = {
        let mut session = lock(&loaded.encoder);
        let outputs = session
            .run(ort::inputs![
                "audio_signal" => Tensor::from_array((features_shape, features)).map_err(|e| ort_error("audio_signal", e))?,
                // Keep the actual unpadded feature length for the encoder's length input.
                "length" => Tensor::from_array(([features_len.len()], features_len)).map_err(|e| ort_error("length", e))?,
            ])
            .map_err(|e| ort_error("encoder", e))?;
        encoder_run_ms = millis(started);
        let (shape, data) = outputs["outputs"]
            .try_extract_tensor::<f32>()
            .map_err(|e| ort_error("encoder outputs", e))?;
        let lengths = outputs["encoded_lengths"]
            .try_extract_tensor::<i64>()
            .map_err(|e| ort_error("encoded_lengths", e))?
            .1;
        if shape.len() != 3 || shape[0] != 1 || shape[1] as usize != HIDDEN {
            return Err(ort_error(
                "encoder outputs",
                format!("unexpected shape {shape:?}"),
            ));
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
    let mut joint_steps = 0;
    let mut joint_run_ms = 0;
    let mut boost_us = 0u64;
    let boosting = !request.boost_terms.is_empty() && request.boost_weight > 0.0;
    let mut boost_active = boosting.then_some(true);
    let raw = if valid == 0 {
        Vec::new()
    } else {
        let mut session = lock(&loaded.decoder);
        let mut joint = OrtJoint {
            session: &mut session,
            steps: 0,
            run_us: 0,
        };
        let trie = boosting.then(|| {
            let (trie, micros) = loaded.boost_tries.get(&request.boost_terms, &loaded.vocab);
            boost_us = micros;
            trie
        });
        let trie = trie.filter(|t| !t.is_empty());
        if boosting && trie.is_none() {
            boost_active = Some(false); // no term could be tokenized
        }
        let params = crate::boost::BiasParams::from_env(request.boost_weight);
        let blank = loaded.vocab.blank_id();
        let decoded = tdt::decode_biased(
            &frames,
            &mut joint,
            loaded.vocab.len(),
            blank,
            loaded.max_tokens_per_step,
            is_cancelled,
            trie.as_deref()
                .map(|t| crate::boost::BiasState::new(t, params, blank)),
        );
        joint_steps = joint.steps;
        joint_run_ms = (joint.run_us / 1000) as u64;
        decoded.map_err(|e| match e {
            DecodeError::Cancelled => cancelled(),
            DecodeError::Joint(message) => HostError::new(ErrorCode::Engine, message),
        })?
    };
    let decode_ms = millis(started);
    if loaded.trace {
        eprintln!(
            "ort-trace window: audio_s={audio_s:.2} frames={valid} read_ms={read_ms} preprocess_ms={preprocess_ms} \
             encode_ms={encode_ms} (session_run_ms={encoder_run_ms}) decode_ms={decode_ms} \
             (joint_steps={joint_steps} joint_run_ms={joint_run_ms}) tokens={} total_ms={}",
            raw.len(),
            millis(total_started)
        );
    }

    let mut tokens = Vec::with_capacity(raw.len());
    for token in raw {
        let text = loaded.vocab.piece(token.id).ok_or_else(|| {
            HostError::new(
                ErrorCode::Engine,
                format!("decoder produced unknown token id {}", token.id),
            )
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
        timings: WindowTimings {
            preprocess_ms,
            encode_ms,
            decode_ms,
            boost_us,
        },
        boost_active,
        boost_reason: (boost_active == Some(false)).then(|| "no_usable_terms".to_string()),
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
        // Invalid lab configuration is rejected by load; hello cannot return an error.
        let window_s = effective_window_s().unwrap_or(WINDOW_S);
        Capabilities {
            sample_rate: SAMPLE_RATE,
            max_window_s: window_s,
            preferred_window_s: window_s,
            preferred_overlap_s: 6.0_f64.min(window_s / 2.0),
            max_in_flight: self.config.max_in_flight,
            token_timestamps: true,
            languages: vec!["sv".into()],
            compute_units: vec!["cpu".into()],
            min_macos: None,
            context_biasing: true,
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
        let _qos = crate::power_qos::high_qos_scope();
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

#[cfg(test)]
mod dll_path_tests {
    use super::*;

    #[test]
    fn env_override_needs_dev_overrides() {
        let dir = Path::new("app");
        let custom = Some(std::ffi::OsString::from("custom/ort.dll"));
        assert_eq!(
            library_path(dir, custom.clone(), true),
            dir.join("custom/ort.dll")
        );
        assert_eq!(library_path(dir, custom, false), dir.join(ORT_LIBRARY));
        assert_eq!(library_path(dir, None, false), dir.join(ORT_LIBRARY));
        assert_eq!(
            library_path(dir, Some("".into()), true),
            dir.join(ORT_LIBRARY)
        );
    }
}

#[cfg(all(test, feature = "dev-overrides"))]
mod lab_padding_tests {
    use super::{pad_encoder_features, parse_lab_pad_frames, window_for_pad_frames};

    #[test]
    fn advertised_window_fits_preprocessor_frames_with_center_padding() {
        assert_eq!(window_for_pad_frames(None), 30.0);
        assert_eq!(window_for_pad_frames(Some(2048)), 20.0);
        assert_eq!(window_for_pad_frames(Some(3001)), 30.0);
        assert_eq!(window_for_pad_frames(Some(768)), 7.0);
        assert_eq!(window_for_pad_frames(Some(4096)), 30.0);
    }

    #[test]
    fn pads_zeros_at_the_end_of_each_channel_time_axis() {
        let shape = [1, 2, 3];
        let input = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];

        let (padded_shape, padded) = pad_encoder_features(&shape, input, 5).unwrap();

        assert_eq!(padded_shape, [1, 2, 5]);
        assert_eq!(padded, [1.0, 2.0, 3.0, 0.0, 0.0, 4.0, 5.0, 6.0, 0.0, 0.0]);
    }

    #[test]
    fn refuses_to_truncate_encoder_features() {
        let result = pad_encoder_features(&[1, 2, 3], vec![0.0; 6], 2);
        assert!(result.unwrap_err().contains("refusing to truncate"));
    }

    #[test]
    fn pad_frames_setting_must_be_a_positive_onnx_dimension() {
        assert_eq!(parse_lab_pad_frames(None), Ok(None));
        assert_eq!(parse_lab_pad_frames(Some("768")), Ok(Some(768)));
        assert!(parse_lab_pad_frames(Some("0")).is_err());
        assert!(parse_lab_pad_frames(Some("100")).is_err());
        assert_eq!(parse_lab_pad_frames(Some("101")), Ok(Some(101)));
        assert!(parse_lab_pad_frames(Some("many")).is_err());
    }
}

#[cfg(all(test, feature = "dev-overrides"))]
mod lab_execution_provider_tests {
    use super::{parse_lab_ep, validate_lab_qnn_paths, LabExecutionProvider};
    use std::path::Path;

    #[test]
    fn lab_ep_defaults_to_cpu_and_accepts_supported_choices() {
        assert_eq!(parse_lab_ep(None), Ok(LabExecutionProvider::Cpu));
        assert_eq!(parse_lab_ep(Some("cpu")), Ok(LabExecutionProvider::Cpu));
        assert_eq!(
            parse_lab_ep(Some("dml")),
            Ok(LabExecutionProvider::DirectMl)
        );
        assert_eq!(parse_lab_ep(Some("qnn")), Ok(LabExecutionProvider::Qnn));
        assert!(parse_lab_ep(Some("cuda")).is_err());
    }

    #[test]
    fn qnn_configuration_requires_existing_absolute_libraries_and_cache_parent() {
        let executable = std::env::current_exe().expect("test executable path");
        let parent = executable.parent().expect("test executable parent");
        let cache_path = parent.join("qnn-context.bin");

        assert!(validate_lab_qnn_paths(&executable, &executable, Some(&cache_path)).is_ok());
        assert!(validate_lab_qnn_paths(Path::new("relative.dll"), &executable, None).is_err());
        assert!(
            validate_lab_qnn_paths(&parent.join("missing-qnn-plugin.dll"), &executable, None,)
                .is_err()
        );
        assert!(validate_lab_qnn_paths(
            &executable,
            &executable,
            Some(Path::new("relative-context.bin")),
        )
        .is_err());
        assert!(validate_lab_qnn_paths(
            &executable,
            &executable,
            Some(Path::new("Z:\\missing\\qnn-context.bin")),
        )
        .is_err());
    }
}
