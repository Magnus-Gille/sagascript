//! One explicitly started session, shared by CLI and desktop adapters.
use super::{
    recording::Recording,
    state::{Snapshot, State},
};
use crate::meeting::MeetingExportFormat;
use crate::transcription::engine_host::CancelToken;
use crate::{fsutil, settings::Settings, sync::lock_or_recover};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

pub struct Session {
    id: String,
    pub(crate) state: Mutex<State>,
    pub(crate) recording: Mutex<Option<Recording>>,
    pub(crate) stop: AtomicBool,
    pub(crate) cancel: CancelToken,
    pub(crate) done: AtomicBool,
    #[cfg(all(target_os = "macos", feature = "record"))]
    pub(crate) epoch_ns: std::sync::atomic::AtomicU64,
}
impl Session {
    pub fn supported() -> bool {
        #[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "record"))]
        {
            super::capture::supported()
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64", feature = "record")))]
        {
            false
        }
    }
    pub fn start(settings: Settings) -> Result<Arc<Self>, String> {
        Self::start_with_id(settings, uuid::Uuid::new_v4().to_string())
    }
    pub fn start_with_id(settings: Settings, id: String) -> Result<Arc<Self>, String> {
        if !Self::supported() {
            return Err(
                "Live meetings require Apple Silicon, macOS 14.2+, and the local Pianissimo model."
                    .into(),
            );
        }
        let session = Arc::new(Self {
            id: id.clone(),
            state: Mutex::new(State::new(id).map_err(|e| e.to_string())?),
            recording: Mutex::new(None),
            stop: AtomicBool::new(false),
            cancel: CancelToken::new(),
            done: AtomicBool::new(false),
            #[cfg(all(target_os = "macos", feature = "record"))]
            epoch_ns: std::sync::atomic::AtomicU64::new(0),
        });
        #[cfg(all(target_os = "macos", feature = "record"))]
        {
            let worker = session.clone();
            std::thread::Builder::new()
                .name("live-meeting".into())
                .spawn(move || {
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        super::runtime::run(&worker, settings)
                    }));
                    if let Err(panic) = outcome {
                        // No panic payload: it can contain audio, model output, or a local path.
                        drop(panic);
                        let mut state = lock_or_recover(&worker.state, "live.state");
                        let _ = state.fail(
                            "Live processing stopped unexpectedly. The session is incomplete.",
                        );
                    }
                    worker.done.store(true, Ordering::Release);
                })
                .map_err(|e| e.to_string())?;
        }
        #[cfg(not(all(target_os = "macos", feature = "record")))]
        {
            let _ = (settings, &session);
            return Err("Live capture is unavailable on this platform".into());
        }
        #[allow(unreachable_code)]
        Ok(session)
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn snapshot(&self) -> Snapshot {
        lock_or_recover(&self.state, "live.state").snapshot()
    }
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
    /// Stop capture, then drain all final windows. This never cancels queued speech.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if lock_or_recover(&self.state, "live.state").phase() == super::state::Phase::Starting {
            self.cancel.cancel();
        }
    }
    /// App shutdown only. Explicit discard in the UI is allowed only after draining.
    pub fn shutdown(&self) {
        self.stop();
        self.cancel.cancel();
    }
    pub fn discard(&self) -> Result<(), String> {
        if !self.is_done() {
            return Err("Wait for capture and transcription to stop before discarding.".into());
        }
        lock_or_recover(&self.recording, "live.recording").take();
        Ok(())
    }
    pub fn export_text(
        &self,
        path: &Path,
        format: MeetingExportFormat,
    ) -> Result<fsutil::Published, String> {
        if !self.is_done() {
            return Err("Wait for the final transcript before exporting.".into());
        }
        let document = lock_or_recover(&self.state, "live.state")
            .export_document("sv", "pianissimo-sv", None)
            .map_err(|e| e.to_string())?;
        let text = document.export(format).map_err(|e| e.to_string())?;
        fsutil::publish_new(path, text.as_bytes()).map_err(|e| e.to_string())
    }
    pub fn export_audio(&self, path: &Path) -> Result<fsutil::Published, String> {
        if !self.is_done() {
            return Err("Wait for recording to stop before exporting.".into());
        }
        let mut slot = lock_or_recover(&self.recording, "live.recording");
        slot.as_mut()
            .ok_or("No temporary recording is available")?
            .export(path)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> Session {
        Session {
            id: "test".into(),
            state: Mutex::new(State::new("test".into()).unwrap()),
            recording: Mutex::new(None),
            stop: AtomicBool::new(false),
            cancel: CancelToken::new(),
            done: AtomicBool::new(false),
            #[cfg(all(target_os = "macos", feature = "record"))]
            epoch_ns: std::sync::atomic::AtomicU64::new(0),
        }
    }
    #[test]
    fn starting_stop_cancels_warmup_but_running_stop_drains() {
        let starting = session();
        starting.stop();
        assert!(starting.cancel.is_cancelled());
        let running = session();
        running.state.lock().unwrap().started().unwrap();
        running.stop();
        assert!(running.stop.load(Ordering::Acquire));
        assert!(!running.cancel.is_cancelled());
    }
    #[test]
    fn active_session_cannot_export_or_discard_and_failed_export_is_retryable() {
        let session = session();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.wav");
        assert!(session.discard().is_err());
        assert!(session.export_audio(&path).is_err());
        assert!(session
            .export_text(&path, MeetingExportFormat::Plain)
            .is_err());
        assert!(!path.exists());
        let mut recording = Recording::new().unwrap();
        recording.append(&[0.1; 160], &[0.2; 160]).unwrap();
        recording.finalize().unwrap();
        *session.recording.lock().unwrap() = Some(recording);
        session.done.store(true, Ordering::Release);
        std::fs::write(&path, b"existing").unwrap();
        assert!(session.export_audio(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"existing");
        let retry = dir.path().join("saved.wav");
        session.export_audio(&retry).unwrap();
        assert_eq!(&std::fs::read(retry).unwrap()[..4], b"RIFF");
        session.discard().unwrap();
        assert!(session.recording.lock().unwrap().is_none());
    }
}
