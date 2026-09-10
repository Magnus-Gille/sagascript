import type { MeetingSegment } from "./meeting-types";

/** A segment is active on a half-open interval: start <= time < end. */
export function isSegmentActive(segment: MeetingSegment, time: number): boolean {
  return Number.isFinite(time) && segment.start <= time && time < segment.end;
}

/** Return every active overlap in chronological order; gaps and EOF return []. */
export function activeSegmentsAtTime(
  segments: readonly MeetingSegment[],
  time: number,
): MeetingSegment[] {
  return segments
    .filter((segment) => isSegmentActive(segment, time))
    .sort((a, b) => a.start - b.start || a.end - b.end || a.id.localeCompare(b.id));
}
