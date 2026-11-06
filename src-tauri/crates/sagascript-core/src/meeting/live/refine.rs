//! Bounded second pass over the entire retained recording, with independent source lanes.
//! Longer silence context replaces live preview deadlines; no permanent files are created.
use super::{
    document::{LiveSegment, SegmentStatus, Source},
    recording::Recording,
    segmenter::{Config, Segmenter, WindowJob},
};
use crate::transcription::engine_host::{CancelToken, Transcription};

const FRAME: usize = 160;
const READ_FRAMES: usize = 16_000;
const MAX_SEGMENTS: usize = 20_000;
const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;

/// Recognition and echo processing are injected so ordering, cancellation and fallback can
/// be verified without opening a device or loading a model. The returned transcript is staged:
/// callers must publish it atomically only after both sources succeed.
pub(super) fn run(
    recording: &mut Recording,
    cancel: &CancelToken,
    mut clean: impl FnMut(&[f32; FRAME], &[f32; FRAME]) -> Result<[f32; FRAME], String>,
    mut recognize: impl FnMut(&WindowJob) -> Result<Transcription, String>,
    mut progress: impl FnMut(u8) -> Result<(), String>,
) -> Result<Vec<LiveSegment>, String> {
    let config = Config {
        silence_ms: 1_500,
        preview_interval_ms: 30_000,
        overlap_ms: 2_000,
        ..Config::default()
    };
    let mut segmenters = [
        Segmenter::new(config, Source::Microphone).map_err(|e| e.to_string())?,
        Segmenter::new(config, Source::System).map_err(|e| e.to_string())?,
    ];
    let mut result = Staged::default();
    let total = recording.duration_samples();
    let mut offset = 0;
    let mut reported = None;
    while offset < total {
        check_cancel(cancel)?;
        let (mic, system) = recording
            .read_frames(offset, READ_FRAMES)
            .map_err(|e| e.to_string())?;
        if mic.is_empty() || mic.len() != system.len() {
            return Err("Retained audio ended unexpectedly.".into());
        }
        for base in (0..mic.len()).step_by(FRAME) {
            check_cancel(cancel)?;
            let count = (mic.len() - base).min(FRAME);
            let mut near = [0.; FRAME];
            let mut far = [0.; FRAME];
            near[..count].copy_from_slice(&mic[base..base + count]);
            far[..count].copy_from_slice(&system[base..base + count]);
            let cleaned = clean(&near, &far)?;
            for (index, pcm) in [&cleaned[..count], &far[..count]].into_iter().enumerate() {
                let speech = crate::audio::has_audio_signal(pcm).map_err(str::to_string)?;
                let jobs = segmenters[index]
                    .push(offset + base as u64, pcm, speech)
                    .map_err(|e| e.to_string())?;
                for job in jobs.into_iter().filter(|job| job.final_window) {
                    check_cancel(cancel)?;
                    let recognized = recognize(&job)?;
                    check_cancel(cancel)?;
                    result.add(job, recognized)?;
                }
            }
        }
        offset += mic.len() as u64;
        let percent = (offset.saturating_mul(99) / total.max(1)) as u8;
        if reported != Some(percent) {
            progress(percent)?;
            reported = Some(percent);
        }
    }
    for segmenter in &mut segmenters {
        for job in segmenter.finish() {
            check_cancel(cancel)?;
            let recognized = recognize(&job)?;
            check_cancel(cancel)?;
            result.add(job, recognized)?;
        }
    }
    check_cancel(cancel)?;
    Ok(result.segments)
}

fn check_cancel(cancel: &CancelToken) -> Result<(), String> {
    if cancel.is_cancelled() {
        Err("Transcript refinement was cancelled.".into())
    } else {
        Ok(())
    }
}

#[derive(Default)]
struct Staged {
    segments: Vec<LiveSegment>,
    committed: [u64; 2],
    text_bytes: usize,
}
impl Staged {
    fn add(&mut self, job: WindowJob, recognized: Transcription) -> Result<(), String> {
        let index = match job.source {
            Source::Microphone => 0,
            Source::System => 1,
        };
        let overlap = self.committed[index].saturating_sub(job.start_sample) as f64 / 16_000.;
        let text = if overlap > 0. {
            if recognized.words.is_empty() && !recognized.text.trim().is_empty() {
                return Err("Refinement returned overlapping speech without word timing.".into());
            }
            recognized
                .words
                .iter()
                .filter(|word| word.end > overlap)
                .map(|word| word.word.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            recognized.text
        };
        let start = job.start_sample.max(self.committed[index]);
        self.committed[index] = job.end_sample;
        if text.trim().is_empty() {
            return Ok(());
        }
        self.text_bytes = self
            .text_bytes
            .checked_add(text.len())
            .ok_or("Refined transcript is too large.")?;
        if self.text_bytes > MAX_TEXT_BYTES || self.segments.len() >= MAX_SEGMENTS {
            return Err("Refined transcript is too large.".into());
        }
        self.segments.push(LiveSegment {
            id: format!("refined-{index}-{}", self.segments.len()),
            source: job.source,
            start_ms: start / 16,
            end_ms: job.end_sample / 16,
            text,
            status: SegmentStatus::Final,
            revision: 1,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn transcript(text: &str) -> Transcription {
        Transcription {
            text: text.into(),
            words: vec![],
            tokens: vec![],
            windows: vec![],
            audio_s: 0.,
        }
    }
    fn recording(seconds: usize) -> Recording {
        let mut r = Recording::new().unwrap();
        // Distinct channel content; both contain stable signal.
        r.append(&vec![0.2; seconds * 16_000], &vec![0.4; seconds * 16_000])
            .unwrap();
        r.finalize().unwrap();
        r
    }
    #[test]
    fn replays_both_sources_without_previews_and_keeps_recording_exportable() {
        let mut r = recording(2);
        let hash = r.sha256().unwrap();
        let mut frames = 0;
        let mut jobs = Vec::new();
        let mut progress = Vec::new();
        let result = run(
            &mut r,
            &CancelToken::new(),
            |mic, far| {
                frames += 1;
                assert!((mic[0] - 0.2).abs() < 0.001);
                assert!((far[0] - 0.4).abs() < 0.001);
                Ok(*mic)
            },
            |job| {
                assert!(job.final_window);
                jobs.push(job.source);
                Ok(transcript(if job.source == Source::Microphone {
                    "local"
                } else {
                    "remote"
                }))
            },
            |p| {
                progress.push(p);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(frames, 200);
        assert_eq!(jobs, vec![Source::Microphone, Source::System]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].text, "local");
        assert_eq!(result[1].text, "remote");
        assert_eq!(r.sha256().unwrap(), hash);
        assert!(progress.windows(2).all(|p| p[0] <= p[1]));
    }
    #[test]
    fn cancelled_or_failed_second_source_cannot_return_partial_replacement() {
        let token = CancelToken::new();
        let mut r = recording(1);
        let result = run(
            &mut r,
            &token,
            |m, _| Ok(*m),
            |job| {
                if job.source == Source::System {
                    token.cancel();
                }
                Ok(transcript("partial"))
            },
            |_| Ok(()),
        );
        assert!(result.is_err());
        let result = run(
            &mut r,
            &CancelToken::new(),
            |m, _| Ok(*m),
            |job| {
                if job.source == Source::System {
                    Err("model failed".into())
                } else {
                    Ok(transcript("partial"))
                }
            },
            |_| Ok(()),
        );
        assert!(result.is_err());
    }
    #[test]
    fn brief_pause_retains_context_that_live_finalization_would_split() {
        let mut pcm = vec![0.2; 16_000];
        pcm.extend(vec![0.; 12_800]); // 800ms: longer than live600ms, shorter than offline1500ms.
        pcm.extend(vec![0.2; 16_000]);
        let mut r = Recording::new().unwrap();
        r.append(&pcm, &vec![0.; pcm.len()]).unwrap();
        r.finalize().unwrap();
        let mut jobs = 0;
        let result = run(
            &mut r,
            &CancelToken::new(),
            |m, _| Ok(*m),
            |job| {
                jobs += 1;
                assert_eq!(job.source, Source::Microphone);
                assert!(job.pcm.len() >= 44_000);
                Ok(transcript("one sentence"))
            },
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(jobs, 1);
        assert_eq!(result.len(), 1);
    }
    #[test]
    fn long_speech_is_bounded_and_requires_timing_for_overlap() {
        let mut r = recording(32);
        let mut calls = 0;
        let result = run(
            &mut r,
            &CancelToken::new(),
            |m, _| Ok(*m),
            |job| {
                calls += 1;
                assert!(job.pcm.len() <= 30 * 16_000);
                Ok(transcript("untimed speech"))
            },
            |_| Ok(()),
        );
        assert!(calls >= 2);
        assert!(result.unwrap_err().contains("without word timing"));
    }
    #[test]
    fn silence_does_not_invoke_model_or_invent_segments() {
        let mut r = Recording::new().unwrap();
        r.append(&[0.; 1600], &[0.; 1600]).unwrap();
        r.finalize().unwrap();
        assert!(run(
            &mut r,
            &CancelToken::new(),
            |m, _| Ok(*m),
            |_| panic!("silence ASR"),
            |_| Ok(())
        )
        .unwrap()
        .is_empty());
    }
}
