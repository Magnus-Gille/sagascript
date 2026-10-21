use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::time::Instant;

use super::inference_wait::{wait_for_inference, WaitOutcome};

#[tokio::test(start_paused = true)]
async fn immediate_completion_preserves_generic_success_and_error_outputs_without_abort() {
    let aborts = AtomicUsize::new(0);
    let mut success = tokio::spawn(async { Ok::<_, &'static str>(("text", 17_u64)) });
    match wait_for_inference(
        &mut success,
        Duration::from_secs(60),
        Duration::from_secs(5),
        || {
            aborts.fetch_add(1, Ordering::SeqCst);
        },
    )
    .await
    {
        WaitOutcome::Completed(Ok(Ok((text, timing)))) => assert_eq!((text, timing), ("text", 17)),
        _ => panic!("immediate success must be returned unchanged"),
    }

    let mut failure = tokio::spawn(async { Err::<(String, u64), _>("inference failed") });
    match wait_for_inference(
        &mut failure,
        Duration::from_secs(60),
        Duration::from_secs(5),
        || {
            aborts.fetch_add(1, Ordering::SeqCst);
        },
    )
    .await
    {
        WaitOutcome::Completed(Ok(Err(error))) => assert_eq!(error, "inference failed"),
        _ => panic!("immediate error must be returned unchanged"),
    }
    assert_eq!(aborts.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn cancelled_task_returns_its_join_error_without_requesting_abort() {
    let aborts = AtomicUsize::new(0);
    let mut task = tokio::spawn(async {
        std::future::pending::<()>().await;
        "unreachable"
    });
    let task_id = task.id();
    task.abort();

    match wait_for_inference(
        &mut task,
        Duration::from_secs(60),
        Duration::from_secs(5),
        || {
            aborts.fetch_add(1, Ordering::SeqCst);
        },
    )
    .await
    {
        WaitOutcome::Completed(Err(error)) => {
            assert!(error.is_cancelled());
            assert_eq!(error.id(), task_id);
        }
        _ => panic!("cancelled task must return its original join error"),
    }
    assert_eq!(aborts.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn completion_before_deadline_does_not_abort() {
    let aborts = AtomicUsize::new(0);
    let mut task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(4)).await;
        "finished"
    });
    assert!(matches!(
        wait_for_inference(
            &mut task,
            Duration::from_secs(5),
            Duration::from_secs(2),
            || {
                aborts.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await,
        WaitOutcome::Completed(Ok("finished"))
    ));
    assert_eq!(aborts.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn timeout_aborts_once_and_grace_completion_still_returns_timed_out() {
    let aborts = Arc::new(AtomicUsize::new(0));
    let abort_counter = aborts.clone();
    let start = Instant::now();
    let abort_at = std::cell::Cell::new(None);
    let mut task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(12)).await;
        Ok::<_, &'static str>(("late success", 123_u64))
    });

    match wait_for_inference(
        &mut task,
        Duration::from_secs(10),
        Duration::from_secs(5),
        || {
            abort_counter.fetch_add(1, Ordering::SeqCst);
            abort_at.set(Some(Instant::now()));
        },
    )
    .await
    {
        WaitOutcome::TimedOut { unwound } => assert!(unwound),
        WaitOutcome::Completed(_) => panic!("grace completion must not replace timeout"),
    }
    assert_eq!(aborts.load(Ordering::SeqCst), 1);
    assert_eq!(abort_at.get(), Some(start + Duration::from_secs(10)));
    assert_eq!(Instant::now(), start + Duration::from_secs(12));
}

#[tokio::test(start_paused = true)]
async fn error_during_grace_still_returns_timeout_after_unwind() {
    let aborts = AtomicUsize::new(0);
    let start = Instant::now();
    let abort_at = std::cell::Cell::new(None);
    let mut task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(4)).await;
        Err::<(), _>("late inference error")
    });

    assert!(matches!(
        wait_for_inference(
            &mut task,
            Duration::from_secs(3),
            Duration::from_secs(2),
            || {
                aborts.fetch_add(1, Ordering::SeqCst);
                abort_at.set(Some(Instant::now()));
            },
        )
        .await,
        WaitOutcome::TimedOut { unwound: true }
    ));
    assert_eq!(aborts.load(Ordering::SeqCst), 1);
    assert_eq!(abort_at.get(), Some(start + Duration::from_secs(3)));
    assert_eq!(Instant::now(), start + Duration::from_secs(4));
}

#[tokio::test(start_paused = true)]
async fn grace_expiry_keeps_same_future_awaitable_and_does_not_change_outcome() {
    let (sender, receiver) = tokio::sync::oneshot::channel::<(String, u64)>();
    let aborts = AtomicUsize::new(0);
    let start = Instant::now();
    let mut task = receiver;
    let abort_at = std::cell::Cell::new(None);

    assert!(matches!(
        wait_for_inference(
            &mut task,
            Duration::from_secs(3),
            Duration::from_secs(2),
            || {
                aborts.fetch_add(1, Ordering::SeqCst);
                abort_at.set(Some(Instant::now()));
            },
        )
        .await,
        WaitOutcome::TimedOut { unwound: false }
    ));
    assert_eq!(aborts.load(Ordering::SeqCst), 1);
    assert_eq!(abort_at.get(), Some(start + Duration::from_secs(3)));
    assert_eq!(Instant::now(), start + Duration::from_secs(5));

    sender.send(("late result".to_owned(), 456)).unwrap();
    assert_eq!(task.await.unwrap(), ("late result".to_owned(), 456));
}
