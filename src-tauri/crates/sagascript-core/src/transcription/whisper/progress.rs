use std::sync::{Arc, Mutex};

use crate::sync::lock_or_recover;

use super::{PARALLEL_PROGRESS_LOCK, PROGRESS_LOCK};

/// Adapt whisper.cpp's native progress values to the backend's public
/// percentage contract.
pub(super) fn clamped_progress_callback(
    mut on_progress: impl FnMut(i32) + 'static,
) -> impl FnMut(i32) + 'static {
    move |percentage| on_progress(percentage.clamp(0, 100))
}

/// Wrap a progress callback so the sequence is: one initial `1` (work has
/// started), then strictly increasing real percentages up to 100. Values at or
/// below the last reported one (including whisper's initial 0) are dropped.
///
/// Returns the wrapped callback and a `begin` hook. The initial `1` is emitted
/// lazily by whichever comes first: `begin()` (called when encoding starts) or
/// the first real callback; it is never emitted twice.
pub(super) fn started_progress_callback<F>(
    on_progress: F,
) -> (
    impl FnMut(i32) + Send + 'static,
    impl Fn() + Send + Sync + 'static,
)
where
    F: FnMut(i32) + Send + 'static,
{
    struct State<F> {
        on_progress: F,
        last: i32,
        started: bool,
    }
    impl<F: FnMut(i32)> State<F> {
        fn begin(&mut self) {
            if !self.started {
                self.started = true;
                self.last = 1;
                (self.on_progress)(1);
            }
        }
    }
    let state = Arc::new(Mutex::new(State {
        on_progress,
        last: 0,
        started: false,
    }));
    let begin_state = Arc::clone(&state);
    let begin = move || lock_or_recover(&begin_state, PROGRESS_LOCK).begin();
    let callback = move |percentage: i32| {
        let mut state = lock_or_recover(&state, PROGRESS_LOCK);
        state.begin();
        let percentage = percentage.clamp(0, 100);
        if percentage > state.last {
            state.last = percentage;
            (state.on_progress)(percentage);
        }
    };
    (callback, begin)
}

pub(super) struct ParallelProgress {
    pub(super) percentages: Vec<i32>,
    pub(super) weights: Vec<usize>,
    pub(super) total_weight: usize,
    pub(super) last_reported: i32,
    pub(super) callback: Box<dyn FnMut(i32) + Send>,
}

pub(super) fn parallel_progress_callback(
    progress: Arc<Mutex<ParallelProgress>>,
    chunk_index: usize,
) -> impl FnMut(i32) + Send + 'static {
    move |percentage| {
        let mut progress = lock_or_recover(&progress, PARALLEL_PROGRESS_LOCK);
        progress.percentages[chunk_index] = percentage.clamp(0, 100);
        let weighted_sum = progress
            .percentages
            .iter()
            .zip(&progress.weights)
            .map(|(percentage, weight)| *percentage as usize * *weight)
            .sum::<usize>();
        let aggregate = (weighted_sum / progress.total_weight.max(1)) as i32;
        if aggregate > progress.last_reported {
            progress.last_reported = aggregate;
            (progress.callback)(aggregate);
        }
    }
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
