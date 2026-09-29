//! Long-audio pipeline: one PCM file, planned windows, bounded in-flight
//! windows, strictly ordered merge, progress and cancellation.

use super::client::{batch_lane, EngineHostClient, WindowOutput, WindowRequest, WindowTicket};
use super::{EngineHostError, Result};
use crate::transcription::chunk_merge::{
    merge_window, offset_tokens, plan_windows, tokens_to_text, tokens_to_words, AlignedToken,
    Window, Word,
};
use sagascript_engine_protocol::Priority;
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Cooperative cancellation flag shared between the caller and a running job.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct JobOptions {
    pub priority: Priority,
}

impl Default for JobOptions {
    fn default() -> Self {
        Self {
            priority: Priority::Batch,
        }
    }
}

/// Per-window timing report.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowTiming {
    pub index: usize,
    pub start_sample: usize,
    pub end_sample: usize,
    pub tokens: usize,
    pub preprocess_ms: u64,
    pub encode_ms: u64,
    pub decode_ms: u64,
    /// Client-observed submit-to-result time.
    pub round_trip_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Transcription {
    pub text: String,
    pub words: Vec<Word>,
    /// Merged tokens on the global timeline (text with a leading space per word).
    pub tokens: Vec<AlignedToken>,
    pub windows: Vec<WindowTiming>,
    pub audio_s: f64,
}

/// Raw f32le PCM in a private (0700) temp dir, removed on drop.
pub struct PcmFile {
    dir: tempfile::TempDir,
    path: PathBuf,
}

impl PcmFile {
    pub fn write(samples: &[f32], parent: Option<&Path>) -> Result<Self> {
        let mut builder = tempfile::Builder::new();
        builder.prefix("sagascript-pcm-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let dir = match parent {
            Some(p) => builder.tempdir_in(p)?,
            None => builder.tempdir()?,
        };
        let path = dir.path().join("audio.f32le");
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = std::io::BufWriter::with_capacity(1 << 20, opts.open(&path)?);
        for chunk in samples.chunks(16 * 1024) {
            let mut bytes = Vec::with_capacity(chunk.len() * 4);
            for s in chunk {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            f.write_all(&bytes)?;
        }
        f.flush()?;
        Ok(Self { dir, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn dir(&self) -> &Path {
        self.dir.path()
    }
}

/// Progress callback: `(merged_windows, total_windows)`.
pub type ProgressFn<'a> = &'a mut dyn FnMut(usize, usize);

impl EngineHostClient {
    /// Dictation: one window with `interactive` priority when the utterance
    /// fits `max_window_s`, otherwise the windowed path (still interactive).
    pub fn transcribe_dictation(
        &self,
        samples: &[f32],
        cancel: &CancelToken,
    ) -> Result<Transcription> {
        self.transcribe_samples(
            samples,
            JobOptions {
                priority: Priority::Interactive,
            },
            cancel,
            None,
        )
    }

    /// File job: batch priority, windows merged in order.
    pub fn transcribe_file_samples(
        &self,
        samples: &[f32],
        cancel: &CancelToken,
        progress: Option<ProgressFn<'_>>,
    ) -> Result<Transcription> {
        self.transcribe_samples(samples, JobOptions::default(), cancel, progress)
    }

    /// Transcribe 16 kHz mono f32 samples through the host.
    pub fn transcribe_samples(
        &self,
        samples: &[f32],
        opts: JobOptions,
        cancel: &CancelToken,
        mut progress: Option<ProgressFn<'_>>,
    ) -> Result<Transcription> {
        if cancel.is_cancelled() {
            return Err(EngineHostError::Cancelled);
        }
        if samples.is_empty() {
            return Ok(Transcription {
                text: String::new(),
                words: vec![],
                tokens: vec![],
                windows: vec![],
                audio_s: 0.0,
            });
        }
        let cfg = self.config();
        let caps = self.warm_with_cancel(cancel)?;
        let sr = caps.sample_rate;
        let max_window = (caps.max_window_s * f64::from(sr)) as usize;
        let plan: Vec<Window> = if samples.len() <= max_window {
            vec![Window {
                start_sample: 0,
                end_sample: samples.len(),
            }]
        } else {
            plan_windows(
                samples.len(),
                sr,
                caps.preferred_window_s.min(caps.max_window_s),
                caps.preferred_overlap_s,
            )
        };
        let total = plan.len();
        // Batch work leaves one host slot free for dictation (see `batch_lane`).
        let lane = match opts.priority {
            Priority::Batch => batch_lane(caps.max_in_flight),
            Priority::Interactive => caps.max_in_flight.max(1) as usize,
        };
        let max_in_flight = lane.min(total);
        let pcm = PcmFile::write(samples, cfg.temp_dir.as_deref())?;
        let max_retries = cfg.max_crash_retries;

        let req = |i: usize| WindowRequest {
            pcm_path: pcm.path().to_path_buf(),
            offset_samples: plan[i].start_sample as u64,
            num_samples: (plan[i].end_sample - plan[i].start_sample) as u64,
            sample_rate: sr,
            priority: opts.priority,
        };

        let mut queue: VecDeque<(usize, WindowTicket)> = VecDeque::new();
        let mut retries = vec![0u32; total];
        let mut next = 0usize;
        let mut merged = 0usize;
        let mut acc: Vec<AlignedToken> = Vec::new();
        let mut timings: Vec<WindowTiming> = Vec::with_capacity(total);

        while merged < total {
            if cancel.is_cancelled() {
                return Err(EngineHostError::Cancelled); // queued tickets cancel on drop
            }
            let mut failure: Option<EngineHostError> = None;
            let mut failed_idx: Option<usize> = None;
            while queue.len() < max_in_flight && next < total {
                let wait = if queue.is_empty() {
                    Duration::from_millis(50)
                } else {
                    Duration::ZERO
                };
                match self.submit_window(&req(next), cancel, wait) {
                    Ok(Some(t)) => {
                        queue.push_back((next, t));
                        next += 1;
                    }
                    Ok(None) => break,
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            }
            if failure.is_none() {
                let Some((idx, ticket)) = queue.pop_front() else {
                    continue; // no slot yet (an interactive request is running)
                };
                match ticket.wait(cancel) {
                    Ok(out) => {
                        debug_assert_eq!(idx, merged);
                        acc = merge_output(acc, &plan[idx], idx, sr, &caps, out, &mut timings);
                        merged += 1;
                        if let Some(p) = progress.as_mut() {
                            p(merged, total);
                        }
                        continue;
                    }
                    Err(e) => {
                        failed_idx = Some(idx);
                        failure = Some(e);
                    }
                }
            }
            let err = failure.expect("failure set on this path");
            if !err.is_crash() {
                return Err(err); // queued tickets cancel on drop
            }
            // The host died: every window in flight is affected. Resubmit them
            // in order, each at most `max_crash_retries` times.
            let mut affected: Vec<usize> = failed_idx.into_iter().collect();
            affected.extend(queue.iter().map(|(i, _)| *i));
            if failed_idx.is_none() {
                affected.push(next);
            }
            for &i in &affected {
                retries[i] += 1;
                if retries[i] > max_retries {
                    return Err(err);
                }
            }
            tracing::warn!(windows = ?affected, "engine host crashed; resubmitting in-flight windows");
            next = affected[0];
            queue.clear();
        }
        Ok(finish(acc, timings, samples.len(), sr))
    }
}

fn merge_output(
    acc: Vec<AlignedToken>,
    window: &Window,
    index: usize,
    sr: u32,
    caps: &sagascript_engine_protocol::Capabilities,
    out: WindowOutput,
    timings: &mut Vec<WindowTiming>,
) -> Vec<AlignedToken> {
    let mut tokens: Vec<AlignedToken> = out
        .tokens
        .into_iter()
        .map(|t| {
            let mut a = AlignedToken::new(t.id, t.text.replace('▁', " "), t.start, t.duration);
            a.confidence = t.confidence;
            a
        })
        .collect();
    offset_tokens(&mut tokens, window.start_sample as f64 / f64::from(sr));
    timings.push(WindowTiming {
        index,
        start_sample: window.start_sample,
        end_sample: window.end_sample,
        tokens: tokens.len(),
        preprocess_ms: out.timings.preprocess_ms,
        encode_ms: out.timings.encode_ms,
        decode_ms: out.timings.decode_ms,
        round_trip_ms: out.round_trip_ms,
    });
    if tokens.is_empty() {
        acc
    } else if acc.is_empty() {
        tokens
    } else {
        merge_window(&acc, &tokens, caps.preferred_overlap_s)
    }
}

fn finish(
    tokens: Vec<AlignedToken>,
    windows: Vec<WindowTiming>,
    n_samples: usize,
    sr: u32,
) -> Transcription {
    Transcription {
        text: tokens_to_text(&tokens),
        words: tokens_to_words(&tokens),
        tokens,
        windows,
        audio_s: n_samples as f64 / f64::from(sr),
    }
}
