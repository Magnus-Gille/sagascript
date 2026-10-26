use super::*;
use crate::meeting::live::document::Source;

fn job(
    source: Source,
    id: u64,
    start_sample: u64,
    samples: usize,
    final_window: bool,
    revision: u64,
) -> WindowJob {
    WindowJob {
        id,
        source,
        start_sample,
        end_sample: start_sample + samples as u64,
        pcm: vec![0.25; samples],
        final_window,
        revision,
    }
}

#[test]
fn newest_preview_replaces_same_source_and_id() {
    let mut queue = WindowQueue::new(20);
    assert_eq!(
        queue.push(job(Source::Microphone, 1, 0, 8, false, 1)),
        Ok(PushOutcome::Queued)
    );
    assert_eq!(
        queue.push(job(Source::Microphone, 1, 0, 10, false, 2)),
        Ok(PushOutcome::ReplacedPreview)
    );
    assert_eq!(queue.pending_samples(), 10);
    assert_eq!(
        queue.push(job(Source::Microphone, 1, 0, 12, false, 1)),
        Ok(PushOutcome::IgnoredStale)
    );
    assert_eq!(queue.pending_samples(), 10);
    let popped = queue.pop().expect("preview");
    assert_eq!(popped.revision, 2);
    assert!(!popped.final_window);
}

#[test]
fn final_replaces_preview_and_preview_cannot_replace_final() {
    let mut queue = WindowQueue::new(20);
    queue
        .push(job(Source::System, 7, 10, 8, false, 1))
        .expect("preview");
    assert_eq!(
        queue.push(job(Source::System, 7, 10, 12, true, 2)),
        Ok(PushOutcome::ReplacedPreview)
    );
    assert_eq!(
        queue.push(job(Source::System, 7, 10, 12, false, 99)),
        Ok(PushOutcome::IgnoredStale)
    );
    assert!(queue.pop().expect("final").final_window);
    assert_eq!(queue.pending_samples(), 0);
}

#[test]
fn final_arrival_can_evict_previews_but_full_final_queue_errors_without_mutation() {
    let mut queue = WindowQueue::new(10);
    queue
        .push(job(Source::Microphone, 1, 0, 6, false, 1))
        .expect("preview");
    assert_eq!(
        queue.push(job(Source::System, 2, 2, 8, true, 1)),
        Ok(PushOutcome::EvictedPreviews(1))
    );
    assert_eq!(queue.pending_samples(), 8);

    let result = queue.push(job(Source::Microphone, 3, 3, 6, true, 1));
    assert_eq!(result, Err(QueueError::FinalQueueFull));
    assert_eq!(queue.pending_samples(), 8);
    assert!(queue.pop().expect("existing final").final_window);
    assert!(queue.pop().is_none());
}

#[test]
fn finals_pop_before_previews_then_order_chronologically_and_fairly_by_source() {
    let mut queue = WindowQueue::new(100);
    queue
        .push(job(Source::Microphone, 2, 20, 5, false, 1))
        .expect("preview");
    queue
        .push(job(Source::System, 1, 10, 5, false, 1))
        .expect("preview");
    queue
        .push(job(Source::Microphone, 1, 10, 5, false, 1))
        .expect("preview");
    queue
        .push(job(Source::System, 3, 5, 5, true, 1))
        .expect("final");

    let first = queue.pop().expect("final");
    assert!(first.final_window);
    assert_eq!((first.source, first.id), (Source::System, 3));
    assert_eq!(
        (
            queue.pop().expect("microphone").source,
            queue.pop().expect("system").source
        ),
        (Source::Microphone, Source::System)
    );
}

#[test]
fn invalid_input_and_nonfinite_pcm_do_not_mutate_queue() {
    let mut queue = WindowQueue::new(10);
    let invalid_bounds = WindowJob {
        end_sample: 2,
        ..job(Source::Microphone, 1, 4, 1, false, 1)
    };
    assert_eq!(queue.push(invalid_bounds), Err(QueueError::InvalidJob));
    assert_eq!(queue.pending_samples(), 0);

    let mut invalid_pcm = job(Source::Microphone, 2, 0, 1, false, 1);
    invalid_pcm.pcm[0] = f32::NAN;
    assert_eq!(queue.push(invalid_pcm), Err(QueueError::NonFinitePcm));
    assert_eq!(
        queue.push(job(Source::Microphone, 0, 0, 1, false, 1)),
        Err(QueueError::InvalidJob)
    );
    assert_eq!(
        queue.push(job(Source::Microphone, 3, 0, 1, false, 0)),
        Err(QueueError::InvalidJob)
    );
    assert_eq!(queue.pending_samples(), 0);
    assert!(queue.pop().is_none());
}
