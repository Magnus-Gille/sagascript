use sagascript_testsupport::TempDir;

use super::*;

fn start() -> JournalEvent {
    JournalEvent::Start {
        domain: "mach_continuous".into(),
        host_ns: 1_000,
        sample_rate: 48_000,
        channels: 1,
    }
}

#[test]
fn events_round_trip_through_a_file() {
    let dir = TempDir::new();
    let path = dir.path().join(journal_file_name("mic"));
    assert!(path.ends_with("mic.events.jsonl"));
    let mut writer = JournalWriter::create(&path).unwrap();
    writer.append(&start()).unwrap();
    writer
        .append(&JournalEvent::Anchor {
            host_ns: 2_000,
            frame: 480_000,
            clipped: 3,
        })
        .unwrap();
    writer
        .append(&JournalEvent::Gap {
            at_frame: 500_000,
            length: 480,
            reason: GapReason::Overrun,
            clipped: 5,
        })
        .unwrap();
    writer.sync().unwrap();

    let summary = read_journal(&path).unwrap();
    assert_eq!(summary.start, Some(("mach_continuous".into(), 1_000)));
    assert_eq!(summary.anchors, vec![(1_000, 0), (2_000, 480_000)]);
    assert_eq!(
        summary.gaps,
        vec![Gap::new(500_000, 480, GapReason::Overrun)]
    );
    assert_eq!(summary.clipped, 5);
    assert_eq!(summary.ignored_lines, 0);
}

#[test]
fn one_event_is_one_line() {
    let dir = TempDir::new();
    let path = dir.path().join("t.events.jsonl");
    let mut writer = JournalWriter::create(&path).unwrap();
    writer.append(&start()).unwrap();
    writer.append(&start()).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 2);
    assert!(text.ends_with('\n'));
    assert!(text.starts_with(r#"{"t":"start","#), "{text}");
}

#[test]
fn a_torn_last_line_is_skipped_not_fatal() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"{\"t\":\"start\",\"domain\":\"qpc\",\"host_ns\":7,\"sample_rate\":44100,\"channels\":2}\n");
    bytes.extend_from_slice(b"{\"t\":\"gap\",\"at_frame\":10,\"length\":5,\"reason\":\"stall\"}\n");
    // Torn: cut in the middle of the next event, no newline.
    bytes.extend_from_slice(b"{\"t\":\"gap\",\"at_frame\":99,\"len");
    let summary = summarize(&bytes);
    assert_eq!(summary.start, Some(("qpc".into(), 7)));
    assert_eq!(summary.gaps, vec![Gap::new(10, 5, GapReason::Stall)]);
    assert_eq!(summary.ignored_lines, 1);
}

#[test]
fn a_complete_looking_line_without_a_newline_is_still_torn() {
    let summary = summarize(b"{\"t\":\"anchor\",\"host_ns\":1,\"frame\":2}");
    assert!(summary.anchors.is_empty());
    assert_eq!(summary.ignored_lines, 1);
}

#[test]
fn events_of_a_newer_build_and_garbage_are_counted_and_skipped() {
    let bytes =
        b"{\"t\":\"start\",\"domain\":\"d\",\"host_ns\":1,\"sample_rate\":8000,\"channels\":1}\n\
                  {\"t\":\"future_event\",\"x\":1}\n\
                  not json at all\n\
                  \n\
                  {\"t\":\"anchor\",\"host_ns\":5,\"frame\":40}\n";
    let summary = summarize(bytes);
    assert_eq!(summary.anchors, vec![(1, 0), (5, 40)]);
    assert_eq!(summary.ignored_lines, 2);
}

#[test]
fn a_missing_journal_is_an_empty_summary() {
    let dir = TempDir::new();
    let summary = read_journal(&dir.path().join("nope.events.jsonl")).unwrap();
    assert_eq!(summary, JournalSummary::default());
}

#[test]
fn only_the_first_start_counts() {
    let mut bytes = Vec::new();
    for host_ns in [1u64, 2] {
        bytes.extend_from_slice(
            format!("{{\"t\":\"start\",\"domain\":\"d\",\"host_ns\":{host_ns},\"sample_rate\":8000,\"channels\":1}}\n")
                .as_bytes(),
        );
    }
    let summary = summarize(&bytes);
    assert_eq!(summary.start, Some(("d".into(), 1)));
    assert_eq!(summary.anchors, vec![(1, 0)]);
}

#[test]
fn a_journal_is_never_overwritten() {
    let dir = TempDir::new();
    let path = dir.path().join("mic.events.jsonl");
    JournalWriter::create(&path).unwrap();
    assert!(JournalWriter::create(&path).is_err());
}

#[cfg(unix)]
#[test]
fn a_journal_is_private_to_the_owner() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new();
    let path = dir.path().join("mic.events.jsonl");
    JournalWriter::create(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
