use super::*;

#[cfg(test)]
mod decode_flags_tests {
    use super::*;

    #[test]
    fn every_path_decodes_with_timestamps() {
        for segment_timestamps in [false, true] {
            let flags = DecodeFlags::for_path(DecodePath::Text { segment_timestamps });
            assert!(!flags.no_timestamps);
            assert_eq!(flags.token_timestamps, segment_timestamps);
        }
        #[cfg(feature = "diarization")]
        {
            let flags = DecodeFlags::for_path(DecodePath::WordTimestamps);
            assert!(!flags.no_timestamps);
            assert!(flags.token_timestamps);
        }
    }
}

#[cfg(test)]
mod language_detection_preview_tests {
    use super::language_detection_preview;

    #[test]
    fn keeps_short_audio_intact() {
        let audio = vec![0.1; 16_000];
        assert_eq!(language_detection_preview(&audio).len(), audio.len());
    }

    #[test]
    fn chooses_loudest_thirty_second_window() {
        let window = 30 * 16_000;
        let mut audio = vec![0.001; window * 3];
        audio[window..window * 2].fill(0.1);

        let preview = language_detection_preview(&audio);

        assert_eq!(preview.len(), window);
        assert!(preview.iter().all(|&sample| sample == 0.1));
    }
}

/// Moved from the former `segment_confidence_tests` (#371); the module name is kept so
/// the test ids stay stable.
#[cfg(test)]
mod segment_confidence_tests {
    use super::*;

    #[test]
    fn silent_audio_skips_decoder_without_a_loaded_model() {
        let backend = WhisperBackend::new();
        for language in [Language::Swedish, Language::English] {
            for samples in [4052, 6400, 8000] {
                let audio = vec![0.0; samples];
                let options = TranscribeOptions::default();
                assert!(backend
                    .transcribe_text(TranscribeRequest::new(&audio, language, &options).live())
                    .unwrap()
                    .is_empty());
                assert!(matches!(
                    backend.transcribe_segments(TranscribeRequest::new(&audio, language, &options)),
                    Err(DictationError::ModelNotLoaded)
                ));
            }
        }
    }

    #[test]
    fn live_dictation_guard_rejects_empty_and_non_finite_audio_without_model_access() {
        let backend = WhisperBackend::new();
        let options = TranscribeOptions::default();

        let mut empty_timings = DictationTimings {
            model_ms: 12.0,
            inference_ms: 34.0,
            model_cached: true,
            model_acquisition_started: true,
            inference_started: true,
            model_ready_at: Some(Instant::now()),
        };
        assert!(matches!(
            backend.transcribe_dictation(
                WhisperModel::Base,
                TranscribeRequest::new(&[], Language::English, &options).live(),
                &mut empty_timings
            ),
            Err(DictationError::NoAudioCaptured)
        ));
        assert_eq!(empty_timings.model_ms, 0.0);
        assert_eq!(empty_timings.inference_ms, 0.0);
        assert!(!empty_timings.model_cached);
        assert!(!empty_timings.model_acquisition_started);
        assert!(!empty_timings.inference_started);
        assert!(empty_timings.model_ready_at.is_none());

        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let audio = [0.0, invalid, 0.0];
            let mut timings = DictationTimings::default();
            assert!(matches!(
                backend.transcribe_dictation(WhisperModel::Base, TranscribeRequest::new(&audio, Language::English, &options).live(), &mut timings),
                Err(DictationError::TranscriptionFailed(message))
                    if message.contains("non-finite")
            ));
            assert!(!timings.model_acquisition_started);
            assert!(!timings.inference_started);
            assert!(timings.model_ready_at.is_none());
        }
    }

    #[test]
    fn live_silence_resets_timings_and_skips_model_access() {
        let backend = WhisperBackend::new();
        let mut timings = DictationTimings {
            model_ms: 12.0,
            inference_ms: 34.0,
            model_cached: true,
            model_acquisition_started: true,
            inference_started: true,
            model_ready_at: Some(Instant::now()),
        };

        let result = backend.transcribe_dictation(
            WhisperModel::Base,
            TranscribeRequest::new(
                &[0.0; 8_000],
                Language::English,
                &TranscribeOptions::default(),
            )
            .live(),
            &mut timings,
        );

        assert_eq!(result.unwrap(), "");
        assert_eq!(backend.loaded_model(), None);
        assert_eq!(timings.model_ms, 0.0);
        assert_eq!(timings.inference_ms, 0.0);
        assert!(!timings.model_cached);
        assert!(!timings.model_acquisition_started);
        assert!(!timings.inference_started);
        assert!(timings.model_ready_at.is_none());
    }

    #[test]
    fn raw_dictation_resets_timings_and_marks_only_model_acquisition_on_failure() {
        let backend = WhisperBackend::new();
        let mut timings = DictationTimings {
            model_ms: 12.0,
            inference_ms: 34.0,
            model_cached: true,
            model_acquisition_started: false,
            inference_started: true,
            model_ready_at: Some(Instant::now()),
        };

        let poisoning = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _load = backend.load_lock.lock().unwrap();
            panic!("poison load lock for deterministic timing test");
        }));
        assert!(poisoning.is_err());

        let result = backend.transcribe_dictation(
            WhisperModel::Base,
            TranscribeRequest::new(
                &[0.0; 8_000],
                Language::English,
                &TranscribeOptions::default(),
            ),
            &mut timings,
        );

        assert!(
            matches!(result, Err(DictationError::TranscriptionFailed(ref message))
            if message.contains("restart Sagascript"))
        );
        assert!(timings.model_acquisition_started);
        assert!(!timings.inference_started);
        assert!(timings.model_ready_at.is_none());
        assert!(timings.model_ms.is_finite());
        assert!(timings.model_ms >= 0.0);
        assert_eq!(timings.inference_ms, 0.0);
    }

    #[test]
    fn warmup_still_requires_a_model_and_does_not_skip_silent_inference() {
        assert!(matches!(
            WhisperBackend::new().warmup(Language::English),
            Err(DictationError::ModelNotLoaded)
        ));
    }

    #[test]
    fn mean_logprob_empty_is_none() {
        assert_eq!(mean_logprob(&[]), None);
    }

    #[test]
    fn mean_logprob_averages() {
        let got = mean_logprob(&[-0.2, -0.4, -0.6]).unwrap();
        assert!((got - (-0.4)).abs() < 1e-6, "got {got}");
    }

    #[test]
    fn mean_logprob_single_value() {
        assert_eq!(mean_logprob(&[-1.5]), Some(-1.5));
    }
}

/// The request-based entry points (#371), driven as far as they go without a
/// model: the checks that come before the decoder, in the order they happen,
/// and what the hooks see when a request ends there.
#[cfg(test)]
mod entry_point_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    static SILENCE: [f32; 8_000] = [0.0; 8_000];

    fn tone() -> Vec<f32> {
        vec![0.3; 8_000]
    }

    /// Records everything the hooks of a request are told.
    #[derive(Default)]
    struct Hooks {
        progress: Arc<Mutex<Vec<i32>>>,
        encode_starts: AtomicU32,
    }

    impl Hooks {
        fn request<'a>(
            &'a self,
            audio: &'a [f32],
            options: &'a TranscribeOptions,
            encode_start: &'a dyn Fn(),
        ) -> TranscribeRequest<'a> {
            let progress = Arc::clone(&self.progress);
            TranscribeRequest::new(audio, Language::English, options)
                .on_progress(move |percent| progress.lock().unwrap().push(percent))
                .on_encode_start(encode_start)
        }

        fn told_nothing(&self) -> bool {
            self.progress.lock().unwrap().is_empty()
                && self.encode_starts.load(Ordering::SeqCst) == 0
        }
    }

    #[test]
    fn segments_refuse_empty_audio_before_looking_for_a_model() {
        let backend = WhisperBackend::new();
        let options = TranscribeOptions::default();
        let hooks = Hooks::default();
        let encode_start = || {
            hooks.encode_starts.fetch_add(1, Ordering::SeqCst);
        };

        let result = backend.transcribe_segments(hooks.request(&[], &options, &encode_start));

        assert!(matches!(result, Err(DictationError::NoAudioCaptured)));
        assert!(
            hooks.told_nothing(),
            "no progress for work that never began"
        );
    }

    #[test]
    fn segments_without_a_loaded_model_fail_before_any_hook_fires() {
        let backend = WhisperBackend::new();
        let options = TranscribeOptions::default();
        let hooks = Hooks::default();
        let encode_start = || {
            hooks.encode_starts.fetch_add(1, Ordering::SeqCst);
        };

        let result = backend.transcribe_segments(hooks.request(&SILENCE, &options, &encode_start));

        assert!(matches!(result, Err(DictationError::ModelNotLoaded)));
        assert!(hooks.told_nothing(), "no '1%' for a run that cannot start");
    }

    #[test]
    fn live_segments_skip_near_silence_and_still_refuse_bad_audio() {
        let backend = WhisperBackend::new();
        let options = TranscribeOptions::default();

        let skipped = backend
            .transcribe_segments(
                TranscribeRequest::new(&SILENCE, Language::English, &options).live(),
            )
            .unwrap();
        assert!(
            skipped.is_empty(),
            "nothing to transcribe, and no model needed"
        );

        assert!(matches!(
            backend.transcribe_segments(
                TranscribeRequest::new(&[], Language::English, &options).live()
            ),
            Err(DictationError::NoAudioCaptured)
        ));
        let audible = tone();
        assert!(
            matches!(
                backend.transcribe_segments(
                    TranscribeRequest::new(&audible, Language::English, &options).live()
                ),
                Err(DictationError::ModelNotLoaded)
            ),
            "speech goes on to the decoder, which needs a model"
        );
    }

    #[test]
    fn text_follows_the_same_checks_as_segments() {
        let backend = WhisperBackend::new();
        let options = TranscribeOptions::default();
        let audible = tone();

        assert!(matches!(
            backend.transcribe_text(TranscribeRequest::new(&[], Language::English, &options)),
            Err(DictationError::NoAudioCaptured)
        ));
        assert!(matches!(
            backend.transcribe_text(TranscribeRequest::new(
                &audible,
                Language::English,
                &options
            )),
            Err(DictationError::ModelNotLoaded)
        ));
        assert_eq!(
            backend
                .transcribe_text(
                    TranscribeRequest::new(&SILENCE, Language::English, &options).live()
                )
                .unwrap(),
            "",
            "live near-silence is an empty transcript"
        );
    }

    #[test]
    fn gap_recovery_reports_the_first_pass_failure_unchanged() {
        let backend = WhisperBackend::new();
        let options = TranscribeOptions::default();
        let audible = tone();

        assert!(matches!(
            backend.transcribe_with_gap_recovery(TranscribeRequest::new(
                &[],
                Language::English,
                &options
            )),
            Err(DictationError::NoAudioCaptured)
        ));
        assert!(matches!(
            backend.transcribe_with_gap_recovery(TranscribeRequest::new(
                &audible,
                Language::English,
                &options
            )),
            Err(DictationError::ModelNotLoaded)
        ));
    }

    /// The guard is spent once, at the top: near-silence is "nothing to
    /// transcribe", not a first pass that came back empty and now needs its
    /// gaps recovered (which would also need a model, to read its thresholds).
    #[test]
    fn live_gap_recovery_skips_near_silence_without_a_model_or_a_second_pass() {
        let backend = WhisperBackend::new();
        let options = TranscribeOptions::default();

        let skipped = backend
            .transcribe_with_gap_recovery(
                TranscribeRequest::new(&SILENCE, Language::English, &options).live(),
            )
            .expect("near-silence is skipped, not an error");
        assert!(skipped.is_empty());

        assert!(matches!(
            backend.transcribe_with_gap_recovery(
                TranscribeRequest::new(&[], Language::English, &options).live()
            ),
            Err(DictationError::NoAudioCaptured)
        ));
        let audible = tone();
        assert!(
            matches!(
                backend.transcribe_with_gap_recovery(
                    TranscribeRequest::new(&audible, Language::English, &options).live()
                ),
                Err(DictationError::ModelNotLoaded)
            ),
            "speech goes on to the decoder, which needs a model"
        );
    }

    #[test]
    fn a_dictation_request_goes_through_the_load_lock() {
        let backend = Arc::new(WhisperBackend::new());
        let holder = Arc::clone(&backend);
        let poisoned = std::thread::spawn(move || {
            let _load = holder.load_lock.lock().unwrap();
            panic!("poison the load lock");
        })
        .join();
        assert!(poisoned.is_err());
        let options = TranscribeOptions::default();
        let mut timings = DictationTimings::default();

        // Both the dictation and the warm-ups stop at the fail-closed lock.
        let dictation = backend.transcribe_dictation(
            WhisperModel::Base,
            TranscribeRequest::new(&SILENCE, Language::English, &options),
            &mut timings,
        );
        let warmup = backend.warmup_model(WhisperModel::Base, Language::English);

        for result in [dictation.map(|_| ()), warmup] {
            assert!(
                matches!(&result, Err(DictationError::TranscriptionFailed(message))
                    if message.contains("restart Sagascript")),
                "expected the fail-closed restart error, got {result:?}"
            );
        }
    }
}

/// The wiring of plain-file gap recovery (#273), with a fake decoder behind
/// `recover_gaps`: which slices are decoded again, as what kind of request, and
/// what a cancel does. The coverage analysis and the merge are tested in
/// `gap_redecode`.
#[cfg(test)]
mod gap_recovery_tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn seg(start: f64, end: f64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            start,
            end,
            text: text.to_string(),
            avg_logprob: Some(-0.2),
            no_speech_prob: 0.0,
        }
    }

    /// 16 kHz audio: a loud tone where `speech` ranges say, silence elsewhere.
    fn audio(total_seconds: usize, speech: &[(usize, usize)]) -> Vec<f32> {
        let mut samples = vec![0.0f32; total_seconds * 16_000];
        for &(from, to) in speech {
            for (i, sample) in samples[from * 16_000..to * 16_000].iter_mut().enumerate() {
                *sample = 0.3 * (i as f32 * 0.05).sin();
            }
        }
        samples
    }

    fn texts(segments: &[TranscriptSegment]) -> Vec<&str> {
        segments.iter().map(|s| s.text.as_str()).collect()
    }

    /// What the fake decoder was asked for.
    #[derive(Debug, PartialEq)]
    struct Asked {
        samples: usize,
        language: Language,
        beam_size: u32,
        prompt: Option<String>,
        parallel_chunks: usize,
        hooks: bool,
        live: bool,
    }

    fn asked(request: &TranscribeRequest<'_>) -> Asked {
        Asked {
            samples: request.audio.len(),
            language: request.language,
            beam_size: request.options.beam_size,
            prompt: request.options.prompt.clone(),
            parallel_chunks: request.options.parallel_chunks,
            hooks: request.hooks.on_progress.is_some() || request.hooks.on_encode_start.is_some(),
            live: request.live,
        }
    }

    fn options() -> TranscribeOptions {
        TranscribeOptions {
            beam_size: 5,
            prompt: Some("Sagascript".to_string()),
            parallel_chunks: 4,
            ..TranscribeOptions::default()
        }
    }

    #[test]
    fn a_first_pass_without_gaps_is_returned_as_it_is() {
        let samples = audio(30, &[(0, 30)]);
        let first_pass = vec![seg(0.0, 30.0, "all of it")];
        let decoded = Cell::new(0);

        let result = recover_gaps(
            &samples,
            Language::English,
            &options(),
            WhisperModel::Base,
            first_pass,
            &|| false,
            |_| {
                decoded.set(decoded.get() + 1);
                Ok(Vec::new())
            },
        )
        .unwrap();

        assert_eq!(texts(&result), ["all of it"]);
        assert_eq!(decoded.get(), 0);
    }

    #[test]
    fn a_gap_is_decoded_again_as_a_plain_serial_request_and_merged_in_order() {
        let samples = audio(60, &[(0, 10), (20, 40), (50, 60)]);
        let first_pass = vec![seg(0.0, 10.0, "first"), seg(50.0, 60.0, "last")];
        let requests = RefCell::new(Vec::new());
        let caller_options = options();

        let result = recover_gaps(
            &samples,
            Language::Swedish,
            &caller_options,
            WhisperModel::Base,
            first_pass,
            &|| false,
            |request| {
                requests.borrow_mut().push(asked(&request));
                Ok(vec![seg(0.0, 40.0, "the middle")])
            },
        )
        .unwrap();

        assert_eq!(texts(&result), ["first", "the middle", "last"]);
        assert_eq!(
            *requests.borrow(),
            [Asked {
                samples: 40 * 16_000,
                language: Language::Swedish,
                beam_size: 5,
                prompt: Some("Sagascript".to_string()),
                parallel_chunks: 1,
                hooks: false,
                live: false,
            }],
            "one decode of the 10s-50s gap: same decoding, but serial and unobserved"
        );
        assert_eq!(
            caller_options.parallel_chunks, 4,
            "the caller's options are not changed"
        );
    }

    #[test]
    fn a_cancel_during_the_redecode_discards_the_result() {
        let samples = audio(60, &[(0, 10), (20, 40), (50, 60)]);
        let first_pass = vec![seg(0.0, 10.0, "first"), seg(50.0, 60.0, "last")];
        let cancelled = Cell::new(false);

        let result = recover_gaps(
            &samples,
            Language::English,
            &options(),
            WhisperModel::Base,
            first_pass,
            &|| cancelled.get(),
            |_| {
                cancelled.set(true);
                Ok(vec![seg(0.0, 40.0, "the middle")])
            },
        );

        assert!(
            matches!(result, Err(DictationError::Cancelled)),
            "got {result:?}"
        );
    }

    #[test]
    fn a_cancel_before_the_redecode_decodes_nothing() {
        let samples = audio(60, &[(0, 10), (20, 40), (50, 60)]);
        let first_pass = vec![seg(0.0, 10.0, "first"), seg(50.0, 60.0, "last")];
        let decoded = Cell::new(0);

        let result = recover_gaps(
            &samples,
            Language::English,
            &options(),
            WhisperModel::Base,
            first_pass,
            &|| true,
            |_| {
                decoded.set(decoded.get() + 1);
                Ok(Vec::new())
            },
        );

        assert!(
            matches!(result, Err(DictationError::Cancelled)),
            "got {result:?}"
        );
        assert_eq!(decoded.get(), 0);
    }

    #[test]
    fn a_failed_redecode_keeps_the_first_pass() {
        let samples = audio(60, &[(0, 10), (20, 40), (50, 60)]);
        let first_pass = vec![seg(0.0, 10.0, "first"), seg(50.0, 60.0, "last")];

        let result = recover_gaps(
            &samples,
            Language::English,
            &options(),
            WhisperModel::Base,
            first_pass,
            &|| false,
            |_| Err(DictationError::TranscriptionFailed("decoder failed".into())),
        )
        .unwrap();

        assert_eq!(texts(&result), ["first", "last"]);
    }
}

/// The bookkeeping of the dictation transaction (#371): the order of the
/// steps, what is recorded for each, and when the load lock is held.
#[cfg(test)]
mod dictation_transaction_tests {
    use super::*;
    use std::sync::TryLockError;

    fn load_lock_is_held(backend: &WhisperBackend) -> bool {
        matches!(backend.load_lock.try_lock(), Err(TryLockError::WouldBlock))
    }

    #[test]
    fn selection_and_inference_both_run_under_the_load_lock_and_are_timed() {
        let backend = WhisperBackend::new();
        let mut timings = DictationTimings::default();
        let order = std::cell::RefCell::new(Vec::new());

        let text = backend
            .dictation_transaction(
                &mut timings,
                |timings| {
                    assert!(load_lock_is_held(&backend));
                    assert!(timings.model_acquisition_started);
                    assert!(!timings.inference_started);
                    order.borrow_mut().push("select");
                    Ok(())
                },
                || {
                    assert!(load_lock_is_held(&backend));
                    order.borrow_mut().push("infer");
                    Ok("hello".to_string())
                },
            )
            .unwrap();

        assert_eq!(text, "hello");
        assert_eq!(*order.borrow(), ["select", "infer"]);
        assert!(!load_lock_is_held(&backend), "released afterwards");
        assert!(timings.model_acquisition_started && timings.inference_started);
        assert!(timings.model_ready_at.is_some());
        assert!(timings.model_ms.is_finite() && timings.model_ms >= 0.0);
        assert!(timings.inference_ms.is_finite() && timings.inference_ms >= 0.0);
    }

    #[test]
    fn a_failed_selection_stops_before_inference_but_keeps_its_timing() {
        let backend = WhisperBackend::new();
        let mut timings = DictationTimings::default();
        let mut inferred = false;

        let result = backend.dictation_transaction(
            &mut timings,
            |_| Err(DictationError::TranscriptionFailed("not downloaded".into())),
            || {
                inferred = true;
                Ok(String::new())
            },
        );

        assert!(matches!(
            result,
            Err(DictationError::TranscriptionFailed(_))
        ));
        assert!(!inferred);
        assert!(timings.model_acquisition_started);
        assert!(!timings.inference_started);
        assert!(timings.model_ready_at.is_none());
        assert!(timings.model_ms.is_finite() && timings.model_ms >= 0.0);
        assert_eq!(timings.inference_ms, 0.0);
        assert!(!load_lock_is_held(&backend), "released after the failure");
    }

    #[test]
    fn stale_timings_from_an_earlier_run_are_cleared() {
        let backend = WhisperBackend::new();
        let mut timings = DictationTimings {
            model_ms: 99.0,
            inference_ms: 99.0,
            model_cached: true,
            model_acquisition_started: false,
            inference_started: true,
            model_ready_at: Some(Instant::now()),
        };

        let result = backend.dictation_transaction(
            &mut timings,
            |_| Err(DictationError::ModelNotLoaded),
            || Ok(String::new()),
        );

        assert!(matches!(result, Err(DictationError::ModelNotLoaded)));
        assert!(!timings.model_cached);
        assert!(!timings.inference_started);
        assert!(timings.model_ready_at.is_none());
        assert_eq!(timings.inference_ms, 0.0);
        assert_ne!(timings.model_ms, 99.0);
    }
}
