use std::future::Future;
use std::time::Duration;

/// Result of waiting for inference and, after a timeout, for its abort grace period.
pub(crate) enum WaitOutcome<T> {
    /// The task completed before the initial deadline.
    Completed(T),
    /// The initial deadline elapsed and abort was requested.
    TimedOut { unwound: bool },
}

/// Wait for inference, request abort once on timeout, and give it a bounded grace period.
///
/// The future remains borrowed throughout, so a caller may continue awaiting it after the
/// grace period without restarting or cancelling the underlying task.
pub(crate) async fn wait_for_inference<F, A>(
    task: &mut F,
    timeout: Duration,
    abort_grace: Duration,
    abort: A,
) -> WaitOutcome<F::Output>
where
    F: Future + Unpin,
    A: FnOnce(),
{
    match tokio::time::timeout(timeout, &mut *task).await {
        Ok(output) => WaitOutcome::Completed(output),
        Err(_) => {
            abort();
            let unwound = tokio::time::timeout(abort_grace, &mut *task).await.is_ok();
            WaitOutcome::TimedOut { unwound }
        }
    }
}
