use std::sync::{Arc, Mutex};

use tracing::info;

use whisper_rs::WhisperState;

use crate::error::DictationError;
use crate::models::WhisperModel;
use crate::transcription::chunking::AudioChunk;

use super::entry::{ChunkJob, DecodeSettings};
use super::progress::{parallel_progress_callback, ParallelProgress};
use super::stitch::stitch_chunk_results;
use super::{TranscriptSegment, WhisperBackend};

/// Parallel file transcription is deliberately bounded: each state owns GPU
/// buffers and a Metal command queue, so unbounded fan-out quickly wastes RAM.
pub const MAX_PARALLEL_CHUNKS: usize = 4;

/// Do not split into sub-minute pieces. Whisper loses decoder context at every
/// boundary, and short chunks do not amortize state creation.
pub(super) const MIN_PARALLEL_CHUNK_SAMPLES: usize = 60 * 16_000;
const AUTO_PARALLEL_MIN_SAMPLES: usize = 10 * 60 * 16_000;
const AUTO_PARALLEL_MAX_MODEL_MB: u32 = 500;

/// Select the measured-safe automatic file-transcription parallelism.
///
/// Beam search benefits from two Metal command queues on long files. Greedy
/// decoding did not speed up in the benchmark, and large models have not yet
/// passed the memory gate, so both remain serial unless explicitly overridden.
pub fn recommended_parallel_chunks(
    audio_samples: usize,
    model: WhisperModel,
    beam_size: u32,
) -> usize {
    if beam_size >= 2
        && audio_samples >= AUTO_PARALLEL_MIN_SAMPLES
        && model.size_mb() <= AUTO_PARALLEL_MAX_MODEL_MB
    {
        2
    } else {
        1
    }
}

/// A chunk's progress callback. Boxed because the decoder is called through one
/// shared reference by every worker.
pub(super) type ChunkProgress = Box<dyn FnMut(i32) + Send + 'static>;

/// Everything a parallel transcription needs besides the backend: the whole
/// recording, its plan, how to decode and who watches.
pub(super) struct ParallelJob<'a> {
    pub(super) audio: &'a [f32],
    pub(super) chunks: &'a [AudioChunk],
    pub(super) settings: DecodeSettings<'a>,
    /// Told the length-weighted progress of all chunks together.
    pub(super) on_progress: ChunkProgress,
    /// Fires once, when every state exists and decoding is about to start.
    pub(super) on_encode_start: Option<&'a dyn Fn()>,
}

impl WhisperBackend {
    /// Decode the chunks of a long recording at once, each on its own Whisper
    /// state, and stitch the results; see [`decode_in_parallel`]. The first
    /// chunk runs on the warm state, the others on states built from the
    /// active context.
    pub(super) fn transcribe_parallel_segments(
        &self,
        job: ParallelJob<'_>,
    ) -> Result<Vec<TranscriptSegment>, DictationError> {
        self.with_warm_state(|warm_state| {
            let context = self.active_context()?;
            decode_in_parallel(
                job,
                warm_state,
                move || {
                    context.create_state().map_err(|error| {
                        DictationError::TranscriptionFailed(format!(
                            "Failed to create parallel whisper state: {error}"
                        ))
                    })
                },
                &|state: &mut WhisperState, chunk: ChunkJob<'_>, progress: ChunkProgress| {
                    self.transcribe_chunk_segments(state, chunk, progress)
                },
            )
        })
    }
}

/// Decode the chunks of `job` at once, one per state and one thread each, and
/// stitch the results into one list of segments.
///
/// Generic over the state `S` and the `decode` step so that the orchestration
/// (state creation, progress aggregation, workers, error and panic mapping,
/// stitching) can be tested with a fake decoder; the backend passes Whisper
/// states and `transcribe_chunk_segments`.
///
/// Order of events: build the secondary states (`create_state`, once per chunk
/// after the first), fire `on_encode_start`, run all chunks, then join the
/// workers in chunk order. The first chunk's error, or panic, wins over a later
/// chunk's.
fn decode_in_parallel<S, D>(
    job: ParallelJob<'_>,
    warm_state: &mut S,
    mut create_state: impl FnMut() -> Result<S, DictationError>,
    decode: &D,
) -> Result<Vec<TranscriptSegment>, DictationError>
where
    S: Send,
    D: Fn(&mut S, ChunkJob<'_>, ChunkProgress) -> Result<Vec<TranscriptSegment>, DictationError>
        + Sync,
{
    let ParallelJob {
        audio,
        chunks,
        settings,
        on_progress,
        on_encode_start,
    } = job;
    let progress = Arc::new(Mutex::new(ParallelProgress {
        percentages: vec![0; chunks.len()],
        weights: chunks
            .iter()
            .map(|chunk| chunk.end_sample - chunk.start_sample)
            .collect(),
        total_weight: audio.len(),
        // Suppress zero-only updates before any chunk has progressed.
        last_reported: 0,
        callback: on_progress,
    }));

    let mut secondary_states = (1..chunks.len())
        .map(|_| create_state())
        .collect::<Result<Vec<_>, _>>()?;
    // The states exist; whatever `create_state` held on to is not needed again.
    drop(create_state);

    // Compute resources held from here: mel/encoder work begins with the first
    // chunk dispatch below.
    if let Some(on_encode_start) = on_encode_start {
        on_encode_start();
    }

    let chunk_job = |chunk: AudioChunk| ChunkJob {
        audio: &audio[chunk.decode_start_sample..chunk.decode_end_sample],
        offset_seconds: chunk.decode_start_sample as f64 / 16_000.0,
        settings,
    };

    let mut chunk_results = std::thread::scope(|scope| {
        let first_job = chunk_job(chunks[0]);
        let first_progress = parallel_progress_callback(Arc::clone(&progress), 0);
        let first = scope.spawn(move || decode(warm_state, first_job, Box::new(first_progress)));

        let others = secondary_states
            .iter_mut()
            .zip(chunks.iter().copied().skip(1))
            .enumerate()
            .map(|(index, (state, chunk))| {
                let job = chunk_job(chunk);
                let chunk_progress = parallel_progress_callback(Arc::clone(&progress), index + 1);
                scope.spawn(move || decode(state, job, Box::new(chunk_progress)))
            })
            .collect::<Vec<_>>();

        let mut results = Vec::with_capacity(chunks.len());
        results.push(first.join().map_err(|_| {
            DictationError::TranscriptionFailed("Parallel whisper worker panicked".to_string())
        })??);
        for worker in others {
            results.push(worker.join().map_err(|_| {
                DictationError::TranscriptionFailed("Parallel whisper worker panicked".to_string())
            })??);
        }
        Ok::<_, DictationError>(results)
    })?;

    let segments = stitch_chunk_results(
        std::mem::take(&mut chunk_results),
        audio.len() as f64 / 16_000.0,
    );
    info!(
        "Parallel local transcription complete: {} segment(s) across {} chunks",
        segments.len(),
        chunks.len()
    );
    Ok(segments)
}

#[cfg(test)]
#[path = "parallel_tests.rs"]
mod tests;
