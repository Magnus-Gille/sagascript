// From scripts/test-meeting-integration.mjs (#361).
import { expect, test } from "vitest";
import { initialMeetingDrafts, reconcileMeetingDrafts, type MeetingTranscript } from "./meeting-types";

test("review draft reconciliation preserves dirty segment B while applying segment A and another speaker rename", () => {
  const transcript: MeetingTranscript = {
    schema_version: 1,
    source_sha256: "source-a",
    language: "en",
    model: "model",
    duration_seconds: 4,
    segments: [
      { id: "a", start: 0, end: 1, text: "A original", speaker: "s1" },
      { id: "b", start: 1, end: 2, text: "B original", speaker: "s2" },
    ],
    speakers: [
      { id: "s1", label: "Speaker 1" },
      { id: "s2", label: "Speaker 2" },
    ],
  };
  const drafts = initialMeetingDrafts(transcript);
  drafts.texts.a = "A saved";
  drafts.texts.b = "B still being edited";
  const afterSegmentA = {
    ...transcript,
    segments: [{ ...transcript.segments[0], text: "A saved" }, transcript.segments[1]],
  };
  const reconciledA = reconcileMeetingDrafts(transcript, afterSegmentA, drafts, [
    { kind: "edit_segment", segment_id: "a", text: "A saved" },
  ]);
  expect(reconciledA.texts.a).toBe("A saved");
  expect(reconciledA.texts.b).toBe("B still being edited");

  reconciledA.labels.s2 = "Unsubmitted speaker name";
  const afterSpeakerRename = {
    ...afterSegmentA,
    speakers: [{ id: "s1", label: "Alice" }, afterSegmentA.speakers[1]],
  };
  const reconciledB = reconcileMeetingDrafts(afterSegmentA, afterSpeakerRename, reconciledA, [
    { kind: "rename_speaker", speaker_id: "s1", label: "Alice" },
  ]);
  expect(reconciledB.labels.s1).toBe("Alice");
  expect(reconciledB.labels.s2).toBe("Unsubmitted speaker name");
  expect(reconciledB.texts.b).toBe("B still being edited");
});

test("an edit field Rust sends as null is not part of the edit", () => {
  // Rust serializes an EditSegment field that the edit leaves unchanged as
  // null (contracts/transport/get_meeting_job.json, #347). Here only the text
  // of segment b changed; its unsaved speaker choice must survive.
  const transcript: MeetingTranscript = {
    schema_version: 1,
    source_sha256: "source-a",
    language: "en",
    model: "model",
    duration_seconds: 4,
    segments: [
      { id: "a", start: 0, end: 1, text: "A", speaker: "s1" },
      { id: "b", start: 1, end: 2, text: "B original", speaker: "s2" },
    ],
    speakers: [
      { id: "s1", label: "Speaker 1" },
      { id: "s2", label: "Speaker 2" },
    ],
  };
  const drafts = initialMeetingDrafts(transcript);
  drafts.speakers.b = "s1";
  const saved = { ...transcript, segments: [transcript.segments[0], { ...transcript.segments[1], text: "B saved" }] };
  const reconciled = reconcileMeetingDrafts(transcript, saved, drafts, [
    { kind: "edit_segment", segment_id: "b", text: "B saved", speaker_id: null },
  ]);
  expect(reconciled.texts.b).toBe("B saved");
  expect(reconciled.speakers.b).toBe("s1");
});
