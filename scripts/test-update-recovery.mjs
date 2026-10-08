import assert from "node:assert/strict";
import test from "node:test";
import ts from "typescript";
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../src/lib/update-recovery.ts", import.meta.url), "utf8");
const moduleSource = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
}).outputText;
const recovery = await import(`data:text/javascript;base64,${Buffer.from(moduleSource).toString("base64")}`);

const transcript = {
  schema_version: 1,
  source_sha256: "source-hash",
  language: "sv",
  model: "pianissimo-large",
  duration_seconds: 12.5,
  segments: [{ id: "seg-1", start: 0, end: 2, text: "Hej", speaker: "speaker-1" }],
  speakers: [{ id: "speaker-1", label: "Talare 1" }],
};

function completePayload() {
  return recovery.createUpdateRecoveryPayload({
    dictation: { text: "En osparad diktering" },
    files: [{ job_id: "file-1", path: "/tmp/interview.wav", text: "En osparad filtranskribering" }],
    meetings: [{ job_id: "meeting-1", path: "/tmp/interview-meeting.wav",
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
          steps: [{
            original: { kind: "edit_segment", segment_id: "seg-1", text: "Hej" },
            mapped: null,
            status: "applied",
          }],
        },
        candidate: transcript,
      },
    }],
  }, "2026-09-26T10:00:00.000Z");
}

const diarizationReport = {
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
  assert.deepEqual(parsed.meetings[0].review.transcript.diarization, diarizationReport);
  assert.equal(parsed.meetings[0].review.transcript.diarization.activity[0].speakers.length, 2);
});

test("silent acoustic reports round trip with zero-duration ASR evidence", () => {
  const report = {
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
  assert.deepEqual(parsed.meetings[0].review.transcript.diarization, report);

  for (const asr of [
    { start: 2, end: 1, avg_logprob: null, no_speech_prob: 1 },
    { start: Number.NaN, end: 1, avg_logprob: null, no_speech_prob: 1 },
  ]) {
    const invalid = structuredClone(payload);
    invalid.meetings[0].review.transcript.diarization.asr_segments = [asr];
    assert.throws(() => recovery.createUpdateRecoveryPayload(invalid), /Invalid updater recovery payload/);
  }
});

test("Rust schema1 reports default omitted acoustic fields while rejecting explicit malformed values", () => {
  for (const omitted of [
    ["asr_segments", "transcript_modified"],
    ["asr_segments"],
    ["transcript_modified"],
  ]) {
    const report = structuredClone(diarizationReport);
    for (const field of omitted) delete report[field];
    const payload = completePayload();
    payload.meetings[0].review.transcript = {
      ...transcript,
      schema_version: 2,
      source_sha256: report.source_sha256,
      diarization: report,
    };

    const parsed = recovery.parseUpdateRecoveryPayload(JSON.stringify(payload));
    assert.deepEqual(parsed.meetings[0].review.transcript.diarization, {
      ...report,
      asr_segments: [],
      transcript_modified: false,
    }, omitted.join(","));
  }

  for (const [field, replacement] of [
    ["asr_segments", null],
    ["asr_segments", {}],
    ["transcript_modified", null],
    ["transcript_modified", 0],
  ]) {
    const payload = completePayload();
    payload.meetings[0].review.transcript = {
      ...transcript,
      schema_version: 2,
      source_sha256: diarizationReport.source_sha256,
      diarization: { ...diarizationReport, [field]: replacement },
    };
    assert.equal(recovery.parseUpdateRecoveryPayload(JSON.stringify(payload)), null, `${field}=${replacement}`);
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
  assert.throws(() => recovery.createUpdateRecoveryPayload(payload), /Invalid updater recovery payload/);
  assert.equal(recovery.parseUpdateRecoveryPayload(JSON.stringify(payload)), null);
});

test("sample-quantized region support survives recovery within one 16kHz sample", () => {
  const payload = completePayload();
  const region = {
    index: 0, start: 0.00006, end: 1, track: 0, speaker: "speaker-1",
    embedding_status: "missing", used_track_fallback: true,
    assigned_centroid_distance: null, nearest_other_centroid_distance: null,
    active_speech_seconds: 1, overlapping_speech_seconds: 0,
  };
  payload.meetings[0].review.transcript = {
    ...transcript, schema_version: 2, source_sha256: diarizationReport.source_sha256,
    diarization: { ...diarizationReport, regions: [region] },
  };
  assert.deepEqual(recovery.createUpdateRecoveryPayload(payload).meetings[0].review.transcript.diarization.regions[0], region);
  region.active_speech_seconds = 1.00007;
  assert.throws(() => recovery.createUpdateRecoveryPayload(payload), /Invalid updater recovery payload/);
});

test("invalid acoustic evidence rejects recovery instead of dropping it", () => {
  const payload = completePayload();
  payload.meetings[0].review.transcript = {
    ...transcript,
    schema_version: 2,
    source_sha256: diarizationReport.source_sha256,
    diarization: { ...diarizationReport, activity: [{ start: 0, end: 99_999, speakers: ["speaker-1"] }] },
  };
  assert.throws(() => recovery.createUpdateRecoveryPayload(payload), /Invalid updater recovery payload/);
  assert.equal(recovery.parseUpdateRecoveryPayload(JSON.stringify(payload)), null);
});

test("zero-length diagnostic evidence is retained while strict decoder and unknown fields fail closed", () => {
  const payload = completePayload();
  const report = {
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
    regions: [{
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
    }],
    attributions: [{
      index: 0,
      start: 1,
      end: 1,
      speaker: "speaker-1",
      reason: "invalid_timestamp",
      support: [],
      margin_seconds: null,
      gap_seconds: null,
    }],
  };
  const transcriptWithReport = {
    ...transcript,
    schema_version: 2,
    source_sha256: report.source_sha256,
    diarization: report,
  };
  const valid = completePayload();
  valid.meetings[0].review.transcript = transcriptWithReport;
  const parsed = recovery.parseUpdateRecoveryPayload(recovery.serializeUpdateRecoveryPayload(valid));
  assert.deepEqual(parsed.meetings[0].review.transcript.diarization.regions, report.regions);
  assert.deepEqual(parsed.meetings[0].review.transcript.diarization.attributions, report.attributions);

  for (const [field, replacement] of [
    ["beam_size", 1],
    ["strategy", "mystery"],
  ]) {
    const invalid = structuredClone(transcriptWithReport);
    invalid.diarization.decoder = { ...report.decoder, [field]: replacement };
    const candidate = completePayload();
    candidate.meetings[0].review.transcript = invalid;
    assert.equal(recovery.parseUpdateRecoveryPayload(JSON.stringify(candidate)), null, field);
  }
  const unknown = structuredClone(transcriptWithReport);
  unknown.diarization.activity[0].future_evidence = { nested: ["unbounded"] };
  const candidate = completePayload();
  candidate.meetings[0].review.transcript = unknown;
  assert.equal(recovery.parseUpdateRecoveryPayload(JSON.stringify(candidate)), null);
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
  assert.deepEqual(parsed.meetings[0].review.transcript.speaker_hint, { exact: 2 });
  assert.equal(parsed.meetings[0].review.transcript.speaker_hint_satisfied, true);
  assert.equal(parsed.meetings[0].review.transcript.speaker_hint_delivered, 2);
});

test("payload round trips all unsaved result types", () => {
  const original = completePayload();
  const serialized = recovery.serializeUpdateRecoveryPayload(original);
  const parsed = recovery.parseUpdateRecoveryPayload(serialized);
  assert.deepEqual(parsed, original);
  assert.equal(parsed.meetings[0].editor_draft.texts["seg-1"], "Hej där");
  assert.equal(parsed.meetings[0].proposal.proposal.revision, "proposal-1");
});

test("Rust-shaped meeting proposals preserve optional correction fields serialized as null", () => {
  const payload = completePayload();
  const editText = { kind: "edit_segment", segment_id: "seg-1", text: "Hej där", speaker_id: null };
  const editSpeaker = { kind: "edit_segment", segment_id: "seg-1", text: null, speaker_id: "speaker-1" };
  payload.meetings[0].review.review.batches = [{ operations: [editText] }];
  payload.meetings[0].proposal.proposal.resolutions = [null, [editSpeaker]];
  payload.meetings[0].proposal.preview.steps = [{
    original: editText,
    mapped: [editSpeaker],
    status: "applied",
  }];

  const created = recovery.createUpdateRecoveryPayload(payload);
  const parsed = recovery.parseUpdateRecoveryPayload(recovery.serializeUpdateRecoveryPayload(created));

  assert.deepEqual(parsed, created);
  assert.deepEqual(parsed.meetings[0].review.review.batches[0].operations, [
    { kind: "edit_segment", segment_id: "seg-1", text: "Hej där" },
  ]);
  assert.deepEqual(parsed.meetings[0].proposal.proposal.resolutions, [null, [
    { kind: "edit_segment", segment_id: "seg-1", speaker_id: "speaker-1" },
  ]]);
  assert.deepEqual(parsed.meetings[0].proposal.preview.steps[0].mapped, [
    { kind: "edit_segment", segment_id: "seg-1", speaker_id: "speaker-1" },
  ]);
});

test("untrusted input is normalized, bounded, and stripped to known fields", () => {
  const parsed = recovery.parseUpdateRecoveryPayload(JSON.stringify({
    schema_version: 1,
    saved_at: "saved",
    dictation: { text: "x".repeat(600_000), audio_bytes: "must be discarded" },
    files: [{ job_id: "file-1", path: "/tmp/a", text: "file", raw_audio: [1, 2, 3] }],
    meetings: [],
    unexpected: { token: "discard" },
  }));
  assert.equal(parsed.dictation.text.length, recovery.UPDATE_RECOVERY_LIMITS.maxTextLength);
  assert.deepEqual(parsed.files, [{ job_id: "file-1", path: "/tmp/a", text: "file" }]);
  assert.equal(Object.hasOwn(parsed, "unexpected"), false);
  assert.equal(Object.hasOwn(parsed.dictation, "audio_bytes"), false);
});

test("invalid JSON, schema versions, and malformed nested results fail closed", () => {
  assert.equal(recovery.parseUpdateRecoveryPayload("not-json"), null);
  assert.equal(recovery.parseUpdateRecoveryPayload(JSON.stringify({ schema_version: 99 })), null);
  const parsed = recovery.parseUpdateRecoveryPayload(JSON.stringify({
    schema_version: 1,
    saved_at: "saved",
    dictation: { text: "kept" },
    files: [{ job_id: "file-1", path: 42, text: "discarded" }],
    meetings: [{ job_id: "meeting-1", review: null, editor_draft: {} }],
  }));
  assert.deepEqual(parsed.files, []);
  assert.deepEqual(parsed.meetings, []);
  assert.deepEqual(parsed.dictation, { text: "kept" });
});

test("oversized serialized payloads are rejected", () => {
  const payload = completePayload();
  payload.dictation.text = "x".repeat(recovery.UPDATE_RECOVERY_LIMITS.maxTextLength);
  assert.doesNotThrow(() => recovery.serializeUpdateRecoveryPayload(payload));
  assert.throws(() => recovery.createUpdateRecoveryPayload({
    dictation: { text: "x".repeat(recovery.UPDATE_RECOVERY_LIMITS.maxTextLength + 1) },
    files: [],
    meetings: [],
  }), /exceed the updater recovery limits/i);
  assert.throws(() => recovery.serializeUpdateRecoveryPayload({
    ...payload,
    dictation: { text: "x".repeat(recovery.UPDATE_RECOVERY_LIMITS.maxTextLength + 1) },
  }), /exceed the updater recovery limits/i);
  assert.equal(
    recovery.parseUpdateRecoveryPayload("x".repeat(recovery.UPDATE_RECOVERY_LIMITS.maxPayloadBytes + 1)),
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
  assert.ok(input.length < recovery.UPDATE_RECOVERY_LIMITS.maxPayloadBytes);
  assert.ok(Buffer.byteLength(input, "utf8") > recovery.UPDATE_RECOVERY_LIMITS.maxPayloadBytes);
  assert.equal(recovery.parseUpdateRecoveryPayload(input), null);
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

  assert.throws(() => recovery.createUpdateRecoveryPayload(payload), /exceed the updater recovery limits/i);
  assert.equal(payload.files.at(-1).job_id, "file-50");
  assert.equal(payload.meetings.at(-1).job_id, "meeting-50");
});

test("multiple file and meeting entries round trip in queue order", () => {
  const payload = completePayload();
  payload.files.push({ job_id: "file-2", path: "/tmp/second.wav", text: "Andra filen" });
  payload.meetings.push({ ...payload.meetings[0], job_id: "meeting-2" });
  const parsed = recovery.parseUpdateRecoveryPayload(recovery.serializeUpdateRecoveryPayload(payload));
  assert.deepEqual(parsed.files.map((entry) => entry.job_id), ["file-1", "file-2"]);
  assert.deepEqual(parsed.meetings.map((entry) => entry.job_id), ["meeting-1", "meeting-2"]);
});

test("duplicate job IDs keep the first valid entry", () => {
  const parsed = recovery.parseUpdateRecoveryPayload(JSON.stringify({
    schema_version: 1,
    saved_at: "saved",
    dictation: null,
    files: [
      { job_id: "same", path: "/tmp/first.wav", text: "first" },
      { job_id: "same", path: "/tmp/second.wav", text: "second" },
    ],
    meetings: [],
  }));
  assert.deepEqual(parsed.files, [{ job_id: "same", path: "/tmp/first.wav", text: "first" }]);
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
  assert.equal(parsed.files.length, recovery.UPDATE_RECOVERY_LIMITS.maxRecoveryEntries);
  assert.equal(parsed.meetings.length, recovery.UPDATE_RECOVERY_LIMITS.maxRecoveryEntries);
  assert.equal(parsed.files.at(-1).job_id, "file-49");
  assert.equal(parsed.meetings.at(-1).job_id, "meeting-49");
});
