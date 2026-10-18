use super::*;

/// Moved from the former `segment_confidence_tests` (#371); the module name is kept so
/// the test ids stay stable.
#[cfg(test)]
mod segment_confidence_tests {
    use super::*;

    #[test]
    fn auto_parallelism_requires_long_beam_audio_and_bounded_model() {
        let long_audio = 10 * 60 * 16_000;
        assert_eq!(
            recommended_parallel_chunks(long_audio, WhisperModel::KbWhisperSmall, 5),
            2
        );
        assert_eq!(
            recommended_parallel_chunks(long_audio - 1, WhisperModel::KbWhisperSmall, 5),
            1
        );
        assert_eq!(
            recommended_parallel_chunks(long_audio, WhisperModel::KbWhisperSmall, 0),
            1
        );
        assert_eq!(
            recommended_parallel_chunks(long_audio, WhisperModel::KbWhisperLarge, 5),
            1
        );
    }
}

/// Characterisation of the parallel-chunk limits (#371). Chunk planning itself
/// is tested in `chunking.rs`; this pins the numbers this module feeds it and
/// the progress weights it derives from the plan.
#[cfg(test)]
mod parallel_limit_tests {
    use super::*;
    use crate::transcription::chunking::plan_chunks;

    const SECOND: usize = 16_000;

    fn planned(seconds: usize, requested: usize) -> Vec<AudioChunk> {
        let audio = vec![0.25; seconds * SECOND];
        plan_chunks(
            &audio,
            requested.clamp(1, MAX_PARALLEL_CHUNKS),
            MIN_PARALLEL_CHUNK_SAMPLES,
        )
    }

    #[test]
    fn the_limits_are_four_states_and_one_minute_pieces() {
        assert_eq!(MAX_PARALLEL_CHUNKS, 4);
        assert_eq!(MIN_PARALLEL_CHUNK_SAMPLES, 60 * SECOND);
        assert_eq!(AUTO_PARALLEL_MIN_SAMPLES, 10 * 60 * SECOND);
        assert_eq!(AUTO_PARALLEL_MAX_MODEL_MB, 500);
    }

    #[test]
    fn a_recording_is_never_split_into_pieces_shorter_than_a_minute() {
        assert_eq!(planned(59, 4).len(), 1);
        assert_eq!(planned(119, 4).len(), 1);
        assert_eq!(planned(120, 4).len(), 2);
        assert_eq!(planned(300, 4).len(), 4);
    }

    #[test]
    fn requests_of_zero_or_above_the_maximum_are_clamped() {
        assert_eq!(planned(600, 0).len(), 1);
        assert_eq!(planned(600, 1).len(), 1);
        assert_eq!(planned(600, 99).len(), MAX_PARALLEL_CHUNKS);
    }

    #[test]
    fn automatic_parallelism_starts_with_beam_two_and_never_asks_for_more_than_two() {
        let long = AUTO_PARALLEL_MIN_SAMPLES;
        let small = WhisperModel::KbWhisperSmall;
        assert_eq!(recommended_parallel_chunks(long, small, 1), 1);
        assert_eq!(recommended_parallel_chunks(long, small, 2), 2);
        assert_eq!(recommended_parallel_chunks(usize::MAX, small, 8), 2);
        assert!(recommended_parallel_chunks(usize::MAX, small, u32::MAX) <= MAX_PARALLEL_CHUNKS);
    }
}

/// The orchestration of a parallel transcription (#371), driven through
/// `decode_in_parallel` with a fake decoder: which state decodes which slice,
/// the order of events, what the workers' progress adds up to, and how a failed
/// chunk or a panicking worker ends the run. The native decoder behind
/// `transcribe_chunk_segments` needs a model and is not part of this.
#[cfg(test)]
mod orchestration_tests {
    use super::*;
    use crate::models::Language;
    use crate::transcription::whisper::TranscribeOptions;
    use std::sync::mpsc;
    use std::time::Duration;

    const SECOND: usize = 16_000;
    /// Generous for a wait that is released by an event, never by the clock.
    const PATIENT: Duration = Duration::from_secs(30);

    /// A Whisper state stand-in: the warm state is 0, built ones count up.
    struct FakeState(usize);

    fn chunk(start: usize, end: usize, decode_start: usize, decode_end: usize) -> AudioChunk {
        AudioChunk {
            start_sample: start * SECOND,
            end_sample: end * SECOND,
            decode_start_sample: decode_start * SECOND,
            decode_end_sample: decode_end * SECOND,
        }
    }

    /// Three equal chunks of 150 s, as the planner cuts them: each decodes five
    /// seconds of context on either internal side.
    fn three_chunks() -> Vec<AudioChunk> {
        vec![
            chunk(0, 50, 0, 55),
            chunk(50, 100, 45, 105),
            chunk(100, 150, 95, 150),
        ]
    }

    fn segment(start: f64, end: f64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            start,
            end,
            text: text.to_string(),
            avg_logprob: None,
            no_speech_prob: 0.0,
        }
    }

    fn audio_of(seconds: usize) -> Vec<f32> {
        vec![0.25; seconds * SECOND]
    }

    fn settings(options: &TranscribeOptions) -> DecodeSettings<'_> {
        DecodeSettings {
            model: WhisperModel::KbWhisperBase,
            language: Language::Swedish,
            options,
            token_eot: 50_257,
        }
    }

    fn job<'a>(
        audio: &'a [f32],
        chunks: &'a [AudioChunk],
        options: &'a TranscribeOptions,
        on_progress: ChunkProgress,
        on_encode_start: Option<&'a dyn Fn()>,
    ) -> ParallelJob<'a> {
        ParallelJob {
            audio,
            chunks,
            settings: settings(options),
            on_progress,
            on_encode_start,
        }
    }

    /// Builds states 1, 2, ... like `context.create_state()` would.
    fn counting_states() -> impl FnMut() -> Result<FakeState, DictationError> {
        let mut built = 0;
        move || {
            built += 1;
            Ok(FakeState(built))
        }
    }

    fn texts(segments: &[TranscriptSegment]) -> Vec<&str> {
        segments.iter().map(|s| s.text.as_str()).collect()
    }

    fn silent() -> ChunkProgress {
        Box::new(|_| {})
    }

    fn failed(message: &str) -> DictationError {
        DictationError::TranscriptionFailed(message.to_string())
    }

    fn message(error: DictationError) -> String {
        match error {
            DictationError::TranscriptionFailed(message) => message,
            other => panic!("expected TranscriptionFailed, got {other:?}"),
        }
    }

    #[test]
    fn every_chunk_is_decoded_on_its_own_state_with_its_own_slice_and_offset() {
        let audio = audio_of(150);
        let chunks = three_chunks();
        let options = TranscribeOptions::default();
        let calls = Mutex::new(Vec::new());

        let segments = decode_in_parallel(
            job(&audio, &chunks, &options, silent(), None),
            &mut FakeState(0),
            counting_states(),
            &|state: &mut FakeState, chunk: ChunkJob<'_>, _progress| {
                calls.lock().unwrap().push((
                    state.0,
                    chunk.audio.len(),
                    chunk.offset_seconds,
                    chunk.settings.model,
                    chunk.settings.language,
                    chunk.settings.token_eot,
                    std::ptr::eq(chunk.settings.options, &options),
                ));
                Ok(vec![segment(
                    chunk.offset_seconds,
                    chunk.offset_seconds + 1.0,
                    &format!(" from state {}", state.0),
                )])
            },
        )
        .unwrap();

        let mut calls = calls.into_inner().unwrap();
        calls.sort_by_key(|call| call.0);
        let (model, language, eot) = (WhisperModel::KbWhisperBase, Language::Swedish, 50_257);
        assert_eq!(
            calls,
            [
                (0, 55 * SECOND, 0.0, model, language, eot, true),
                (1, 60 * SECOND, 45.0, model, language, eot, true),
                (2, 55 * SECOND, 95.0, model, language, eot, true),
            ],
            "the warm state takes the first chunk, a built state each of the others, \
             and every chunk its decode window (with context) and its start offset"
        );
        assert_eq!(
            texts(&segments),
            [" from state 0", " from state 1", " from state 2"]
        );
    }

    #[test]
    fn results_follow_the_chunk_order_not_the_order_the_workers_finish_in() {
        let audio = audio_of(150);
        let chunks = three_chunks();
        let options = TranscribeOptions::default();
        let (finished_tx, finished_rx) = mpsc::channel::<()>();
        let finished_tx = Mutex::new(finished_tx);
        let finished_rx = Mutex::new(finished_rx);

        let segments = decode_in_parallel(
            job(&audio, &chunks, &options, silent(), None),
            &mut FakeState(0),
            counting_states(),
            &|state: &mut FakeState, chunk: ChunkJob<'_>, _progress| {
                if state.0 == 0 {
                    // The first chunk finishes last, and only if the others ran
                    // alongside it: a serial run would never see their signal.
                    for _ in 0..2 {
                        finished_rx
                            .lock()
                            .unwrap()
                            .recv_timeout(PATIENT)
                            .map_err(|_| failed("the other chunks never ran alongside"))?;
                    }
                } else {
                    finished_tx.lock().unwrap().send(()).unwrap();
                }
                Ok(vec![segment(
                    chunk.offset_seconds,
                    chunk.offset_seconds + 1.0,
                    &format!(" chunk {}", state.0),
                )])
            },
        )
        .unwrap();

        assert_eq!(texts(&segments), [" chunk 0", " chunk 1", " chunk 2"]);
    }

    #[test]
    fn states_are_built_first_then_encoding_starts_once_then_the_chunks_run() {
        let audio = audio_of(150);
        let chunks = three_chunks();
        let options = TranscribeOptions::default();
        let events = Mutex::new(Vec::new());
        let on_encode_start = || events.lock().unwrap().push("encode");
        let create_state = || {
            events.lock().unwrap().push("state");
            Ok(FakeState(1))
        };

        decode_in_parallel(
            job(&audio, &chunks, &options, silent(), Some(&on_encode_start)),
            &mut FakeState(0),
            create_state,
            &|_: &mut FakeState, _: ChunkJob<'_>, _progress| {
                events.lock().unwrap().push("decode");
                Ok(Vec::new())
            },
        )
        .unwrap();

        assert_eq!(
            *events.lock().unwrap(),
            ["state", "state", "encode", "decode", "decode", "decode"]
        );
    }

    #[test]
    fn a_state_that_cannot_be_built_fails_the_run_before_anything_starts() {
        let audio = audio_of(150);
        let chunks = three_chunks();
        let options = TranscribeOptions::default();
        let events = Mutex::new(Vec::new());
        let on_encode_start = || events.lock().unwrap().push("encode");
        let mut built = 0;
        let create_state = || {
            built += 1;
            if built == 2 {
                Err(failed("no GPU memory for a second state"))
            } else {
                Ok(FakeState(built))
            }
        };

        let result = decode_in_parallel(
            job(&audio, &chunks, &options, silent(), Some(&on_encode_start)),
            &mut FakeState(0),
            create_state,
            &|_: &mut FakeState, _: ChunkJob<'_>, _progress| {
                events.lock().unwrap().push("decode");
                Ok(Vec::new())
            },
        );

        assert_eq!(
            message(result.unwrap_err()),
            "no GPU memory for a second state"
        );
        assert!(
            events.lock().unwrap().is_empty(),
            "neither encoding nor any chunk may start"
        );
    }

    #[test]
    fn the_first_failing_chunk_in_chunk_order_decides_the_error() {
        let audio = audio_of(150);
        let chunks = three_chunks();
        let options = TranscribeOptions::default();

        let run = |failing: &'static [usize]| {
            decode_in_parallel(
                job(&audio, &chunks, &options, silent(), None),
                &mut FakeState(0),
                counting_states(),
                &|state: &mut FakeState, _: ChunkJob<'_>, _progress| {
                    if failing.contains(&state.0) {
                        Err(failed(&format!("chunk {} failed", state.0)))
                    } else {
                        Ok(Vec::new())
                    }
                },
            )
        };

        assert_eq!(message(run(&[1, 2]).unwrap_err()), "chunk 1 failed");
        assert_eq!(message(run(&[0, 2]).unwrap_err()), "chunk 0 failed");
        assert_eq!(message(run(&[2]).unwrap_err()), "chunk 2 failed");
    }

    #[test]
    fn a_cancelled_chunk_ends_the_run_as_cancelled() {
        let audio = audio_of(150);
        let chunks = three_chunks();
        let options = TranscribeOptions::default();

        let result = decode_in_parallel(
            job(&audio, &chunks, &options, silent(), None),
            &mut FakeState(0),
            counting_states(),
            &|state: &mut FakeState, _: ChunkJob<'_>, _progress| {
                if state.0 == 1 {
                    Err(DictationError::Cancelled)
                } else {
                    Ok(Vec::new())
                }
            },
        );

        assert!(
            matches!(result, Err(DictationError::Cancelled)),
            "got {result:?}"
        );
    }

    #[test]
    fn a_panicking_worker_becomes_an_error_whichever_chunk_it_was() {
        let audio = audio_of(150);
        let chunks = three_chunks();
        let options = TranscribeOptions::default();

        for panicking in [0, 1, 2] {
            let result = decode_in_parallel(
                job(&audio, &chunks, &options, silent(), None),
                &mut FakeState(0),
                counting_states(),
                &|state: &mut FakeState, _: ChunkJob<'_>, _progress| {
                    if state.0 == panicking {
                        panic!("injected panic in chunk {panicking}");
                    }
                    Ok(Vec::new())
                },
            );

            assert_eq!(
                message(result.unwrap_err()),
                "Parallel whisper worker panicked",
                "chunk {panicking}"
            );
        }
    }

    /// Reports every percentage handed to the run's `on_progress`.
    fn recording() -> (ChunkProgress, Arc<Mutex<Vec<i32>>>) {
        let reported = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&reported);
        (
            Box::new(move |percent| sink.lock().unwrap().push(percent)),
            reported,
        )
    }

    #[test]
    fn progress_is_the_weighted_sum_of_the_chunks_and_only_ever_moves_forward() {
        let audio = audio_of(120);
        let chunks = [chunk(0, 60, 0, 65), chunk(60, 120, 55, 120)];
        let options = TranscribeOptions::default();
        let (on_progress, reported) = recording();
        let (first_done_tx, first_done_rx) = mpsc::channel::<()>();
        let first_done_tx = Mutex::new(first_done_tx);
        let first_done_rx = Mutex::new(first_done_rx);

        decode_in_parallel(
            job(&audio, &chunks, &options, on_progress, None),
            &mut FakeState(0),
            counting_states(),
            &|state: &mut FakeState, _: ChunkJob<'_>, mut progress| {
                if state.0 == 0 {
                    progress(0); // whisper's initial 0: nothing to report yet
                    progress(60); // half of the recording is the first chunk: 30%
                    progress(60); // no news
                    progress(40); // a step back is not reported either
                    progress(150); // out of range counts as 100: 50%
                    first_done_tx.lock().unwrap().send(()).unwrap();
                } else {
                    first_done_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(PATIENT)
                        .map_err(|_| failed("the first chunk never reported"))?;
                    progress(50); // 50% of 50% on top of the first chunk's 50%: 75%
                    progress(-5); // below range counts as 0
                    progress(100);
                }
                Ok(Vec::new())
            },
        )
        .unwrap();

        assert_eq!(*reported.lock().unwrap(), [30, 50, 75, 100]);
    }

    #[test]
    fn progress_is_weighted_by_the_audio_a_chunk_owns_not_by_what_it_decodes() {
        let audio = audio_of(160);
        // 120 s and 40 s owned; the second decodes 5 s of context, the first 5 s.
        let chunks = [chunk(0, 120, 0, 125), chunk(120, 160, 115, 160)];
        let options = TranscribeOptions::default();
        let (on_progress, reported) = recording();

        decode_in_parallel(
            job(&audio, &chunks, &options, on_progress, None),
            &mut FakeState(0),
            counting_states(),
            &|state: &mut FakeState, _: ChunkJob<'_>, mut progress| {
                if state.0 == 0 {
                    progress(100);
                }
                Ok(Vec::new())
            },
        )
        .unwrap();

        assert_eq!(*reported.lock().unwrap(), [75]);
    }
}
