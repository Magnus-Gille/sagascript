//! Greedy TDT (token-and-duration transducer) decode loop.
//!
//! A line-for-line port of `decode_tdt` in the benchmark runner `run_onnx.py`
//! (which mirrors `onnx_asr.models.nemo.NemoConformerTdt`): one joint call per
//! visited encoder frame; the decoder state advances only on non-blank tokens; the
//! duration head (argmax over the trailing logits) is a frame skip in `0..=4`.

pub const HIDDEN: usize = 1024;
/// Seconds per encoder frame: 10 ms hop x subsampling 8.
pub const FRAME_S: f64 = 0.08;

/// Prediction-network state (two LSTM layers).
#[derive(Debug, Clone, PartialEq)]
pub struct DecoderState {
    pub s1: Vec<f32>,
    pub s2: Vec<f32>,
}

/// One decoder+joint evaluation.
pub trait Joint {
    /// `frame` has `HIDDEN` values. Returns the flat logits (`vocab_size` token logits
    /// followed by the duration logits) and the state after consuming `target`.
    fn step(
        &mut self,
        frame: &[f32],
        target: i32,
        state: &DecoderState,
    ) -> Result<(Vec<f32>, DecoderState), String>;

    fn initial_state(&self) -> DecoderState;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawToken {
    pub id: u32,
    pub frame: usize,
    pub duration_frames: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    Cancelled,
    Joint(String),
}

fn argmax(values: &[f32]) -> usize {
    // First maximum wins, like numpy.argmax.
    let mut best = 0;
    for (index, value) in values.iter().enumerate() {
        if *value > values[best] {
            best = index;
        }
    }
    best
}

/// Decode `frames` (row-major `t x HIDDEN`, `t` = valid encoder length).
pub fn decode(
    frames: &[f32],
    joint: &mut dyn Joint,
    vocab_size: usize,
    blank: u32,
    max_tokens_per_step: usize,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Vec<RawToken>, DecodeError> {
    debug_assert_eq!(frames.len() % HIDDEN, 0);
    let total = frames.len() / HIDDEN;
    let mut state = joint.initial_state();
    let mut out = Vec::new();
    let mut last = blank as i32;
    let mut t = 0usize;
    let mut emitted = 0usize;
    while t < total {
        if is_cancelled() {
            return Err(DecodeError::Cancelled);
        }
        let (logits, next_state) = joint
            .step(&frames[t * HIDDEN..(t + 1) * HIDDEN], last, &state)
            .map_err(DecodeError::Joint)?;
        if logits.len() <= vocab_size {
            return Err(DecodeError::Joint(format!(
                "joint returned {} logits for vocab size {vocab_size}",
                logits.len()
            )));
        }
        let token = argmax(&logits[..vocab_size]) as u32;
        let step = argmax(&logits[vocab_size..]);
        if token != blank {
            state = next_state;
            out.push(RawToken {
                id: token,
                frame: t,
                duration_frames: step,
            });
            last = token as i32;
            emitted += 1;
        }
        if step > 0 {
            t += step;
            emitted = 0;
        } else if token == blank || emitted == max_tokens_per_step {
            t += 1;
            emitted = 0;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const V: usize = 4; // ids 0..=2 real, 3 = blank
    const BLANK: u32 = 3;

    /// Scripted joint: `script[call]` gives (token, duration_step); the state counts
    /// non-blank updates so we can assert state handling.
    struct Scripted {
        script: Vec<(usize, usize)>,
        calls: usize,
        seen_targets: Vec<i32>,
        seen_frames: Vec<f32>,
    }

    impl Joint for Scripted {
        fn step(
            &mut self,
            frame: &[f32],
            target: i32,
            state: &DecoderState,
        ) -> Result<(Vec<f32>, DecoderState), String> {
            let (token, step) = self.script[self.calls.min(self.script.len() - 1)];
            self.calls += 1;
            self.seen_targets.push(target);
            self.seen_frames.push(frame[0]);
            let mut logits = vec![0.0f32; V + 5];
            logits[token] = 1.0;
            logits[V + step] = 1.0;
            let mut next = state.clone();
            next.s1[0] += 1.0;
            Ok((logits, next))
        }

        fn initial_state(&self) -> DecoderState {
            DecoderState {
                s1: vec![0.0; 2],
                s2: vec![0.0; 2],
            }
        }
    }

    fn frames(n: usize) -> Vec<f32> {
        let mut f = vec![0.0; n * HIDDEN];
        for t in 0..n {
            f[t * HIDDEN] = t as f32;
        }
        f
    }

    fn run(script: Vec<(usize, usize)>, n: usize, max_tokens: usize) -> (Vec<RawToken>, Scripted) {
        let mut joint = Scripted {
            script,
            calls: 0,
            seen_targets: vec![],
            seen_frames: vec![],
        };
        let tokens = decode(&frames(n), &mut joint, V, BLANK, max_tokens, &|| false).unwrap();
        (tokens, joint)
    }

    #[test]
    fn blank_with_zero_duration_advances_one_frame() {
        let (tokens, joint) = run(vec![(3, 0)], 3, 10);
        assert!(tokens.is_empty());
        assert_eq!(joint.seen_frames, vec![0.0, 1.0, 2.0]);
    }

    #[test]
    fn token_with_duration_skips_frames_and_updates_target() {
        // frame 0: token 1 dur 2 -> t=2; frame 2: token 2 dur 1 -> t=3 (end)
        let (tokens, joint) = run(vec![(1, 2), (2, 1)], 3, 10);
        assert_eq!(
            tokens,
            vec![
                RawToken { id: 1, frame: 0, duration_frames: 2 },
                RawToken { id: 2, frame: 2, duration_frames: 1 },
            ]
        );
        assert_eq!(joint.seen_targets, vec![3, 1]); // blank first, then last emitted
    }

    #[test]
    fn zero_duration_token_stays_on_frame_until_max_tokens_per_step() {
        // Emits token 0 with duration 0 repeatedly; after max_tokens (2) advances one frame.
        let (tokens, joint) = run(vec![(0, 0)], 1, 2);
        assert_eq!(tokens.len(), 2);
        assert!(tokens.iter().all(|t| t.frame == 0 && t.duration_frames == 0));
        assert_eq!(joint.calls, 2);
    }

    #[test]
    fn blank_with_positive_duration_skips_without_state_update() {
        let (tokens, joint) = run(vec![(3, 3)], 6, 10);
        assert!(tokens.is_empty());
        assert_eq!(joint.seen_frames, vec![0.0, 3.0]);
    }

    #[test]
    fn duration_may_jump_past_the_end() {
        let (tokens, joint) = run(vec![(1, 4)], 2, 10);
        assert_eq!(tokens.len(), 1);
        assert_eq!(joint.calls, 1);
    }

    #[test]
    fn ties_pick_the_first_maximum() {
        assert_eq!(argmax(&[1.0, 3.0, 3.0, 2.0]), 1);
    }

    #[test]
    fn cancellation_is_observed_between_frames() {
        let mut joint = Scripted {
            script: vec![(3, 0)],
            calls: 0,
            seen_targets: vec![],
            seen_frames: vec![],
        };
        let polls = Cell::new(0);
        let result = decode(&frames(5), &mut joint, V, BLANK, 10, &|| {
            polls.set(polls.get() + 1);
            polls.get() > 2
        });
        assert_eq!(result, Err(DecodeError::Cancelled));
        assert_eq!(joint.calls, 2);
    }

    #[test]
    fn joint_errors_and_short_logits_surface() {
        struct Bad;
        impl Joint for Bad {
            fn step(&mut self, _: &[f32], _: i32, s: &DecoderState) -> Result<(Vec<f32>, DecoderState), String> {
                Ok((vec![0.0; V], s.clone()))
            }
            fn initial_state(&self) -> DecoderState {
                DecoderState { s1: vec![], s2: vec![] }
            }
        }
        let err = decode(&frames(1), &mut Bad, V, BLANK, 10, &|| false).unwrap_err();
        assert!(matches!(err, DecodeError::Joint(_)));
    }
}
