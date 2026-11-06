use serde_json::json;

use super::*;

const GOLDEN_V1: &str = include_str!("../../tests/fixtures/recording/manifest-v1.json");

fn golden_manifest() -> Manifest {
    Manifest {
        schema_version: 1,
        min_reader_version: 1,
        recording_id: "6f1d2c3a-0b7e-4c1d-9a55-3e2f8c1b7d40".into(),
        title: "Team meeting".into(),
        state: RecordingState::Complete,
        recovered: false,
        started_at: "2026-11-05T14:02:11.482+01:00".into(),
        client: Client {
            kind: "desktop".into(),
            app_version: "1.4.8".into(),
            build: "abc1234".into(),
            extra: BTreeMap::new(),
        },
        quality: Quality::Lossless,
        tracks: vec![Track {
            id: "mic".into(),
            role: "mic".into(),
            file: "mic.flac".into(),
            codec: "flac".into(),
            lossless: true,
            sample_rate: 48_000,
            bits: 24,
            channels: 1,
            frames: 168_837_120,
            audio_sha256: Some(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            ),
            clock: Clock {
                domain: "mach_continuous".into(),
                start_ns: 123_456_789,
                anchors: vec![(123_456_789, 0), (10_123_456_789, 480_000)],
                extra: BTreeMap::new(),
            },
            gaps: vec![Gap::new(1_234_567, 480, GapReason::Overrun)],
            ended_reason: Some(EndedReason::Stopped),
            clipped_samples: 0,
            resampled_from: None,
            device: Some("MacBook Pro Microphone".into()),
            extra: BTreeMap::new(),
        }],
        extra: BTreeMap::new(),
    }
}

fn golden_value() -> Value {
    serde_json::from_str(GOLDEN_V1).expect("the fixture is JSON")
}

/// The schema test: the serialised shape of version 1 is the checked-in file.
/// A change that fails this is a schema change: bump the version (module
/// documentation) instead of editing the fixture.
#[test]
fn version_1_serialises_to_the_golden_file() {
    let written = serde_json::to_value(golden_manifest()).unwrap();
    assert_eq!(written, golden_value());
}

#[test]
fn the_golden_file_parses_to_the_golden_manifest() {
    assert_eq!(
        parse_manifest(GOLDEN_V1.as_bytes()).unwrap(),
        golden_manifest()
    );
}

#[test]
fn a_manifest_survives_serialise_and_parse() {
    let bytes = serialize_manifest(&golden_manifest()).unwrap();
    assert_eq!(bytes.last(), Some(&b'\n'));
    assert_eq!(parse_manifest(&bytes).unwrap(), golden_manifest());
}

#[test]
fn an_in_progress_manifest_has_nulls_where_the_end_is_unknown() {
    let mut manifest = golden_manifest();
    manifest.state = RecordingState::Recording;
    let track = &mut manifest.tracks[0];
    track.frames = 0;
    track.audio_sha256 = None;
    track.ended_reason = None;
    track.gaps.clear();
    let value = serde_json::to_value(&manifest).unwrap();
    assert_eq!(value["state"], "recording");
    assert_eq!(value["tracks"][0]["audio_sha256"], Value::Null);
    assert_eq!(value["tracks"][0]["ended_reason"], Value::Null);
    assert_eq!(
        parse_manifest(&serialize_manifest(&manifest).unwrap()).unwrap(),
        manifest
    );
}

/// A reader of version 1 meets a manifest of a later shape that it may still
/// read (`min_reader_version` 1): unknown fields at every level are ignored
/// and survive a rewrite, and the versions are left as written.
#[test]
fn an_old_reader_ignores_unknown_fields_and_keeps_them() {
    let mut value = golden_value();
    value["schema_version"] = json!(2);
    value["future_top_level"] = json!({"nested": [1, 2, 3]});
    value["client"]["future_client"] = json!("x");
    value["tracks"][0]["future_track"] = json!(true);
    value["tracks"][0]["clock"]["future_clock"] = json!(7);
    value["tracks"][0]["gaps"][0]["future_gap"] = json!("g");
    let manifest = parse_manifest(value.to_string().as_bytes()).expect("readable");

    assert_eq!(manifest.schema_version, 2, "a newer file keeps its version");
    assert_eq!(
        manifest.extra["future_top_level"],
        json!({"nested": [1, 2, 3]})
    );
    assert_eq!(manifest.client.extra["future_client"], json!("x"));
    assert_eq!(manifest.tracks[0].extra["future_track"], json!(true));
    assert_eq!(manifest.tracks[0].clock.extra["future_clock"], json!(7));
    assert_eq!(manifest.tracks[0].gaps[0].extra["future_gap"], json!("g"));

    let rewritten: Value = serde_json::from_slice(&serialize_manifest(&manifest).unwrap()).unwrap();
    assert_eq!(
        rewritten, value,
        "nothing the reader did not understand is lost"
    );
}

#[test]
fn a_manifest_that_needs_a_newer_reader_is_refused() {
    let mut value = golden_value();
    value["schema_version"] = json!(3);
    value["min_reader_version"] = json!(2);
    match parse_manifest(value.to_string().as_bytes()) {
        Err(RecordingError::ManifestTooNew {
            required: 2,
            supported: 1,
        }) => {}
        other => panic!("expected ManifestTooNew, got {other:?}"),
    }
}

#[test]
fn unknown_reasons_are_kept_as_written() {
    let mut value = golden_value();
    value["tracks"][0]["ended_reason"] = json!("battery_low");
    value["tracks"][0]["gaps"][0]["reason"] = json!("thermal");
    let manifest = parse_manifest(value.to_string().as_bytes()).unwrap();
    assert_eq!(
        manifest.tracks[0].ended_reason,
        Some(EndedReason::Other("battery_low".into()))
    );
    assert_eq!(
        manifest.tracks[0].gaps[0].reason,
        GapReason::Other("thermal".into())
    );
    let rewritten: Value = serde_json::from_slice(&serialize_manifest(&manifest).unwrap()).unwrap();
    assert_eq!(rewritten, value);
}

#[test]
fn the_export_view_leaves_out_the_device() {
    let manifest = golden_manifest();
    assert!(serde_json::to_string(&manifest)
        .unwrap()
        .contains("MacBook"));
    let exported = serde_json::to_string(&manifest.export_view()).unwrap();
    assert!(
        !exported.contains("MacBook") && !exported.contains("\"device\""),
        "{exported}"
    );
    // The manifest itself still has it.
    assert_eq!(
        manifest.tracks[0].device.as_deref(),
        Some("MacBook Pro Microphone")
    );
}

#[test]
fn file_names_that_could_leave_the_folder_are_refused() {
    for bad in [
        "../mic.flac",
        "sub/mic.flac",
        "sub\\mic.flac",
        "C:mic.flac",
        "/etc/passwd",
        "..",
        "",
        "mic\u{0}.flac",
    ] {
        let mut value = golden_value();
        value["tracks"][0]["file"] = json!(bad);
        assert!(
            matches!(
                parse_manifest(value.to_string().as_bytes()),
                Err(RecordingError::ManifestInvalid { .. })
            ),
            "file {bad:?} must be refused"
        );
    }
    for bad_id in ["a/b", "..", ".hidden", "trailing.", ""] {
        let mut value = golden_value();
        value["tracks"][0]["id"] = json!(bad_id);
        assert!(
            parse_manifest(value.to_string().as_bytes()).is_err(),
            "id {bad_id:?} must be refused"
        );
    }
}

#[test]
fn duplicate_tracks_and_missing_formats_are_refused() {
    let mut value = golden_value();
    let track = value["tracks"][0].clone();
    value["tracks"].as_array_mut().unwrap().push(track);
    assert!(parse_manifest(value.to_string().as_bytes()).is_err());

    let mut value = golden_value();
    value["tracks"][0]["sample_rate"] = json!(0);
    assert!(parse_manifest(value.to_string().as_bytes()).is_err());
}

#[test]
fn garbage_is_an_invalid_manifest_not_a_panic() {
    for bytes in [
        &b""[..],
        b"not json",
        b"[]",
        b"{}",
        br#"{"schema_version": 0}"#,
        br#"{"schema_version": "1"}"#,
        br#"{"schema_version": 1, "min_reader_version": "x"}"#,
        br#"{"schema_version": 1}"#,
    ] {
        assert!(
            matches!(
                parse_manifest(bytes),
                Err(RecordingError::ManifestInvalid { .. })
            ),
            "{:?}",
            String::from_utf8_lossy(bytes)
        );
    }
}

fn add_a(value: &mut Value) {
    value["a"] = json!(1);
}

fn add_b_from_a(value: &mut Value) {
    let a = value["a"].as_i64().unwrap_or(0);
    value["b"] = json!(a + 1);
}

#[test]
fn migration_steps_run_in_order_from_the_files_version() {
    let steps: &[Migration] = &[add_a, add_b_from_a];

    let mut from_1 = json!({"schema_version": 1, "min_reader_version": 1});
    migrate_with(&mut from_1, 1, steps, 3);
    assert_eq!(
        from_1,
        json!({"schema_version": 3, "min_reader_version": 1, "a": 1, "b": 2})
    );

    let mut from_2 = json!({"schema_version": 2, "min_reader_version": 1, "a": 5});
    migrate_with(&mut from_2, 2, steps, 3);
    assert_eq!(from_2["b"], json!(6), "only the step from version 2 ran");
    assert_eq!(from_2["a"], json!(5));
}

#[test]
fn migration_is_idempotent_and_leaves_newer_files_alone() {
    let steps: &[Migration] = &[add_a, add_b_from_a];
    let mut value = json!({"schema_version": 1, "min_reader_version": 1});
    migrate_with(&mut value, 1, steps, 3);
    let once = value.clone();
    migrate_with(&mut value, 3, steps, 3);
    assert_eq!(value, once, "a file at the target version is not touched");

    let mut newer = json!({"schema_version": 9, "min_reader_version": 1, "x": 1});
    let before = newer.clone();
    migrate_with(&mut newer, 9, steps, 3);
    assert_eq!(newer, before);
}

#[test]
fn the_step_list_matches_the_version() {
    assert_eq!(
        MIGRATIONS.len() as u32,
        SCHEMA_VERSION - 1,
        "one migration step per version bump"
    );
    const { assert!(MIN_READER_VERSION <= SCHEMA_VERSION) };
}

#[test]
fn a_track_cannot_claim_a_file_the_recording_keeps_for_itself() {
    // Recovery writes into the track file: it must never be the manifest, the
    // lock, a journal or a tail.
    for bad in [
        "recording.json",
        "recording.json.tmp",
        "recording.lock",
        "mic.events.jsonl",
        "mic.tail",
        "mic.tail.1",
    ] {
        let mut value = golden_value();
        value["tracks"][0]["file"] = json!(bad);
        assert!(
            matches!(
                parse_manifest(value.to_string().as_bytes()),
                Err(RecordingError::ManifestInvalid { .. })
            ),
            "file {bad:?} must be refused"
        );
    }
    // Ordinary names, including ones that merely look similar, are fine.
    for good in [
        "mic.flac",
        "system audio.flac",
        "recording.flac",
        "tail.flac",
    ] {
        let mut value = golden_value();
        value["tracks"][0]["file"] = json!(good);
        assert!(
            parse_manifest(value.to_string().as_bytes()).is_ok(),
            "{good}"
        );
    }
}

#[test]
fn a_sealed_manifest_is_an_ordinary_manifest_that_checks_itself() {
    let manifest = parse_manifest(&serialize_manifest(&golden_manifest()).unwrap()).unwrap();
    let sealed = serialize_manifest_sealed(&manifest).unwrap();
    assert_eq!(manifest_integrity(&sealed), Integrity::Valid);
    assert_eq!(
        manifest_integrity(&serialize_manifest(&manifest).unwrap()),
        Integrity::Absent
    );
    // Readers see the same manifest: the seal is not part of the description.
    assert_eq!(parse_manifest(&sealed).unwrap(), manifest);
    let value: serde_json::Value = serde_json::from_slice(&sealed).unwrap();
    assert_eq!(value["integrity"].as_str().unwrap().len(), 64);

    // Any change to the bytes breaks it, a hybrid of two manifests included.
    let mut changed = sealed.clone();
    let at = changed.windows(5).position(|w| w == b"title").unwrap_or(40);
    changed[at] ^= 0x01;
    assert_eq!(manifest_integrity(&changed), Integrity::Invalid);
    let other = serialize_manifest_sealed(&Manifest {
        title: "Other".into(),
        ..manifest.clone()
    })
    .unwrap();
    for cut in [at + 14, sealed.len() / 2, sealed.len() - 5] {
        let mut hybrid = sealed.clone();
        let n = cut.min(other.len());
        hybrid[..n].copy_from_slice(&other[..n]);
        assert_eq!(manifest_integrity(&hybrid), Integrity::Invalid, "cut {cut}");
    }
    // Sealing again replaces the seal rather than adding to it.
    assert_eq!(
        serialize_manifest_sealed(&parse_manifest(&sealed).unwrap()).unwrap(),
        sealed
    );
}

#[test]
fn a_title_that_looks_like_a_seal_cannot_stand_in_for_one() {
    let mut manifest = golden_manifest();
    manifest.title = "\"integrity\": \"".to_string() + &"a".repeat(64);
    let sealed = serialize_manifest_sealed(&manifest).unwrap();
    assert_eq!(manifest_integrity(&sealed), Integrity::Valid);
    assert_eq!(parse_manifest(&sealed).unwrap().title, manifest.title);
}
