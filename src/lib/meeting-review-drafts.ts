// Meeting review editor drafts: the serializable snapshot the parent keeps
// for update recovery, and its validated restore. Moved unchanged from
// MeetingReview.svelte's module script so it is a plain module (#361).
import type { MeetingDraftState as DraftState, MeetingTranscript as Transcript } from "./meeting-types";

/** Serializable editor state owned by the parent recovery store. */
export interface MeetingReviewDraftSnapshot {
  source_sha256: string;
  review_revision: string;
  drafts: DraftState;
}

function stringMap(value: unknown): Record<string, string> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>).filter(([key, entry]) => key.length > 0 && typeof entry === "string"),
  ) as Record<string, string>;
}

function mapForIds(value: unknown, ids: readonly string[], fallback: Record<string, string>): Record<string, string> {
  const source = stringMap(value);
  return Object.fromEntries(ids.map((id) => [id, source[id] ?? fallback[id] ?? ""]));
}

/**
 * Restore only editor fields belonging to this exact transcript/review.
 * Unknown IDs and invalid speaker merge targets are intentionally dropped.
 */
export function restoreMeetingDraftSnapshot(
  base: DraftState,
  snapshot: MeetingReviewDraftSnapshot | null | undefined,
  transcript: Transcript,
  sourceSha256: string,
  reviewRevision: string,
): DraftState {
  if (!snapshot || snapshot.source_sha256 !== sourceSha256 || snapshot.review_revision !== reviewRevision) return base;

  const speakerIds = transcript.speakers.map((speaker) => speaker.id);
  const segmentIds = transcript.segments.map((segment) => segment.id);
  const speakerIdSet = new Set(speakerIds);
  const mergeSource = stringMap(snapshot.drafts?.mergeTargets);
  const mergeTargets = Object.fromEntries(
    Object.entries(mergeSource).filter(([from, into]) => from !== into && speakerIdSet.has(from) && speakerIdSet.has(into)),
  );
  const speakerDrafts = mapForIds(snapshot.drafts?.speakers, segmentIds, base.speakers);
  for (const id of segmentIds) {
    if (!speakerIdSet.has(speakerDrafts[id])) speakerDrafts[id] = base.speakers[id] ?? "";
  }

  return {
    labels: mapForIds(snapshot.drafts?.labels, speakerIds, base.labels),
    mergeTargets,
    texts: mapForIds(snapshot.drafts?.texts, segmentIds, base.texts),
    speakers: speakerDrafts,
  };
}

export function createMeetingReviewDraftSnapshot(
  sourceSha256: string,
  reviewRevision: string,
  drafts: DraftState,
): MeetingReviewDraftSnapshot {
  return {
    source_sha256: sourceSha256,
    review_revision: reviewRevision,
    drafts: {
      labels: { ...drafts.labels },
      mergeTargets: { ...drafts.mergeTargets },
      texts: { ...drafts.texts },
      speakers: { ...drafts.speakers },
    },
  };
}
