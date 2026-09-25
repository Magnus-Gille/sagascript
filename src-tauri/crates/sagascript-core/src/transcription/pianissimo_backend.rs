//! Local subprocess adapter for the native NeMo-Speech.cpp Pianissimo runtime.
//!
//! The native executable is started for each transcription request. Keeping the
//! process short lived makes cancellation reliable and avoids shipping Python,
//! while the model itself remains in Sagascript's normal local model cache.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::audio::wav::encode_wav;
use crate::error::DictationError;
use crate::transcription::pianissimo_model;

const MIN_TRANSCRIPTION_TIMEOUT: Duration = Duration::from_secs(180);
const SAMPLE_RATE: u64 = 16_000;

fn transcription_timeout(sample_count: usize) -> Duration {
    let audio_seconds = (sample_count as u64).saturating_add(SAMPLE_RATE - 1) / SAMPLE_RATE;
    MIN_TRANSCRIPTION_TIMEOUT.max(Duration::from_secs(audio_seconds.saturating_mul(10)))
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PianissimoWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PianissimoResult {
    pub text: String,
    pub words: Vec<PianissimoWord>,
}

pub struct PianissimoBackend {
    executable: PathBuf,
    active_child: Mutex<Option<Child>>,
    operation: Mutex<()>,
}

struct TemporaryWav(PathBuf);

impl Drop for TemporaryWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Return whether this OS can run the bundled native runtime.
///
/// The native NeMo-Speech.cpp runtime is built for macOS 13 and later. Other
/// platforms use the CPU backend and are allowed through for development and
/// future packaging.
pub fn runtime_supported_on_this_os() -> bool {
    #[cfg(target_os = "macos")]
    {
        let Ok(output) = Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
        else {
            return false;
        };
        output.status.success()
            && std::str::from_utf8(&output.stdout)
                .ok()
                .is_some_and(macos_version_supported)
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[cfg(target_os = "macos")]
fn macos_version_supported(version: &str) -> bool {
    version
        .trim()
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .is_some_and(|major| major >= 13)
}

fn bundled_executable(current_exe: &Path) -> Option<PathBuf> {
    let contents = current_exe.parent()?.parent()?;
    let executable = contents.join("Resources/PianissimoRuntime/bin/nemo-speech");
    executable.is_file().then_some(executable)
}

fn resolve_executable() -> PathBuf {
    if let Some(executable) = std::env::var_os("SAGASCRIPT_PIANISSIMO_EXECUTABLE") {
        return PathBuf::from(executable);
    }

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(executable) = bundled_executable(&current_exe) {
            return executable;
        }
    }

    // Leave PATH lookup to Command::new so development can use an installed
    // `nemo-speech` without requiring a machine-specific absolute path.
    PathBuf::from("nemo-speech")
}

fn backend_device() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "metal"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "cpu"
    }
}

fn parse_transcript_json(bytes: &[u8]) -> Result<PianissimoResult, String> {
    #[derive(Deserialize)]
    struct NativeTranscript {
        text: String,
        words: Vec<PianissimoWord>,
    }

    let transcript: NativeTranscript = serde_json::from_slice(bytes)
        .map_err(|error| format!("native runtime returned invalid JSON: {error}"))?;
    Ok(PianissimoResult {
        text: transcript.text,
        words: transcript.words,
    })
}

impl PianissimoBackend {
    pub fn start() -> Result<Self, DictationError> {
        static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);
        Self::start_with_cancel(&NEVER_CANCEL)
    }

    pub fn start_with_cancel(cancelled: &AtomicBool) -> Result<Self, DictationError> {
        if !runtime_supported_on_this_os() {
            return Err(DictationError::TranscriptionFailed(
                "Pianissimo requires macOS 13 or later".into(),
            ));
        }
        if cancelled.load(Ordering::SeqCst) {
            return Err(DictationError::TranscriptionFailed(
                "Pianissimo load cancelled".into(),
            ));
        }
        pianissimo_model::verify_downloaded()?;

        Ok(Self {
            executable: resolve_executable(),
            active_child: Mutex::new(None),
            operation: Mutex::new(()),
        })
    }

    pub fn transcribe(
        &self,
        samples: &[f32],
        progress: impl Fn(u8),
    ) -> Result<PianissimoResult, DictationError> {
        let _operation = self.operation.lock().map_err(|_| {
            DictationError::TranscriptionFailed("Pianissimo operation lock was poisoned".into())
        })?;
        let path = std::env::temp_dir().join(format!(
            "sagascript-pianissimo-{}.wav",
            uuid::Uuid::new_v4()
        ));
        let wav = TemporaryWav(path);
        std::fs::write(&wav.0, encode_wav(samples)).map_err(|error| {
            DictationError::FileDecodeError(format!("Could not prepare Pianissimo audio: {error}"))
        })?;

        progress(0);
        let mut child = Command::new(&self.executable)
            .arg("transcribe")
            .arg(&wav.0)
            .arg("--model")
            .arg(pianissimo_model::path())
            .arg("--language")
            .arg("sv")
            .arg("--device")
            .arg(backend_device())
            .arg("--format")
            .arg("json")
            .arg("--word-times")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| {
                DictationError::TranscriptionFailed(format!(
                    "Could not start Pianissimo native runtime ({}): {error}",
                    self.executable.display()
                ))
            })?;
        let mut stdout = child.stdout.take().ok_or_else(|| {
            DictationError::TranscriptionFailed(
                "Pianissimo native runtime did not provide stdout".into(),
            )
        })?;
        let stdout_reader = std::thread::spawn(move || {
            let mut output = Vec::new();
            let result = stdout.read_to_end(&mut output);
            (result, output)
        });

        {
            let mut active = self.active_child.lock().map_err(|_| {
                DictationError::TranscriptionFailed("Pianissimo process lock was poisoned".into())
            })?;
            *active = Some(child);
        }

        let deadline = Instant::now() + transcription_timeout(samples.len());
        let status = loop {
            if Instant::now() >= deadline {
                self.request_abort();
                let _ = stdout_reader.join();
                return Err(DictationError::TranscriptionFailed(
                    "Pianissimo native runtime timed out while transcribing".into(),
                ));
            }
            let status = {
                let mut active = self.active_child.lock().map_err(|_| {
                    DictationError::TranscriptionFailed(
                        "Pianissimo process lock was poisoned".into(),
                    )
                })?;
                active.as_mut().map(|child| child.try_wait())
            };
            match status {
                Some(Ok(Some(status))) => break status,
                Some(Ok(None)) => std::thread::sleep(Duration::from_millis(25)),
                Some(Err(error)) => {
                    self.request_abort();
                    let _ = stdout_reader.join();
                    return Err(DictationError::TranscriptionFailed(format!(
                        "Pianissimo native runtime status check failed: {error}"
                    )));
                }
                None => {
                    let _ = stdout_reader.join();
                    return Err(DictationError::TranscriptionFailed(
                        "Pianissimo native runtime disappeared before completing".into(),
                    ));
                }
            }
        };

        let output = stdout_reader.join().map_err(|_| {
            DictationError::TranscriptionFailed(
                "Pianissimo native runtime output reader stopped".into(),
            )
        })?;
        {
            let mut active = self.active_child.lock().map_err(|_| {
                DictationError::TranscriptionFailed("Pianissimo process lock was poisoned".into())
            })?;
            let _ = active.take();
        }

        if !status.success() {
            return Err(DictationError::TranscriptionFailed(format!(
                "Pianissimo native runtime exited with {status}"
            )));
        }
        let (read_result, output) = output;
        read_result.map_err(|error| {
            DictationError::TranscriptionFailed(format!(
                "Pianissimo native runtime output read failed: {error}"
            ))
        })?;
        let result = parse_transcript_json(&output).map_err(DictationError::TranscriptionFailed)?;
        progress(100);
        Ok(result)
    }

    pub fn request_abort(&self) {
        if let Ok(mut active) = self.active_child.lock() {
            if let Some(child) = active.as_mut() {
                let _ = child.kill();
            }
        }
    }
}

impl Drop for PianissimoBackend {
    fn drop(&mut self) {
        self.request_abort();
        if let Ok(mut active) = self.active_child.lock() {
            if let Some(mut child) = active.take() {
                let _ = child.wait();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_files_get_a_duration_scaled_deadline() {
        assert_eq!(transcription_timeout(16_000), Duration::from_secs(180));
        assert_eq!(transcription_timeout(16_000 * 60), Duration::from_secs(600));
    }

    #[test]
    fn parses_native_json_with_extra_metadata_and_confidence() {
        let result = parse_transcript_json(
            r#"{"file":"recording.wav","text":"Hej världen.","confidence":0.98,"duration":1.2,"languages":["sv"],"words":[{"word":"Hej","start":0.1,"end":0.4,"confidence":0.99},{"word":"världen.","start":0.5,"end":1.1,"confidence":0.97}]}"#
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(result.text, "Hej världen.");
        assert_eq!(result.words.len(), 2);
        assert_eq!(result.words[1].word, "världen.");
        assert!((result.words[1].start - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn rejects_native_json_without_word_timestamps() {
        let error = parse_transcript_json(br#"{"text":"Hej"}"#).unwrap_err();
        assert!(error.contains("missing field `words`"), "{error}");
    }

    #[test]
    fn bundled_runtime_resolves_relative_to_app_executable() {
        let root =
            std::env::temp_dir().join(format!("sagascript-runtime-test-{}", uuid::Uuid::new_v4()));
        let contents = root.join("Sagascript.app/Contents");
        let runtime = contents.join("Resources/PianissimoRuntime/bin");
        std::fs::create_dir_all(&runtime).unwrap();
        let executable = runtime.join("nemo-speech");
        std::fs::write(&executable, b"").unwrap();
        let found = bundled_executable(&contents.join("MacOS/sagascript")).unwrap();
        assert_eq!(found, executable);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn request_abort_kills_fake_sleeping_runtime() {
        let child = Command::new("/bin/sleep").arg("10").spawn().unwrap();
        let backend = PianissimoBackend {
            executable: PathBuf::from("/bin/sleep"),
            active_child: Mutex::new(Some(child)),
            operation: Mutex::new(()),
        };
        let started = Instant::now();
        backend.request_abort();
        let mut active = backend.active_child.lock().unwrap();
        let status = active.as_mut().unwrap().wait().unwrap();
        assert!(!status.success());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_13_is_supported_by_native_runtime() {
        assert!(macos_version_supported("13.7.6"));
        assert!(macos_version_supported("14.0"));
        assert!(macos_version_supported("26.6.2\n"));
        assert!(!macos_version_supported("12.6.9"));
        assert!(!macos_version_supported("unknown"));
    }
}
