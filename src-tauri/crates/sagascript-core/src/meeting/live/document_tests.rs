use super::*;

fn segment(id: &str, source: Source, start_ms: u64, end_ms: u64, text: &str) -> LiveSegment {
    LiveSegment {
        id: id.to_string(),
        source,
        start_ms,
        end_ms,
        text: text.to_string(),
        status: SegmentStatus::Final,
        revision: 1,
    }
}

fn gap(source: Source, start_ms: u64, end_ms: u64, reason: &str) -> Gap {
    Gap {
        source,
        start_ms,
        end_ms,
        reason: reason.to_string(),
    }
}

fn document() -> LiveDocument {
    LiveDocument::new(
        "session-1",
        "sv-SE",
        "whisper-local",
        12_000,
        vec![
            segment("segment-10", Source::System, 2_000, 4_000, "system text"),
            segment("segment-2", Source::Microphone, 0, 3_000, "hello"),
        ],
        vec![gap(Source::System, 4_000, 5_000, "device paused")],
        false,
        Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string()),
    )
    .expect("fixture is valid")
}

#[test]
fn rejects_preview_segments() {
    let mut document = document();
    document.segments[0].status = SegmentStatus::Preview;

    assert_eq!(document.validate(), Err(LiveDocumentError::PreviewSegment));
    assert!(document.export(MeetingExportFormat::Plain).is_err());
}

#[test]
fn rejects_duplicate_ids_invalid_ranges_oversize_and_bad_hashes() {
    let mut duplicate = document();
    duplicate.segments[1].id = duplicate.segments[0].id.clone();
    assert_eq!(duplicate.validate(), Err(LiveDocumentError::DuplicateId));

    let mut range = document();
    range.segments[0].end_ms = range.duration_ms + 1;
    assert_eq!(range.validate(), Err(LiveDocumentError::InvalidRange));

    let mut oversize = document();
    oversize.segments[0].text = "x".repeat(MAX_TEXT_BYTES_PER_SEGMENT + 1);
    assert_eq!(oversize.validate(), Err(LiveDocumentError::TextTooLarge));

    let mut hash = document();
    hash.audio_sha256 = Some("not-a-sha256".to_string());
    assert_eq!(hash.validate(), Err(LiveDocumentError::InvalidAudioHash));
    assert_eq!(
        LiveDocumentError::InvalidAudioHash.to_string(),
        "live_document.invalid_audio_hash"
    );
    assert!(!LiveDocumentError::InvalidAudioHash
        .to_string()
        .contains("not-a-sha256"));
}

#[test]
fn json_roundtrip_and_invalid_json_validation() {
    let document = document();
    let json = serde_json::to_string(&document).expect("serialize");
    let roundtrip: LiveDocument = serde_json::from_str(&json).expect("roundtrip");
    assert_eq!(roundtrip, document);

    let invalid = json.replace("\"final\"", "\"preview\"");
    assert!(serde_json::from_str::<LiveDocument>(&invalid).is_err());
    assert!(LiveDocument::from_json(&invalid).is_err());
}

#[test]
fn all_exports_have_source_labels_and_preserve_overlap() {
    let mut document = document();
    document.segments.push(segment(
        "segment-11",
        Source::Microphone,
        1_000,
        3_500,
        "overlap",
    ));
    document
        .segments
        .sort_by_key(|segment| (segment.start_ms, segment.end_ms));

    let plain = document.export(MeetingExportFormat::Plain).expect("plain");
    let markdown = document
        .export(MeetingExportFormat::Markdown)
        .expect("markdown");
    let json = document.export(MeetingExportFormat::Json).expect("json");
    let srt = document.export(MeetingExportFormat::Srt).expect("srt");
    let vtt = document.export(MeetingExportFormat::Vtt).expect("vtt");

    for output in [&plain, &markdown] {
        assert!(output.contains("INCOMPLETE"));
        assert!(output.contains("GAP"));
    }
    for output in [&plain, &markdown, &srt, &vtt] {
        assert!(output.contains("Jag · mikrofon"));
        assert!(output.contains("Mötet · systemljud"));
    }
    assert!(srt.contains("00:00:00,000 --> 00:00:03,000"));
    assert!(srt.contains("00:00:01,000 --> 00:00:03,500"));
    assert!(vtt.contains("00:00:00.000 --> 00:00:03.000"));
    assert!(vtt.contains("00:00:01.000 --> 00:00:03.500"));
    assert!(json.contains("\"complete\":false"));
    assert!(json.contains("\"gaps\":["));
}

#[test]
fn subtitle_exports_include_gap_cues_and_incomplete_status() {
    let document = LiveDocument::new(
        "session-subtitles",
        "sv-SE",
        "whisper-local",
        6_000,
        vec![segment(
            "speech-1",
            Source::Microphone,
            500,
            2_500,
            "spoken",
        )],
        vec![gap(Source::System, 2_000, 3_000, "device\u{0007} paused")],
        false,
        None,
    )
    .expect("fixture is valid");

    let srt = document.export(MeetingExportFormat::Srt).expect("srt");
    let vtt = document.export(MeetingExportFormat::Vtt).expect("vtt");

    assert!(srt.starts_with("1\n00:00:00,000 --> 00:00:01,000\n[INCOMPLETE TRANSCRIPT]"));
    assert!(vtt.starts_with("WEBVTT\n\n1\n00:00:00.000 --> 00:00:01.000\n[INCOMPLETE TRANSCRIPT]"));
    for output in [&srt, &vtt] {
        assert!(output.contains("[Jag · mikrofon] spoken"));
        assert!(output.contains("[Mötet · systemljud] [AUDIO GAP] device paused"));
        assert!(!output.contains('\u{0007}'));
        assert!(
            output.contains("00:00:02,000 --> 00:00:03,000")
                || output.contains("00:00:02.000 --> 00:00:03.000")
        );
    }
}

#[test]
fn zero_duration_incomplete_subtitle_is_explicit_and_nonempty() {
    let document = LiveDocument::new(
        "session-empty",
        "sv-SE",
        "whisper-local",
        0,
        Vec::new(),
        Vec::new(),
        false,
        None,
    )
    .expect("fixture is valid");

    let srt = document.export(MeetingExportFormat::Srt).expect("srt");
    let vtt = document.export(MeetingExportFormat::Vtt).expect("vtt");
    assert_eq!(
        srt,
        "1\n00:00:00,000 --> 00:00:00,001\n[INCOMPLETE TRANSCRIPT]\n\n"
    );
    assert_eq!(
        vtt,
        "WEBVTT\n\n1\n00:00:00.000 --> 00:00:00.001\n[INCOMPLETE TRANSCRIPT]\n\n"
    );
}

#[test]
fn new_and_exports_are_numerically_chronological_and_stable() {
    let document = document();
    assert_eq!(
        document
            .segments
            .iter()
            .map(|segment| segment.id.as_str())
            .collect::<Vec<_>>(),
        vec!["segment-2", "segment-10"]
    );

    let first = document.export(MeetingExportFormat::Plain).expect("plain");
    let second = document.export(MeetingExportFormat::Plain).expect("plain");
    assert_eq!(first, second);
    assert!(first.find("00:00:00.000").unwrap() < first.find("00:00:02.000").unwrap());
}
