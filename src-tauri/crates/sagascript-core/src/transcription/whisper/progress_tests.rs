use super::*;

#[cfg(test)]
mod progress_callback_tests {
    use super::{
        clamped_progress_callback, parallel_progress_callback, started_progress_callback,
        ParallelProgress,
    };
    use std::sync::{Arc, Mutex};

    #[test]
    fn file_progress_leaves_zero_at_encode_start_is_monotonic_and_ends_at_100() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&received);
        let (mut callback, begin) = started_progress_callback(move |percentage| {
            captured.lock().unwrap().push(percentage);
        });
        // Nothing is emitted before the encode-start hook (lock wait, prep).
        assert!(received.lock().unwrap().is_empty());

        // Mirrors the real hook order: encode-start event, then begin().
        let order = Arc::clone(&received);
        let encode_start = || order.lock().unwrap().push(-1);
        encode_start();
        begin();
        assert_eq!(*received.lock().unwrap(), [-1, 1]);
        begin(); // idempotent

        for percentage in [0, 0, 30, 20, 30, 70, 100, 101] {
            callback(percentage);
        }

        let seen = received.lock().unwrap().clone();
        assert_eq!(seen, [-1, 1, 30, 70, 100]);
        assert!(seen[1..].windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(*seen.last().unwrap(), 100);
    }

    #[test]
    fn first_real_callback_emits_the_initial_one_when_no_encode_hook_ran() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&received);
        let (mut callback, _begin) = started_progress_callback(move |percentage| {
            captured.lock().unwrap().push(percentage);
        });
        callback(0);
        callback(40);
        assert_eq!(*received.lock().unwrap(), [1, 40]);
    }

    #[test]
    fn final_window_progress_is_clamped_to_audio_percentage_range() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&received);
        let mut callback = clamped_progress_callback(move |percentage| {
            captured.lock().unwrap().push(percentage);
        });

        for percentage in [-1, 0, 99, 100, 101] {
            callback(percentage);
        }

        assert_eq!(*received.lock().unwrap(), [0, 0, 99, 100, 100]);
    }

    #[test]
    fn parallel_progress_is_weighted_and_never_moves_backwards() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&received);
        let progress = Arc::new(Mutex::new(ParallelProgress {
            percentages: vec![0, 0],
            weights: vec![1, 3],
            total_weight: 4,
            last_reported: -1,
            callback: Box::new(move |percentage| {
                captured.lock().unwrap().push(percentage);
            }),
        }));
        let mut first = parallel_progress_callback(Arc::clone(&progress), 0);
        let mut second = parallel_progress_callback(progress, 1);

        first(100);
        second(50);
        first(0);
        second(100);
        first(100);

        assert_eq!(*received.lock().unwrap(), [25, 62, 75, 100]);
    }
}
