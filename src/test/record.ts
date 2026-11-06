// Fixtures for the recorder window's tests (SAGA-564): payloads shaped as the
// backend sends them (docs/handoffs/saga-564-spec.md).
import type { RecordingFinished, RecordingSessionInfo, RecordingSummary } from "../lib/record-types";

export function sessionInfo(overrides: Partial<RecordingSessionInfo> = {}): RecordingSessionInfo {
  return {
    state: "recording",
    recording_id: "0d2c1f3a-1111-4222-8333-444455556666",
    folder_name: "2026-11-06 10.00 Interview",
    title: "Interview",
    quality: "lossless",
    elapsed_ms: 0,
    level: 0,
    clipped: false,
    gap_count: 0,
    gap_ms: 0,
    stalled: false,
    device_lost: false,
    bytes_written: 0,
    free_disk_bytes: 50_000_000_000,
    warnings: [],
    ended_early: null,
    error_code: null,
    ...overrides,
  };
}

export function summary(overrides: Partial<RecordingSummary> = {}): RecordingSummary {
  return { ...finishedRecording.summary, ...overrides };
}

export const finishedRecording: RecordingFinished = {
  ended_early: null,
  summary: {
    folder_name: "2026-11-06 10.00 Interview",
    recording_id: "0d2c1f3a-1111-4222-8333-444455556666",
    title: "Interview",
    state: "complete",
    recovered: false,
    started_at: "2026-11-06T10:00:00Z",
    quality: "lossless",
    duration_seconds: 62,
    bytes: 5_100_000,
    track_count: 1,
    gap_count: 0,
    gap_seconds: 0,
    ended_reason: null,
    needs_recovery: false,
    playable: true,
  },
};
