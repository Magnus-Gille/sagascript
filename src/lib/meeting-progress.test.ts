import { expect, test } from "vitest";
import {
  formatDuration,
  phaseDetail,
  phaseLabel,
  statusCheckAnnouncement,
  statusCheckMessage,
  statusCheckState,
  statusTitle,
} from "./meeting-progress";

test("meeting progress formats supplied durations without inventing precision", () => {
  expect(formatDuration(0)).toBe("00:00");
  expect(formatDuration(65_999)).toBe("01:05");
  expect(formatDuration(3_661_000)).toBe("1:01:01");
  expect(formatDuration(-1)).toBe("00:00");
});

test("meeting phases have human-readable labels and the long-running analysis explanation", () => {
  expect(phaseLabel("loading_model")).toBe("Loading speech model");
  expect(phaseLabel("unknown")).toBe("Processing meeting");
  expect(phaseDetail("analyzing")).toBe(
    "Transcribing speech and identifying speakers. This can take several minutes for long recordings.",
  );
});

test("status titles cover running, cancellation, and terminal outcomes", () => {
  expect(statusTitle("running")).toBe("Processing meeting");
  expect(statusTitle("cancelling")).toBe("Cancelling meeting processing");
  expect(statusTitle("completed")).toBe("Meeting processing complete");
  expect(statusTitle("cancelled")).toBe("Meeting processing cancelled");
  expect(statusTitle("failed")).toBe("Meeting processing failed");
});

test("status feedback distinguishes recent checks, stale responses, and failed checks", () => {
  expect(statusCheckMessage(0, false)).toBe("Status checked just now");
  expect(statusCheckMessage(2_000, false)).toBe("Status checked 2 seconds ago");
  expect(statusCheckMessage(10_001, false)).toBe(
    "No status response for 10 seconds — processing may still be running.",
  );
  expect(statusCheckMessage(0, true)).toBe("Status check failed — processing may still be running.");
});

test("status announcements change only when the polling state changes", () => {
  expect(statusCheckState(null, false)).toBe("waiting");
  expect(statusCheckState(2_000, false)).toBe("healthy");
  expect(statusCheckState(9_999, false)).toBe("healthy");
  expect(statusCheckState(10_001, false)).toBe("stale");
  expect(statusCheckState(0, true)).toBe("failed");
  expect(statusCheckAnnouncement("healthy")).toBe("Status checks are up to date.");
  expect(statusCheckAnnouncement("stale")).toBe("No recent status response — processing may still be running.");
  expect(statusCheckAnnouncement("failed")).toBe("Status check failed — processing may still be running.");
});
