export interface MeetingSpeaker {
  id: string;
  label: string;
}

export interface MeetingSegment {
  id: string;
  start: number;
  end: number;
  text: string;
  speaker: string;
}

export interface MeetingTranscript {
  schema_version: number;
  source_sha256: string;
  language: string;
  model: string;
  duration_seconds: number;
  segments: MeetingSegment[];
  speakers: MeetingSpeaker[];
}

export type MeetingExportFormat = "plain" | "markdown" | "json" | "srt" | "vtt";

export type CorrectionOperation =
  | {
      kind: "edit_segment";
      segment_id: string;
      text?: string;
      speaker_id?: string;
    }
  | {
      kind: "rename_speaker";
      speaker_id: string;
      label: string;
    }
  | {
      kind: "merge_speakers";
      from_id: string;
      into_id: string;
    };

export interface CorrectionBatch {
  operations: CorrectionOperation[];
}

export interface CorrectionFile {
  schema_version: number;
  source_sha256: string;
  original_revision: string;
  expected_revision: string;
  operations: CorrectionOperation[];
}

export interface MeetingReview {
  schema_version: number;
  original: MeetingTranscript;
  original_revision: string;
  generation: number;
  batches: CorrectionBatch[];
  revision: string;
}

export interface MeetingReviewState {
  review: MeetingReview;
  transcript: MeetingTranscript;
}

export interface MeetingAudioAttachment {
  token: string;
  mime: string;
}

export interface MeetingDraftState {
  labels: Record<string, string>;
  mergeTargets: Record<string, string>;
  texts: Record<string, string>;
  speakers: Record<string, string>;
}

export function initialMeetingDrafts(transcript: MeetingTranscript): MeetingDraftState {
  return {
    labels: Object.fromEntries(transcript.speakers.map((speaker) => [speaker.id, speaker.label])),
    mergeTargets: {},
    texts: Object.fromEntries(transcript.segments.map((segment) => [segment.id, segment.text])),
    speakers: Object.fromEntries(transcript.segments.map((segment) => [segment.id, segment.speaker])),
  };
}

/** Reconcile a committed review without discarding unrelated, unsaved fields. */
export function reconcileMeetingDrafts(
  previousTranscript: MeetingTranscript | null,
  nextTranscript: MeetingTranscript,
  previousDrafts: MeetingDraftState,
  operations: readonly CorrectionOperation[],
  resetAll = false,
): MeetingDraftState {
  if (resetAll || previousTranscript === null) return initialMeetingDrafts(nextTranscript);

  const previousSegments = new Map(previousTranscript.segments.map((segment) => [segment.id, segment]));
  const affectedLabels = new Set<string>();
  const affectedTexts = new Set<string>();
  const affectedSpeakers = new Set<string>();
  const clearedMergeTargets = new Set<string>();

  for (const operation of operations) {
    if (operation.kind === "edit_segment") {
      if (operation.text !== undefined) affectedTexts.add(operation.segment_id);
      if (operation.speaker_id !== undefined) affectedSpeakers.add(operation.segment_id);
    } else if (operation.kind === "rename_speaker") {
      affectedLabels.add(operation.speaker_id);
      clearedMergeTargets.add(operation.speaker_id);
    } else {
      clearedMergeTargets.add(operation.from_id);
      clearedMergeTargets.add(operation.into_id);
      for (const segment of nextTranscript.segments) {
        const previous = previousSegments.get(segment.id);
        if (previous && previous.speaker !== segment.speaker) affectedSpeakers.add(segment.id);
      }
    }
  }

  const labels: Record<string, string> = {};
  for (const speaker of nextTranscript.speakers) {
    const previous = previousTranscript.speakers.find((candidate) => candidate.id === speaker.id);
    const previousDraft = previousDrafts.labels[speaker.id];
    labels[speaker.id] =
      affectedLabels.has(speaker.id) || !previous || previousDraft === previous.label
        ? speaker.label
        : previousDraft ?? speaker.label;
  }

  const texts: Record<string, string> = {};
  const speakers: Record<string, string> = {};
  for (const segment of nextTranscript.segments) {
    const previous = previousSegments.get(segment.id);
    const previousTextDraft = previousDrafts.texts[segment.id];
    const previousSpeakerDraft = previousDrafts.speakers[segment.id];
    texts[segment.id] =
      affectedTexts.has(segment.id) || !previous || previousTextDraft === previous.text
        ? segment.text
        : previousTextDraft ?? segment.text;
    speakers[segment.id] =
      affectedSpeakers.has(segment.id) || !previous || previousSpeakerDraft === previous.speaker
        ? segment.speaker
        : previousSpeakerDraft ?? segment.speaker;
  }

  const speakerIds = new Set(nextTranscript.speakers.map((speaker) => speaker.id));
  const mergeTargets: Record<string, string> = {};
  for (const [speakerId, targetId] of Object.entries(previousDrafts.mergeTargets)) {
    if (!clearedMergeTargets.has(speakerId) && speakerIds.has(speakerId) && speakerIds.has(targetId)) {
      mergeTargets[speakerId] = targetId;
    }
  }

  return { labels, mergeTargets, texts, speakers };
}
