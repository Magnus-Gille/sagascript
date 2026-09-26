import type {
  CorrectionOperation,
  MeetingDraftState,
  MeetingReview,
  MeetingReviewState,
  MeetingSegment,
  MeetingSpeaker,
  MeetingTranscript,
} from "./meeting-types";
import type {
  MeetingReprocessingProposal,
  MigrationStep,
  ProposalState,
} from "./meeting-reprocessing-types";

/** The persisted shape used when an update must restart the application. */
export const UPDATE_RECOVERY_SCHEMA_VERSION = 1;

export const UPDATE_RECOVERY_LIMITS = Object.freeze({
  maxPayloadBytes: 20 * 1024 * 1024,
  maxTextLength: 500_000,
  maxPathLength: 4_096,
  maxIdLength: 256,
  maxLanguageLength: 64,
  maxModelLength: 256,
  maxRevisionLength: 256,
  maxSegments: 50_000,
  maxSpeakers: 1_000,
  maxBatches: 1_000,
  maxOperations: 20_000,
  maxResolutions: 20_000,
  maxMigrationSteps: 20_000,
  maxDraftEntries: 50_000,
});

export interface UpdateRecoveryDictation {
  text: string;
}

export interface UpdateRecoveryFile {
  path: string;
  text: string;
}

export interface UpdateRecoveryMeeting {
  review: MeetingReviewState;
  editor_draft: MeetingDraftState;
  proposal: ProposalState | null;
}

export interface UpdateRecoveryPayload {
  schema_version: typeof UPDATE_RECOVERY_SCHEMA_VERSION;
  saved_at: string;
  dictation: UpdateRecoveryDictation | null;
  file: UpdateRecoveryFile | null;
  meeting: UpdateRecoveryMeeting | null;
}

type UnknownRecord = Record<string, unknown>;

function record(value: unknown): UnknownRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as UnknownRecord
    : null;
}

function stringValue(value: unknown, maximum: number, allowEmpty = true): string | null {
  if (typeof value !== "string") return null;
  const result = value.slice(0, maximum);
  return allowEmpty || result.length > 0 ? result : null;
}

function integerValue(value: unknown, minimum: number, maximum: number, fallback = minimum): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  return Math.min(maximum, Math.max(minimum, Math.trunc(value)));
}

function numberValue(value: unknown, minimum: number, maximum: number, fallback = minimum): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  return Math.min(maximum, Math.max(minimum, value));
}

function boundedArray(value: unknown, maximum: number): unknown[] {
  return Array.isArray(value) ? value.slice(0, maximum) : [];
}

function normalizeRecord(value: unknown, maximumEntries: number): Record<string, string> {
  const source = record(value);
  if (!source) return {};
  const result: Record<string, string> = {};
  for (const [key, item] of Object.entries(source).slice(0, maximumEntries)) {
    const safeKey = stringValue(key, UPDATE_RECOVERY_LIMITS.maxIdLength, false);
    const safeValue = stringValue(item, UPDATE_RECOVERY_LIMITS.maxTextLength);
    if (safeKey !== null && safeValue !== null) result[safeKey] = safeValue;
  }
  return result;
}

function normalizeSpeaker(value: unknown): MeetingSpeaker | null {
  const source = record(value);
  if (!source) return null;
  const id = stringValue(source.id, UPDATE_RECOVERY_LIMITS.maxIdLength, false);
  const label = stringValue(source.label, UPDATE_RECOVERY_LIMITS.maxTextLength);
  return id === null || label === null ? null : { id, label };
}

function normalizeSegment(value: unknown): MeetingSegment | null {
  const source = record(value);
  if (!source) return null;
  const id = stringValue(source.id, UPDATE_RECOVERY_LIMITS.maxIdLength, false);
  const text = stringValue(source.text, UPDATE_RECOVERY_LIMITS.maxTextLength);
  const speaker = stringValue(source.speaker, UPDATE_RECOVERY_LIMITS.maxIdLength);
  if (id === null || text === null || speaker === null) return null;
  return {
    id,
    start: numberValue(source.start, 0, 7 * 24 * 60 * 60),
    end: numberValue(source.end, 0, 7 * 24 * 60 * 60),
    text,
    speaker,
  };
}

function normalizeTranscript(value: unknown): MeetingTranscript | null {
  const source = record(value);
  if (!source) return null;
  const segments = boundedArray(source.segments, UPDATE_RECOVERY_LIMITS.maxSegments)
    .map(normalizeSegment)
    .filter((item): item is MeetingSegment => item !== null);
  const speakers = boundedArray(source.speakers, UPDATE_RECOVERY_LIMITS.maxSpeakers)
    .map(normalizeSpeaker)
    .filter((item): item is MeetingSpeaker => item !== null);
  const sourceSha = stringValue(source.source_sha256, UPDATE_RECOVERY_LIMITS.maxRevisionLength);
  const language = stringValue(source.language, UPDATE_RECOVERY_LIMITS.maxLanguageLength);
  const model = stringValue(source.model, UPDATE_RECOVERY_LIMITS.maxModelLength);
  if (sourceSha === null || language === null || model === null) return null;
  return {
    schema_version: integerValue(source.schema_version, 0, 100),
    source_sha256: sourceSha,
    language,
    model,
    duration_seconds: numberValue(source.duration_seconds, 0, 7 * 24 * 60 * 60),
    segments,
    speakers,
  };
}

function normalizeCorrection(value: unknown): CorrectionOperation | null {
  const source = record(value);
  if (!source || typeof source.kind !== "string") return null;
  if (source.kind === "edit_segment") {
    const segmentId = stringValue(source.segment_id, UPDATE_RECOVERY_LIMITS.maxIdLength, false);
    if (segmentId === null) return null;
    const result: CorrectionOperation = { kind: "edit_segment", segment_id: segmentId };
    if (source.text !== undefined) {
      const text = stringValue(source.text, UPDATE_RECOVERY_LIMITS.maxTextLength);
      if (text === null) return null;
      result.text = text;
    }
    if (source.speaker_id !== undefined) {
      const speakerId = stringValue(source.speaker_id, UPDATE_RECOVERY_LIMITS.maxIdLength);
      if (speakerId === null) return null;
      result.speaker_id = speakerId;
    }
    return result;
  }
  if (source.kind === "rename_speaker") {
    const speakerId = stringValue(source.speaker_id, UPDATE_RECOVERY_LIMITS.maxIdLength, false);
    const label = stringValue(source.label, UPDATE_RECOVERY_LIMITS.maxTextLength);
    return speakerId === null || label === null ? null : { kind: "rename_speaker", speaker_id: speakerId, label };
  }
  if (source.kind === "merge_speakers") {
    const fromId = stringValue(source.from_id, UPDATE_RECOVERY_LIMITS.maxIdLength, false);
    const intoId = stringValue(source.into_id, UPDATE_RECOVERY_LIMITS.maxIdLength, false);
    return fromId === null || intoId === null ? null : { kind: "merge_speakers", from_id: fromId, into_id: intoId };
  }
  return null;
}

function normalizeReview(value: unknown): MeetingReview | null {
  const source = record(value);
  if (!source) return null;
  const original = normalizeTranscript(source.original);
  const originalRevision = stringValue(source.original_revision, UPDATE_RECOVERY_LIMITS.maxRevisionLength);
  const revision = stringValue(source.revision, UPDATE_RECOVERY_LIMITS.maxRevisionLength);
  if (original === null || originalRevision === null || revision === null) return null;
  const batches = boundedArray(source.batches, UPDATE_RECOVERY_LIMITS.maxBatches)
    .map((batch) => {
      const batchRecord = record(batch);
      if (!batchRecord) return null;
      const operations = boundedArray(batchRecord.operations, UPDATE_RECOVERY_LIMITS.maxOperations)
        .map(normalizeCorrection)
        .filter((item): item is CorrectionOperation => item !== null);
      return { operations };
    })
    .filter((item): item is { operations: CorrectionOperation[] } => item !== null);
  return {
    schema_version: integerValue(source.schema_version, 0, 100),
    original,
    original_revision: originalRevision,
    generation: integerValue(source.generation, 0, 1_000_000),
    batches,
    revision,
  };
}

function normalizeReviewState(value: unknown): MeetingReviewState | null {
  const source = record(value);
  if (!source) return null;
  const review = normalizeReview(source.review);
  const transcript = normalizeTranscript(source.transcript);
  return review === null || transcript === null ? null : { review, transcript };
}

function normalizeMigrationStep(value: unknown): MigrationStep | null {
  const source = record(value);
  if (!source) return null;
  const original = normalizeCorrection(source.original);
  if (original === null || (source.status !== "applied" && source.status !== "conflict" && source.status !== "blocked")) return null;
  let mapped: CorrectionOperation[] | null = null;
  if (source.mapped !== null && source.mapped !== undefined) {
    mapped = boundedArray(source.mapped, UPDATE_RECOVERY_LIMITS.maxOperations)
      .map(normalizeCorrection)
      .filter((item): item is CorrectionOperation => item !== null);
  }
  return { original, mapped, status: source.status };
}

function normalizeProposal(value: unknown): MeetingReprocessingProposal | null {
  const source = record(value);
  if (!source) return null;
  const previous = normalizeReview(source.previous);
  const proposed = normalizeTranscript(source.proposed);
  const revision = stringValue(source.revision, UPDATE_RECOVERY_LIMITS.maxRevisionLength);
  if (previous === null || proposed === null || revision === null) return null;
  const resolutions = boundedArray(source.resolutions, UPDATE_RECOVERY_LIMITS.maxResolutions)
    .map((resolution) => resolution === null
      ? null
      : boundedArray(resolution, UPDATE_RECOVERY_LIMITS.maxOperations)
        .map(normalizeCorrection)
        .filter((item): item is CorrectionOperation => item !== null));
  return {
    schema_version: integerValue(source.schema_version, 0, 100),
    previous,
    proposed,
    generation: integerValue(source.generation, 0, 1_000_000),
    resolutions,
    revision,
  };
}

function normalizeProposalState(value: unknown): ProposalState | null {
  const source = record(value);
  if (!source) return null;
  const proposal = normalizeProposal(source.proposal);
  const previewSource = record(source.preview);
  const candidate = normalizeTranscript(source.candidate);
  if (proposal === null || !previewSource || candidate === null) return null;
  const previewCandidate = normalizeReview(record(previewSource.candidate));
  if (previewCandidate === null) return null;
  const steps = boundedArray(previewSource.steps, UPDATE_RECOVERY_LIMITS.maxMigrationSteps)
    .map(normalizeMigrationStep)
    .filter((item): item is MigrationStep => item !== null);
  return { proposal, preview: { candidate: previewCandidate, steps }, candidate };
}

function normalizeMeeting(value: unknown): UpdateRecoveryMeeting | null {
  const source = record(value);
  if (!source) return null;
  const review = normalizeReviewState(source.review);
  const editorDraftSource = record(source.editor_draft);
  if (review === null || !editorDraftSource) return null;
  const editor_draft: MeetingDraftState = {
    labels: normalizeRecord(editorDraftSource.labels, UPDATE_RECOVERY_LIMITS.maxDraftEntries),
    mergeTargets: normalizeRecord(editorDraftSource.mergeTargets, UPDATE_RECOVERY_LIMITS.maxDraftEntries),
    texts: normalizeRecord(editorDraftSource.texts, UPDATE_RECOVERY_LIMITS.maxDraftEntries),
    speakers: normalizeRecord(editorDraftSource.speakers, UPDATE_RECOVERY_LIMITS.maxDraftEntries),
  };
  return { review, editor_draft, proposal: normalizeProposalState(source.proposal) };
}

function normalizePayload(value: unknown): UpdateRecoveryPayload | null {
  const source = record(value);
  if (!source || source.schema_version !== UPDATE_RECOVERY_SCHEMA_VERSION) return null;
  const savedAt = stringValue(source.saved_at, 128, false);
  if (savedAt === null) return null;
  const dictationSource = record(source.dictation);
  const dictationText = dictationSource === null ? null : stringValue(dictationSource.text, UPDATE_RECOVERY_LIMITS.maxTextLength);
  const dictation = dictationText === null ? null : { text: dictationText };
  const fileSource = record(source.file);
  const filePath = fileSource === null ? null : stringValue(fileSource.path, UPDATE_RECOVERY_LIMITS.maxPathLength, false);
  const fileText = fileSource === null ? null : stringValue(fileSource.text, UPDATE_RECOVERY_LIMITS.maxTextLength);
  const file = filePath === null || fileText === null ? null : { path: filePath, text: fileText };
  return {
    schema_version: UPDATE_RECOVERY_SCHEMA_VERSION,
    saved_at: savedAt,
    dictation,
    file,
    meeting: normalizeMeeting(source.meeting),
  };
}

/** Build a normalized payload with a fresh timestamp for durable local recovery. */
export function createUpdateRecoveryPayload(
  value: Omit<UpdateRecoveryPayload, "schema_version" | "saved_at">,
  savedAt = new Date().toISOString(),
): UpdateRecoveryPayload {
  const normalized = normalizePayload({ ...value, schema_version: UPDATE_RECOVERY_SCHEMA_VERSION, saved_at: savedAt });
  if (normalized === null) throw new TypeError("Invalid updater recovery payload");
  return normalized;
}

/** Serialize only the known, bounded recovery fields. No audio data is accepted. */
export function serializeUpdateRecoveryPayload(payload: UpdateRecoveryPayload): string {
  const normalized = normalizePayload(payload);
  if (normalized === null) throw new TypeError("Invalid updater recovery payload");
  const serialized = JSON.stringify(normalized);
  if (serialized.length > UPDATE_RECOVERY_LIMITS.maxPayloadBytes) {
    throw new RangeError("Updater recovery payload exceeds the maximum size");
  }
  return serialized;
}

/** Parse and normalize untrusted JSON. Invalid top-level data returns null. */
export function parseUpdateRecoveryPayload(input: string | unknown): UpdateRecoveryPayload | null {
  if (typeof input === "string") {
    if (input.length > UPDATE_RECOVERY_LIMITS.maxPayloadBytes) return null;
    try {
      return normalizePayload(JSON.parse(input));
    } catch {
      return null;
    }
  }
  return normalizePayload(input);
}
