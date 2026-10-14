use super::*;
use sagascript_testsupport::TempDir;

#[cfg(feature = "diarization")]
#[test]
fn meeting_progress_event_is_indeterminate_and_elapsed() {
    let event = progress_event_with_decoder("analyzing", None, None, None, 42, None);
    assert_eq!(event["event"], "transcription_progress");
    assert_eq!(event["phase"], "analyzing");
    assert!(event["percent"].is_null());
    assert!(event["step"].is_null());
    assert!(event["steps_total"].is_null());
    assert_eq!(event["elapsed_ms"], 42);
}

#[cfg(feature = "diarization")]
#[test]
fn resolved_decoder_evidence_can_be_attached_to_progress() {
    for (beam_size, vad_model_path) in [(0, None), (4, None), (0, Some("synthetic-vad"))] {
        let options = TranscribeOptions {
            beam_size,
            vad_model_path: vad_model_path.map(str::to_owned),
            segment_timestamps: true,
            ..Default::default()
        };
        let evidence = effective_diarization_decoder_evidence(&options).unwrap();
        let expected = serde_json::to_value(&evidence).unwrap();
        let event = progress_event_with_decoder("analyzing", None, None, None, 42, Some(&expected));

        assert_eq!(event["decoder"], expected);
        assert_eq!(event["decoder"]["beam_size"], beam_size);
        assert_eq!(event["decoder"]["vad_enabled"], vad_model_path.is_some());
        assert!(!serde_json::to_string(&event)
            .unwrap()
            .contains("synthetic-vad"));
    }
}

#[cfg(feature = "diarization")]
#[test]
fn progress_helper_carries_decoder_for_each_phase_name() {
    let options = TranscribeOptions {
        beam_size: 4,
        vad_model_path: Some("synthetic-vad".into()),
        segment_timestamps: true,
        ..Default::default()
    };
    let evidence = effective_diarization_decoder_evidence(&options).unwrap();
    let expected = serde_json::to_value(evidence).unwrap();
    for phase in [
        "decoding",
        "resampling",
        "loading",
        "language_detection",
        "analyzing",
        "clustering",
        "finalizing",
        "completed",
    ] {
        let event = progress_event_with_decoder(phase, None, None, None, 42, Some(&expected));
        assert_eq!(event["decoder"], expected, "phase={phase}");
    }
}

#[cfg(feature = "diarization")]
#[test]
fn progress_decoder_matches_each_output_serializers_float_representation() {
    let evidence = effective_diarization_decoder_evidence(&TranscribeOptions {
        beam_size: 0,
        segment_timestamps: true,
        ..Default::default()
    })
    .unwrap();
    let native = decoder_progress_value(&evidence, true);
    let legacy = decoder_progress_value(&evidence, false);
    let native_report: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&evidence).unwrap()).unwrap();
    assert_eq!(native, native_report);
    assert_eq!(native["vad_samples_overlap"], serde_json::json!(0.1));
    assert_eq!(legacy, serde_json::to_value(&evidence).unwrap());
    assert_eq!(
        serde_json::from_value::<DecoderEvidence>(native).unwrap(),
        serde_json::from_value::<DecoderEvidence>(legacy).unwrap()
    );
}

#[test]
fn ordinary_progress_event_is_unchanged_without_decoder_snapshot() {
    let ordinary = progress_event("transcribing", Some(50), Some(3), Some(3), 42);
    let through_snapshot_helper =
        progress_event_with_decoder("transcribing", Some(50), Some(3), Some(3), 42, None);
    assert_eq!(through_snapshot_helper, ordinary);
    assert!(through_snapshot_helper.get("decoder").is_none());
}

#[cfg(feature = "diarization")]
#[test]
fn cache_path_cannot_alias_the_input_recording() {
    let root = TempDir::new();
    let input = root.join("recording.m4a");
    let other = root.join("cache.json");
    std::fs::write(&input, b"audio").unwrap();
    std::fs::write(&other, b"cache").unwrap();

    assert!(paths_refer_to_same_file(&input, &input));
    assert!(!paths_refer_to_same_file(&input, &other));
    #[cfg(unix)]
    {
        let hard_link = root.join("recording-cache.json");
        std::fs::hard_link(&input, &hard_link).unwrap();
        assert!(paths_refer_to_same_file(&input, &hard_link));
    }
}

#[test]
fn directory_expansion_filters_sorts_recurses_and_deduplicates() {
    let temp = TempDir::new();
    let root = temp.path().to_path_buf();
    let nested = root.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let a = root.join("a.WAV");
    let b = root.join("b.mp3");
    let ignored = root.join("notes.txt");
    let c = nested.join("c.m4a");
    for path in [&a, &b, &ignored, &c] {
        std::fs::write(path, b"fixture").unwrap();
    }

    assert_eq!(
        expand_inputs(std::slice::from_ref(&root), false).unwrap(),
        vec![a.clone(), b.clone()]
    );
    assert_eq!(
        expand_inputs(&[root.clone(), a.clone()], true).unwrap(),
        vec![a, b, c]
    );
}

#[test]
fn explicit_unsupported_or_missing_inputs_remain_item_level_work() {
    let inputs = vec![PathBuf::from("bad.txt"), PathBuf::from("missing.wav")];
    assert_eq!(expand_inputs(&inputs, false).unwrap(), inputs);
}

#[test]
fn batch_continues_after_item_failure_by_default() {
    let files = vec![
        PathBuf::from("one.wav"),
        PathBuf::from("bad.wav"),
        PathBuf::from("three.wav"),
    ];
    let mut visited = Vec::new();
    let mut executions = Vec::new();
    let counts = process_batch(
        &files,
        false,
        |_, file| {
            visited.push(file.to_path_buf());
            if file == Path::new("bad.wav") {
                Err(DictationError::FileDecodeError(
                    "corrupt fixture".to_string(),
                ))
            } else {
                Ok(FileTranscription {
                    json: serde_json::json!({"text": file.display().to_string()}),
                    plain: file.display().to_string(),
                    #[cfg(feature = "diarization")]
                    meeting: None,
                })
            }
        },
        |execution| {
            executions.push(execution);
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(visited, files);
    assert_eq!(counts, (3, 1));
    assert_eq!(executions.len(), 3);
    assert!(executions[0].output.is_ok());
    assert!(executions[1].output.is_err());
    assert!(executions[2].output.is_ok());
}

#[test]
fn batch_fail_fast_stops_after_first_failure() {
    let files = vec![PathBuf::from("bad.wav"), PathBuf::from("never.wav")];
    let mut executions = Vec::new();
    let counts = process_batch(
        &files,
        true,
        |_, file| {
            Err(DictationError::FileDecodeError(format!(
                "cannot decode {}",
                file.display()
            )))
        },
        |execution| {
            executions.push(execution);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(counts, (1, 1));
    assert_eq!(executions.len(), 1);
    assert_eq!(executions[0].source, PathBuf::from("bad.wav"));
}

#[test]
fn batch_cancellation_stops_a_fake_decoder_emits_nothing_and_resets_state() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let canceller = crate::cancel::Canceller::new();
    // Fake decoder: spins until its abort flag flips, like whisper.cpp's
    // abort callback polling between compute steps.
    let abort = Arc::new(AtomicBool::new(false));
    let files = vec![PathBuf::from("long.wav"), PathBuf::from("never.wav")];
    let mut visited = Vec::new();
    let mut executions = Vec::new();
    let started = Instant::now();
    let outcome = std::thread::scope(|scope| {
        let guard_flag = abort.clone();
        let _guard = canceller.register(move || guard_flag.store(true, Ordering::SeqCst));
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            canceller.request();
        });
        process_batch_with(
            &canceller,
            &files,
            false,
            |_, file| {
                visited.push(file.to_path_buf());
                while !abort.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(DictationError::TranscriptionFailed("aborted".into()))
            },
            |execution| {
                executions.push(execution);
                Ok(())
            },
        )
    });
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    let error = outcome.unwrap_err();
    assert_eq!(error.to_string(), "Transcription failed: Cancelled");
    assert_eq!(visited, vec![PathBuf::from("long.wav")], "batch must stop");
    assert!(
        executions.is_empty(),
        "no partial result is emitted on cancel"
    );
}

#[test]
fn supported_extensions_are_case_insensitive() {
    assert!(has_supported_extension(Path::new("AUDIO.FLAC")));
    assert!(has_supported_extension(Path::new("video.mov")));
    assert!(!has_supported_extension(Path::new("transcript.json")));
}

#[test]
fn explicit_language_keeps_region_redetection_fast_path_disabled() {
    let one_hour = 60 * 60 * 16_000;
    for language in [
        Language::English,
        Language::Swedish,
        Language::Norwegian,
        Language::Finnish,
    ] {
        assert!(language_window_plan(one_hour, language).is_empty());
        let result = detect_language_regions_with(
            &Out::capture().0,
            &vec![0.1; 60 * 16_000],
            language,
            |_| panic!("explicit language must not invoke region detection"),
        )
        .unwrap();
        assert!(result.is_none());
    }
}

#[test]
fn auto_language_region_detection_is_bounded_and_skips_silence() {
    let nine_hours = 9 * 60 * 60 * 16_000;
    assert!(
        language_window_plan(nine_hours, Language::Auto).len() <= LANGUAGE_REDETECTION_MAX_WINDOWS
    );

    let mut audio = vec![0.05; 100 * 16_000];
    audio[40 * 16_000..60 * 16_000].fill(0.0);
    let detections = ["en", "en", "sv", "sv"];
    let mut call_index = 0usize;
    let diagnostics =
        detect_language_regions_with(&Out::capture().0, &audio, Language::Auto, |_| {
            let language = detections[call_index];
            call_index += 1;
            Ok(Some(LanguageDetection {
                language: language.to_string(),
                probability: 0.97,
            }))
        })
        .unwrap()
        .expect("long auto-language audio enables region detection");

    assert_eq!(call_index, 4, "silent window must not be classified");
    assert_eq!(diagnostics.regions.len(), 2);
    assert_eq!(diagnostics.warnings[0].code, "mixed_language_audio");
}

fn segment(text: &str, avg_logprob: Option<f32>, no_speech_prob: f32) -> TranscriptSegment {
    TranscriptSegment {
        start: 0.0,
        end: 1.0,
        text: text.to_string(),
        avg_logprob,
        no_speech_prob,
    }
}

#[test]
fn cli_plain_assembly_matches_core_no_speech_filter() {
    let segments = vec![
        segment(" First.", Some(-0.1), 0.01),
        segment("<|nospeech|>-Hej Tack! Tack! Tack!", Some(-0.1), 0.9),
        segment(" Second.", Some(-0.1), 0.01),
    ];

    assert_eq!(assemble_transcript(&segments), " First. Second.");
    assert_eq!(
        segments[1].text, "<|nospeech|>-Hej Tack! Tack! Tack!",
        "display assembly must not mutate diagnostic segment text"
    );
}

#[cfg(feature = "diarization")]
#[test]
fn diarized_plain_assembly_discards_no_speech_segments() {
    let segments = vec![
        DiarizedSegment {
            start: 0.0,
            end: 1.0,
            speaker: "SPEAKER_0".to_string(),
            text: "First.".to_string(),
        },
        DiarizedSegment {
            start: 1.0,
            end: 1.3,
            speaker: "SPEAKER_1".to_string(),
            text: "<|nospeech|>-Hej Tack! Tack! Tack!".to_string(),
        },
        DiarizedSegment {
            start: 1.3,
            end: 2.0,
            speaker: "SPEAKER_1".to_string(),
            text: "Second.".to_string(),
        },
    ];

    assert_eq!(
        assemble_diarized_plain_text(&segments),
        "[SPEAKER_0] First.\n[SPEAKER_1] Second."
    );
    assert!(contains_no_speech_marker(&segments[1].text));
}

#[cfg(feature = "diarization")]
#[test]
fn diarized_plain_filter_precedes_consolidation_without_mutating_diagnostics() {
    let segments = vec![
        DiarizedSegment {
            start: 0.0,
            end: 1.0,
            speaker: "SPEAKER_0".to_string(),
            text: "First hello Music Music Music".to_string(),
        },
        DiarizedSegment {
            start: 1.0,
            end: 1.3,
            speaker: "SPEAKER_0".to_string(),
            text: "<|nospeech|>-Hej Tack! Tack! Tack!".to_string(),
        },
        DiarizedSegment {
            start: 1.3,
            end: 2.0,
            speaker: "SPEAKER_0".to_string(),
            text: "Second.".to_string(),
        },
    ];
    let consolidated = sagascript_core::diarization::merge::consolidate(&segments);
    let diagnostic_text = consolidated[0].text.clone();

    let glossary = Glossary::parse("Greeting = hello");
    let plain_segments = prepare_diarized_plain_segments(&segments, Language::English, &glossary);

    assert_eq!(consolidated.len(), 1);
    assert_eq!(consolidated[0].text, diagnostic_text);
    assert_eq!(
        diagnostic_text,
        "First hello Music Music Music <|nospeech|>-Hej Tack! Tack! Tack! Second."
    );
    assert_eq!(
        assemble_diarized_plain_text(&plain_segments),
        "[SPEAKER_0] First Greeting [MUSIC] Second."
    );
    assert_eq!(plain_segments.len(), 1);
    assert!(plain_segments
        .iter()
        .all(|segment| !contains_no_speech_marker(&segment.text)));
}

#[cfg(feature = "diarization")]
#[test]
fn cached_transcribe_uses_coarse_coverage_across_threshold_sweep() {
    use sagascript_core::diarization::embedding::EMBEDDING_DIM;
    use sagascript_core::diarization::DiarizationAnalysis;
    use sagascript_core::transcription::diagnostics::CoverageProfile;

    let root = TempDir::new();
    let input = root.join("synthetic.wav");
    let cache = root.join("synthetic-cache.json");
    std::fs::write(&input, b"synthetic source bytes").unwrap();

    let mut first_embedding = vec![0.0_f32; EMBEDDING_DIM];
    first_embedding[0] = 1.0;
    let mut second_embedding = vec![0.0_f32; EMBEDDING_DIM];
    second_embedding[1] = 1.0;
    let analysis: DiarizationAnalysis = serde_json::from_value(serde_json::json!({
        "raw_segments": [[0.0, 6.0, 0], [6.0, 11.0, 1]],
        "embeddings": [[0, first_embedding], [1, second_embedding]],
    }))
    .unwrap();
    let coverage_segments = vec![
        TranscriptSegment {
            start: 0.0,
            end: 2.0,
            text: "first".to_string(),
            avg_logprob: None,
            no_speech_prob: 0.0,
        },
        TranscriptSegment {
            start: 9.0,
            end: 11.02,
            text: "second".to_string(),
            avg_logprob: None,
            no_speech_prob: 0.0,
        },
    ];
    let coverage_profile = CoverageProfile::from_audio(&vec![0.05_f32; 11 * 16_000]);
    let coverage_segments =
        canonicalize_coverage_segments(coverage_segments, coverage_profile.duration_seconds());
    assert_eq!(coverage_segments[1].end, 11.0);
    let source_identity = crate::diarization_cache::CacheIdentity::for_input(
        &input,
        "en",
        model_id_string(WhisperModel::TinyEn),
        Some("alpha"),
    )
    .unwrap();
    let cache_value = crate::diarization_cache::DiarizationCache::new_with_coverage_segments(
        source_identity.clone(),
        analysis,
        vec![
            (1.0, 1.0, "first".to_string()),
            (9.0, 9.0, "second".to_string()),
        ],
        coverage_segments,
        coverage_profile,
        None,
        None,
    );
    crate::diarization_cache::save(&cache, &cache_value).unwrap();
    let loaded = crate::diarization_cache::load(&cache, &source_identity).unwrap();
    let crate::diarization_cache::CacheLookup::Hit(loaded) = loaded else {
        panic!("canonicalized coarse coverage should produce a cache hit");
    };
    assert_eq!(loaded.coverage_segments.as_ref().unwrap()[1].end, 11.0);

    let args_for_threshold = |threshold| TranscribeArgs {
        progress_json: false,
        files: vec![input.clone()],
        recursive: false,
        language: Some("en".to_string()),
        profile: None,
        model: Some("tiny.en".to_string()),
        json: true,
        jsonl: false,
        fail_fast: true,
        clipboard: false,
        diarize: true,
        diarize_exclusive_speech_embeddings: false,
        meeting_json: false,
        diarize_threshold: threshold,
        diarize_diagnostics: false,
        speaker_count: crate::speaker_args::SpeakerCountArgs::default(),
        diarize_cache: Some(cache.clone()),
        prompt: Some("alpha".to_string()),
        prompt_file: None,
        glossary_boost: None,
        boost_terms_file: None,
        correct_hints: false,
        vad: false,
        no_vad: false,
        beam_size: None,
        parallel: None,
    };
    let run = |threshold| {
        let args = args_for_threshold(threshold);
        let backend = WhisperBackend::new();
        let mut model_loaded = false;
        transcribe_file(
            &args,
            &input,
            &backend,
            &Settings::default(),
            Language::English,
            WhisperModel::TinyEn,
            &mut model_loaded,
            &Glossary::parse("alpha"),
            &[],
            None,
            CachePolicy::Normal,
            &Out::capture().0,
        )
        .unwrap()
        .json
    };

    let strict = run(0.34);
    let broad = run(1.1);
    assert_eq!(strict["speakers"].as_array().unwrap().len(), 2);
    assert_eq!(broad["speakers"].as_array().unwrap().len(), 1);
    for json in [&strict, &broad] {
        assert!((json["coverage_ratio"].as_f64().unwrap() - 4.0 / 11.0).abs() < 1e-6);
        assert_eq!(json["uncovered_spans"].as_array().unwrap().len(), 1);
        assert_eq!(json["uncovered_spans"][0]["start"], 2.0);
        assert_eq!(json["uncovered_spans"][0]["end"], 9.0);
        assert_eq!(json["warnings"], strict["warnings"]);
        assert_eq!(json["warnings"][0]["code"], "uncovered_speech");
    }
}

#[cfg(feature = "diarization")]
#[test]
fn meeting_document_uses_plain_segments_and_canonical_metadata() {
    let segments = vec![
        DiarizedSegment {
            start: 1.0,
            end: 2.0,
            speaker: "SPEAKER_1".to_string(),
            text: "second".to_string(),
        },
        DiarizedSegment {
            start: 0.0,
            end: 1.0,
            speaker: "SPEAKER_0".to_string(),
            text: "first".to_string(),
        },
        DiarizedSegment {
            start: 2.0,
            end: 3.0,
            speaker: "SPEAKER_1".to_string(),
            text: "third".to_string(),
        },
    ];
    let document = meeting_transcript_from_plain_segments(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
        Language::Swedish,
        WhisperModel::KbWhisperBase,
        3.0,
        &segments,
    )
    .expect("plain diarized segments are valid");

    assert_eq!(document.language, "sv");
    assert_eq!(document.model, "kb-whisper-base");
    assert_eq!(document.segments[0].id, "seg-000001");
    assert_eq!(document.segments[0].text, "first");
    assert_eq!(document.segments[1].text, "second");
    assert_eq!(document.speakers[0].id, "SPEAKER_1");
    assert_eq!(document.speakers[0].label, "SPEAKER_1");
}

#[cfg(feature = "diarization")]
#[test]
fn meeting_document_clamps_finite_padded_end_without_mutating_source() {
    let segments = vec![DiarizedSegment {
        start: 0.5,
        end: 1.2,
        speaker: "S0".into(),
        text: "last word".into(),
    }];
    let document = meeting_transcript_from_plain_segments(
        "0".repeat(64),
        Language::English,
        WhisperModel::BaseEn,
        1.0,
        &segments,
    )
    .expect("last padded Whisper window can overshoot decoded audio");
    assert_eq!(document.segments[0].start, 0.5);
    assert_eq!(document.segments[0].end, 1.0);
    assert_eq!(document.segments[0].text, "last word");
    assert_eq!(segments[0].end, 1.2);
}

#[cfg(feature = "diarization")]
#[test]
fn meeting_document_still_rejects_nonfinite_and_outside_starts() {
    for (start, end) in [
        (0.5, f64::NAN),
        (0.5, f64::INFINITY),
        (1.1, 1.2),
        (-0.1, 0.5),
    ] {
        let segments = vec![DiarizedSegment {
            start,
            end,
            speaker: "S0".into(),
            text: "invalid".into(),
        }];
        assert!(meeting_transcript_from_plain_segments(
            "0".repeat(64),
            Language::English,
            WhisperModel::BaseEn,
            1.0,
            &segments,
        )
        .is_err());
    }
}

#[cfg(feature = "diarization")]
#[test]
fn meeting_source_hash_is_bounded_and_detects_mutation() {
    let directory = TempDir::new();
    let path = directory.join("meeting-hash.wav");
    std::fs::write(&path, b"first bytes").expect("fixture write");
    let before = stable_file_sha256(&path, None).expect("hash fixture");
    assert_eq!(before.len(), 64);
    assert!(before.bytes().all(|byte| byte.is_ascii_hexdigit()));
    std::fs::write(&path, b"other bytes").expect("mutate fixture");
    assert!(verify_meeting_source_unchanged(&path, &before, None).is_err());
}

#[cfg(feature = "diarization")]
#[test]
fn meeting_control_precancel_checks_before_file_or_model_access() {
    use std::sync::atomic::AtomicBool;

    let cancellation = AtomicBool::new(true);
    let progress = |_| panic!("cancelled job must not emit progress");
    let control = MeetingControl {
        cancellation: &cancellation,
        progress: &progress,
    };
    let backend = WhisperBackend::new();
    let error = transcribe_meeting_file_with_control(
        Path::new("/definitely/missing/meeting.wav"),
        &Settings::default(),
        Language::English,
        WhisperModel::Base,
        &Glossary::parse(""),
        &backend,
        &control,
    )
    .expect_err("pre-cancelled job must fail");
    assert_eq!(
        error.to_string(),
        "Transcription failed: Meeting transcription cancelled"
    );
    assert!(cancellation.load(Ordering::Acquire));
}

#[cfg(feature = "diarization")]
#[test]
fn meeting_control_emits_ordered_phases_and_never_clears_cancellation() {
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    let cancellation = AtomicBool::new(false);
    let phases = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&phases);
    let progress = move |phase| observed.lock().expect("phase lock").push(phase);
    let control = MeetingControl {
        cancellation: &cancellation,
        progress: &progress,
    };
    let expected = [
        MeetingPhase::Preparing,
        MeetingPhase::Decoding,
        MeetingPhase::LoadingModel,
        MeetingPhase::Analyzing,
        MeetingPhase::Clustering,
        MeetingPhase::Finalizing,
    ];
    for phase in expected {
        meeting_checkpoint(Some(&control), phase).expect("checkpoint is live");
    }
    assert_eq!(*phases.lock().expect("phase lock"), expected);
    assert_eq!(
        serde_json::to_string(&MeetingPhase::LoadingModel).expect("phase serializes"),
        "\"loading_model\""
    );

    cancellation.store(true, Ordering::Release);
    assert!(meeting_checkpoint(Some(&control), MeetingPhase::Finalizing).is_err());
    assert!(cancellation.load(Ordering::Acquire));
}

#[cfg(feature = "diarization")]
#[test]
fn meeting_json_scope_requires_one_explicit_regular_file() {
    let temp = TempDir::new();
    let root = temp.path().to_path_buf();
    let file = root.join("input.wav");
    let second = root.join("second.wav");
    std::fs::write(&file, b"audio fixture").expect("fixture file");
    std::fs::write(&second, b"second fixture").expect("second fixture");

    assert!(validate_meeting_input_scope(
        std::slice::from_ref(&file),
        false,
        std::slice::from_ref(&file)
    )
    .is_ok());
    assert!(validate_meeting_input_scope(
        &[file.clone(), second.clone()],
        false,
        &[file.clone(), second]
    )
    .is_err());
    assert!(validate_meeting_input_scope(std::slice::from_ref(&root), false, &[]).is_err());
    assert!(validate_meeting_input_scope(
        std::slice::from_ref(&file),
        true,
        std::slice::from_ref(&file)
    )
    .is_err());
}

#[test]
fn explicit_aliases_correct_without_confidence_guessing() {
    let glossary = Glossary::parse("OpenRouter = open router\nmerge = merch");
    let mut segments = vec![segment(" Open router och Merch.", None, 0.0)];
    let corrections = apply_glossary_corrections(&mut segments, &glossary);

    assert_eq!(segments[0].text, " OpenRouter och merge.");
    assert_eq!(corrections.len(), 2);
    assert_eq!(corrections[0].method, "explicit_alias");
    assert_eq!(corrections[0].avg_logprob, None);
}

#[test]
fn explicit_phrase_aliases_cross_whisper_segment_boundaries() {
    let glossary = Glossary::parse("OpenRouter = open router");
    let mut segments = vec![
        segment("Jag använder open", Some(-0.2), 0.0),
        segment(" router varje dag.", Some(-0.4), 0.0),
    ];
    let corrections = apply_glossary_corrections(&mut segments, &glossary);

    assert_eq!(segments[0].text, "Jag använder OpenRouter");
    assert_eq!(segments[1].text, " varje dag.");
    assert_eq!(corrections.len(), 1);
    assert_eq!(corrections[0].segment_index, 0);
    assert_eq!(corrections[0].avg_logprob, Some(-0.2));
}

#[test]
fn correction_vocabulary_accepts_only_single_word_hint_items() {
    assert_eq!(
        Glossary::parse("Grimnir, grimnir, Erik Fredlund\nHugin, M5, AI-agent").single_word_terms(),
        vec!["Grimnir", "Hugin"]
    );
}

#[test]
fn corrects_repeated_unambiguous_one_edit_hint_matches() {
    let mut segments = vec![segment(" Grimner och Grimner.", Some(-0.313), 0.0)];
    let corrections = apply_hint_corrections(&mut segments, &["Grimnir".to_string()]);

    assert_eq!(segments[0].text, " Grimnir och Grimnir.");
    assert_eq!(corrections.len(), 2);
    assert_eq!(corrections[0].original, "Grimner");
    assert_eq!(corrections[0].replacement, "Grimnir");
    assert_eq!(corrections[0].segment_index, 0);
    assert_eq!(corrections[0].method, "fuzzy_one_edit");
    assert_eq!(corrections[0].avg_logprob, Some(-0.313));
}

#[test]
fn does_not_correct_confident_suspect_or_non_speech_segments() {
    let vocabulary = ["Grimnir".to_string()];
    for (avg_logprob, no_speech_prob) in [
        (Some(-0.2), 0.0),
        (Some(-0.9), 0.0),
        (None, 0.0),
        (Some(-0.313), 0.6),
    ] {
        let mut segments = vec![segment(" Grimner", avg_logprob, no_speech_prob)];
        assert!(apply_hint_corrections(&mut segments, &vocabulary).is_empty());
        assert_eq!(segments[0].text, " Grimner");
    }
}

#[test]
fn does_not_correct_ambiguous_or_more_distant_matches() {
    let mut segments = vec![segment(" Grimner Grimmer", Some(-0.313), 0.0)];
    let corrections = apply_hint_corrections(
        &mut segments,
        &["Grimnir".to_string(), "Grimnera".to_string()],
    );

    assert!(corrections.is_empty());
    assert_eq!(segments[0].text, " Grimner Grimmer");
}

#[test]
fn does_not_replace_a_word_that_is_already_an_explicit_hint() {
    let mut segments = vec![segment(" Grimner", Some(-0.313), 0.0)];
    let corrections = apply_hint_corrections(
        &mut segments,
        &["Grimner".to_string(), "Grimnir".to_string()],
    );

    assert!(corrections.is_empty());
    assert_eq!(segments[0].text, " Grimner");
}

// -- resolve_one_run_prompt --

#[test]
fn one_run_prompt_inline_is_trimmed() {
    let got = resolve_one_run_prompt(Some("  Notre Dame  "), None).unwrap();
    assert_eq!(got.as_deref(), Some("Notre Dame"));
}

#[test]
fn one_run_prompt_empty_input_is_deferred_to_saved_settings() {
    assert!(resolve_one_run_prompt(None, None).unwrap().is_none());
    assert!(resolve_one_run_prompt(Some("   "), None).unwrap().is_none());
}

#[test]
fn one_run_prompt_file_wins_and_is_trimmed() {
    let dir = TempDir::new();
    let path = dir.join("hint.txt");
    std::fs::write(&path, "  Estrid, Grimnir\n").unwrap();

    // File beats the inline flag; saved settings are composed by the core helper.
    let got = resolve_one_run_prompt(Some("inline"), Some(&path)).unwrap();
    assert_eq!(got.as_deref(), Some("Estrid, Grimnir"));
}

#[test]
fn one_run_prompt_empty_file_is_deferred_to_saved_settings() {
    let dir = TempDir::new();
    let path = dir.join("hint.txt");
    std::fs::write(&path, "   \n\t").unwrap();

    let got = resolve_one_run_prompt(None, Some(&path)).unwrap();
    assert!(got.is_none());
}

#[test]
fn one_run_prompt_missing_file_errors() {
    let path = Path::new("/nonexistent/sagascript/hint/vocab.txt");
    let err = resolve_one_run_prompt(None, Some(path)).unwrap_err();
    assert!(matches!(err, DictationError::FileDecodeError(_)));
}

#[test]
fn scoped_glossary_keeps_global_aliases_hint_only_and_profiles_isolated() {
    let mut settings = Settings {
        initial_prompt: "merge = merch\nOpenRouter".to_string(),
        hotkey_profiles: vec![
            HotkeyProfile {
                id: "swedish".to_string(),
                name: "Swedish".to_string(),
                shortcut: "F13".to_string(),
                language: Language::Swedish,
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
            },
            HotkeyProfile {
                id: "english".to_string(),
                name: "English".to_string(),
                shortcut: "F14".to_string(),
                language: Language::English,
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
            },
        ],
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("swedish".to_string(), "merge = merch".to_string());

    let global = effective_glossary(&settings, None, None, None).unwrap();
    assert_eq!(global.correct_text("merch").0, "merch");

    let swedish = effective_glossary(&settings, Some("swedish"), None, None).unwrap();
    assert_eq!(swedish.correct_text("merch").0, "merge");

    let english = effective_glossary(&settings, Some("english"), None, None).unwrap();
    assert_eq!(english.correct_text("merch").0, "merch");
}

#[test]
fn one_run_prompt_replaces_global_hint_but_keeps_profile_scope() {
    let mut settings = Settings {
        initial_prompt: "merge = merch".to_string(),
        hotkey_profiles: vec![HotkeyProfile {
            id: "swedish".to_string(),
            name: "Swedish".to_string(),
            shortcut: "F13".to_string(),
            language: Language::Swedish,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        }],
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("swedish".to_string(), "fika = coffee".to_string());

    let glossary = effective_glossary(
        &settings,
        Some("swedish"),
        Some("  OpenRouter = router  "),
        None,
    )
    .unwrap();

    assert_eq!(glossary.correct_text("merch").0, "merch");
    assert_eq!(glossary.correct_text("coffee").0, "fika");

    let empty_override =
        effective_glossary(&settings, Some("swedish"), Some("  \n"), None).unwrap();
    assert_eq!(empty_override.correct_text("merch").0, "merch");
    assert_eq!(empty_override.correct_text("coffee").0, "fika");
}

#[test]
fn auto_or_missing_profile_never_infers_alias_scope() {
    let mut settings = Settings {
        hotkey_profiles: vec![HotkeyProfile {
            id: "swedish".to_string(),
            name: "Swedish".to_string(),
            shortcut: "F13".to_string(),
            language: Language::Swedish,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        }],
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("swedish".to_string(), "merge = merch".to_string());

    let glossary = effective_glossary(&settings, None, None, None).unwrap();
    assert_eq!(glossary.correct_text("merch").0, "merch");
}

// -- parse_language --

#[test]
fn parse_language_valid_codes() {
    assert_eq!(parse_language("en").unwrap(), Language::English);
    assert_eq!(parse_language("sv").unwrap(), Language::Swedish);
    assert_eq!(parse_language("no").unwrap(), Language::Norwegian);
    assert_eq!(parse_language("fi").unwrap(), Language::Finnish);
    assert_eq!(parse_language("auto").unwrap(), Language::Auto);
}

#[test]
fn parse_language_long_names() {
    assert_eq!(parse_language("english").unwrap(), Language::English);
    assert_eq!(parse_language("swedish").unwrap(), Language::Swedish);
    assert_eq!(parse_language("norwegian").unwrap(), Language::Norwegian);
    assert_eq!(parse_language("finnish").unwrap(), Language::Finnish);
}

#[test]
fn parse_language_invalid() {
    assert!(parse_language("de").is_err());
    assert!(parse_language("").is_err());
    assert!(parse_language("ENGLISH").is_err()); // case-sensitive
}

#[test]
fn parse_language_error_message() {
    let err = parse_language("fr").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("fr"), "error should mention input: {msg}");
    assert!(msg.contains("en"), "error should list valid options: {msg}");
}

// -- parse_model --

#[test]
fn parse_model_all_valid_ids() {
    let cases = [
        ("tiny.en", WhisperModel::TinyEn),
        ("tiny", WhisperModel::Tiny),
        ("base.en", WhisperModel::BaseEn),
        ("base", WhisperModel::Base),
        ("fi-whisper-tiny", WhisperModel::FinnishWhisperTiny),
        ("kb-whisper-tiny", WhisperModel::KbWhisperTiny),
        ("kb-whisper-base", WhisperModel::KbWhisperBase),
        ("kb-whisper-small", WhisperModel::KbWhisperSmall),
        ("kb-whisper-medium", WhisperModel::KbWhisperMedium),
        ("kb-whisper-large", WhisperModel::KbWhisperLarge),
        ("nb-whisper-tiny", WhisperModel::NbWhisperTiny),
        ("nb-whisper-base", WhisperModel::NbWhisperBase),
        ("nb-whisper-small", WhisperModel::NbWhisperSmall),
        ("nb-whisper-medium", WhisperModel::NbWhisperMedium),
        ("nb-whisper-large", WhisperModel::NbWhisperLarge),
        ("small.en", WhisperModel::SmallEn),
        ("small", WhisperModel::Small),
        ("medium.en", WhisperModel::MediumEn),
        ("medium", WhisperModel::Medium),
        ("large-v3-turbo", WhisperModel::LargeV3Turbo),
        ("large-v3-turbo-q8_0", WhisperModel::LargeV3TurboQ8),
    ];
    for (id, expected) in cases {
        assert_eq!(parse_model(id).unwrap(), expected, "parse_model({id})");
    }
}

#[test]
fn parse_model_invalid() {
    assert!(parse_model("large-v3").is_err());
    assert!(parse_model("fi-whisper-medium").is_err());
    assert!(parse_model("").is_err());
    assert!(parse_model("BASE").is_err()); // case-sensitive
}

#[test]
fn parse_model_error_message() {
    let err = parse_model("nonexistent").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("nonexistent"));
    assert!(msg.contains("list-models"));
}

// -- model_id_string --

#[test]
fn model_id_string_all_variants() {
    let models = [
        (WhisperModel::TinyEn, "tiny.en"),
        (WhisperModel::Tiny, "tiny"),
        (WhisperModel::BaseEn, "base.en"),
        (WhisperModel::Base, "base"),
        (WhisperModel::FinnishWhisperTiny, "fi-whisper-tiny"),
        (WhisperModel::KbWhisperTiny, "kb-whisper-tiny"),
        (WhisperModel::KbWhisperBase, "kb-whisper-base"),
        (WhisperModel::KbWhisperSmall, "kb-whisper-small"),
        (WhisperModel::NbWhisperTiny, "nb-whisper-tiny"),
        (WhisperModel::NbWhisperBase, "nb-whisper-base"),
        (WhisperModel::NbWhisperSmall, "nb-whisper-small"),
        (WhisperModel::SmallEn, "small.en"),
        (WhisperModel::Small, "small"),
        (WhisperModel::MediumEn, "medium.en"),
        (WhisperModel::Medium, "medium"),
        (WhisperModel::LargeV3Turbo, "large-v3-turbo"),
        (WhisperModel::LargeV3TurboQ8, "large-v3-turbo-q8_0"),
    ];
    for (model, expected) in models {
        assert_eq!(model_id_string(model), expected);
    }
}

#[test]
fn model_id_string_roundtrip_with_parse() {
    let all_models = [
        WhisperModel::TinyEn,
        WhisperModel::Tiny,
        WhisperModel::BaseEn,
        WhisperModel::Base,
        WhisperModel::FinnishWhisperTiny,
        WhisperModel::KbWhisperTiny,
        WhisperModel::KbWhisperBase,
        WhisperModel::KbWhisperSmall,
        WhisperModel::NbWhisperTiny,
        WhisperModel::NbWhisperBase,
        WhisperModel::NbWhisperSmall,
        WhisperModel::SmallEn,
        WhisperModel::Small,
        WhisperModel::MediumEn,
        WhisperModel::Medium,
        WhisperModel::LargeV3Turbo,
        WhisperModel::LargeV3TurboQ8,
    ];
    for model in all_models {
        let id = model_id_string(model);
        let parsed = parse_model(id).unwrap();
        assert_eq!(parsed, model, "roundtrip failed for {id}");
    }
}

#[test]
fn explicit_model_ids_stay_accepted_and_whisper_only_auto_avoids_pianissimo() {
    // Auto resolution depends on what is downloaded on the host, so its
    // fallback order is covered hermetically in sagascript-core; here only
    // host-independent properties are asserted.
    let settings = Settings::default();
    assert!(matches!(
        resolve_file_model_for_run(None, Language::Swedish, &settings, true).unwrap(),
        FileModel::Whisper(_)
    ));
    assert!(resolve_file_model(Some("auto"), Language::Swedish, &settings).is_err());
    // Retired models remain usable through an explicit --model.
    for id in [
        "kb-whisper-tiny",
        "kb-whisper-base",
        "kb-whisper-small",
        "kb-whisper-medium",
        "kb-whisper-large",
    ] {
        assert!(matches!(
            resolve_file_model_for_run(Some(id), Language::Swedish, &settings, true).unwrap(),
            FileModel::Whisper(_)
        ));
    }
}

// -- resolve_effective_model --
//
// These exercise the exact branch used by both transcribe::run() and
// record::run(): explicit arg wins -> auto_select_model recommends by
// language -> otherwise the stored fallback model (language ignored).

#[test]
fn resolve_effective_model_none_auto_recommends_by_language() {
    let result =
        resolve_effective_model(None, Language::Swedish, true, WhisperModel::Base).unwrap();
    assert_eq!(result, WhisperModel::KbWhisperMedium);
}

#[test]
fn resolve_effective_model_none_no_auto_uses_fallback_ignoring_language() {
    let result =
        resolve_effective_model(None, Language::Swedish, false, WhisperModel::LargeV3Turbo)
            .unwrap();
    assert_eq!(result, WhisperModel::LargeV3Turbo);
}

#[test]
fn resolve_effective_model_explicit_arg_wins_over_auto_select() {
    let result =
        resolve_effective_model(Some("tiny.en"), Language::Swedish, true, WhisperModel::Base)
            .unwrap();
    assert_eq!(result, WhisperModel::TinyEn);
}

#[test]
fn resolve_effective_model_invalid_arg_errors() {
    assert!(
        resolve_effective_model(Some("bogus"), Language::Auto, true, WhisperModel::Base).is_err()
    );
}

#[test]
fn file_model_resolution_keeps_live_choice_independent() {
    let settings = Settings {
        auto_select_model: false,
        whisper_model: WhisperModel::KbWhisperLarge,
        file_transcription_model: FileModelPreference::PianissimoOriginal,
        ..Default::default()
    };
    assert_eq!(
        resolve_file_model(None, Language::Swedish, &settings).unwrap(),
        FileModel::PianissimoOriginal
    );
    assert_eq!(
        resolve_file_model(Some("kb-whisper-small"), Language::Swedish, &settings).unwrap(),
        FileModel::Whisper(WhisperModel::KbWhisperSmall)
    );
    assert_eq!(
        settings.effective_model_for(Language::Swedish),
        WhisperModel::KbWhisperLarge
    );
    assert!(resolve_file_model(None, Language::English, &settings).is_err());
}

#[test]
fn file_model_auto_uses_language_recommendation_independent_of_profile_choice() {
    let settings = Settings {
        auto_select_model: false,
        whisper_model: WhisperModel::KbWhisperLarge,
        ..Default::default()
    };
    assert_eq!(
        resolve_file_model(None, Language::English, &settings).unwrap(),
        FileModel::Whisper(WhisperModel::BaseEn)
    );
    assert!(resolve_file_model(Some("pianissimo-sv"), Language::English, &settings).is_err());
    assert!(resolve_file_model(Some("auto"), Language::Swedish, &settings).is_err());
}

#[test]
fn language_detection_never_downloads_an_encoder() {
    // Local mode makes no network requests after setup: detection may use
    // a Core ML encoder only if `download-model` already installed it.
    let production = include_str!("transcribe.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    let start = production.find("fn detect_file_language(").unwrap();
    let end = production[start..]
        .find("fn neutral_language_detection_model(")
        .unwrap();
    let body = &production[start..start + end];
    for forbidden in [
        "backfill",
        "download_model",
        "ensure_coreml_encoder",
        "block_on",
    ] {
        assert!(
            !body.contains(forbidden),
            "detection path must not call {forbidden}"
        );
    }
}

#[test]
fn neutral_language_detector_prefers_downloaded_base() {
    let selected = neutral_language_detection_model(|model| {
        matches!(model, WhisperModel::Base | WhisperModel::Tiny)
    });
    assert_eq!(selected, Some(WhisperModel::Base));
}

#[test]
fn neutral_language_detector_falls_back_and_can_be_unavailable() {
    let selected = neutral_language_detection_model(|model| model == WhisperModel::LargeV3Turbo);
    assert_eq!(selected, Some(WhisperModel::LargeV3Turbo));
    assert_eq!(neutral_language_detection_model(|_| false), None);
}

#[test]
fn uncertain_language_detector_escalates_to_downloaded_turbo() {
    let selected = accurate_language_detection_model(
        |model| {
            matches!(
                model,
                WhisperModel::LargeV3TurboQ8 | WhisperModel::LargeV3Turbo
            )
        },
        WhisperModel::Base,
    );
    assert_eq!(selected, Some(WhisperModel::LargeV3TurboQ8));
    assert_eq!(
        accurate_language_detection_model(
            |model| model == WhisperModel::LargeV3Turbo,
            WhisperModel::LargeV3Turbo,
        ),
        None
    );
}

#[cfg(test)]
mod parallel_chunks_tests {
    use super::parse_parallel_chunks;

    #[test]
    fn accepts_supported_parallel_chunk_counts() {
        assert_eq!(parse_parallel_chunks("1"), Ok(1));
        assert_eq!(parse_parallel_chunks("4"), Ok(4));
    }

    #[test]
    fn rejects_zero_and_unbounded_parallelism() {
        assert!(parse_parallel_chunks("0").is_err());
        assert!(parse_parallel_chunks("5").is_err());
        assert!(parse_parallel_chunks("many").is_err());
    }
}

#[cfg(all(test, feature = "diarization"))]
mod cache_policy_tests {
    use super::CachePolicy;

    #[test]
    fn refresh_new_never_routes_to_existing_cache_loader() {
        assert!(CachePolicy::Normal.reads_existing_cache());
        assert!(!CachePolicy::RefreshNew.reads_existing_cache());
    }
}

// -- diarize_threshold validation --
//
// Exercised via clap's try_parse_from against a small wrapper so the
// value_parser attribute itself (not just the bare parsing function) is
// under test.
#[cfg(all(test, feature = "diarization"))]
mod diarize_threshold_tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        args: TranscribeArgs,
    }

    fn parse_threshold(value: &str) -> Result<f32, String> {
        TestCli::try_parse_from(["sagascript", "file.wav", "--diarize-threshold", value])
            .map(|cli| cli.args.diarize_threshold)
            .map_err(|e| e.to_string())
    }

    #[test]
    fn rejects_nan() {
        assert!(parse_threshold("nan").is_err());
    }

    #[test]
    fn rejects_negative() {
        assert!(parse_threshold("-1.0").is_err());
    }

    #[test]
    fn rejects_above_range() {
        assert!(parse_threshold("3.0").is_err());
    }

    #[test]
    fn accepts_default_value() {
        assert_eq!(parse_threshold("0.75").unwrap(), 0.75);
    }

    #[test]
    fn accepts_in_range_value() {
        assert_eq!(parse_threshold("1.5").unwrap(), 1.5);
    }

    #[test]
    fn default_applies_when_flag_omitted() {
        let cli = TestCli::try_parse_from(["sagascript", "file.wav"]).unwrap();
        assert_eq!(cli.args.diarize_threshold, 0.34);
        #[cfg(feature = "diarization")]
        assert_eq!(
            cli.args.diarize_threshold,
            sagascript_core::diarization::DEFAULT_THRESHOLD
        );
    }

    #[test]
    fn accepts_boundary_values() {
        assert_eq!(parse_threshold("0.0").unwrap(), 0.0);
        assert_eq!(parse_threshold("2.0").unwrap(), 2.0);
    }

    #[test]
    fn diarized_decoder_flags_resolve_to_effective_options() {
        let cli =
            TestCli::try_parse_from(["sagascript", "f.wav", "--diarize", "--beam", "4", "--vad"])
                .unwrap();
        let options = resolve_diarization_options(&cli.args, &Settings::default()).unwrap();
        assert_eq!(options.beam_size, 4);
        assert!(options.vad_model_path.is_some());

        let stored = Settings {
            beam_size: 5,
            vad_enabled: true,
            temperature_fallback: false,
            ..Settings::default()
        };
        let defaulted = TestCli::try_parse_from(["sagascript", "f.wav", "--diarize"]).unwrap();
        let options = resolve_diarization_options(&defaulted.args, &stored).unwrap();
        assert_eq!(options.beam_size, 0);
        assert!(options.vad_model_path.is_none());
        assert!(options.temperature_fallback);

        let no_vad =
            TestCli::try_parse_from(["sagascript", "f.wav", "--diarize", "--no-vad"]).unwrap();
        let options = resolve_diarization_options(&no_vad.args, &stored).unwrap();
        assert_eq!(options.beam_size, 0);
        assert!(options.vad_model_path.is_none());

        let invalid_saved = Settings {
            beam_size: 99,
            ..stored
        };
        let options = resolve_diarization_options(&defaulted.args, &invalid_saved).unwrap();
        assert_eq!(options.beam_size, 0);
        let explicit =
            TestCli::try_parse_from(["sagascript", "f.wav", "--diarize", "--beam", "2", "--vad"])
                .unwrap();
        let options = resolve_diarization_options(&explicit.args, &invalid_saved).unwrap();
        assert_eq!(options.beam_size, 2);
        assert!(options.vad_model_path.is_some());
        assert!(options.temperature_fallback);
    }

    #[test]
    fn speaker_count_flags_parse_and_require_diarize() {
        let cli = TestCli::try_parse_from(["sagascript", "f.wav", "--diarize", "--speakers", "3"])
            .unwrap();
        assert_eq!(
            speaker_hint_from_args(&cli.args).unwrap(),
            Some(sagascript_core::speaker_hint::SpeakerCountHint::exact(3))
        );
        let none = TestCli::try_parse_from(["sagascript", "f.wav", "--diarize"]).unwrap();
        assert_eq!(speaker_hint_from_args(&none.args).unwrap(), None);
        // Without --diarize a hint is an error, not silently ignored.
        let no_diarize =
            TestCli::try_parse_from(["sagascript", "f.wav", "--min-speakers", "2"]).unwrap();
        assert!(speaker_hint_from_args(&no_diarize.args).is_err());
        // Invalid combinations are rejected at parse or validation time.
        assert!(
            TestCli::try_parse_from(["sagascript", "f.wav", "--diarize", "--speakers", "0"])
                .is_err()
        );
        assert!(TestCli::try_parse_from([
            "sagascript",
            "f.wav",
            "--diarize",
            "--speakers",
            "2",
            "--max-speakers",
            "3"
        ])
        .is_err());
        let inverted = TestCli::try_parse_from([
            "sagascript",
            "f.wav",
            "--diarize",
            "--min-speakers",
            "4",
            "--max-speakers",
            "2",
        ])
        .unwrap();
        assert!(speaker_hint_from_args(&inverted.args).is_err());
    }

    #[test]
    fn meeting_json_requires_diarize_and_conflicts_with_legacy_outputs() {
        assert!(TestCli::try_parse_from(["sagascript", "file.wav", "--meeting-json"]).is_err());
        let cli =
            TestCli::try_parse_from(["sagascript", "file.wav", "--diarize", "--meeting-json"])
                .expect("meeting JSON should parse with diarization");
        assert!(cli.args.meeting_json);
        for conflict in ["--json", "--jsonl", "--clipboard", "--correct-hints"] {
            assert!(
                TestCli::try_parse_from([
                    "sagascript",
                    "file.wav",
                    "--diarize",
                    "--meeting-json",
                    conflict,
                ])
                .is_err(),
                "meeting JSON should conflict with {conflict}"
            );
        }
    }

    #[test]
    fn pianissimo_json_keeps_the_pre_sidecar_keys_and_only_adds_engine_fields() {
        use sagascript_core::transcription::engine_host::WindowTiming;
        use sagascript_core::transcription::pianissimo_backend::PianissimoWord;
        let words = vec![PianissimoWord {
            word: "hej".into(),
            start: 0.1,
            end: 0.4,
        }];
        let corrections: Vec<String> = Vec::new();
        let build = |engine: &str| {
            pianissimo_file_json(
                "hej",
                &words,
                Language::Swedish,
                Path::new("a.wav"),
                1.5,
                &corrections,
                &PianissimoPerformance {
                    model_load_seconds: 1.25,
                    model_verification_seconds: 0.5,
                    decode_resample_seconds: 0.1,
                    total_seconds: 2.0,
                    engine: engine.to_string(),
                    engine_warm: false,
                    windows: vec![WindowTiming {
                        index: 0,
                        start_sample: 0,
                        end_sample: 24_000,
                        tokens: 3,
                        preprocess_ms: 4,
                        encode_ms: 500,
                        decode_ms: 70,
                        boost_us: 0,
                        round_trip_ms: 580,
                    }],
                },
            )
        };
        // The reported engine is the backend identity, not a constant.
        for engine in ["coreml", "onnx"] {
            assert_eq!(build(engine)["performance"]["engine"], engine);
        }
        let json = build("onnx");
        assert_eq!(json["performance"]["windows"][0]["encode_ms"], 500);
        assert_eq!(json["performance"]["windows"][0]["audio_seconds"], 1.5);

        // Frozen from the NeMo-era release: removing or renaming any of these breaks scripts.
        const FROZEN_TOP: [&str; 15] = [
            "text",
            "segments",
            "language",
            "model",
            "file",
            "duration_seconds",
            "coverage_ratio",
            "uncovered_spans",
            "repetition_spans",
            "detected_language",
            "language_redetection_enabled",
            "language_regions",
            "warnings",
            "vocabulary_corrections",
            "performance",
        ];
        const FROZEN_PERFORMANCE: [&str; 4] = [
            "model_load_seconds",
            "model_verification_seconds",
            "decode_resample_seconds",
            "total_seconds",
        ];
        const FROZEN_SEGMENT: [&str; 6] = [
            "start",
            "end",
            "text",
            "avg_logprob",
            "no_speech_prob",
            "quarantined",
        ];
        let keys = |value: &serde_json::Value| -> std::collections::BTreeSet<String> {
            value.as_object().unwrap().keys().cloned().collect()
        };
        let expect = |frozen: &[&str], extra: &[&str]| -> std::collections::BTreeSet<String> {
            frozen.iter().chain(extra).map(|k| k.to_string()).collect()
        };
        assert_eq!(keys(&json), expect(&FROZEN_TOP, &[]));
        assert_eq!(
            keys(&json["performance"]),
            expect(&FROZEN_PERFORMANCE, &["engine", "engine_warm", "windows"])
        );
        assert_eq!(keys(&json["segments"][0]), expect(&FROZEN_SEGMENT, &[]));
        assert_eq!(json["performance"]["engine"], "onnx");
        assert_eq!(json["performance"]["engine_warm"], false);
        assert_eq!(json["performance"]["model_load_seconds"], 1.25);
        assert_eq!(json["model"], "pianissimo-sv");
        assert_eq!(json["language"], "sv");
    }
}
