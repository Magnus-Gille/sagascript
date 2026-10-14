import { expect, test } from "vitest";
import { activeSegmentsAtTime, isSegmentActive } from "./meeting-playback";
import type { MeetingSegment } from "./meeting-types";

const segments: MeetingSegment[] = [
  { id: "early", start: 0, end: 2, text: "early", speaker: "a" },
  { id: "overlap", start: 1, end: 3, text: "overlap", speaker: "b" },
  { id: "late", start: 3, end: 4, text: "late", speaker: "a" },
];

test("playback uses half-open boundaries and preserves overlap order", () => {
  expect(activeSegmentsAtTime(segments, 0).map((segment) => segment.id)).toStrictEqual(["early"]);
  expect(activeSegmentsAtTime(segments, 1).map((segment) => segment.id)).toStrictEqual(["early", "overlap"]);
  expect(activeSegmentsAtTime(segments, 2).map((segment) => segment.id)).toStrictEqual(["overlap"]);
  expect(activeSegmentsAtTime(segments, 3).map((segment) => segment.id)).toStrictEqual(["late"]);
});

test("playback leaves gaps and EOF inactive", () => {
  expect(activeSegmentsAtTime(segments, 4)).toStrictEqual([]);
  expect(activeSegmentsAtTime(segments, 2.5)).toStrictEqual([segments[1]]);
  expect(activeSegmentsAtTime(segments, Number.NaN)).toStrictEqual([]);
  expect(isSegmentActive(segments[0], 2)).toBe(false);
});
