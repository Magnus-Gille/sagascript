//! Bounded, deterministic storage for live ASR windows.
//!
//! The queue owns no worker or capture state.  It only validates and orders
//! caller-supplied [`WindowJob`] values, keeping final windows safe from
//! eviction while allowing previews to be replaced or discarded when needed.

use super::document::Source;
use super::segmenter::WindowJob;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    InvalidJob,
    NonFinitePcm,
    FinalTooLarge,
    QueueFull,
    FinalQueueFull,
    SequenceExhausted,
}

impl std::fmt::Display for QueueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidJob => "live_queue.invalid_job",
            Self::NonFinitePcm => "live_queue.non_finite_pcm",
            Self::FinalTooLarge => "live_queue.final_too_large",
            Self::QueueFull => "live_queue.queue_full",
            Self::FinalQueueFull => "live_queue.final_queue_full",
            Self::SequenceExhausted => "live_queue.sequence_exhausted",
        })
    }
}

impl std::error::Error for QueueError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// A new window was added without replacing or evicting another window.
    Queued,
    /// A newer preview replaced an older preview with the same source and id.
    ReplacedPreview,
    /// A newer final window replaced a final window with the same source and id.
    ReplacedFinal,
    /// A final window was admitted after evicting this many previews.
    EvictedPreviews(usize),
    /// The submitted revision was not newer than the pending revision.
    IgnoredStale,
}

#[derive(Debug, Clone)]
struct PendingJob {
    job: WindowJob,
    sequence: u64,
}

/// A bounded queue of pending inference windows.
#[derive(Debug, Clone)]
pub struct WindowQueue {
    max_samples: usize,
    pending_samples: usize,
    next_sequence: u64,
    entries: Vec<PendingJob>,
}

impl WindowQueue {
    pub fn new(max_samples: usize) -> Self {
        Self {
            max_samples,
            pending_samples: 0,
            next_sequence: 0,
            entries: Vec::new(),
        }
    }

    pub fn pending_samples(&self) -> usize {
        self.pending_samples
    }

    pub fn pending_samples_for(&self, source: Source) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.job.source == source)
            .map(|entry| entry.job.pcm.len())
            .sum()
    }

    pub fn push(&mut self, job: WindowJob) -> Result<PushOutcome, QueueError> {
        validate_job(&job)?;

        let existing_index = self
            .entries
            .iter()
            .position(|entry| entry.job.source == job.source && entry.job.id == job.id);
        if let Some(index) = existing_index {
            let existing = &self.entries[index].job;
            if job.revision <= existing.revision || (!job.final_window && existing.final_window) {
                return Ok(PushOutcome::IgnoredStale);
            }
        }

        let sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(QueueError::SequenceExhausted)?;
        let existing_samples = existing_index.map_or(0, |index| self.entries[index].job.pcm.len());
        let base_samples = self
            .pending_samples
            .checked_sub(existing_samples)
            .ok_or(QueueError::InvalidJob)?;
        let base_plus_new = base_samples
            .checked_add(job.pcm.len())
            .ok_or(QueueError::QueueFull)?;

        if !job.final_window {
            if base_plus_new > self.max_samples {
                return Err(QueueError::QueueFull);
            }
            let outcome = match existing_index {
                Some(index) if self.entries[index].job.final_window => {
                    unreachable!("final preview was filtered above")
                }
                Some(_) => PushOutcome::ReplacedPreview,
                None => PushOutcome::Queued,
            };
            self.replace_entry(existing_index, job, sequence);
            return Ok(outcome);
        }

        if job.pcm.len() > self.max_samples {
            return Err(QueueError::FinalTooLarge);
        }

        let mut deficit = base_plus_new.saturating_sub(self.max_samples);
        let mut evictions = Vec::new();
        if deficit > 0 {
            let mut preview_indices: Vec<usize> = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    (Some(index) != existing_index && !entry.job.final_window).then_some(index)
                })
                .collect();
            preview_indices
                .sort_by(|left, right| entry_order(&self.entries[*right], &self.entries[*left]));
            for index in preview_indices {
                if deficit == 0 {
                    break;
                }
                deficit = deficit.saturating_sub(self.entries[index].job.pcm.len());
                evictions.push(index);
            }
        }
        if deficit > 0 {
            return Err(QueueError::FinalQueueFull);
        }

        let replaced_final =
            existing_index.is_some_and(|index| self.entries[index].job.final_window);
        let replaced_preview = existing_index.is_some() && !replaced_final;
        let evicted_count = evictions.len();
        let mut removals = evictions;
        if let Some(index) = existing_index {
            removals.push(index);
        }
        self.remove_indices(&removals);
        // remove_indices has already adjusted accounting for all removed jobs;
        // this assignment adds the incoming job using checked arithmetic.
        self.pending_samples = self
            .pending_samples
            .checked_add(job.pcm.len())
            .ok_or(QueueError::QueueFull)?;
        self.next_sequence = sequence;
        self.entries.push(PendingJob {
            job,
            sequence: sequence - 1,
        });

        if evicted_count > 0 {
            Ok(PushOutcome::EvictedPreviews(evicted_count))
        } else if replaced_preview {
            Ok(PushOutcome::ReplacedPreview)
        } else if replaced_final {
            Ok(PushOutcome::ReplacedFinal)
        } else {
            Ok(PushOutcome::Queued)
        }
    }

    pub fn pop(&mut self) -> Option<WindowJob> {
        let index = self
            .entries
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| entry_order(left, right))
            .map(|(index, _)| index)?;
        let pending = self.entries.remove(index);
        self.pending_samples = self.pending_samples.saturating_sub(pending.job.pcm.len());
        Some(pending.job)
    }

    fn replace_entry(&mut self, existing_index: Option<usize>, job: WindowJob, sequence: u64) {
        if let Some(index) = existing_index {
            self.pending_samples = self
                .pending_samples
                .saturating_sub(self.entries[index].job.pcm.len());
            self.entries.remove(index);
        }
        self.pending_samples = self.pending_samples.saturating_add(job.pcm.len());
        self.next_sequence = sequence;
        self.entries.push(PendingJob {
            job,
            sequence: sequence - 1,
        });
    }

    fn remove_indices(&mut self, indices: &[usize]) {
        let mut indices = indices.to_vec();
        indices.sort_unstable();
        indices.dedup();
        for index in indices.into_iter().rev() {
            if index < self.entries.len() {
                let pending = self.entries.remove(index);
                self.pending_samples = self.pending_samples.saturating_sub(pending.job.pcm.len());
            }
        }
    }
}

fn validate_job(job: &WindowJob) -> Result<(), QueueError> {
    if job.pcm.is_empty()
        || job.id == 0
        || job.revision == 0
        || job.end_sample.checked_sub(job.start_sample) != u64::try_from(job.pcm.len()).ok()
    {
        return Err(QueueError::InvalidJob);
    }
    if job.pcm.iter().any(|sample| !sample.is_finite()) {
        return Err(QueueError::NonFinitePcm);
    }
    Ok(())
}

fn source_order(source: Source) -> u8 {
    match source {
        Source::Microphone => 0,
        Source::System => 1,
    }
}

fn entry_order(left: &PendingJob, right: &PendingJob) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match (left.job.final_window, right.job.final_window) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => left
            .job
            .start_sample
            .cmp(&right.job.start_sample)
            .then_with(|| source_order(left.job.source).cmp(&source_order(right.job.source)))
            .then_with(|| left.job.id.cmp(&right.job.id))
            .then_with(|| left.job.revision.cmp(&right.job.revision))
            .then_with(|| left.sequence.cmp(&right.sequence)),
    }
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod queue_tests;
