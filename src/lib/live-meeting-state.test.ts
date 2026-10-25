import { describe, expect, test } from "vitest";
import { deriveSourceStatus, initialLiveMeetingState, reduceLiveMeetingState, sortLiveSegments } from "./live-meeting-state";
import type { LiveSnapshot, SourceStatus } from "./live-meeting-types";

const source = (overrides: Partial<SourceStatus> = {}): SourceStatus => ({
  source: "microphone",
  connected: true,
  signal_level: 0.5,
  last_audio_ms: 100,
  last_text_ms: 100,
  text_delay_ms: null,
  queued_ms: 0,
  dropped_frames: 0,
  error: null,
  ...overrides,
});

function snapshot(overrides: Partial<LiveSnapshot> = {}): LiveSnapshot {
  return {
    id: "session-1",
    revision: 1,
    phase: "running",
    elapsed_ms: 2_000,
    sources: [source()],
    segments: [],
    gaps: [],
    error: null,
    audio_available: true,
    ...overrides,
  };
}

describe("live meeting state", () => {
  test("replaces a same-source preview and orders final segments numerically", () => {
    const state = reduceLiveMeetingState(initialLiveMeetingState, {
      type: "snapshot",
      snapshot: snapshot({
        segments: [
          { id: "segment-10", source: "microphone", start_ms: 10, end_ms: 12, text: "final", status: "final", revision: 1 },
          { id: "preview-1", source: "microphone", start_ms: 12, end_ms: 13, text: "old", status: "preview", revision: 2 },
          { id: "preview-2", source: "microphone", start_ms: 12, end_ms: 13, text: "new", status: "preview", revision: 3 },
          { id: "segment-2", source: "system", start_ms: 10, end_ms: 11, text: "system", status: "final", revision: 1 },
        ],
      }),
    });

    expect(state.snapshot?.segments.map(({ id, text }) => ({ id, text }))).toEqual([
      { id: "segment-10", text: "final" },
      { id: "segment-2", text: "system" },
      { id: "preview-2", text: "new" },
    ]);
  });

  test("ignores stale revisions and snapshots from another session", () => {
    const current = reduceLiveMeetingState(initialLiveMeetingState, { type: "snapshot", snapshot: snapshot() });
    const stale = reduceLiveMeetingState(current, { type: "update", snapshot: snapshot({ revision: 1, phase: "failed" }) });
    const wrongSession = reduceLiveMeetingState(current, { type: "update", snapshot: snapshot({ id: "session-2", revision: 2 }) });
    expect(stale).toBe(current);
    expect(wrongSession).toBe(current);
  });

  test("sorting uses time, source, then numeric id", () => {
    const segments = sortLiveSegments([
      { id: "segment-10", source: "microphone", start_ms: 1, end_ms: 2, text: "", status: "final", revision: 1 },
      { id: "segment-2", source: "microphone", start_ms: 1, end_ms: 2, text: "", status: "final", revision: 1 },
      { id: "segment-1", source: "system", start_ms: 1, end_ms: 2, text: "", status: "final", revision: 1 },
    ]);
    expect(segments.map((segment) => segment.id)).toEqual(["segment-2", "segment-10", "segment-1"]);
  });

  test("silence is not reported as an error, while source failures are", () => {
    expect(deriveSourceStatus(source({ last_audio_ms: null, last_text_ms: null }))).toMatchObject({ health: "silent", label: "Silent" });
    expect(deriveSourceStatus(source({ error: "input failed" }))).toMatchObject({ health: "failed", label: "input failed" });
  });
});

test("silence does not accumulate a fictitious transcription delay", () => {
  expect(deriveSourceStatus(source({ signal_level: 0, text_delay_ms: 650 }))).toMatchObject({ health: "silent", lag_ms: 650 });
});
