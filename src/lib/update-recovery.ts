import type {
  CorrectionOperation,
  MeetingDraftState,
  MeetingReview,
  MeetingReviewState,
  MeetingSegment,
  MeetingSpeaker,
  MeetingTranscript,
  DiarizationReport,
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
  maxRecoveryEntries: 50,
  maxSegments: 50_000,
  maxSpeakers: 1_000,
  maxBatches: 1_000,
  maxOperations: 20_000,
  maxResolutions: 20_000,
  maxMigrationSteps: 20_000,
  maxDraftEntries: 50_000,
  maxDiarizationItems: 500_000,
  maxDiarizationSpeakers: 64,
});

export interface UpdateRecoveryDictation {
  text: string;
}

export interface UpdateRecoveryFile {
  job_id: string;
  path: string;
  text: string;
}

export interface UpdateRecoveryMeeting {
  job_id: string;
  path: string;
  review: MeetingReviewState;
  editor_draft: MeetingDraftState;
  proposal: ProposalState | null;
}

export interface UpdateRecoveryPayload {
  schema_version: typeof UPDATE_RECOVERY_SCHEMA_VERSION;
  saved_at: string;
  dictation: UpdateRecoveryDictation | null;
  files: UpdateRecoveryFile[];
  meetings: UpdateRecoveryMeeting[];
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

function strictStringValue(value: unknown, maximum: number, allowEmpty = true): string | null {
  if (typeof value !== "string" || value.length > maximum) return null;
  return allowEmpty || value.length > 0 ? value : null;
}

function finiteNumber(value: unknown, minimum: number, maximum: number): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= minimum && value <= maximum
    ? value
    : null;
}

function integerNumber(value: unknown, minimum: number, maximum: number): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= minimum && value <= maximum;
}

function sha256Value(value: unknown): string | null {
  return typeof value === "string" && /^[0-9a-f]{64}$/.test(value) ? value : null;
}

function hasOnlyKeys(source: UnknownRecord, allowed: readonly string[]): boolean {
  const keys = new Set(allowed);
  return Object.keys(source).every((key) => keys.has(key));
}

function boundedArray(value: unknown, maximum: number): unknown[] {
  return Array.isArray(value) ? value.slice(0, maximum) : [];
}

function hasKeyDeep(value: unknown, key: string, depth = 0): boolean {
  if (depth > 8) return false;
  if (Array.isArray(value)) return value.slice(0, 1_000).some((item) => hasKeyDeep(item, key, depth + 1));
  const source = record(value);
  if (source === null) return false;
  return Object.hasOwn(source, key)
    || Object.values(source).some((item) => hasKeyDeep(item, key, depth + 1));
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

function normalizeDiarization(value: unknown): DiarizationReport | null {
  const source = record(value);
  if (!source) return null;
  if (!hasOnlyKeys(source, [
    "schema_version", "source_sha256", "duration_seconds", "build_revision", "build_version",
    "segmentation_model_sha256", "embedding_model_sha256", "decoder", "diagnostics_included",
    "speaker_hint", "speaker_hint_outcome", "asr_segments", "transcript_modified", "parameters",
    "activity", "regions", "attributions",
  ])) return null;
  const sourceSha = strictStringValue(source.source_sha256, 64, false);
  const buildRevision = strictStringValue(source.build_revision, UPDATE_RECOVERY_LIMITS.maxRevisionLength, false);
  const buildVersion = strictStringValue(source.build_version, UPDATE_RECOVERY_LIMITS.maxRevisionLength, false);
  const duration = finiteNumber(source.duration_seconds, 0, 14_400);
  const activity = boundedArray(source.activity, UPDATE_RECOVERY_LIMITS.maxDiarizationItems);
  const regions = boundedArray(source.regions, UPDATE_RECOVERY_LIMITS.maxDiarizationItems);
  const attributions = boundedArray(source.attributions, UPDATE_RECOVERY_LIMITS.maxDiarizationItems);
  const asrSegments = boundedArray(source.asr_segments, UPDATE_RECOVERY_LIMITS.maxDiarizationItems);
  const maxSupportEntries = 64;
  if (
    source.schema_version !== 1
    || sourceSha === null
    || sha256Value(source.source_sha256) === null
    || buildRevision === null
    || buildVersion === null
    || duration === null
    || !Array.isArray(source.activity)
    || !Array.isArray(source.regions)
    || !Array.isArray(source.attributions)
    || !Array.isArray(source.asr_segments)
    || source.activity.length > UPDATE_RECOVERY_LIMITS.maxDiarizationItems
    || source.regions.length > UPDATE_RECOVERY_LIMITS.maxDiarizationItems
    || source.attributions.length > UPDATE_RECOVERY_LIMITS.maxDiarizationItems
    || source.asr_segments.length > UPDATE_RECOVERY_LIMITS.maxDiarizationItems
    || typeof source.transcript_modified !== "boolean"
    || (source.diagnostics_included !== undefined && typeof source.diagnostics_included !== "boolean")
    || !record(source.parameters)
  ) return null;

  const parameters = source.parameters as UnknownRecord;
  if (!hasOnlyKeys(parameters, [
    "threshold", "min_segment_seconds", "min_gap_seconds", "min_speaker_seconds",
    "absorb_max_distance", "hint_merge_max_distance", "exclusive_speech_embeddings",
  ])) return null;
  if (
    finiteNumber(parameters.threshold, 0, 2) === null
    || finiteNumber(parameters.min_segment_seconds, 0, 14_400) === null
    || finiteNumber(parameters.min_gap_seconds, 0, 14_400) === null
    || finiteNumber(parameters.min_speaker_seconds, 0, 14_400) === null
    || finiteNumber(parameters.absorb_max_distance, 0, 2) === null
    || finiteNumber(parameters.hint_merge_max_distance, 0, 2) === null
    || (parameters.exclusive_speech_embeddings !== undefined
      && typeof parameters.exclusive_speech_embeddings !== "boolean")
  ) return null;

  for (const hash of [source.segmentation_model_sha256, source.embedding_model_sha256]) {
    if (hash !== undefined && hash !== null && sha256Value(hash) === null) return null;
  }
  const hint = source.speaker_hint;
  if (hint !== undefined && hint !== null) {
    const hintRecord = record(hint);
    if (
      !hintRecord
      || !hasOnlyKeys(hintRecord, ["exact", "min", "max", "force"])
      || (hintRecord.exact !== undefined && !integerNumber(hintRecord.exact, 1, 1_000))
      || (hintRecord.min !== undefined && !integerNumber(hintRecord.min, 1, 1_000))
      || (hintRecord.max !== undefined && !integerNumber(hintRecord.max, 1, 1_000))
      || (hintRecord.force !== undefined && typeof hintRecord.force !== "boolean")
    ) return null;
  }
  const hintOutcome = source.speaker_hint_outcome;
  if (hintOutcome !== undefined && hintOutcome !== null) {
    const outcomeRecord = record(hintOutcome);
    if (
      !outcomeRecord
      || !hasOnlyKeys(outcomeRecord, ["satisfied", "delivered"])
      || typeof outcomeRecord.satisfied !== "boolean"
      || !integerNumber(outcomeRecord.delivered, 0, 1_000_000)
    ) return null;
  }

  const decoder = source.decoder;
  if (decoder !== undefined && decoder !== null) {
    const decoderRecord = record(decoder);
    if (
      !decoderRecord
      || !hasOnlyKeys(decoderRecord, [
        "strategy", "beam_size", "temperature_fallback", "vad_enabled", "vad_threshold",
        "vad_min_silence_duration_ms", "vad_speech_pad_ms", "vad_samples_overlap", "timestamp_method",
      ])
      || strictStringValue(decoderRecord.strategy, UPDATE_RECOVERY_LIMITS.maxIdLength, false) === null
      || typeof decoderRecord.beam_size !== "number"
      || !Number.isInteger(decoderRecord.beam_size)
      || typeof decoderRecord.temperature_fallback !== "boolean"
      || typeof decoderRecord.vad_enabled !== "boolean"
      || decoderRecord.beam_size < 0
      || decoderRecord.beam_size > 8
      || (decoderRecord.beam_size === 1)
      || ((decoderRecord.beam_size < 2) !== (decoderRecord.strategy === "greedy"))
      || (decoderRecord.beam_size >= 2 && decoderRecord.strategy !== "beam_search")
      || finiteNumber(decoderRecord.vad_threshold, 0, 1) === null
      || !integerNumber(decoderRecord.vad_min_silence_duration_ms, 0, 14_400_000)
      || !integerNumber(decoderRecord.vad_speech_pad_ms, 0, 14_400_000)
      || finiteNumber(decoderRecord.vad_samples_overlap, 0, 1) === null
      || strictStringValue(decoderRecord.timestamp_method, UPDATE_RECOVERY_LIMITS.maxIdLength, false) === null
    ) return null;
  }

  let previousActivityEnd = 0;
  const validActivity = activity.every((item) => {
    const span = record(item);
    const speakers = span ? boundedArray(span.speakers, UPDATE_RECOVERY_LIMITS.maxDiarizationSpeakers) : [];
    const valid = span !== null
      && hasOnlyKeys(span, ["start", "end", "speakers"])
      && finiteNumber(span.start, 0, duration) !== null
      && finiteNumber(span.end, 0, duration) !== null
      && finiteNumber(span.start, 0, duration)! < finiteNumber(span.end, 0, duration)!
      && finiteNumber(span.start, 0, duration)! >= previousActivityEnd
      && Array.isArray(span.speakers)
      && span.speakers.length > 0
      && span.speakers.length <= UPDATE_RECOVERY_LIMITS.maxDiarizationSpeakers
      && new Set(speakers).size === speakers.length
      && speakers.every((speaker) => strictStringValue(speaker, UPDATE_RECOVERY_LIMITS.maxIdLength, false) !== null);
    if (valid) previousActivityEnd = finiteNumber(span!.end, 0, duration)!;
    return valid;
  });
  const validRegions = regions.every((item, index) => {
    const region = record(item);
    const status = region?.embedding_status;
    return region !== null
      && hasOnlyKeys(region, [
        "index", "start", "end", "track", "speaker", "embedding_status",
        "assigned_centroid_distance", "nearest_other_centroid_distance", "used_track_fallback",
        "active_speech_seconds", "overlapping_speech_seconds",
      ])
      && typeof region.index === "number"
      && Number.isInteger(region.index)
      && region.index >= 0
      && region.index === index
      && finiteNumber(region.start, 0, duration) !== null
      && finiteNumber(region.end, 0, duration) !== null
      && finiteNumber(region.start, 0, duration)! <= finiteNumber(region.end, 0, duration)!
      && strictStringValue(region.speaker, UPDATE_RECOVERY_LIMITS.maxIdLength, false) !== null
      && (status === "usable" || status === "missing" || status === "degenerate")
      && typeof region.track === "number"
      && Number.isInteger(region.track)
      && region.track >= 0
      && typeof region.used_track_fallback === "boolean"
      && region.used_track_fallback === (status !== "usable")
      && [region.assigned_centroid_distance, region.nearest_other_centroid_distance]
        .every((distance) => distance === undefined || distance === null || finiteNumber(distance, 0, 2) !== null)
      && [region.active_speech_seconds, region.overlapping_speech_seconds]
        .every((seconds) => seconds === undefined || seconds === null
          || (typeof seconds === "number" && finiteNumber(seconds, 0, 14_400) !== null
            && seconds <= finiteNumber(region.end, 0, duration)! - finiteNumber(region.start, 0, duration)! + 1e-6));
  });
  const validRegionKeys = regions.every((item) => {
    const region = record(item);
    return region !== null
      && typeof region.index === "number"
      && Number.isInteger(region.index)
      && region.index >= 0
      && region.index < regions.length;
  });
  const validAttributions = attributions.every((item, index) => {
    const attribution = record(item);
    const support = attribution === null ? [] : boundedArray(attribution.support, maxSupportEntries);
    return attribution !== null
      && hasOnlyKeys(attribution, ["index", "start", "end", "speaker", "reason", "support", "margin_seconds", "gap_seconds"])
      && typeof attribution.index === "number"
      && Number.isInteger(attribution.index)
      && attribution.index >= 0
      && attribution.index === index
      && finiteNumber(attribution.start, 0, duration) !== null
      && finiteNumber(attribution.end, 0, duration) !== null
      && finiteNumber(attribution.start, 0, duration)! <= finiteNumber(attribution.end, 0, duration)!
      && strictStringValue(attribution.speaker, UPDATE_RECOVERY_LIMITS.maxIdLength, false) !== null
      && ["temporal_overlap", "tied_overlap", "nearest_gap", "no_speaker_evidence", "invalid_timestamp"].includes(String(attribution.reason))
      && Array.isArray(attribution.support)
      && attribution.support.length <= maxSupportEntries
      && new Set(support.map((entry) => record(entry)?.speaker)).size === support.length
      && support.every((entry) => {
        const item = record(entry);
        return item !== null
          && hasOnlyKeys(item, ["speaker", "overlap_seconds"])
          && strictStringValue(item.speaker, UPDATE_RECOVERY_LIMITS.maxIdLength, false) !== null
          && finiteNumber(item.overlap_seconds, 0, 14_400) !== null;
      })
      && (attribution.margin_seconds === undefined
        || attribution.margin_seconds === null
        || finiteNumber(attribution.margin_seconds, 0, 14_400) !== null)
      && (attribution.gap_seconds === undefined
        || attribution.gap_seconds === null
        || finiteNumber(attribution.gap_seconds, 0, 14_400) !== null);
  });
  const validEvidence = validRegionKeys && validRegions && validAttributions;
  const validAsr = asrSegments.every((item) => {
    const segment = record(item);
    return segment !== null
      && hasOnlyKeys(segment, ["start", "end", "avg_logprob", "no_speech_prob"])
      && finiteNumber(segment.start, 0, duration) !== null
      && finiteNumber(segment.end, 0, duration) !== null
      && finiteNumber(segment.start, 0, duration)! <= finiteNumber(segment.end, 0, duration)!
      && (segment.avg_logprob === undefined
        || segment.avg_logprob === null
        || (typeof segment.avg_logprob === "number" && Number.isFinite(segment.avg_logprob)))
      && (segment.no_speech_prob === undefined
        || segment.no_speech_prob === null
        || (typeof segment.no_speech_prob === "number"
          && Number.isFinite(segment.no_speech_prob)
          && segment.no_speech_prob >= 0
          && segment.no_speech_prob <= 1));
  });
  if (!validActivity || !validEvidence || !validAsr) return null;

  // Return the original bounded object so newly added evidence fields are
  // retained losslessly rather than silently discarded during recovery.
  return source as DiarizationReport;
}

function normalizeSpeakerHint(value: unknown): Record<string, unknown> | null {
  const source = record(value);
  if (!source) return null;
  if (
    !hasOnlyKeys(source, ["exact", "min", "max", "force"])
    || (source.exact === undefined && source.min === undefined && source.max === undefined && source.force === undefined)
    || (source.exact !== undefined && !integerNumber(source.exact, 1, 1_000))
    || (source.min !== undefined && !integerNumber(source.min, 1, 1_000))
    || (source.max !== undefined && !integerNumber(source.max, 1, 1_000))
    || (source.force !== undefined && typeof source.force !== "boolean")
    || (source.exact !== undefined && (source.min !== undefined || source.max !== undefined))
    || (source.force === true && source.exact === undefined)
    || (typeof source.min === "number" && typeof source.max === "number" && source.min > source.max)
  ) return null;
  return source;
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
  const schemaVersion = integerValue(source.schema_version, 0, 100);
  const diarization = source.diarization === undefined || source.diarization === null
    ? source.diarization === null ? null : undefined
    : normalizeDiarization(source.diarization);
  if (source.diarization !== undefined && source.diarization !== null && diarization === null) return null;
  if ((schemaVersion === 2) !== (diarization !== undefined && diarization !== null)) return null;
  if (diarization !== undefined && diarization !== null
    && (diarization.source_sha256 !== sourceSha || diarization.duration_seconds !== numberValue(source.duration_seconds, 0, 7 * 24 * 60 * 60))) return null;
  const speakerHint = source.speaker_hint === undefined || source.speaker_hint === null
    ? source.speaker_hint === null ? null : undefined
    : normalizeSpeakerHint(source.speaker_hint);
  if (source.speaker_hint !== undefined && source.speaker_hint !== null && speakerHint === null) return null;
  const hintSatisfied = source.speaker_hint_satisfied;
  const hintDelivered = source.speaker_hint_delivered;
  if (
    (hintSatisfied !== undefined && hintSatisfied !== null && typeof hintSatisfied !== "boolean")
    || (hintDelivered !== undefined && hintDelivered !== null && !integerNumber(hintDelivered, 0, 1_000_000))
    || ((hintSatisfied !== undefined && hintSatisfied !== null) !== (hintDelivered !== undefined && hintDelivered !== null))
    || ((hintSatisfied !== undefined && hintSatisfied !== null) && (speakerHint === undefined || speakerHint === null))
  ) return null;
  const normalized: MeetingTranscript = {
    schema_version: schemaVersion,
    source_sha256: sourceSha,
    language,
    model,
    duration_seconds: numberValue(source.duration_seconds, 0, 7 * 24 * 60 * 60),
    segments,
    speakers,
  };
  if (source.diarization !== undefined) normalized.diarization = diarization;
  if (source.speaker_hint !== undefined) normalized.speaker_hint = speakerHint as MeetingTranscript["speaker_hint"];
  if (source.speaker_hint_satisfied !== undefined) normalized.speaker_hint_satisfied = hintSatisfied as boolean | null;
  if (source.speaker_hint_delivered !== undefined) normalized.speaker_hint_delivered = hintDelivered as number | null;
  return normalized;
}

function normalizeCorrection(value: unknown): CorrectionOperation | null {
  const source = record(value);
  if (!source || typeof source.kind !== "string") return null;
  if (source.kind === "edit_segment") {
    const segmentId = stringValue(source.segment_id, UPDATE_RECOVERY_LIMITS.maxIdLength, false);
    if (segmentId === null) return null;
    const result: CorrectionOperation = { kind: "edit_segment", segment_id: segmentId };
    if (source.text !== undefined && source.text !== null) {
      const text = stringValue(source.text, UPDATE_RECOVERY_LIMITS.maxTextLength);
      if (text === null) return null;
      result.text = text;
    }
    if (source.speaker_id !== undefined && source.speaker_id !== null) {
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
  const jobId = stringValue(source.job_id, UPDATE_RECOVERY_LIMITS.maxIdLength, false);
  if (jobId === null) return null;
  const path = stringValue(source.path, UPDATE_RECOVERY_LIMITS.maxPathLength, false)
    ?? `Meeting ${jobId}`;
  return { job_id: jobId, path, review, editor_draft, proposal: normalizeProposalState(source.proposal) };
}

function normalizeFile(value: unknown): UpdateRecoveryFile | null {
  const source = record(value);
  if (!source) return null;
  const jobId = stringValue(source.job_id, UPDATE_RECOVERY_LIMITS.maxIdLength, false);
  const path = stringValue(source.path, UPDATE_RECOVERY_LIMITS.maxPathLength, false);
  const text = stringValue(source.text, UPDATE_RECOVERY_LIMITS.maxTextLength);
  return jobId === null || path === null || text === null ? null : { job_id: jobId, path, text };
}

function normalizeUniqueEntries<T extends { job_id: string }>(value: unknown, normalizer: (item: unknown) => T | null): T[] {
  const result: T[] = [];
  const seen = new Set<string>();
  for (const item of boundedArray(value, UPDATE_RECOVERY_LIMITS.maxRecoveryEntries)) {
    const normalized = normalizer(item);
    if (normalized === null || seen.has(normalized.job_id)) continue;
    seen.add(normalized.job_id);
    result.push(normalized);
  }
  return result;
}

function normalizePayload(value: unknown): UpdateRecoveryPayload | null {
  const source = record(value);
  if (!source || source.schema_version !== UPDATE_RECOVERY_SCHEMA_VERSION) return null;
  const savedAt = stringValue(source.saved_at, 128, false);
  if (savedAt === null) return null;
  const dictationSource = record(source.dictation);
  const dictationText = dictationSource === null ? null : stringValue(dictationSource.text, UPDATE_RECOVERY_LIMITS.maxTextLength);
  const dictation = dictationText === null ? null : { text: dictationText };
  const rawMeetings = boundedArray(source.meetings, UPDATE_RECOVERY_LIMITS.maxRecoveryEntries);
  if (rawMeetings.some((meeting) => hasKeyDeep(meeting, "diarization") && normalizeMeeting(meeting) === null)) {
    return null;
  }
  return {
    schema_version: UPDATE_RECOVERY_SCHEMA_VERSION,
    saved_at: savedAt,
    dictation,
    files: normalizeUniqueEntries(source.files, normalizeFile),
    meetings: normalizeUniqueEntries(source.meetings, normalizeMeeting),
  };
}

function sameNormalizedValue(input: unknown, normalized: unknown): boolean {
  if (Object.is(input, normalized)) return true;
  if (Array.isArray(input) || Array.isArray(normalized)) {
    return Array.isArray(input)
      && Array.isArray(normalized)
      && input.length === normalized.length
      && input.every((item, index) => sameNormalizedValue(item, normalized[index]));
  }
  const inputRecord = record(input);
  const normalizedRecord = record(normalized);
  if (inputRecord === null || normalizedRecord === null) return false;
  const inputKeys = Object.keys(inputRecord).filter((key) => {
    // Rust serializes Option fields as null. For edit operations, null is the
    // wire representation of an absent optional value, which normalization
    // intentionally omits from the TypeScript shape.
    return !(inputRecord.kind === "edit_segment"
      && normalizedRecord.kind === "edit_segment"
      && (key === "text" || key === "speaker_id")
      && inputRecord[key] === null
      && !Object.hasOwn(normalizedRecord, key));
  });
  const normalizedKeys = Object.keys(normalizedRecord);
  return inputKeys.length === normalizedKeys.length
    && inputKeys.every((key) => Object.hasOwn(normalizedRecord, key)
      && sameNormalizedValue(inputRecord[key], normalizedRecord[key]));
}

function assertLosslessNormalization(input: unknown, normalized: UpdateRecoveryPayload): void {
  if (!sameNormalizedValue(input, normalized)) {
    throw new RangeError(
      "Unsaved results exceed the updater recovery limits. Save or remove some results before updating.",
    );
  }
}

function utf8ByteLength(value: string, maximum: number): number {
  let bytes = 0;
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 0x7f) bytes += 1;
    else if (code <= 0x7ff) bytes += 2;
    else if (code >= 0xd800 && code <= 0xdbff
      && index + 1 < value.length
      && value.charCodeAt(index + 1) >= 0xdc00
      && value.charCodeAt(index + 1) <= 0xdfff) {
      bytes += 4;
      index += 1;
    } else bytes += 3;
    if (bytes > maximum) return bytes;
  }
  return bytes;
}

/** Build a normalized payload with a fresh timestamp for durable local recovery. */
export function createUpdateRecoveryPayload(
  value: Omit<UpdateRecoveryPayload, "schema_version" | "saved_at">,
  savedAt = new Date().toISOString(),
): UpdateRecoveryPayload {
  const normalized = normalizePayload({ ...value, schema_version: UPDATE_RECOVERY_SCHEMA_VERSION, saved_at: savedAt });
  if (normalized === null) throw new TypeError("Invalid updater recovery payload");
  assertLosslessNormalization({ ...value, schema_version: UPDATE_RECOVERY_SCHEMA_VERSION, saved_at: savedAt }, normalized);
  return normalized;
}

/** Serialize only the known, bounded recovery fields. No audio data is accepted. */
export function serializeUpdateRecoveryPayload(payload: UpdateRecoveryPayload): string {
  const normalized = normalizePayload(payload);
  if (normalized === null) throw new TypeError("Invalid updater recovery payload");
  assertLosslessNormalization(payload, normalized);
  const serialized = JSON.stringify(normalized);
  if (utf8ByteLength(serialized, UPDATE_RECOVERY_LIMITS.maxPayloadBytes) > UPDATE_RECOVERY_LIMITS.maxPayloadBytes) {
    throw new RangeError("Updater recovery payload exceeds the maximum size. Save or remove some results before updating.");
  }
  return serialized;
}

/** Parse and normalize untrusted JSON. Invalid top-level data returns null. */
export function parseUpdateRecoveryPayload(input: string | unknown): UpdateRecoveryPayload | null {
  if (typeof input === "string") {
    if (input.length > UPDATE_RECOVERY_LIMITS.maxPayloadBytes
      || utf8ByteLength(input, UPDATE_RECOVERY_LIMITS.maxPayloadBytes) > UPDATE_RECOVERY_LIMITS.maxPayloadBytes) return null;
    try {
      return normalizePayload(JSON.parse(input));
    } catch {
      return null;
    }
  }
  return normalizePayload(input);
}
