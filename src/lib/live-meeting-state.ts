import type { Gap, LiveSegment, LiveSnapshot, LiveSource, SourceStatus } from "./live-meeting-types";

export interface LiveMeetingState {
  snapshot: LiveSnapshot | null;
  sessionId: string | null;
  revision: number;
}

export type LiveState = LiveMeetingState;

export const initialLiveMeetingState: LiveMeetingState = {
  snapshot: null,
  sessionId: null,
  revision: -1,
};

export function createInitialLiveMeetingState(): LiveMeetingState {
  return { ...initialLiveMeetingState };
}

export type LiveMeetingAction =
  | { type: "snapshot"; snapshot: LiveSnapshot }
  | { type: "update"; snapshot: LiveSnapshot }
  | { type: "clear" }
  | { type: "reset"; sessionId?: string | null };

const sourceOrder: Record<LiveSource, number> = { microphone: 0, system: 1 };

function finiteNumber(value: number, fallback = 0): number {
  return Number.isFinite(value) ? value : fallback;
}

function numericIdPart(id: string): number | null {
  const match = id.match(/(\d+)(?!.*\d)/);
  return match ? Number(match[1]) : null;
}

/** Compare identifiers naturally, so item 10 follows item 9. */
export function compareNumericIds(left: string, right: string): number {
  const leftNumber = numericIdPart(left);
  const rightNumber = numericIdPart(right);
  if (leftNumber !== null && rightNumber !== null && leftNumber !== rightNumber) return leftNumber - rightNumber;
  return left.localeCompare(right);
}

function compareSources(left: LiveSource, right: LiveSource): number {
  return sourceOrder[left] - sourceOrder[right];
}

/** Stable display order for transcript segments. */
export function sortLiveSegments(segments: readonly LiveSegment[]): LiveSegment[] {
  return [...segments].sort(
    (left, right) =>
      finiteNumber(left.start_ms) - finiteNumber(right.start_ms) ||
      compareSources(left.source, right.source) ||
      compareNumericIds(left.id, right.id) ||
      finiteNumber(left.end_ms) - finiteNumber(right.end_ms),
  );
}

/** Stable display order for source diagnostics. */
export function sortSourceStatuses(sources: readonly SourceStatus[]): SourceStatus[] {
  return [...sources].sort((left, right) => compareSources(left.source, right.source));
}

/** Stable display order for audio gaps. */
export function sortLiveGaps(gaps: readonly Gap[]): Gap[] {
  return [...gaps].sort(
    (left, right) =>
      finiteNumber(left.start_ms) - finiteNumber(right.start_ms) ||
      compareSources(left.source, right.source) ||
      finiteNumber(left.end_ms) - finiteNumber(right.end_ms) ||
      left.reason.localeCompare(right.reason),
  );
}

function compareSegmentRevision(left: LiveSegment, right: LiveSegment): number {
  return (
    finiteNumber(left.revision) - finiteNumber(right.revision) ||
    finiteNumber(left.end_ms) - finiteNumber(right.end_ms) ||
    compareNumericIds(left.id, right.id)
  );
}

/**
 * Keep one in-flight preview per channel. Final segments are retained, and a
 * later preview replaces the earlier one from the same channel. This mirrors
 * the backend's whole-snapshot shape while keeping the transcript bounded to
 * the currently meaningful preview.
 */
export function replaceLivePreviews(segments: readonly LiveSegment[]): LiveSegment[] {
  const finalSegments = segments.filter((segment) => segment.status === "final");
  const latestPreview = new Map<LiveSource, LiveSegment>();
  for (const segment of segments) {
    if (segment.status !== "preview") continue;
    const previous = latestPreview.get(segment.source);
    if (!previous || compareSegmentRevision(previous, segment) <= 0) latestPreview.set(segment.source, segment);
  }
  return sortLiveSegments([...finalSegments, ...latestPreview.values()]);
}

export function normalizeLiveSnapshot(snapshot: LiveSnapshot): LiveSnapshot {
  return {
    ...snapshot,
    sources: sortSourceStatuses(snapshot.sources),
    segments: replaceLivePreviews(snapshot.segments),
    gaps: sortLiveGaps(snapshot.gaps),
  };
}

export type SourceHealth = "healthy" | "silent" | "disconnected" | "failed";

export interface DerivedSourceStatus {
  health: SourceHealth;
  lag_ms: number | null;
  label: string;
}

/** Silence is a useful state, not an error. */
export function deriveSourceStatus(source: SourceStatus): DerivedSourceStatus {
  const lag = source.text_delay_ms;
  if (source.error) return { health: "failed", lag_ms: lag, label: source.error };
  if (!source.connected) return { health: "disconnected", lag_ms: lag, label: "Disconnected" };
  if (source.last_audio_ms === null && source.last_text_ms === null) return { health: "silent", lag_ms: null, label: "Silent" };
  if (source.signal_level < 0.0015) return { health: "silent", lag_ms: lag, label: "Silent" };
  return { health: "healthy", lag_ms: lag, label: "Receiving audio" };
}

export const deriveChannelStatus = deriveSourceStatus;

export function reduceLiveMeetingState(state: LiveMeetingState, action: LiveMeetingAction): LiveMeetingState {
  if (action.type === "clear") return createInitialLiveMeetingState();
  if (action.type === "reset") {
    if (action.sessionId && state.sessionId && action.sessionId !== state.sessionId) return state;
    return createInitialLiveMeetingState();
  }

  const snapshot = action.snapshot;
  if (state.sessionId !== null && snapshot.id !== state.sessionId) return state;
  if (state.snapshot !== null && snapshot.revision <= state.revision) return state;
  const normalized = normalizeLiveSnapshot(snapshot);
  return { snapshot: normalized, sessionId: normalized.id, revision: normalized.revision };
}

export const reduceLiveState = reduceLiveMeetingState;
export const reduceLiveMeeting = reduceLiveMeetingState;
