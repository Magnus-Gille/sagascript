// From scripts/test-meeting-integration.mjs (#361): the draft snapshot helpers
// used to be MeetingReview.svelte's module script.
import { expect, test } from "vitest";
import { createMeetingReviewDraftSnapshot, restoreMeetingDraftSnapshot } from "./meeting-review-drafts";
import { initialMeetingDrafts, type MeetingDraftState, type MeetingTranscript } from "./meeting-types";

test("meeting draft snapshots round-trip and reject another source or review revision", () => {
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
  const clean = initialMeetingDrafts(transcript);
  const dirty: MeetingDraftState = {
    labels: { ...clean.labels, s1: "Alice" },
    mergeTargets: { s1: "s2", ignored: "s1", s2: "s2" },
    texts: { ...clean.texts, a: "A edited", ignored: "should be dropped" },
    speakers: { ...clean.speakers, b: "s1", ignored: "s1" },
  };
  const snapshot = createMeetingReviewDraftSnapshot("source-a", "revision-1", dirty);
  dirty.texts.a = "mutated after snapshot";
  expect(snapshot.drafts.texts.a, "snapshot must own serializable copies").toBe("A edited");

  const restored = restoreMeetingDraftSnapshot(clean, snapshot, transcript, "source-a", "revision-1");
  expect(restored.labels.s1).toBe("Alice");
  expect(restored.texts.a).toBe("A edited");
  expect(restored.speakers.b).toBe("s1");
  expect(restored.mergeTargets, "invalid merge targets and IDs are dropped").toStrictEqual({ s1: "s2" });
  expect(restored.texts.ignored).toBe(undefined);
  expect(
    restoreMeetingDraftSnapshot(clean, snapshot, transcript, "source-b", "revision-1"),
    "a different source must not hydrate editor state",
  ).toStrictEqual(clean);
  expect(
    restoreMeetingDraftSnapshot(clean, snapshot, transcript, "source-a", "revision-2"),
    "a different committed revision must not hydrate editor state",
  ).toStrictEqual(clean);
});
