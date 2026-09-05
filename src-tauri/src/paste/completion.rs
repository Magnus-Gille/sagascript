use std::time::Duration;

use tokio::sync::oneshot;

pub const COMPLETION_TIMEOUT_MS: u64 = 2_000;
#[cfg(any(target_os = "windows", test))]
pub const WINDOWS_MODIFIER_WAIT_MS: u64 = 1_000;

/// Never move focus while a timed-out native paste may still be running.
pub fn should_open_copy_fallback(outcome: &str) -> bool {
    matches!(outcome, "failed" | "dispatch_failed" | "completion_dropped")
}

const COMPLETION_DROPPED_ERROR: &str =
    "Automatic paste did not report a result. Copy the recognized text from Dictate.";
const TIMED_OUT_ERROR: &str = "Automatic paste did not finish in time and may still complete. \
    The recognized text is available in Dictate; check the editor before pasting again.";

#[derive(Debug, PartialEq, Eq)]
pub struct PasteCompletion {
    pub outcome: &'static str,
    pub error: Option<String>,
    pub call_completed: bool,
}

pub async fn wait(
    receiver: oneshot::Receiver<Result<(), String>>,
    timeout: Duration,
) -> PasteCompletion {
    match tokio::time::timeout(timeout, receiver).await {
        Ok(Ok(Ok(()))) => PasteCompletion {
            outcome: "succeeded",
            error: None,
            call_completed: true,
        },
        Ok(Ok(Err(error))) => PasteCompletion {
            outcome: "failed",
            error: Some(error),
            call_completed: true,
        },
        Ok(Err(_)) => PasteCompletion {
            outcome: "completion_dropped",
            error: Some(COMPLETION_DROPPED_ERROR.to_string()),
            call_completed: false,
        },
        Err(_) => PasteCompletion {
            outcome: "timed_out",
            error: Some(TIMED_OUT_ERROR.to_string()),
            call_completed: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_completed_or_undispatched_failures_open_copy_fallback() {
        for (outcome, expected) in [
            ("failed", true),
            ("dispatch_failed", true),
            ("completion_dropped", true),
            ("timed_out", false),
            ("succeeded", false),
            ("disabled", false),
            ("", false),
            ("unknown", false),
        ] {
            assert_eq!(should_open_copy_fallback(outcome), expected, "{outcome}");
        }
    }

    #[test]
    fn modifier_wait_leaves_completion_slack() {
        let slack = COMPLETION_TIMEOUT_MS
            .checked_sub(WINDOWS_MODIFIER_WAIT_MS)
            .unwrap();
        assert!(slack >= 500);
    }

    #[tokio::test]
    async fn reports_success_when_paste_returns_ok() {
        let (sender, receiver) = oneshot::channel();
        sender.send(Ok(())).unwrap();

        assert_eq!(
            wait(receiver, Duration::from_secs(1)).await,
            PasteCompletion {
                outcome: "succeeded",
                error: None,
                call_completed: true,
            }
        );
    }

    #[tokio::test]
    async fn preserves_paste_error_when_call_returns_err() {
        let (sender, receiver) = oneshot::channel();
        sender
            .send(Err("Accessibility permission denied".to_string()))
            .unwrap();

        assert_eq!(
            wait(receiver, Duration::from_secs(1)).await,
            PasteCompletion {
                outcome: "failed",
                error: Some("Accessibility permission denied".to_string()),
                call_completed: true,
            }
        );
    }

    #[tokio::test]
    async fn reports_dropped_completion_with_actionable_error() {
        let (sender, receiver) = oneshot::channel();
        drop(sender);

        let completion = wait(receiver, Duration::from_secs(1)).await;
        assert_eq!(completion.outcome, "completion_dropped");
        assert_eq!(completion.error.as_deref(), Some(COMPLETION_DROPPED_ERROR));
        assert!(!completion.call_completed);
    }

    #[tokio::test]
    async fn reports_timeout_that_may_still_complete() {
        let (sender, receiver) = oneshot::channel();
        let completion = wait(receiver, Duration::ZERO).await;
        drop(sender);

        assert_eq!(completion.outcome, "timed_out");
        assert_eq!(completion.error.as_deref(), Some(TIMED_OUT_ERROR));
        assert!(!completion.call_completed);
    }
}
