import { expect, test } from "vitest";
import * as recovery from "./update-recovery";
import type { DiarizationRegionEvidence, DiarizationReport, MeetingTranscript } from "./meeting-types";

// Rust serializes an absent Option as null; the TypeScript type says
// "optional". Tests that feed Rust-shaped or deliberately malformed drafts
// write through this wider view of an edit operation.
type WireEditOperation = { kind: "edit_segment"; segment_id: string; text?: string | null; speaker_id?: string | null };

const transcript: MeetingTranscript = {
  schema_version: 1,
  source_sha256: "source-hash",
  language: "sv",
  model: "pianissimo-large",
  duration_seconds: 12.5,
  segments: [{ id: "seg-1", start: 0, end: 2, text: "Hej", speaker: "speaker-1" }],
  speakers: [{ id: "speaker-1", label: "Talare 1" }],
};

function completePayload(): recovery.UpdateRecoveryPayload {
  return recovery.createUpdateRecoveryPayload(
    {
      dictation: { text: "En osparad diktering" },
      files: [{ job_id: "file-1", path: "/tmp/interview.wav", text: "En osparad filtranskribering" }],
      meetings: [
        {
          job_id: "meeting-1",
          path: "/tmp/interview-meeting.wav",
          review: {
            review: {
              schema_version: 1,
              original: transcript,
              original_revision: "original-1",
              generation: 2,
              batches: [{ operations: [{ kind: "edit_segment", segment_id: "seg-1", text: "Hej där" }] }],
              revision: "review-1",
            },
            transcript,
          },
          editor_draft: {
            labels: { "speaker-1": "Magnus" },
            mergeTargets: {},
            texts: { "seg-1": "Hej där" },
            speakers: { "seg-1": "speaker-1" },
          },
          proposal: {
            proposal: {
              schema_version: 1,
              previous: {
                schema_version: 1,
                original: transcript,
                original_revision: "original-1",
                generation: 2,
                batches: [],
                revision: "review-1",
              },
              proposed: transcript,
              generation: 3,
              resolutions: [null],
              revision: "proposal-1",
            },
            preview: {
              candidate: {
                schema_version: 1,
                original: transcript,
                original_revision: "original-1",
                generation: 2,
                batches: [],
                revision: "candidate-1",
              },
              steps: [
                {
                  original: { kind: "edit_segment", segment_id: "seg-1", text: "Hej" },
                  mapped: null,
                  status: "applied",
                },
              ],
            },
            candidate: transcript,
          },
        },
      ],
    },
    "2026-09-26T10:00:00.000Z",
  );
}

/** The first correction of the review in completePayload(), as Rust writes it. */
function firstEdit(payload: recovery.UpdateRecoveryPayload): WireEditOperation {
  const operation = payload.meetings[0].review.review.batches[0].operations[0];
  if (operation.kind !== "edit_segment") throw new Error("the fixture's first correction is an edit");
  return operation;
}

const diarizationReport: DiarizationReport = {
  schema_version: 1,
  source_sha256: "a".repeat(64),
  duration_seconds: 12.5,
  build_revision: "test-revision",
  build_version: "test-version",
  decoder: null,
  asr_segments: [],
  transcript_modified: false,
  parameters: {
    threshold: 0.5,
    min_segment_seconds: 0.1,
    min_gap_seconds: 0.1,
    min_speaker_seconds: 0.1,
    absorb_max_distance: 0.5,
    hint_merge_max_distance: 0.5,
  },
  activity: [{ start: 0, end: 1, speakers: ["speaker-1", "speaker-2"] }],
  regions: [],
  attributions: [],
};

test("persisted recovery distinguishes absence from an unreadable acoustic draft", () => {
  expect(recovery.readPersistedUpdateRecoveryPayload(null)).toBe(null);
  expect(recovery.readPersistedUpdateRecoveryPayload(undefined)).toBe(null);
  const payload = completePayload();
  expect(recovery.readPersistedUpdateRecoveryPayload(payload)).toStrictEqual(payload);
  expect(recovery.readPersistedUpdateRecoveryPayload(JSON.stringify(payload))).toStrictEqual(payload);
  payload.meetings[0].review.transcript = {
    ...transcript,
    schema_version: 2,
    source_sha256: diarizationReport.source_sha256,
    diarization: { ...diarizationReport, schema_version: 99 },
  };
  const original = JSON.stringify(payload);
  expect(() => recovery.readPersistedUpdateRecoveryPayload(payload)).toThrow(/cannot be read/);
  expect(() => recovery.readPersistedUpdateRecoveryPayload(original)).toThrow(/cannot be read/);
  expect(JSON.stringify(payload), "rejected drafts remain untouched for recovery").toBe(original);
  expect(() => recovery.readPersistedUpdateRecoveryPayload("not-json")).toThrow(/cannot be read/);
});

test("persisted recovery rejects lossily normalized proposal branches without mutating the draft", () => {
  for (const corrupt of [
    (payload: recovery.UpdateRecoveryPayload) => {
      payload.meetings[0].proposal!.proposal.proposed.diarization = { ...diarizationReport, schema_version: 99 };
    },
    (payload: recovery.UpdateRecoveryPayload) => {
      payload.meetings[0].proposal!.proposal.previous.original.diarization = { ...diarizationReport, schema_version: 99 };
    },
    (payload: recovery.UpdateRecoveryPayload) => {
      payload.meetings[0].proposal!.candidate.diarization = { ...diarizationReport, schema_version: 99 };
    },
    (payload: recovery.UpdateRecoveryPayload) => {
      payload.meetings[0].proposal!.preview.candidate.original.diarization = { ...diarizationReport, schema_version: 99 };
    },
  ]) {
    const payload = completePayload();
    corrupt(payload);
    const original = JSON.stringify(payload);
    expect(() => recovery.readPersistedUpdateRecoveryPayload(payload)).toThrow(/cannot be read without discarding data/i);
    expect(JSON.stringify(payload), "rejected object drafts remain untouched").toBe(original);
    expect(() => recovery.readPersistedUpdateRecoveryPayload(original)).toThrow(/cannot be read without discarding data/i);
  }
});

test("persisted recovery accepts Rust edit operations with one optional field set", () => {
  for (const [text, speakerId] of [
    ["x", null],
    [null, "speaker-1"],
  ] as const) {
    const payload = completePayload();
    firstEdit(payload).text = text;
    firstEdit(payload).speaker_id = speakerId;
    const serialized = recovery.serializeUpdateRecoveryPayload(payload);
    expect(recovery.readPersistedUpdateRecoveryPayload(serialized)).toStrictEqual(JSON.parse(serialized));
  }
});

test("persisted recovery rejects edit operations with no replacement field without mutation", () => {
  for (const corrupt of [
    (operation: WireEditOperation) => {
      operation.text = null;
      operation.speaker_id = null;
    },
    (operation: WireEditOperation) => {
      delete operation.text;
      delete operation.speaker_id;
    },
  ]) {
    const payload = completePayload();
    corrupt(firstEdit(payload));
    const original = JSON.stringify(payload);
    expect(() => recovery.readPersistedUpdateRecoveryPayload(payload)).toThrow(/cannot be read without discarding data/i);
    expect(JSON.stringify(payload), "rejected object drafts remain untouched").toBe(original);
    expect(() => recovery.readPersistedUpdateRecoveryPayload(original)).toThrow(/cannot be read without discarding data/i);
    expect(original, "the original serialized draft remains unchanged").toBe(JSON.stringify(payload));
  }
});

test("persisted recovery rejects dropped files, queues, and unknown fields without mutation", () => {
  const cases = [
    (payload: recovery.UpdateRecoveryPayload) => {
      Object.assign(payload.files[0], { text: 42 });
    },
    (payload: recovery.UpdateRecoveryPayload) => {
      Object.assign(payload.meetings[0], { unknown_recovery_field: true });
    },
    (payload: recovery.UpdateRecoveryPayload) => {
      payload.files = Array.from({ length: recovery.UPDATE_RECOVERY_LIMITS.maxRecoveryEntries + 1 }, (_, index) => ({
        job_id: `file-${index}`,
        path: `/tmp/${index}.wav`,
        text: "queued",
      }));
    },
  ];
  for (const corrupt of cases) {
    const payload = completePayload();
    corrupt(payload);
    const original = JSON.stringify(payload);
    expect(() => recovery.readPersistedUpdateRecoveryPayload(payload)).toThrow(/cannot be read without discarding data/i);
    expect(JSON.stringify(payload), "lossy persisted inputs remain untouched").toBe(original);
  }
});

test("meeting recovery preserves bounded acoustic diarization evidence", () => {
  const payload = completePayload();
  payload.meetings[0].review.transcript = {
    ...transcript,
    schema_version: 2,
    source_sha256: diarizationReport.source_sha256,
    diarization: structuredClone(diarizationReport),
  };
  const serialized = recovery.serializeUpdateRecoveryPayload(payload);
  const parsed = recovery.parseUpdateRecoveryPayload(serialized);
  expect(parsed!.meetings[0].review.transcript.diarization).toStrictEqual(diarizationReport);
  expect(parsed!.meetings[0].review.transcript.diarization!.activity[0].speakers.length).toBe(2);
});

test("silent acoustic reports round trip with zero-duration ASR evidence", () => {
  const report: DiarizationReport = {
    ...diarizationReport,
    activity: [],
    asr_segments: [{ start: 1, end: 1, avg_logprob: null, no_speech_prob: 1 }],
  };
  const payload = completePayload();
  payload.meetings[0].review.transcript = {
    ...transcript,
    schema_version: 2,
    source_sha256: report.source_sha256,
    diarization: report,
  };

  const parsed = recovery.parseUpdateRecoveryPayload(
    recovery.serializeUpdateRecoveryPayload(recovery.createUpdateRecoveryPayload(payload)),
  );
  expect(parsed!.meetings[0].review.transcript.diarization).toStrictEqual(report);

  for (const asr of [
    { start: 2, end: 1, avg_logprob: null, no_speech_prob: 1 },
    { start: Number.NaN, end: 1, avg_logprob: null, no_speech_prob: 1 },
  ]) {
    const invalid = structuredClone(payload);
    invalid.meetings[0].review.transcript.diarization!.asr_segments = [asr];
    expect(() => recovery.createUpdateRecoveryPayload(invalid)).toThrow(/Invalid updater recovery payload/);
  }
});

test("Rust schema1 reports default omitted acoustic fields while rejecting explicit malformed values", () => {
  for (const omitted of [["asr_segments", "transcript_modified"], ["asr_segments"], ["transcript_modified"]] as const) {
    const report = structuredClone(diarizationReport);
    const omittable: Partial<DiarizationReport> = report;
    for (const field of omitted) delete omittable[field];
    const payload = completePayload();
    payload.meetings[0].review.transcript = {
      ...transcript,
      schema_version: 2,
      source_sha256: report.source_sha256,
      diarization: report,
    };

    const parsed = recovery.parseUpdateRecoveryPayload(JSON.stringify(payload));
    expect(parsed!.meetings[0].review.transcript.diarization, omitted.join(",")).toStrictEqual({
      ...report,
      asr_segments: [],
      transcript_modified: false,
    });
    const persisted = recovery.readPersistedUpdateRecoveryPayload(JSON.stringify(payload));
    expect(persisted!.meetings[0].review.transcript.diarization, `${omitted.join(",")} persisted`).toStrictEqual(
      parsed!.meetings[0].review.transcript.diarization,
    );
  }

  for (const [field, replacement] of [
    ["asr_segments", null],
    ["asr_segments", {}],
    ["transcript_modified", null],
    ["transcript_modified", 0],
  ] as const) {
    const payload = completePayload();
    payload.meetings[0].review.transcript = {
      ...transcript,
      schema_version: 2,
      source_sha256: diarizationReport.source_sha256,
      diarization: { ...diarizationReport, [field]: replacement },
    };
    expect(recovery.parseUpdateRecoveryPayload(JSON.stringify(payload)), `${field}=${replacement}`).toBe(null);
  }
});

test("malformed acoustic spans remain invalid after allowing empty activity", () => {
  const payload = completePayload();
  payload.meetings[0].review.transcript = {
    ...transcript,
    schema_version: 2,
    source_sha256: diarizationReport.source_sha256,
    diarization: { ...diarizationReport, activity: [{ start: 1, end: 1, speakers: ["speaker-1"] }] },
  };
  expect(() => recovery.createUpdateRecoveryPayload(payload)).toThrow(/Invalid updater recovery payload/);
  expect(recovery.parseUpdateRecoveryPayload(JSON.stringify(payload))).toBe(null);
});

test("sample-quantized region support survives recovery within one 16kHz sample", () => {
  const payload = completePayload();
  const region: DiarizationRegionEvidence = {
    index: 0,
    start: 0.00006,
    end: 1,
    track: 0,
    speaker: "speaker-1",
    embedding_status: "missing",
    used_track_fallback: true,
    assigned_centroid_distance: null,
    nearest_other_centroid_distance: null,
    active_speech_seconds: 1,
    overlapping_speech_seconds: 0,
  };
  payload.meetings[0].review.transcript = {
    ...transcript,
    schema_version: 2,
    source_sha256: diarizationReport.source_sha256,
    diarization: { ...diarizationReport, regions: [region] },
  };
  expect(recovery.createUpdateRecoveryPayload(payload).meetings[0].review.transcript.diarization!.regions[0]).toStrictEqual(
    region,
  );
  region.active_speech_seconds = 1.00007;
  expect(() => recovery.createUpdateRecoveryPayload(payload)).toThrow(/Invalid updater recovery payload/);
});

test("invalid acoustic evidence rejects recovery instead of dropping it", () => {
  const payload = completePayload();
  payload.meetings[0].review.transcript = {
    ...transcript,
    schema_version: 2,
    source_sha256: diarizationReport.source_sha256,
    diarization: { ...diarizationReport, activity: [{ start: 0, end: 99_999, speakers: ["speaker-1"] }] },
  };
  expect(() => recovery.createUpdateRecoveryPayload(payload)).toThrow(/Invalid updater recovery payload/);
  expect(recovery.parseUpdateRecoveryPayload(JSON.stringify(payload))).toBe(null);
});

test("zero-length diagnostic evidence is retained while strict decoder and unknown fields fail closed", () => {
  const report: DiarizationReport = {
    ...diarizationReport,
    diagnostics_included: true,
    decoder: {
      strategy: "greedy",
      beam_size: 0,
      temperature_fallback: false,
      vad_enabled: false,
      vad_threshold: 0.5,
      vad_min_silence_duration_ms: 100,
      vad_speech_pad_ms: 30,
      vad_samples_overlap: 0.1,
      timestamp_method: "dtw_words_with_segment_fallback",
    },
    regions: [
      {
        index: 0,
        start: 1,
        end: 1,
        track: 0,
        speaker: "speaker-1",
        embedding_status: "degenerate",
        assigned_centroid_distance: null,
        nearest_other_centroid_distance: null,
        used_track_fallback: true,
        active_speech_seconds: 0,
        overlapping_speech_seconds: 0,
      },
    ],
    attributions: [
      {
        index: 0,
        start: 1,
        end: 1,
        speaker: "speaker-1",
        reason: "invalid_timestamp",
        support: [],
        margin_seconds: null,
        gap_seconds: null,
      },
    ],
  };
  const transcriptWithReport: MeetingTranscript = {
    ...transcript,
    schema_version: 2,
    source_sha256: report.source_sha256,
    diarization: report,
  };
  const valid = completePayload();
  valid.meetings[0].review.transcript = transcriptWithReport;
  const parsed = recovery.parseUpdateRecoveryPayload(recovery.serializeUpdateRecoveryPayload(valid));
  expect(parsed!.meetings[0].review.transcript.diarization!.regions).toStrictEqual(report.regions);
  expect(parsed!.meetings[0].review.transcript.diarization!.attributions).toStrictEqual(report.attributions);

  for (const [field, replacement] of [
    ["beam_size", 1],
    ["strategy", "mystery"],
  ] as const) {
    const invalid = structuredClone(transcriptWithReport);
    invalid.diarization!.decoder = { ...report.decoder!, [field]: replacement };
    const candidate = completePayload();
    candidate.meetings[0].review.transcript = invalid;
    expect(recovery.parseUpdateRecoveryPayload(JSON.stringify(candidate)), field).toBe(null);
  }
  const unknown = structuredClone(transcriptWithReport);
  Object.assign(unknown.diarization!.activity[0], { future_evidence: { nested: ["unbounded"] } });
  const candidate = completePayload();
  candidate.meetings[0].review.transcript = unknown;
  expect(recovery.parseUpdateRecoveryPayload(JSON.stringify(candidate))).toBe(null);
});

test("Rust-invalid diarization report values fail closed during recovery", () => {
  const invalidReports: Record<string, (report: DiarizationReport) => void> = {
    "unknown timestamp method": (report) => {
      report.decoder = {
        strategy: "greedy",
        beam_size: 0,
        temperature_fallback: false,
        vad_enabled: false,
        vad_threshold: 0.5,
        vad_min_silence_duration_ms: 100,
        vad_speech_pad_ms: 30,
        vad_samples_overlap: 0.1,
        timestamp_method: "unknown",
      };
    },
    "support beyond word duration": (report) => {
      report.attributions = [
        {
          index: 0,
          start: 1,
          end: 2,
          speaker: "speaker-1",
          reason: "temporal_overlap",
          support: [{ speaker: "speaker-1", overlap_seconds: 2.00000001 }],
        },
      ];
    },
    "whitespace activity speaker": (report) => {
      report.activity = [{ start: 0, end: 1, speakers: ["speaker 1"] }];
    },
    "control build identity": (report) => {
      report.build_version = "test\u0000version";
    },
    "control region speaker": (report) => {
      report.regions = [
        {
          index: 0,
          start: 0,
          end: 1,
          track: 0,
          speaker: "speaker\u0001",
          embedding_status: "missing",
          assigned_centroid_distance: null,
          nearest_other_centroid_distance: null,
          used_track_fallback: true,
        },
      ];
    },
    "whitespace attribution support speaker": (report) => {
      report.attributions = [
        {
          index: 0,
          start: 1,
          end: 2,
          speaker: "speaker-1",
          reason: "temporal_overlap",
          support: [{ speaker: "speaker\t1", overlap_seconds: 0.5 }],
        },
      ];
    },
    "empty speaker hint": (report) => {
      report.speaker_hint = {};
    },
    "force without a count": (report) => {
      report.speaker_hint = { force: false };
    },
    "speaker hint outcome without hint": (report) => {
      report.speaker_hint_outcome = { satisfied: true, delivered: 1 };
    },
  };

  for (const [name, mutate] of Object.entries(invalidReports)) {
    const payload = completePayload();
    const report = structuredClone(diarizationReport);
    mutate(report);
    payload.meetings[0].review.transcript = {
      ...transcript,
      schema_version: 2,
      source_sha256: report.source_sha256,
      diarization: report,
    };
    expect(() => recovery.createUpdateRecoveryPayload(payload), name).toThrow(/Invalid updater recovery payload/);
    expect(recovery.parseUpdateRecoveryPayload(JSON.stringify(payload)), name).toBe(null);
  }
});

test("meeting speaker hint outcome fields survive recovery with a report", () => {
  const payload = completePayload();
  payload.meetings[0].review.transcript = {
    ...transcript,
    schema_version: 2,
    source_sha256: diarizationReport.source_sha256,
    speaker_hint: { exact: 2 },
    speaker_hint_satisfied: true,
    speaker_hint_delivered: 2,
    diarization: structuredClone(diarizationReport),
  };
  const parsed = recovery.parseUpdateRecoveryPayload(recovery.serializeUpdateRecoveryPayload(payload));
  expect(parsed!.meetings[0].review.transcript.speaker_hint).toStrictEqual({ exact: 2 });
  expect(parsed!.meetings[0].review.transcript.speaker_hint_satisfied).toBe(true);
  expect(parsed!.meetings[0].review.transcript.speaker_hint_delivered).toBe(2);
});

test("payload round trips all unsaved result types", () => {
  const original = completePayload();
  const serialized = recovery.serializeUpdateRecoveryPayload(original);
  const parsed = recovery.parseUpdateRecoveryPayload(serialized);
  expect(parsed).toStrictEqual(original);
  expect(parsed!.meetings[0].editor_draft.texts["seg-1"]).toBe("Hej där");
  expect(parsed!.meetings[0].proposal!.proposal.revision).toBe("proposal-1");
});

test("Rust-shaped meeting proposals preserve optional correction fields serialized as null", () => {
  const payload = completePayload();
  const editText: WireEditOperation = { kind: "edit_segment", segment_id: "seg-1", text: "Hej där", speaker_id: null };
  const editSpeaker: WireEditOperation = { kind: "edit_segment", segment_id: "seg-1", text: null, speaker_id: "speaker-1" };
  Object.assign(payload.meetings[0].review.review, { batches: [{ operations: [editText] }] });
  Object.assign(payload.meetings[0].proposal!.proposal, { resolutions: [null, [editSpeaker]] });
  Object.assign(payload.meetings[0].proposal!.preview, {
    steps: [
      {
        original: editText,
        mapped: [editSpeaker],
        status: "applied",
      },
    ],
  });

  const created = recovery.createUpdateRecoveryPayload(payload);
  const parsed = recovery.parseUpdateRecoveryPayload(recovery.serializeUpdateRecoveryPayload(created));

  expect(parsed).toStrictEqual(created);
  expect(parsed!.meetings[0].review.review.batches[0].operations).toStrictEqual([
    { kind: "edit_segment", segment_id: "seg-1", text: "Hej där" },
  ]);
  expect(parsed!.meetings[0].proposal!.proposal.resolutions).toStrictEqual([
    null,
    [{ kind: "edit_segment", segment_id: "seg-1", speaker_id: "speaker-1" }],
  ]);
  expect(parsed!.meetings[0].proposal!.preview.steps[0].mapped).toStrictEqual([
    { kind: "edit_segment", segment_id: "seg-1", speaker_id: "speaker-1" },
  ]);
});

test("untrusted input is normalized, bounded, and stripped to known fields", () => {
  const parsed = recovery.parseUpdateRecoveryPayload(
    JSON.stringify({
      schema_version: 1,
      saved_at: "saved",
      dictation: { text: "x".repeat(600_000), audio_bytes: "must be discarded" },
      files: [{ job_id: "file-1", path: "/tmp/a", text: "file", raw_audio: [1, 2, 3] }],
      meetings: [],
      unexpected: { token: "discard" },
    }),
  );
  expect(parsed!.dictation!.text.length).toBe(recovery.UPDATE_RECOVERY_LIMITS.maxTextLength);
  expect(parsed!.files).toStrictEqual([{ job_id: "file-1", path: "/tmp/a", text: "file" }]);
  expect(Object.hasOwn(parsed!, "unexpected")).toBe(false);
  expect(Object.hasOwn(parsed!.dictation!, "audio_bytes")).toBe(false);
});

test("invalid JSON, schema versions, and malformed nested results fail closed", () => {
  expect(recovery.parseUpdateRecoveryPayload("not-json")).toBe(null);
  expect(recovery.parseUpdateRecoveryPayload(JSON.stringify({ schema_version: 99 }))).toBe(null);
  const parsed = recovery.parseUpdateRecoveryPayload(
    JSON.stringify({
      schema_version: 1,
      saved_at: "saved",
      dictation: { text: "kept" },
      files: [{ job_id: "file-1", path: 42, text: "discarded" }],
      meetings: [{ job_id: "meeting-1", review: null, editor_draft: {} }],
    }),
  );
  expect(parsed!.files).toStrictEqual([]);
  expect(parsed!.meetings).toStrictEqual([]);
  expect(parsed!.dictation).toStrictEqual({ text: "kept" });
});

test("oversized serialized payloads are rejected", () => {
  const payload = completePayload();
  payload.dictation!.text = "x".repeat(recovery.UPDATE_RECOVERY_LIMITS.maxTextLength);
  expect(() => recovery.serializeUpdateRecoveryPayload(payload)).not.toThrow();
  expect(() =>
    recovery.createUpdateRecoveryPayload({
      dictation: { text: "x".repeat(recovery.UPDATE_RECOVERY_LIMITS.maxTextLength + 1) },
      files: [],
      meetings: [],
    }),
  ).toThrow(/exceed the updater recovery limits/i);
  expect(() =>
    recovery.serializeUpdateRecoveryPayload({
      ...payload,
      dictation: { text: "x".repeat(recovery.UPDATE_RECOVERY_LIMITS.maxTextLength + 1) },
    }),
  ).toThrow(/exceed the updater recovery limits/i);
  expect(recovery.parseUpdateRecoveryPayload("x".repeat(recovery.UPDATE_RECOVERY_LIMITS.maxPayloadBytes + 1))).toBe(
    null,
  );
});

test("persisted payload parsing enforces the limit in UTF-8 bytes", () => {
  const input = JSON.stringify({
    schema_version: 1,
    saved_at: "saved",
    dictation: { text: "é".repeat(10.5 * 1024 * 1024) },
    files: [],
    meetings: [],
  });
  expect(input.length < recovery.UPDATE_RECOVERY_LIMITS.maxPayloadBytes).toBeTruthy();
  expect(new TextEncoder().encode(input).length > recovery.UPDATE_RECOVERY_LIMITS.maxPayloadBytes).toBeTruthy();
  expect(recovery.parseUpdateRecoveryPayload(input)).toBe(null);
});

test("update preparation rejects excess file and meeting entries without dropping later drafts", () => {
  const payload = completePayload();
  payload.files = Array.from({ length: recovery.UPDATE_RECOVERY_LIMITS.maxRecoveryEntries + 1 }, (_, index) => ({
    job_id: `file-${index}`,
    path: `/tmp/${index}.wav`,
    text: `file ${index}`,
  }));
  payload.meetings = Array.from({ length: recovery.UPDATE_RECOVERY_LIMITS.maxRecoveryEntries + 1 }, (_, index) => ({
    ...payload.meetings[0],
    job_id: `meeting-${index}`,
  }));

  expect(() => recovery.createUpdateRecoveryPayload(payload)).toThrow(/exceed the updater recovery limits/i);
  expect(payload.files.at(-1)!.job_id).toBe("file-50");
  expect(payload.meetings.at(-1)!.job_id).toBe("meeting-50");
});

test("multiple file and meeting entries round trip in queue order", () => {
  const payload = completePayload();
  payload.files.push({ job_id: "file-2", path: "/tmp/second.wav", text: "Andra filen" });
  payload.meetings.push({ ...payload.meetings[0], job_id: "meeting-2" });
  const parsed = recovery.parseUpdateRecoveryPayload(recovery.serializeUpdateRecoveryPayload(payload));
  expect(parsed!.files.map((entry) => entry.job_id)).toStrictEqual(["file-1", "file-2"]);
  expect(parsed!.meetings.map((entry) => entry.job_id)).toStrictEqual(["meeting-1", "meeting-2"]);
});

test("duplicate job IDs keep the first valid entry", () => {
  const parsed = recovery.parseUpdateRecoveryPayload(
    JSON.stringify({
      schema_version: 1,
      saved_at: "saved",
      dictation: null,
      files: [
        { job_id: "same", path: "/tmp/first.wav", text: "first" },
        { job_id: "same", path: "/tmp/second.wav", text: "second" },
      ],
      meetings: [],
    }),
  );
  expect(parsed!.files).toStrictEqual([{ job_id: "same", path: "/tmp/first.wav", text: "first" }]);
});

test("file and meeting queues are capped at the configured entry limit", () => {
  const payload = completePayload();
  payload.files = Array.from({ length: recovery.UPDATE_RECOVERY_LIMITS.maxRecoveryEntries + 7 }, (_, index) => ({
    job_id: `file-${index}`,
    path: `/tmp/${index}.wav`,
    text: `file ${index}`,
  }));
  payload.meetings = Array.from({ length: recovery.UPDATE_RECOVERY_LIMITS.maxRecoveryEntries + 7 }, (_, index) => ({
    ...payload.meetings[0],
    job_id: `meeting-${index}`,
  }));
  // Parsing untrusted persisted data remains bounded for safety; update creation
  // and serialization reject this same over-limit payload instead of truncating it.
  const parsed = recovery.parseUpdateRecoveryPayload(JSON.stringify(payload));
  expect(parsed!.files.length).toBe(recovery.UPDATE_RECOVERY_LIMITS.maxRecoveryEntries);
  expect(parsed!.meetings.length).toBe(recovery.UPDATE_RECOVERY_LIMITS.maxRecoveryEntries);
  expect(parsed!.files.at(-1)!.job_id).toBe("file-49");
  expect(parsed!.meetings.at(-1)!.job_id).toBe("meeting-49");
});
