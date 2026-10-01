//! Client for the persistent `sagascript-engine-host` sidecar
//! (`docs/engine-host-protocol.md`).
//!
//! Threading model: the API is **blocking** and thread-based, like
//! `pianissimo_backend` and the Whisper backends (callers run it inside
//! `spawn_blocking` / a worker thread). Per host process there is one stdout
//! reader thread (routes responses to waiters by request id) and one stderr
//! drain thread; per client there is one small supervisor thread for idle
//! unload/shutdown. No async runtime is required, which keeps the CLI's lean
//! build and test doubles simple.
//!
//! Layers:
//! - [`process`]: spawn, JSON-Lines transport, id routing, crash detection.
//! - [`client`]: handshake, lifecycle, restart budget, window scheduling.
//! - [`pipeline`]: long-audio windowing, in-order merge, progress, cancel.

pub mod client;
pub mod pipeline;
pub mod process;

pub use client::{
    ClientIdentity, EngineHostClient, EngineHostConfig, HostSnapshot, LoadEvent, LoadObserver, LoadSpec, TestHook, Timeouts,
    WindowOutput,
};
pub use pipeline::{CancelToken, JobOptions, Transcription, WindowTiming};
pub use process::CrashInfo;
pub use sagascript_engine_protocol as protocol;
pub use sagascript_engine_protocol::{Capabilities, ErrorCode, Priority};

use std::time::Duration;

/// Everything that can go wrong talking to the engine host.
#[derive(Debug, thiserror::Error)]
pub enum EngineHostError {
    #[error("failed to start engine host {path}: {source}")]
    Spawn {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("engine host handshake failed: {0}")]
    Handshake(String),
    #[error("engine host protocol violation: {0}")]
    Protocol(String),
    #[error("engine host error [{}]: {message}", code.as_str())]
    Remote {
        code: ErrorCode,
        message: String,
        retryable: bool,
    },
    #[error("engine host did not answer `{op}` within {after:?}")]
    Timeout { op: &'static str, after: Duration },
    #[error("engine host exited unexpectedly ({status}); stderr tail:\n{stderr_tail}")]
    Crashed { status: String, stderr_tail: String },
    #[error("engine host is marked failed after repeated crashes ({0}); reset required")]
    Failed(String),
    #[error("engine host is not running")]
    NotRunning,
    #[error("transcription cancelled")]
    Cancelled,
    #[error("engine host I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid request: {0}")]
    InvalidInput(String),
}

impl EngineHostError {
    pub fn is_crash(&self) -> bool {
        matches!(self, EngineHostError::Crashed { .. })
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, EngineHostError::Cancelled)
            || matches!(
                self,
                EngineHostError::Remote {
                    code: ErrorCode::Cancelled,
                    ..
                }
            )
    }
}

pub type Result<T> = std::result::Result<T, EngineHostError>;
