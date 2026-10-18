//! Waiting for the main thread from an async command (ADR 0003).
//!
//! Some commands hand a short piece of work to the main thread (AppKit calls
//! must run there) and need its answer. The command runs on a Tokio worker, so
//! the wait must neither block the worker nor last for ever: a main thread
//! that is busy, or gone because the app is quitting, must not park a worker.

use std::time::Duration;

use tokio::sync::oneshot;

use crate::command_error::CommandError;

/// How the main thread answered.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MainThreadReply<T> {
    /// The main thread ran the work and sent this.
    Done(T),
    /// The sender was dropped without an answer: the app is shutting down, or
    /// the closure that held it panicked. Callers go on without the answer, as
    /// they did when a closed channel ended the old `recv()`.
    Disconnected,
    /// No answer within the timeout.
    TimedOut,
}

/// The main thread could not be asked: its event loop is closed. The payload is
/// the runtime's text.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub(crate) struct MainThreadUnreachable(pub(crate) String);

impl From<MainThreadUnreachable> for CommandError {
    fn from(error: MainThreadUnreachable) -> Self {
        Self::Unavailable(error.0)
    }
}

/// Hand `dispatch` a sender, let it schedule work on the main thread that
/// sends its result back, and wait for that result for at most `timeout`
/// without blocking a worker thread. `dispatch` failing (the main thread
/// cannot be reached) is returned as [`MainThreadUnreachable`].
pub(crate) async fn await_main_thread<T>(
    timeout: Duration,
    dispatch: impl FnOnce(oneshot::Sender<T>) -> Result<(), MainThreadUnreachable>,
) -> Result<MainThreadReply<T>, MainThreadUnreachable> {
    let (sender, receiver) = oneshot::channel();
    dispatch(sender)?;
    Ok(match tokio::time::timeout(timeout, receiver).await {
        Ok(Ok(value)) => MainThreadReply::Done(value),
        Ok(Err(_dropped)) => MainThreadReply::Disconnected,
        Err(_elapsed) => MainThreadReply::TimedOut,
    })
}

#[cfg(test)]
#[path = "main_thread_tests.rs"]
mod tests;
