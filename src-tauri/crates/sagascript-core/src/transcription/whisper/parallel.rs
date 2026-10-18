use std::sync::{Arc, Mutex};

use tracing::info;

use crate::error::DictationError;
use crate::models::{Language, WhisperModel};
use crate::transcription::chunking::AudioChunk;

use super::progress::{parallel_progress_callback, ParallelProgress};
use super::stitch::stitch_chunk_results;
use super::{TranscribeOptions, TranscriptSegment, WhisperBackend};

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

impl WhisperBackend {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn transcribe_parallel_segments(
        &self,
        audio: &[f32],
        chunks: &[AudioChunk],
        model: WhisperModel,
        language: Language,
        opts: &TranscribeOptions,
        token_eot: i32,
        on_progress: impl FnMut(i32) + Send + 'static,
        on_encode_start: Option<&dyn Fn()>,
    ) -> Result<Vec<TranscriptSegment>, DictationError> {
        let progress = Arc::new(Mutex::new(ParallelProgress {
            percentages: vec![0; chunks.len()],
            weights: chunks
                .iter()
                .map(|chunk| chunk.end_sample - chunk.start_sample)
                .collect(),
            total_weight: audio.len(),
            // Suppress zero-only updates before any chunk has progressed.
            last_reported: 0,
            callback: Box::new(on_progress),
        }));

        self.with_warm_state(|warm_state| {
            let mut secondary_states = {
                let context = self.active_context()?;
                (1..chunks.len())
                    .map(|_| {
                        context.create_state().map_err(|error| {
                            DictationError::TranscriptionFailed(format!(
                                "Failed to create parallel whisper state: {error}"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?
            };

            // Compute resources held from here: mel/encoder work begins with
            // the first chunk dispatch below.
            if let Some(on_encode_start) = on_encode_start {
                on_encode_start();
            }

            let mut chunk_results = std::thread::scope(|scope| {
                let first_chunk = chunks[0];
                let first_progress = parallel_progress_callback(Arc::clone(&progress), 0);
                let first = scope.spawn(move || {
                    self.transcribe_chunk_segments(
                        warm_state,
                        &audio[first_chunk.decode_start_sample..first_chunk.decode_end_sample],
                        first_chunk.decode_start_sample as f64 / 16_000.0,
                        model,
                        language,
                        opts,
                        token_eot,
                        first_progress,
                    )
                });

                let others = secondary_states
                    .iter_mut()
                    .zip(chunks.iter().copied().skip(1))
                    .enumerate()
                    .map(|(index, (state, chunk))| {
                        let chunk_progress =
                            parallel_progress_callback(Arc::clone(&progress), index + 1);
                        scope.spawn(move || {
                            self.transcribe_chunk_segments(
                                state,
                                &audio[chunk.decode_start_sample..chunk.decode_end_sample],
                                chunk.decode_start_sample as f64 / 16_000.0,
                                model,
                                language,
                                opts,
                                token_eot,
                                chunk_progress,
                            )
                        })
                    })
                    .collect::<Vec<_>>();

                let mut results = Vec::with_capacity(chunks.len());
                results.push(first.join().map_err(|_| {
                    DictationError::TranscriptionFailed(
                        "Parallel whisper worker panicked".to_string(),
                    )
                })??);
                for worker in others {
                    results.push(worker.join().map_err(|_| {
                        DictationError::TranscriptionFailed(
                            "Parallel whisper worker panicked".to_string(),
                        )
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
        })
    }
}

#[cfg(test)]
#[path = "parallel_tests.rs"]
mod tests;
