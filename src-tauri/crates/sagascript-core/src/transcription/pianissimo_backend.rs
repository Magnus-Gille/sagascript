//! Local subprocess adapter for the original Pianissimo NeMo checkpoint.
//! Official macOS builds carry an arm64 Python/NeMo runtime in the app bundle.
//! It is never used by live dictation.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::audio::wav::encode_wav;
use crate::error::DictationError;
use crate::transcription::pianissimo_model;

const WORKER: &str = include_str!("../../../../resources/pianissimo_worker.py");

#[derive(Debug, Clone, Deserialize)]
pub struct PianissimoWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone)]
pub struct PianissimoResult {
    pub text: String,
    pub words: Vec<PianissimoWord>,
}

pub struct PianissimoBackend {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
    operation: Mutex<()>,
}

struct TemporaryWav(PathBuf);

impl Drop for TemporaryWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct RuntimePython {
    executable: OsString,
    python_home: Option<PathBuf>,
    site_packages: Option<PathBuf>,
}

fn runtime_python() -> RuntimePython {
    if let Some(executable) = std::env::var_os("SAGASCRIPT_PIANISSIMO_PYTHON")
        .or_else(|| std::env::var_os("PIANISSIMO_PYTHON"))
    {
        return RuntimePython { executable, python_home: None, site_packages: None };
    }
    #[cfg(target_os = "macos")]
    if let Ok(executable) = std::env::current_exe().and_then(std::fs::canonicalize) {
        if let Some(runtime) = bundled_runtime(&executable) {
            return runtime;
        }
    }
    RuntimePython { executable: OsString::from("python3.12"), python_home: None, site_packages: None }
}

#[cfg(target_os = "macos")]
fn bundled_runtime(executable: &std::path::Path) -> Option<RuntimePython> {
    let contents = executable.parent()?.parent()?;
    let root = contents.join("Resources/PianissimoRuntime");
    let python_home = root.join("python");
    let site_packages = root.join("site-packages");
    let python = python_home.join("bin/python3.12");
    (python.is_file() && site_packages.is_dir()).then_some(RuntimePython {
        executable: python.into_os_string(),
        python_home: Some(python_home),
        site_packages: Some(site_packages),
    })
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn bundled_runtime_resolves_relative_to_app_executable() {
        let root = std::env::temp_dir().join(format!("sagascript-runtime-test-{}", uuid::Uuid::new_v4()));
        let contents = root.join("Sagascript.app/Contents");
        let runtime = contents.join("Resources/PianissimoRuntime");
        std::fs::create_dir_all(runtime.join("python/bin")).unwrap();
        std::fs::create_dir_all(runtime.join("site-packages")).unwrap();
        std::fs::write(runtime.join("python/bin/python3.12"), b"").unwrap();
        let found = bundled_runtime(&contents.join("MacOS/sagascript")).unwrap();
        assert_eq!(PathBuf::from(found.executable), runtime.join("python/bin/python3.12"));
        assert_eq!(found.python_home.unwrap(), runtime.join("python"));
        assert_eq!(found.site_packages.unwrap(), runtime.join("site-packages"));
        std::fs::remove_dir_all(root).unwrap();
    }
}

impl PianissimoBackend {
    pub fn start() -> Result<Self, DictationError> {
        static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);
        Self::start_with_cancel(&NEVER_CANCEL)
    }

    pub fn start_with_cancel(cancelled: &AtomicBool) -> Result<Self, DictationError> {
        pianissimo_model::verify_downloaded()?;
        let runtime = runtime_python();
        let mut command = Command::new(&runtime.executable);
        if let (Some(home), Some(packages)) = (&runtime.python_home, &runtime.site_packages) {
            command.env("PYTHONHOME", home).env("PYTHONPATH", packages).env("PYTHONNOUSERSITE", "1");
        }
        let mut child = command
            .arg("-u")
            .arg("-c")
            .arg(WORKER)
            .arg("--model")
            .arg(pianissimo_model::path())
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .env("HF_DATASETS_OFFLINE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| DictationError::TranscriptionFailed(format!(
                "Could not start Pianissimo Python runtime ({error}). Reinstall the app or set SAGASCRIPT_PIANISSIMO_PYTHON to a Python 3.12 environment with NeMo and PyTorch."
            )))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let result = reader.read_line(&mut line).map(|count| (count, line, reader));
            let _ = sender.send(result);
        });
        let deadline = Instant::now() + Duration::from_secs(180);
        let (count, line, reader) = loop {
            if cancelled.load(Ordering::SeqCst) || Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DictationError::TranscriptionFailed(
                    if cancelled.load(Ordering::SeqCst) { "Pianissimo load cancelled" } else { "Pianissimo load timed out" }.into(),
                ));
            }
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(ready)) => break ready,
                Ok(Err(error)) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(DictationError::TranscriptionFailed(format!(
                        "Pianissimo startup failed: {error}"
                    )));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(DictationError::TranscriptionFailed(
                        "Pianissimo startup reader stopped".into(),
                    ));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        };
        if count == 0 || serde_json::from_str::<Value>(&line).ok()
            .and_then(|event| event.get("type").and_then(Value::as_str).map(str::to_owned))
            .as_deref() != Some("ready")
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(DictationError::TranscriptionFailed(
                "Pianissimo runtime could not load the original checkpoint. Check the local NeMo/PyTorch installation and worker logs.".into(),
            ));
        }
        Ok(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(reader),
            operation: Mutex::new(()),
        })
    }

    pub fn transcribe(&self, samples: &[f32], progress: impl Fn(u8)) -> Result<PianissimoResult, DictationError> {
        let _operation = self.operation.lock().unwrap();
        let path = std::env::temp_dir().join(format!("sagascript-pianissimo-{}.wav", uuid::Uuid::new_v4()));
        let wav = TemporaryWav(path);
        std::fs::write(&wav.0, encode_wav(samples)).map_err(|error| {
            DictationError::FileDecodeError(format!("Could not prepare Pianissimo audio: {error}"))
        })?;
        let id = uuid::Uuid::new_v4().to_string();
        {
            let mut stdin = self.stdin.lock().unwrap();
            writeln!(stdin, "{}", json!({"id": id, "path": wav.0}))
                .and_then(|_| stdin.flush())
                .map_err(|error| DictationError::TranscriptionFailed(format!(
                    "Pianissimo worker stopped before the request: {error}"
                )))?;
        }
        let mut reader = self.stdout.lock().unwrap();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).map_err(|error| DictationError::TranscriptionFailed(
                format!("Pianissimo protocol read failed: {error}")
            ))? == 0 {
                return Err(DictationError::TranscriptionFailed(
                    "Pianissimo worker exited before returning a transcript".into(),
                ));
            }
            let event: Value = serde_json::from_str(&line).map_err(|_| DictationError::TranscriptionFailed(
                "Pianissimo worker returned an invalid protocol message".into(),
            ))?;
            if event.get("id").and_then(Value::as_str) != Some(id.as_str()) {
                return Err(DictationError::TranscriptionFailed(
                    "Pianissimo worker returned a mismatched request ID".into(),
                ));
            }
            match event.get("type").and_then(Value::as_str) {
                Some("progress") => {
                    let completed = event.get("completed").and_then(Value::as_u64).unwrap_or(0);
                    let total = event.get("total").and_then(Value::as_u64).unwrap_or(0);
                    if total > 0 { progress(((completed * 100) / total).min(100) as u8); }
                }
                Some("result") => {
                    let text = event.get("text").and_then(Value::as_str).ok_or_else(||
                        DictationError::TranscriptionFailed("Pianissimo worker returned no text".into()))?.to_owned();
                    let words = serde_json::from_value(event.get("words").cloned().ok_or_else(||
                        DictationError::TranscriptionFailed("Pianissimo worker returned no word array".into()))?)
                        .map_err(|_| DictationError::TranscriptionFailed(
                            "Pianissimo worker returned invalid word timestamps".into()))?;
                    return Ok(PianissimoResult { text, words });
                }
                Some("error") => return Err(DictationError::TranscriptionFailed(format!(
                    "Pianissimo worker failed ({})",
                    event.get("error").and_then(Value::as_str).unwrap_or("unknown")
                ))),
                _ => return Err(DictationError::TranscriptionFailed(
                    "Pianissimo worker returned an unexpected protocol message".into(),
                )),
            }
        }
    }

    pub fn request_abort(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

impl Drop for PianissimoBackend {
    fn drop(&mut self) {
        if let Ok(mut stdin) = self.stdin.lock() {
            let _ = writeln!(stdin, "{{\"type\":\"close\"}}");
            let _ = stdin.flush();
        }
        if let Ok(mut child) = self.child.lock() {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if child.try_wait().ok().flatten().is_some() { return; }
                std::thread::sleep(Duration::from_millis(25));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
